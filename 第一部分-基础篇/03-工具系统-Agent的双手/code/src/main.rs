//! # 第三章：工具系统 —— Claude Code 的执行引擎
//!
//! 本模块演示 Claude Code 的工具系统架构，忠实映射书中第 3 章内容。
//!
//! 对应 claude-code-book 第 3 章（工具系统 —— 连接世界的桥梁）。
//!
//! 核心概念：
//! - Tool trait：工具接口定义（name / spec / execute 三要素）
//! - ToolRegistry：工具注册中心，管理所有可用工具
//! - ToolRouter：工具路由器，将模型的工具调用分发到正确处理器
//! - 并发安全：Send + Sync 约束确保工具可在并发场景安全使用
//! - 内置工具：BashTool / ReadFileTool / WriteFileTool / EditFileTool / GlobTool / GrepTool
//!
//! Claude Code 的工具系统位于：
//!   src/Tool.ts: 工具接口定义
//!   src/tools/: 66+ 内置工具
//!   src/services/tools/: 工具编排引擎
//!
//! 运行方式：
//! ```bash
//! cargo run -p ch03-tool-system
//! ```

use anyhow::Result;
use mini_claude_common::{
    BashTool, EditFileTool, GlobTool, GrepTool, ReadFileTool,
    ToolCallInfo, ToolHandler, ToolRegistry, ToolResult, ToolRouter, ToolSpec,
    WriteFileTool,
};

// ============================================================================
// 第一部分：自定义工具 —— 实现 ToolHandler trait
//
// 对应 Claude Code src/Tool.ts 的 Tool 接口定义。
// 每个工具必须实现三个方法：
//   - name(): 工具名称（唯一标识）
//   - spec(): 工具规范（名称 + 描述 + input_schema）
//   - execute(): 执行逻辑（接收 call_id 和 input，返回 ToolResult）
//
// Claude Code 还有 isReadOnly / isConcurrencySafe / renderToolUseMessage 等方法，
// 我们简化为核心三要素。
// ============================================================================

/// 计算器工具 —— 演示如何实现自定义 ToolHandler
///
/// 对应 Claude Code 的工具接口实现模式。
/// 该工具支持加减乘除四则运算。
struct CalculatorTool;

impl ToolHandler for CalculatorTool {
    fn name(&self) -> &str {
        "calculator"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "calculator".to_string(),
            description: "执行数学运算。支持加减乘除。".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "description": "运算类型: add / subtract / multiply / divide",
                        "enum": ["add", "subtract", "multiply", "divide"]
                    },
                    "a": { "type": "number", "description": "第一个操作数" },
                    "b": { "type": "number", "description": "第二个操作数" }
                },
                "required": ["operation", "a", "b"]
            }),
        }
    }

    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult {
        let op = input.get("operation").and_then(|v| v.as_str()).unwrap_or("");
        let a = input.get("a").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = input.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let start = std::time::Instant::now();

        let result = match op {
            "add" => Ok(a + b),
            "subtract" => Ok(a - b),
            "multiply" => Ok(a * b),
            "divide" => {
                if b == 0.0 {
                    Err("除数不能为零".to_string())
                } else {
                    Ok(a / b)
                }
            }
            _ => Err(format!("未知运算: {op}")),
        };

        match result {
            Ok(value) => ToolResult {
                call_id: call_id.to_string(),
                output: format!("{value}"),
                is_error: false,
                wall_time: start.elapsed(),
            },
            Err(msg) => ToolResult {
                call_id: call_id.to_string(),
                output: msg,
                is_error: true,
                wall_time: start.elapsed(),
            },
        }
    }
}

/// 统计工具 —— 演示并发安全的工具实现
///
/// Claude Code 中每个工具必须实现 Send + Sync，
/// 这意味着工具可以在多线程环境中安全使用。
/// 该 trait 约束在 ToolHandler 定义中自动要求：
///   pub trait ToolHandler: Send + Sync
struct WordCountTool;

