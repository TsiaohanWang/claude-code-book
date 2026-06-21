//! # 第一章：与模型对话 —— 构建 Claude API 客户端
//!
//! 本模块演示如何使用 Anthropic Messages API 与 Claude 模型对话。
//!
//! 对应 claude-code-book 第 0 章（预备知识）和第 1 章（新范式）。
//!
//! 核心概念：
//! - Anthropic Messages API（/v1/messages 端点）
//! - 消息格式：{ role, content }
//! - 流式响应（SSE）
//! - 工具定义（tool_use）
//!
//! 运行方式：
//! ```bash
//! export ANTHROPIC_API_KEY="sk-ant-..."
//! cargo run -p ch01-llm-client -- "What is the capital of France?"
//! ```

use anyhow::Result;
use mini_claude_common::ClaudeClient;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: cargo run -p ch01-llm-client -- \"your question\"");
        eprintln!();
        eprintln!("Environment:");
        eprintln!("  ANTHROPIC_API_KEY - Anthropic API Key (required)");
        eprintln!("  ANTHROPIC_BASE_URL - Custom API URL (optional)");
        std::process::exit(1);
    }

    let prompt = &args[1];
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY not set. Run: export ANTHROPIC_API_KEY=\"sk-ant-...\"");
    let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());

    let mut client = ClaudeClient::new(api_key, model);
    if let Ok(base_url) = std::env::var("ANTHROPIC_BASE_URL") {
        client = client.with_base_url(base_url);
    }

    // 发送非流式请求
    info!("Sending request to Claude API...");
    let messages = vec![mini_claude_common::Message {
        role: "user".to_string(),
        content: prompt.to_string(),
    }];

    let response = client.send(&messages, &[], None).await?;

    match response {
        mini_claude_common::ResponseItem::Message { content } => {
            println!("\n=== Claude Response ===\n");
            println!("{}", content);
        }
        mini_claude_common::ResponseItem::ToolUse { calls } => {
            println!("\n=== Tool Calls ===\n");
            for call in &calls {
                println!("Tool: {}", call.name);
                println!("Input: {}", serde_json::to_string_pretty(&call.input)?);
            }
        }
    }

    Ok(())
}
