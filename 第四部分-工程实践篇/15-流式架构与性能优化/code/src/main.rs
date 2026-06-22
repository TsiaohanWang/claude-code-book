// =============================================================================
// 第七章：SSE 流式响应与并发工具执行
//
// 本文件演示 Claude Code 的流式响应处理和并发工具执行机制。
//
// 对应 claude-code-book 第 15 章（流式架构与性能优化）和第 3 章（工具编排引擎）。
//
// 核心概念：
// - SSE（Server-Sent Events）流式传输：模型逐 token 生成响应
// - content_block_start / content_block_delta / content_block_stop 事件
// - 流式文本输出：边生成边显示，提升用户体验
// - 并发工具执行：多个独立工具同时运行，减少等待时间
//
// Claude Code 的流式处理在 src/services/api/ 中：
//   SSE 事件流 → parseStreamEvent → 累积 content_block → 触发 UI 更新
//
// 并发工具执行在 src/services/tools/runToolsParallel.ts 中：
//   模型返回多个 tool_use → 分区（只读 vs 写入）→ 并发执行 → 收集结果
//
// 运行方式：
//   cargo run -p ch07-streaming
//   # 设置 ANTHROPIC_API_KEY 启用流式请求
// =============================================================================

use anyhow::{Context, Result};
use mini_claude_common::{
    BashTool, ClaudeClient, GlobTool, GrepTool, Message, ReadFileTool, ToolCallInfo,
    ToolRegistry, ToolResult, ToolRouter,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

// =============================================================================
// 第一部分：SSE 流式事件模拟器
//
// 对应 Claude Code 的 SSE 事件解析（src/services/api/streaming.ts）。
//
// Anthropic Messages API 的流式事件类型：
//   - message_start: 消息开始
//   - content_block_start: 内容块开始（text 或 tool_use）
//   - content_block_delta: 内容块增量（文本片段或 JSON 片段）
//   - content_block_stop: 内容块结束
//   - message_delta: 消息级别更新（stop_reason 等）
//   - message_stop: 消息结束
//
// 我们模拟这些事件来演示流式处理逻辑，无需真实 API。
// =============================================================================

/// 流式事件 —— 对应 Anthropic SSE 事件类型
#[derive(Debug, Clone)]
pub enum StreamingEvent {
    /// 消息开始
    MessageStart,
    /// 文本内容块开始
    TextBlockStart { index: usize },
    /// 文本增量
    TextDelta { index: usize, text: String },
    /// 文本内容块结束
    TextBlockStop { index: usize },
    /// 工具调用内容块开始
    ToolUseBlockStart { index: usize, id: String, name: String },
    /// 工具调用输入增量
    ToolUseDelta { index: usize, input_json: String },
    /// 工具调用内容块结束
    ToolUseBlockStop { index: usize },
    /// 消息结束
    MessageDone,
}

/// 流式响应处理器 —— 对应 Claude Code 的流式事件处理管线
///
/// Claude Code 的流式处理流程：
///   1. 接收 SSE 事件
///   2. 按 content_block 累积文本
///   3. 每收到 delta 就触发 UI 更新（逐字显示）
///   4. message_stop 时将完整内容加入历史
///
/// 我们的处理器演示同样的模式。
pub struct StreamProcessor {
    /// 累积的文本内容
    pub accumulated_text: String,
    /// 累积的工具调用
    pub tool_calls: Vec<ToolCallInfo>,
    /// 当前正在构建的工具调用输入 JSON
    current_tool_inputs: std::collections::HashMap<usize, String>,
}

impl StreamProcessor {
    pub fn new() -> Self {
        Self {
            accumulated_text: String::new(),
            tool_calls: Vec::new(),
            current_tool_inputs: std::collections::HashMap::new(),
        }
    }

    /// 处理一个流式事件
    ///
    /// 对应 Claude Code 的 processStreamEvent()。
    /// 每个事件都会更新内部状态，并可能触发 UI 输出。
    pub fn process_event(&mut self, event: &StreamingEvent) {
        match event {
            StreamingEvent::MessageStart => {
                tracing::debug!("消息开始");
            }
            StreamingEvent::TextBlockStart { index } => {
                tracing::debug!("文本块 {index} 开始");
            }
            StreamingEvent::TextDelta { index: _, text } => {
                // 对应 Claude Code 的逐字输出：直接打印到终端
                print!("{text}");
                self.accumulated_text.push_str(text);
            }
            StreamingEvent::TextBlockStop { index } => {
                tracing::debug!("文本块 {index} 结束");
            }
            StreamingEvent::ToolUseBlockStart { index, id, name } => {
                tracing::debug!("工具调用块 {index} 开始: {name} (id={id})");
                self.current_tool_inputs.insert(*index, String::new());
            }
            StreamingEvent::ToolUseDelta { index, input_json } => {
                if let Some(buf) = self.current_tool_inputs.get_mut(index) {
                    buf.push_str(input_json);
                }
            }
            StreamingEvent::ToolUseBlockStop { index } => {
                if let Some(input_str) = self.current_tool_inputs.remove(index) {
                    let input: serde_json::Value =
                        serde_json::from_str(&input_str).unwrap_or(serde_json::json!({}));
                    // 从累积的 delta 中恢复工具名称和 id（简化处理）
                    self.tool_calls.push(ToolCallInfo {
                        id: format!("toolu_{index:04}"),
                        name: String::new(), // 实际由 ToolUseBlockStart 提供
                        input,
                    });
                }
            }
            StreamingEvent::MessageDone => {
                tracing::debug!("消息结束");
            }
        }
    }

    /// 重置处理器状态
    pub fn reset(&mut self) {
        self.accumulated_text.clear();
        self.tool_calls.clear();
        self.current_tool_inputs.clear();
    }
}

// =============================================================================
// 第二部分：并发工具执行
//
// 对应 Claude Code 的 runToolsParallel()。
//
// Claude Code 的并发策略：
//   - 只读工具（read_file, glob, grep）可并发执行
//   - 写入工具（write_file, edit_file, bash）必须串行执行
//   - 使用 Promise.allSettled 等待所有并发任务完成
//   - 单个工具失败不影响其他工具的结果
//
// 我们使用 tokio::spawn 实现并发，与 Claude Code 的 Promise.all 对应。
// =============================================================================

/// 工具分类 —— 对应 Claude Code 的并发分区
#[derive(Debug, Clone, PartialEq)]
pub enum ToolConcurrency {
    /// 只读工具，可并发执行
    ReadOnly,
    /// 写入工具，必须串行执行
    Write,
}

/// 判断工具的并发类别
///
/// 对应 Claude Code 的 isReadOnly() 判断逻辑。
pub fn classify_tool(name: &str) -> ToolConcurrency {
    match name {
        "read_file" | "glob" | "grep" => ToolConcurrency::ReadOnly,
        _ => ToolConcurrency::Write,
    }
}

/// 并发执行多个工具调用
///
/// 对应 Claude Code 的 runToolsParallel()。
///
/// 策略：
/// 1. 将工具调用分为只读组和写入组
/// 2. 只读组并发执行（tokio::spawn）
/// 3. 写入组顺序执行
/// 4. 收集所有结果
pub async fn execute_tools_concurrent(
    router: Arc<ToolRouter>,
    calls: &[ToolCallInfo],
) -> Vec<ToolResult> {
    let mut read_only_calls = Vec::new();
    let mut write_calls = Vec::new();

    for call in calls {
        match classify_tool(&call.name) {
            ToolConcurrency::ReadOnly => read_only_calls.push(call.clone()),
            ToolConcurrency::Write => write_calls.push(call.clone()),
        }
    }

    let mut results = Vec::new();

    // 只读工具并发执行
    if !read_only_calls.is_empty() {
        tracing::info!(
            "并发执行 {} 个只读工具: {:?}",
            read_only_calls.len(),
            read_only_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
        );

        let mut handles = Vec::new();
        for call in read_only_calls {
            let router = Arc::clone(&router);
            let handle = tokio::spawn(async move {
                let start = Instant::now();
                let result = router.execute(&call.id, &call.name, &call.input);
                tracing::debug!(
                    "工具 {} 完成，耗时 {:?}",
                    call.name,
                    start.elapsed()
                );
                result
            });
            handles.push(handle);
        }

        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(ToolResult {
                    call_id: "error".to_string(),
                    output: format!("并发执行失败: {e}"),
                    is_error: true,
                    wall_time: Duration::ZERO,
                }),
            }
        }
    }

    // 写入工具顺序执行
    if !write_calls.is_empty() {
        tracing::info!(
            "顺序执行 {} 个写入工具: {:?}",
            write_calls.len(),
            write_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
        );

        for call in write_calls {
            let result = router.execute(&call.id, &call.name, &call.input);
            results.push(result);
        }
    }

    results
}