impl ToolHandler for WordCountTool {
    fn name(&self) -> &str {
        "word_count"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "word_count".to_string(),
            description: "统计文本的字符数、单词数和行数。".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "要统计的文本" }
                },
                "required": ["text"]
            }),
        }
    }

    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult {
        let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let chars = text.chars().count();
        let words = text.split_whitespace().count();
        let lines = text.lines().count();
        let start = std::time::Instant::now();

        ToolResult {
            call_id: call_id.to_string(),
            output: format!("字符数: {chars}, 单词数: {words}, 行数: {lines}"),
            is_error: false,
            wall_time: start.elapsed(),
        }
    }
}

// ============================================================================
// 第二部分：工具注册表演示 —— ToolRegistry
//
// 对应 Claude Code src/tools.ts 的 getAllBaseTools()。
// ToolRegistry 是一个 HashMap<String, Box<dyn ToolHandler>>，
// 提供 register / get / all_specs 三个核心方法。
// ============================================================================

/// 演示工具注册表的使用
fn demo_registry() {
    println!("=== 工具注册表 (ToolRegistry) ===");
    println!();

    // 创建注册表并注册自定义工具
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CalculatorTool));
    registry.register(Box::new(WordCountTool));

    // 也注册内置工具
    registry.register(Box::new(BashTool));
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(EditFileTool));
    registry.register(Box::new(GlobTool));
    registry.register(Box::new(GrepTool));

    println!("已注册 {} 个工具:", registry.len());
    for spec in registry.all_specs() {
        println!("  - {}: {}", spec.name, truncate(&spec.description, 50));
    }
    println!();

    // 通过名称获取工具
    if let Some(calc) = registry.get("calculator") {
        let result = calc.execute("call_001", &serde_json::json!({"operation": "add", "a": 10, "b": 32}));
        println!("calculator(add, 10, 32) = {}", result.output);
    }
    println!();
}

// ============================================================================
// 第三部分：工具路由器演示 —— ToolRouter
//
// 对应 Claude Code 的 runTools() 工具编排引擎。
// ToolRouter 负责将模型的工具调用分发到正确的处理器。
//
// Claude Code 中 ToolRouter 还负责：
//   - 并发分区（concurrency-safe 工具并行执行）
//   - 流式执行
//   - 权限检查集成
//
// 我们的实现简化为顺序执行。
// ============================================================================

/// 演示工具路由器的分发机制
fn demo_router() {
    println!("=== 工具路由器 (ToolRouter) ===");
    println!();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CalculatorTool));
    registry.register(Box::new(WordCountTool));
    registry.register(Box::new(BashTool));
    let router = ToolRouter::new(registry);

    // 模型可见的工具规范（发送给 API 的 tools 参数）
    println!("模型可见工具 (model_visible_specs):");
    for spec in router.model_visible_specs() {
        println!("  [{}] {}", spec.name, truncate(&spec.description, 40));
    }
    println!();

    // 模拟模型返回的工具调用
    let mock_calls = vec![
        ToolCallInfo {
            id: "call_001".to_string(),
            name: "calculator".to_string(),
            input: serde_json::json!({"operation": "multiply", "a": 6, "b": 7}),
        },
        ToolCallInfo {
            id: "call_002".to_string(),
            name: "word_count".to_string(),
            input: serde_json::json!({"text": "Hello Claude Code"}),
        },
        ToolCallInfo {
            id: "call_003".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "echo 'tool routing works!'"}),
        },
        ToolCallInfo {
            id: "call_004".to_string(),
            name: "nonexistent_tool".to_string(),
            input: serde_json::json!({}),
        },
    ];

    // 逐个分发工具调用
    for call in &mock_calls {
        println!("  [调用] {} (id: {})", call.name, call.id);
        let result = router.execute(&call.id, &call.name, &call.input);
        if result.is_error {
            println!("  [错误] {}", truncate(&result.output, 100));
        } else {
            println!("  [结果] {}", truncate(&result.output, 100));
        }
        println!();
    }
}

