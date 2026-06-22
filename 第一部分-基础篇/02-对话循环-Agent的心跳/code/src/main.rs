//! # 第二章：Agent 的心脏 —— 对话循环
//!
//! 本模块实现了 Claude Code 的核心机制：对话循环（Agent Loop）。
//!
//! 对应 claude-code-book 第 2 章（对话循环 — Agent 的心跳）。
//!
//! 核心概念：
//! - while(true) 循环：调用 LLM → 执行工具 → 结果回填 → 继续
//! - 依赖注入：通过参数注入 LLM 客户端，便于测试
//! - 不可变状态：每次循环创建新的消息历史
//! - 终止条件：模型返回纯文本（无工具调用）时结束
//!
//! Claude Code 的对话循环位于：src/query.ts (queryLoop)
//! Codex 对比：codex-rs/core/src/session/turn.rs (run_turn)
//!
//! 关键差异：
//! - Claude Code 使用 AsyncGenerator 流式产出事件
//! - Codex 使用事件驱动模型，通过 sess.send_event() 发送事件
//! - 两者核心逻辑一致：loop { call_model → execute_tools → continue/break }
//!
//! 运行方式：
//! ```bash
//! export ANTHROPIC_API_KEY="sk-ant-..."
//! cargo run -p ch02-agent-loop -- "your question"
//! ```

use anyhow::{Context, Result};
use mini_claude_common::{
    BashTool, ClaudeClient, EditFileTool, GlobTool, GrepTool, Message, ReadFileTool,
    ResponseItem, ToolRegistry, ToolRouter, WriteFileTool,
};
use std::io::{self, Write};
use std::time::Duration;

// ============================================================================
// 重试逻辑 —— 对应 Claude Code src/agent.ts 的 withRetry
// ============================================================================

/// 带指数退避的重试
///
/// 对应 Claude Code src/agent.ts:36-61 的重试逻辑。
/// 对 429 (rate limit)、503 (service unavailable)、529 (overloaded) 进行自动重试。
async fn retry_with_backoff<F, Fut, T>(max_retries: u32, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut retries = 0;
    loop {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if retries < max_retries && is_retryable(&e) => {
                retries += 1;
                let delay_ms = std::cmp::min(1000 * 2u64.pow(retries), 30000);
                tracing::warn!("Retry {retries}/{max_retries} after {delay_ms}ms: {e}");
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_retryable(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("429") || msg.contains("503") || msg.contains("529")
        || msg.contains("overloaded") || msg.contains("ECONNRESET")
}

// ============================================================================
// Agent Loop —— 对应 Claude Code 的 queryLoop()
// ============================================================================

/// Agent 的核心循环
///
/// 对应 claude-code-book 第 2 章的 while(true) 循环。
///
/// 循环逻辑（与 Claude Code 一致）：
/// 1. 构建消息历史 + 工具定义
/// 2. 发送给模型（Anthropic Messages API）
/// 3. 解析响应
///    - ToolUse → 执行工具 → 结果加入历史 → 回到步骤 2
///    - Message → 记录消息 → 回合结束
///
/// 关键设计：
/// - 状态不可变：每次循环创建新的消息向量
/// - 依赖注入：client 和 router 通过参数传入
/// - 终止原因追踪：返回终止原因字符串
pub async fn agent_loop(
    client: &ClaudeClient,
    router: &ToolRouter,
    messages: &mut Vec<Message>,
    system: Option<&str>,
) -> Result<String> {
    let tools = router.model_visible_specs();

    loop {
        tracing::info!("Sending request ({} messages, {} tools)", messages.len(), tools.len());

        let response = retry_with_backoff(3, || {
            client.send(messages, tools, system)
        })
        .await
        .context("Failed to send LLM request")?;

        match response {
            ResponseItem::ToolUse { calls } => {
                // 将工具调用加入历史（assistant 角色）
                let fc_json = serde_json::json!({
                    "type": "tool_use",
                    "calls": calls.iter().map(|c| {
                        serde_json::json!({
                            "id": c.id,
                            "name": c.name,
                            "input": c.input,
                        })
                    }).collect::<Vec<_>>()
                });
                messages.push(Message {
                    role: "assistant".to_string(),
                    content: fc_json.to_string(),
                });

                // 执行每个工具调用
                for call in &calls {
                    println!("  [tool] {}: {}", call.name, truncate(&call.input.to_string(), 100));

                    let result = router.execute(&call.id, &call.name, &call.input);

                    let output_display = truncate(&result.output, 200);
                    if result.is_error {
                        println!("  [error] {output_display}");
                    } else {
                        println!("  [result] {output_display}");
                    }

                    // 将工具结果加入历史（user 角色，tool_result 内容块）
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
                // 回到循环顶部，将工具结果发回给模型
            }
            ResponseItem::Message { content } => {
                // 终止条件：模型返回纯文本，无工具调用
                if !content.is_empty() {
                    messages.push(Message {
                        role: "assistant".to_string(),
                        content: content.clone(),
                    });
                }
                return Ok(content);
            }
        }
    }
}

// ============================================================================
// 主函数 —— 交互式 REPL
// ============================================================================

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
    let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());

    println!("=== Ch02: Agent Loop ===");
    println!("Model: {model}");
    println!();

    // 注册工具 —— 对应 Claude Code 的 getAllBaseTools()
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(EditFileTool));
    registry.register(Box::new(BashTool));
    registry.register(Box::new(GlobTool));
    registry.register(Box::new(GrepTool));
    let router = ToolRouter::new(registry);

    println!("Registered tools: {}", router.model_visible_specs().len());
    for spec in router.model_visible_specs() {
        println!("  - {}: {}", spec.name, truncate(&spec.description, 60));
    }
    println!();

    // 非交互模式：通过命令行参数传入 prompt
    let cli_prompt = std::env::args().skip(1).collect::<Vec<_>>().join(" ");

    let client = ClaudeClient::new(api_key, model)
        .with_base_url(std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string()));

    // 非交互模式：命令行传入 prompt
    if !cli_prompt.is_empty() {
        let mut messages: Vec<Message> = vec![Message {
            role: "user".to_string(),
            content: cli_prompt.clone(),
        }];
        println!("You: {cli_prompt}");
        println!("[thinking...]");
        match agent_loop(&client, &router, &mut messages, None).await {
            Ok(response) => {
                println!();
                println!("[Agent] {response}");
            }
            Err(e) => {
                println!();
                println!("[Error] {e:#}");
            }
        }
        return Ok(());
    }

    // 交互式 REPL
    let mut messages: Vec<Message> = Vec::new();

    println!("[Agent] Hello! I'm your AI assistant. Type a message to start, 'quit' to exit.");
    println!();

    loop {
        print!("You: ");
        io::stdout().flush()?;

        let mut input = String::new();
        let bytes_read = io::stdin().read_line(&mut input)?;
        if bytes_read == 0 { break; } // EOF
        let input = input.trim();

        if input.is_empty() { continue; }
        if input == "quit" || input == "exit" {
            println!("Goodbye!");
            break;
        }

        messages.push(Message {
            role: "user".to_string(),
            content: input.to_string(),
        });

        println!("[thinking...]");

        match agent_loop(&client, &router, &mut messages, None).await {
            Ok(response) => {
                println!();
                println!("[Agent] {response}");
            }
            Err(e) => {
                println!();
                println!("[Error] {e:#}");
            }
        }
        println!();
    }

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