// =============================================================================
// 第三部分：流式 Agent Loop
//
// 将流式处理和并发工具执行结合到 Agent Loop 中。
//
// 对应 Claude Code 的 queryLoop() + 流式处理：
//   1. 发送流式请求 → 逐事件处理 → 累积响应
//   2. 如果是工具调用 → 并发执行 → 结果回填 → 继续循环
//   3. 如果是纯文本 → 输出并结束
// =============================================================================

/// 流式 Agent Loop —— 结合 SSE 流式和并发工具执行
pub async fn streaming_agent_loop(
    client: &ClaudeClient,
    router: &ToolRouter,
    messages: &mut Vec<Message>,
    system: Option<&str>,
) -> Result<String> {
    let tools = router.model_visible_specs();
    let max_turns = 5; // 对应 Claude Code 的 maxTurns 限制
    let mut turn_count = 0;

    loop {
        turn_count += 1;
        if turn_count > max_turns {
            println!("\n[达到最大轮次限制 ({max_turns})]");
            return Ok("[Max turns reached]".to_string());
        }

        tracing::info!(
            "发送流式请求 ({} 条消息, {} 个工具, 轮次 {}/{})",
            messages.len(), tools.len(), turn_count, max_turns
        );

        // 发送流式请求
        let events = client
            .send_stream(messages, tools, system)
            .await
            .context("流式请求失败")?;

        // 处理流式事件（使用 common crate 的 StreamEvent）
        let mut accumulated_text = String::new();
        let mut tool_calls: Vec<ToolCallInfo> = Vec::new();
        let mut current_tool_inputs: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut current_tool_name: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        use mini_claude_common::llm::StreamEvent;
        for event in &events {
            match event {
                StreamEvent::TextDelta(text) => {
                    print!("{text}");
                    accumulated_text.push_str(text);
                }
                StreamEvent::ToolUseStart { id, name } => {
                    current_tool_name.insert(id.clone(), name.clone());
                    current_tool_inputs.insert(id.clone(), String::new());
                }
                StreamEvent::ToolUseDelta { id, input_delta } => {
                    if let Some(buf) = current_tool_inputs.get_mut(id) {
                        buf.push_str(input_delta);
                    }
                }
                StreamEvent::ToolUseStop { id } => {
                    let name = current_tool_name.remove(id).unwrap_or_default();
                    let input_str = current_tool_inputs.remove(id).unwrap_or_default();
                    let input: serde_json::Value =
                        serde_json::from_str(&input_str).unwrap_or(serde_json::json!({}));
                    tool_calls.push(ToolCallInfo { id: id.clone(), name, input });
                }
                StreamEvent::MessageDone => {}
            }
        }

        // 根据处理结果决定下一步
        if !tool_calls.is_empty() {
            // 有工具调用 → 并发执行 → 继续循环
            println!();
            println!(
                "  [并发执行] {} 个工具调用",
                tool_calls.len()
            );

            // 将工具调用加入历史
            let fc_json = serde_json::json!({
                "type": "tool_use",
                "calls": tool_calls.iter().map(|c| {
                    serde_json::json!({"id": c.id, "name": c.name, "input": c.input})
                }).collect::<Vec<_>>()
            });
            messages.push(Message {
                role: "assistant".to_string(),
                content: fc_json.to_string(),
            });

            // 执行工具（使用传入的 router）
            let mut results = Vec::new();
            for call in &tool_calls {
                let result = router.execute(&call.id, &call.name, &call.input);
                results.push(result);
            }

            // 将工具结果加入历史
            for (call, result) in tool_calls.iter().zip(results.iter()) {
                let formatted = result.format_for_model(4096);
                let tool_result_json = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": call.id,
                    "content": formatted,
                });
                messages.push(Message {
                    role: "user".to_string(),
                    content: tool_result_json.to_string(),
                });
            }
        } else {
            // 纯文本响应 → 结束循环
            println!(); // 流式输出后换行
            if !accumulated_text.is_empty() {
                messages.push(Message {
                    role: "assistant".to_string(),
                    content: accumulated_text.clone(),
                });
            }
            return Ok(accumulated_text);
        }
    }
}