// ============================================================================
// 第四部分：内置工具演示
//
// 对应 Claude Code 的 66+ 内置工具。
// common crate 提供了 6 个核心工具：
//   - BashTool: 执行 shell 命令
//   - ReadFileTool: 读取文件内容（带行号）
//   - WriteFileTool: 写入文件
//   - EditFileTool: 搜索替换编辑
//   - GlobTool: 文件模式匹配
//   - GrepTool: 内容搜索
// ============================================================================

/// 演示内置工具的使用
fn demo_builtin_tools() {
    println!("=== 内置工具演示 ===");
    println!();

    // BashTool
    let bash = BashTool;
    let result = bash.execute("c1", &serde_json::json!({"command": "echo 'Hello from BashTool'"}));
    println!("[BashTool] {}", result.output.trim());

    // GlobTool
    let glob = GlobTool;
    let result = glob.execute("c2", &serde_json::json!({"pattern": "*.toml", "path": "."}));
    println!("[GlobTool] 找到文件:");
    for line in result.output.lines().take(5) {
        println!("  {line}");
    }
    if result.output.lines().count() > 5 {
        println!("  ...");
    }
    println!();

    // ToolSpec 的 API 格式（发送给 Anthropic API 的 tools 参数）
    let bash_spec = BashTool.spec();
    println!("[ToolSpec.to_api_format()] bash 工具的 API 定义:");
    println!("  {}", serde_json::to_string_pretty(&bash_spec.to_api_format()).unwrap_or_default());
    println!();
}

// ============================================================================
// 第五部分：并发安全验证
//
// 对应 Claude Code 的 isConcurrencySafe 属性。
// ToolHandler trait 要求 Send + Sync，确保工具可在并发场景安全使用。
// Claude Code 中工具被分为两类：
//   - concurrency-safe: 可并行执行（如 ReadFileTool、GlobTool）
//   - non-concurrency-safe: 需顺序执行（如 WriteFileTool、BashTool）
//
// Rust 的类型系统在编译期强制执行这一约束。
// ============================================================================

/// 验证工具的并发安全性
///
/// 由于 ToolHandler 要求 Send + Sync，
/// 编译器会自动验证所有工具满足并发安全条件。
fn verify_concurrency_safety() {
    println!("=== 并发安全验证 ===");
    println!();

    // 验证所有内置工具满足 Send + Sync
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BashTool>();
    assert_send_sync::<ReadFileTool>();
    assert_send_sync::<WriteFileTool>();
    assert_send_sync::<EditFileTool>();
    assert_send_sync::<GlobTool>();
    assert_send_sync::<GrepTool>();
    assert_send_sync::<CalculatorTool>();
    assert_send_sync::<WordCountTool>();

    println!("所有工具均满足 Send + Sync 约束（编译期验证）");
    println!("Claude Code 中 concurrency-safe 工具可并行执行:");
    println!("  - read_file (只读，可并行)");
    println!("  - glob (只读，可并行)");
    println!("  - grep (只读，可并行)");
    println!("  - bash (写入，需顺序)");
    println!("  - write_file (写入，需顺序)");
    println!("  - edit_file (写入，需顺序)");
    println!();
}

// ============================================================================
// 主函数
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    println!("=== Ch03: 工具系统 ===");
    println!("对应 claude-code-book 第 3 章");
    println!();

    demo_registry();
    demo_router();
    demo_builtin_tools();
    verify_concurrency_safety();

    Ok(())
}