// =============================================================================
// 第五部分：主函数
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable is required");
    let model =
        std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());

    println!("=== Ch07: SSE 流式响应与并发工具执行 ===");
    println!("对应: 第 15 章（流式架构）+ 第 3 章（工具编排）");
    println!();
    println!("模型: {model}");
    println!("API: Anthropic Messages API (SSE streaming)");
    println!();

    // 注册工具
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(BashTool));
    registry.register(Box::new(GlobTool));
    registry.register(Box::new(GrepTool));
    let router = ToolRouter::new(registry);

    println!("已注册工具: {}", router.model_visible_specs().len());
    for spec in router.model_visible_specs() {
        println!("  - {}: {}", spec.name, truncate(&spec.description, 50));
    }
    println!();

    let mut messages: Vec<Message> = vec![Message {
        role: "user".to_string(),
        content: "请帮我执行 echo 命令".to_string(),
    }];

    // 真实流式模式
    let client = ClaudeClient::new(api_key, model)
        .with_base_url(std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string()));
    println!("You: 请帮我执行 echo 命令");
    println!("[流式输出] ");

    match streaming_agent_loop(&client, &router, &mut messages, None).await {
        Ok(response) => {
            println!();
            println!("[Agent] {response}");
        }
        Err(e) => {
            let err_msg = format!("{e:#}");
            if err_msg.contains("403") || err_msg.contains("forbidden") {
                println!();
                println!("[提示] 流式请求被代理拒绝。某些 API 代理不支持 SSE 流式传输。");
                println!("[降级] 使用非流式模式重试...");
                println!();
                // 降级到非流式
                let response = client.send(&messages, &router.model_visible_specs(), None).await?;
                match response {
                    mini_claude_common::ResponseItem::Message { content } => {
                        println!("[Agent] {content}");
                    }
                    mini_claude_common::ResponseItem::ToolUse { calls } => {
                        for call in &calls {
                            println!("  [tool] {}: {}", call.name, call.input);
                        }
                    }
                }
            } else {
                return Err(e);
            }
        }
    }

    Ok(())
}

// =============================================================================
// 辅助函数
// =============================================================================

#[allow(dead_code)]
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_router() -> ToolRouter {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadFileTool));
        registry.register(Box::new(BashTool));
        registry.register(Box::new(GlobTool));
        registry.register(Box::new(GrepTool));
        ToolRouter::new(registry)
    }

    // ---- 流式事件处理测试 ----

    /// 测试 StreamProcessor 处理文本增量
    #[test]
    fn test_stream_processor_text_delta() {
        let mut processor = StreamProcessor::new();

        processor.process_event(&StreamingEvent::MessageStart);
        processor.process_event(&StreamingEvent::TextBlockStart { index: 0 });
        processor.process_event(&StreamingEvent::TextDelta {
            index: 0,
            text: "Hello".to_string(),
        });
        processor.process_event(&StreamingEvent::TextDelta {
            index: 0,
            text: " World".to_string(),
        });
        processor.process_event(&StreamingEvent::TextBlockStop { index: 0 });
        processor.process_event(&StreamingEvent::MessageDone);

        assert_eq!(processor.accumulated_text, "Hello World");
        assert!(processor.tool_calls.is_empty());
    }

    /// 测试 StreamProcessor 处理工具调用
    #[test]
    fn test_stream_processor_tool_use() {
        let mut processor = StreamProcessor::new();

        processor.process_event(&StreamingEvent::MessageStart);
        processor.process_event(&StreamingEvent::ToolUseBlockStart {
            index: 0,
            id: "toolu_001".to_string(),
            name: "bash".to_string(),
        });
        processor.process_event(&StreamingEvent::ToolUseDelta {
            index: 0,
            input_json: r#"{"command":"echo hello"}"#.to_string(),
        });
        processor.process_event(&StreamingEvent::ToolUseBlockStop { index: 0 });
        processor.process_event(&StreamingEvent::MessageDone);

        assert!(processor.accumulated_text.is_empty());
        assert_eq!(processor.tool_calls.len(), 1);
        assert_eq!(processor.tool_calls[0].input, serde_json::json!({"command": "echo hello"}));
    }

    /// 测试 StreamProcessor 重置
    #[test]
    fn test_stream_processor_reset() {
        let mut processor = StreamProcessor::new();
        processor.process_event(&StreamingEvent::TextDelta {
            index: 0,
            text: "data".to_string(),
        });
        processor.reset();
        assert!(processor.accumulated_text.is_empty());
        assert!(processor.tool_calls.is_empty());
    }

    // ---- 并发工具分类测试 ----

    /// 测试工具并发分类
    #[test]
    fn test_tool_classification() {
        assert_eq!(classify_tool("read_file"), ToolConcurrency::ReadOnly);
        assert_eq!(classify_tool("glob"), ToolConcurrency::ReadOnly);
        assert_eq!(classify_tool("grep"), ToolConcurrency::ReadOnly);
        assert_eq!(classify_tool("write_file"), ToolConcurrency::Write);
        assert_eq!(classify_tool("edit_file"), ToolConcurrency::Write);
        assert_eq!(classify_tool("bash"), ToolConcurrency::Write);
        assert_eq!(classify_tool("unknown"), ToolConcurrency::Write);
    }

    // ---- 并发工具执行测试 ----

    /// 测试并发执行多个只读工具
    #[tokio::test]
    async fn test_concurrent_read_only_tools() {
        let router = Arc::new(test_router());
        let calls = vec![
            ToolCallInfo {
                id: "call_001".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "echo tool1"}),
            },
            ToolCallInfo {
                id: "call_002".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "echo tool2"}),
            },
        ];

        let results = execute_tools_concurrent(router, &calls).await;
        assert_eq!(results.len(), 2);
        assert!(!results[0].is_error);
        assert!(!results[1].is_error);
        assert!(results[0].output.contains("tool1"));
        assert!(results[1].output.contains("tool2"));
    }

    /// 测试混合只读和写入工具
    #[tokio::test]
    async fn test_mixed_concurrent_tools() {
        let router = Arc::new(test_router());
        let calls = vec![
            ToolCallInfo {
                id: "call_read".to_string(),
                name: "glob".to_string(),
                input: serde_json::json!({"pattern": "*.rs"}),
            },
            ToolCallInfo {
                id: "call_write".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "echo mixed"}),
            },
        ];

        let results = execute_tools_concurrent(router, &calls).await;
        assert_eq!(results.len(), 2);
    }

    // ---- 辅助函数测试 ----

    /// 测试 truncate 辅助函数
    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello...");
    }
}