#[allow(dead_code)]
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_router() -> ToolRouter {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(CalculatorTool));
        registry.register(Box::new(WordCountTool));
        registry.register(Box::new(BashTool));
        registry.register(Box::new(ReadFileTool));
        ToolRouter::new(registry)
    }

    #[test]
    fn test_tool_handler_trait() {
        let calc = CalculatorTool;
        assert_eq!(calc.name(), "calculator");

        let spec = calc.spec();
        assert_eq!(spec.name, "calculator");
        assert!(spec.description.contains("数学"));

        let result = calc.execute("c1", &serde_json::json!({"operation": "add", "a": 1, "b": 2}));
        assert!(!result.is_error);
        assert_eq!(result.output, "3");
        assert_eq!(result.call_id, "c1");
    }

    #[test]
    fn test_calculator_operations() {
        let calc = CalculatorTool;

        let r = calc.execute("c", &serde_json::json!({"operation": "subtract", "a": 10, "b": 3}));
        assert_eq!(r.output, "7");

        let r = calc.execute("c", &serde_json::json!({"operation": "multiply", "a": 6, "b": 7}));
        assert_eq!(r.output, "42");

        let r = calc.execute("c", &serde_json::json!({"operation": "divide", "a": 10, "b": 4}));
        assert_eq!(r.output, "2.5");

        let r = calc.execute("c", &serde_json::json!({"operation": "divide", "a": 1, "b": 0}));
        assert!(r.is_error);
        assert!(r.output.contains("零"));
    }

    #[test]
    fn test_calculator_unknown_op() {
        let calc = CalculatorTool;
        let r = calc.execute("c", &serde_json::json!({"operation": "power", "a": 2, "b": 3}));
        assert!(r.is_error);
        assert!(r.output.contains("未知"));
    }

    #[test]
    fn test_word_count_tool() {
        let wc = WordCountTool;
        let r = wc.execute("c", &serde_json::json!({"text": "Hello Claude Code"}));
        assert!(!r.is_error);
        assert!(r.output.contains("字符数: 17"));
        assert!(r.output.contains("单词数: 3"));
        assert!(r.output.contains("行数: 1"));
    }

    #[test]
    fn test_word_count_multiline() {
        let wc = WordCountTool;
        let r = wc.execute("c", &serde_json::json!({"text": "line1\nline2\nline3"}));
        assert!(!r.is_error);
        assert!(r.output.contains("行数: 3"));
    }

    #[test]
    fn test_tool_registry() {
        let mut registry = ToolRegistry::new();
        assert_eq!(registry.len(), 0);

        registry.register(Box::new(CalculatorTool));
        registry.register(Box::new(WordCountTool));
        assert_eq!(registry.len(), 2);

        assert!(registry.get("calculator").is_some());
        assert!(registry.get("word_count").is_some());
        assert!(registry.get("nonexistent").is_none());

        let specs = registry.all_specs();
        assert_eq!(specs.len(), 2);
    }

    #[test]
    fn test_tool_router_dispatch() {
        let router = test_router();

        // 正常分发
        let r = router.execute("c1", "calculator", &serde_json::json!({"operation": "add", "a": 1, "b": 1}));
        assert!(!r.is_error);
        assert_eq!(r.output, "2");

        // 未知工具
        let r = router.execute("c2", "nonexistent", &serde_json::json!({}));
        assert!(r.is_error);
        assert!(r.output.contains("Unknown tool"));
    }

    #[test]
    fn test_tool_spec_api_format() {
        let spec = CalculatorTool.spec();
        let api = spec.to_api_format();
        assert_eq!(api["name"], "calculator");
        assert!(api["description"].as_str().unwrap().contains("数学"));
        assert!(api["input_schema"]["properties"]["operation"].is_object());
    }

    #[test]
    fn test_tool_result_format_for_model() {
        let r = ToolResult {
            call_id: "c".to_string(),
            output: "42".to_string(),
            is_error: false,
            wall_time: Duration::from_millis(100),
        };
        let formatted = r.format_for_model(4096);
        assert!(formatted.contains("Exit code: 0"));
        assert!(formatted.contains("Wall time:"));
        assert!(formatted.contains("42"));
    }

    #[test]
    fn test_concurrency_safety_compile_check() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BashTool>();
        assert_send_sync::<ReadFileTool>();
        assert_send_sync::<WriteFileTool>();
        assert_send_sync::<EditFileTool>();
        assert_send_sync::<GlobTool>();
        assert_send_sync::<GrepTool>();
        assert_send_sync::<CalculatorTool>();
        assert_send_sync::<WordCountTool>();
    }

}
