//! # 第五章：上下文管理 —— Claude Code 的记忆压缩
//!
//! 本模块演示 Claude Code 的上下文管理机制，忠实映射书中第 7 章内容。
//!
//! 对应 claude-code-book 第 7 章（上下文管理 —— 长对话的艺术）。
//!
//! 核心概念：
//! - 有效窗口：模型窗口 - min(最大输出token, 20000)
//! - 五级压缩策略：从轻量裁剪到全面压缩
//! - 断路器：防止上下文溢出的紧急机制
//! - Token 使用量追踪：实时监控上下文占用
//!
//! Claude Code 的上下文管理位于：
//!   src/services/compact/: 压缩核心
//!   src/services/compact/autoCompact.ts: 自动压缩
//!   src/services/compact/compact.ts: 压缩执行
//!
//! Codex 对比（codex-rs/core/src/compact.rs）：
//! Codex 使用 CompactionTrigger::Auto + CompactionReason 枚举控制压缩触发，
//! 支持 inline compact（本地）和 remote compact（服务端）两种模式。
//! Claude Code 的压缩全部在本地执行，Codex 支持将压缩任务发送给服务端处理。
//!
//! 运行方式：
//! ```bash
//! cargo run -p ch05-context
//! ```

use anyhow::Result;
use mini_claude_common::ContextState;

// ============================================================================
// 第一部分：有效窗口计算
//
// 对应 Claude Code 的上下文窗口计算逻辑。
//
// Claude Code 的有效窗口 = 模型窗口 - min(最大输出token, 20000)
// 例如：
//   claude-sonnet-4: 200K 窗口, 8K 输出 → 有效窗口 = 200K - 8K = 192K
//   claude-haiku:    200K 窗口, 4K 输出 → 有效窗口 = 200K - 4K = 196K
//
// 留出输出空间是为了确保模型有足够的 token 生成回复。
// ============================================================================

/// 模型配置
#[derive(Debug, Clone)]
struct ModelConfig {
    name: String,
    model_window: usize,
    max_output_tokens: usize,
}

/// 演示有效窗口计算
fn demo_effective_window() {
    println!("=== 有效窗口计算 ===");
    println!();
    println!("有效窗口 = 模型窗口 - min(最大输出token, 20000)");
    println!();

    let models = vec![
        ModelConfig {
            name: "claude-sonnet-4".to_string(),
            model_window: 200_000,
            max_output_tokens: 8192,
        },
        ModelConfig {
            name: "claude-haiku".to_string(),
            model_window: 200_000,
            max_output_tokens: 4096,
        },
        ModelConfig {
            name: "claude-opus".to_string(),
            model_window: 200_000,
            max_output_tokens: 4096,
        },
        // 模拟小窗口场景
        ModelConfig {
            name: "test-small-window".to_string(),
            model_window: 10_000,
            max_output_tokens: 4096,
        },
    ];

    println!("{:<25} {:<15} {:<15} {:<15}", "模型", "模型窗口", "最大输出", "有效窗口");
    println!("{}", "-".repeat(70));

    for model in &models {
        let state = ContextState::new(model.model_window, model.max_output_tokens);
        println!(
            "{:<25} {:<15} {:<15} {:<15}",
            model.name, model.model_window, model.max_output_tokens, state.effective_window
        );
    }
    println!();
}

// ============================================================================
// 第二部分：五级压缩策略
//
// 对应 Claude Code 的上下文压缩机制。
//
// Claude Code 使用渐进式压缩策略：
//   级别 0: 无压缩（使用率 < 50%）
//   级别 1: 裁剪旧工具输出（使用率 50-65%）
//   级别 2: 压缩工具结果详情（使用率 65-75%）
//   级别 3: 摘要早期对话（使用率 75-85%）
//   级别 4: 全面压缩（使用率 > 85%）
//
// 每一级压缩都会丢弃一些信息，但保留对话的连贯性。
// ============================================================================

/// 压缩级别
#[derive(Debug, Clone, PartialEq)]
enum CompressionLevel {
    None,
    TrimOldToolOutputs,
    CompressToolDetails,
    SummarizeEarly,
    FullCompress,
}

impl CompressionLevel {
    fn description(&self) -> &str {
        match self {
            Self::None => "无压缩（使用率 < 50%）",
            Self::TrimOldToolOutputs => "裁剪旧工具输出（使用率 50-65%）",
            Self::CompressToolDetails => "压缩工具结果详情（使用率 65-75%）",
            Self::SummarizeEarly => "摘要早期对话（使用率 75-85%）",
            Self::FullCompress => "全面压缩（使用率 > 85%）",
        }
    }
}

/// 根据使用率确定压缩级别
fn compression_level(usage_ratio: f64) -> CompressionLevel {
    if usage_ratio < 0.50 {
        CompressionLevel::None
    } else if usage_ratio < 0.65 {
        CompressionLevel::TrimOldToolOutputs
    } else if usage_ratio < 0.75 {
        CompressionLevel::CompressToolDetails
    } else if usage_ratio < 0.85 {
        CompressionLevel::SummarizeEarly
    } else {
        CompressionLevel::FullCompress
    }
}

/// 模拟压缩操作
///
/// 对应 Claude Code 的压缩实现。
/// 返回压缩后节省的 token 数量（模拟值）。
fn apply_compression(messages: &mut Vec<CompactMessage>, level: &CompressionLevel) -> usize {
    let before = messages.iter().map(|m| m.estimated_tokens).sum::<usize>();

    match level {
        CompressionLevel::None => 0,
        CompressionLevel::TrimOldToolOutputs => {
            // 裁剪前半部分消息中的工具输出
            let mid = messages.len() / 2;
            for msg in &mut messages[..mid] {
                if msg.role == "tool" && msg.estimated_tokens > 100 {
                    msg.content = "[工具输出已裁剪]".to_string();
                    msg.estimated_tokens = 20;
                }
            }
            before - messages.iter().map(|m| m.estimated_tokens).sum::<usize>()
        }
        CompressionLevel::CompressToolDetails => {
            // 压缩所有工具输出到摘要
            for msg in messages.iter_mut() {
                if msg.role == "tool" {
                    msg.content = format!("[工具结果摘要: {} 字符]", msg.content.len());
                    msg.estimated_tokens = 15;
                }
            }
            before - messages.iter().map(|m| m.estimated_tokens).sum::<usize>()
        }
        CompressionLevel::SummarizeEarly => {
            // 摘要前半部分对话
            let mid = messages.len() / 2;
            let summary_tokens = 50;
            let removed: Vec<CompactMessage> = messages.drain(..mid).collect();
            let removed_tokens: usize = removed.iter().map(|m| m.estimated_tokens).sum();
            messages.insert(0, CompactMessage {
                role: "system".to_string(),
                content: format!("[以下是 {} 条早期消息的摘要]", removed.len()),
                estimated_tokens: summary_tokens,
            });
            removed_tokens - summary_tokens
        }
        CompressionLevel::FullCompress => {
            // 全面压缩：只保留最近的几条消息
            let keep = 3;
            let removed_count = messages.len().saturating_sub(keep);
            let removed_tokens: usize = messages.iter().take(removed_count).map(|m| m.estimated_tokens).sum();
            let summary = CompactMessage {
                role: "system".to_string(),
                content: format!("[对话历史已压缩，省略 {} 条消息]", removed_count),
                estimated_tokens: 40,
            };
            messages.drain(..removed_count);
            messages.insert(0, summary);
            removed_tokens - 40
        }
    }
}

/// 紧凑消息类型（用于上下文管理演示）
#[derive(Debug, Clone)]
struct CompactMessage {
    role: String,
    content: String,
    estimated_tokens: usize,
}

// ============================================================================
// 第三部分：断路器机制
//
// 对应 Claude Code 的 CircuitBreaker。
//
// 断路器是一种紧急机制，当上下文使用率过高时触发：
//   - 正常状态 (Closed): 正常处理请求
//   - 触发状态 (Open): 拒绝新请求，强制压缩
//   - 恢复状态 (HalfOpen): 尝试恢复正常
//
// 断路器的触发条件：
//   - 使用率 > 90%: 触发断路器
//   - 压缩后使用率 < 70%: 恢复正常
// ============================================================================

/// 断路器状态
#[derive(Debug, Clone, PartialEq)]
enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// 断路器
///
/// 对应 Claude Code 的 CircuitBreaker 实现。
struct CircuitBreaker {
    state: CircuitState,
    open_threshold: f64,
    close_threshold: f64,
    trip_count: usize,
}

impl CircuitBreaker {
    fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            open_threshold: 0.90,
            close_threshold: 0.70,
            trip_count: 0,
        }
    }

    /// 检查使用率并更新断路器状态
    fn check(&mut self, usage_ratio: f64) -> &CircuitState {
        match self.state {
            CircuitState::Closed => {
                if usage_ratio > self.open_threshold {
                    self.state = CircuitState::Open;
                    self.trip_count += 1;
                    tracing::warn!("断路器触发! 使用率 {:.1}%", usage_ratio * 100.0);
                }
            }
            CircuitState::Open => {
                if usage_ratio < self.close_threshold {
                    self.state = CircuitState::HalfOpen;
                    tracing::info!("断路器进入半开状态");
                }
            }
            CircuitState::HalfOpen => {
                if usage_ratio < self.close_threshold {
                    self.state = CircuitState::Closed;
                    tracing::info!("断路器恢复正常");
                } else if usage_ratio > self.open_threshold {
                    self.state = CircuitState::Open;
                    self.trip_count += 1;
                }
            }
        }
        &self.state
    }

    fn is_open(&self) -> bool {
        self.state == CircuitState::Open
    }
}

// ============================================================================
// 第四部分：完整上下文管理流程演示
//
// 将所有组件组合，演示完整的上下文管理流程。
// 模拟一个长对话场景，观察压缩级别和断路器的变化。
// ============================================================================

/// 模拟对话的 token 估算
///
/// 简化估算：每个字符约 0.3 个 token（英文），中文约 1 token/字符
fn estimate_tokens(text: &str) -> usize {
    let chinese_chars = text.chars().filter(|c| *c as u32 > 0x4E00).count();
    let other_chars = text.len() - chinese_chars;
    chinese_chars + (other_chars as f64 * 0.3) as usize
}

/// 演示完整的上下文管理流程
fn demo_full_context_flow() {
    println!("=== 完整上下文管理流程 ===");
    println!();

    // 模拟一个 10K token 的小窗口（方便演示压缩）
    let mut state = ContextState::new(10_000, 2_048);
    let mut messages: Vec<CompactMessage> = Vec::new();
    let mut circuit = CircuitBreaker::new();

    // 模拟系统提示
    messages.push(CompactMessage {
        role: "system".to_string(),
        content: "你是一个有用的 AI 助手。".to_string(),
        estimated_tokens: estimate_tokens("你是一个有用的 AI 助手。"),
    });

    println!("初始状态:");
    println!("  有效窗口: {} tokens", state.effective_window);
    println!("  当前使用: {} tokens", state.current_tokens);
    println!();

    // 模拟多轮对话
    let user_inputs = vec![
        "请帮我查看当前目录的文件",
        "读取 README.md 的内容",
        "解释一下这个项目的架构",
        "帮我修改 main.rs 中的函数",
        "运行测试看看是否通过",
        "再帮我写一些测试用例",
        "检查一下有没有性能问题",
        "优化一下这个循环的性能",
        "把结果写入 REPORT.md",
        "最后总结一下我们做了什么",
    ];

    for (i, input) in user_inputs.iter().enumerate() {
        // 用户消息
        let user_tokens = estimate_tokens(input);
        messages.push(CompactMessage {
            role: "user".to_string(),
            content: input.to_string(),
            estimated_tokens: user_tokens,
        });
        state.current_tokens += user_tokens;

        // 模拟助手回复（较长）
        let response = format!(
            "好的，我来帮你处理：{input}。[模拟的详细回复内容，包含多个步骤和工具调用结果...]"
        );
        let response_tokens = estimate_tokens(&response);
        messages.push(CompactMessage {
            role: "assistant".to_string(),
            content: response,
            estimated_tokens: response_tokens,
        });
        state.current_tokens += response_tokens;

        // 模拟工具输出（可能很长）
        let tool_output = format!("[模拟的工具输出结果 #{} - 包含大量文件内容和命令输出...]", i + 1);
        let tool_tokens = 200 + i * 50; // 递增的 token 数
        messages.push(CompactMessage {
            role: "tool".to_string(),
            content: tool_output,
            estimated_tokens: tool_tokens,
        });
        state.current_tokens += tool_tokens;

        state.turn_count += 1;

        // 检查是否需要压缩
        let ratio = state.usage_ratio();
        let level = compression_level(ratio);
        let circuit_state = circuit.check(ratio);

        if i % 3 == 2 {
            println!(
                "第 {} 轮 | 使用率: {:.1}% | 压缩级别: {:?} | 断路器: {:?} | 消息数: {}",
                i + 1,
                ratio * 100.0,
                level,
                circuit_state,
                messages.len()
            );

            // 执行压缩
            if level != CompressionLevel::None {
                let saved = apply_compression(&mut messages, &level);
                state.current_tokens = state.current_tokens.saturating_sub(saved);
                println!("  压缩完成，节省 {} tokens，当前使用: {} tokens", saved, state.current_tokens);

                // 重新检查断路器
                let new_ratio = state.usage_ratio();
                circuit.check(new_ratio);
            }
        }
    }

    println!();
    println!("最终状态:");
    println!("  消息数: {}", messages.len());
    println!("  Token 使用: {} / {} ({:.1}%)", state.current_tokens, state.effective_window, state.usage_ratio() * 100.0);
    println!("  断路器触发次数: {}", circuit.trip_count);
    println!();
}

// ============================================================================
// 第五部分：Mock 模式演示
//
// 使用固定数据演示上下文管理的关键行为。
// ============================================================================

/// Mock 演示：压缩级别阈值
fn demo_compression_thresholds() {
    println!("=== 压缩级别阈值 ===");
    println!();

    let thresholds = [
        (0.0, "0%"),
        (0.30, "30%"),
        (0.50, "50%"),
        (0.60, "60%"),
        (0.70, "70%"),
        (0.80, "80%"),
        (0.90, "90%"),
        (0.95, "95%"),
    ];

    println!("{:<15} {:<30}", "使用率", "压缩级别");
    println!("{}", "-".repeat(45));
    for (ratio, label) in &thresholds {
        let level = compression_level(*ratio);
        println!("{label:<15} {}", level.description());
    }
    println!();
}

/// Mock 演示：断路器状态转换
fn demo_circuit_breaker() {
    println!("=== 断路器状态转换 ===");
    println!();

    let mut cb = CircuitBreaker::new();
    println!("初始状态: {:?}", cb.state);

    // 模拟使用率变化序列
    let usage_sequence = vec![
        (0.50, "正常请求"),
        (0.70, "中等负载"),
        (0.85, "高负载"),
        (0.92, "超过阈值 → 触发断路器"),
        (0.95, "断路器已打开，拒绝新请求"),
        (0.60, "使用率下降 → 恢复尝试"),
        (0.50, "恢复正常"),
    ];

    for (ratio, desc) in &usage_sequence {
        let state = cb.check(*ratio).clone();
        let is_open = cb.is_open();
        println!(
            "  使用率 {:.0}% ({}) → 断路器: {:?}{}",
            ratio * 100.0,
            desc,
            state,
            if is_open { " [拒绝请求]" } else { "" }
        );
    }
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

    println!("=== Ch05: 上下文管理 ===");
    println!("对应 claude-code-book 第 7 章");
    println!();

    demo_effective_window();
    demo_compression_thresholds();
    demo_circuit_breaker();
    demo_full_context_flow();

    Ok(())
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 有效窗口测试 ----

    #[test]
    fn test_effective_window_calculation() {
        // 200K 窗口, 8K 输出 → 有效窗口 = 200K - 8K
        let state = ContextState::new(200_000, 8192);
        assert_eq!(state.effective_window, 200_000 - 8192);
        assert_eq!(state.current_tokens, 0);
        assert_eq!(state.turn_count, 0);
    }

    #[test]
    fn test_effective_window_capped_output() {
        // 当 max_output_tokens > 20000 时，保留 20000
        let state = ContextState::new(200_000, 50_000);
        assert_eq!(state.effective_window, 200_000 - 20_000);
    }

    #[test]
    fn test_effective_window_small_output() {
        // 当 max_output_tokens < 20000 时，保留实际值
        let state = ContextState::new(100_000, 4096);
        assert_eq!(state.effective_window, 100_000 - 4096);
    }

    // ---- 使用率测试 ----

    #[test]
    fn test_usage_ratio() {
        let mut state = ContextState::new(10_000, 2_000);
        state.current_tokens = 4_000;
        let ratio = state.usage_ratio();
        // effective_window = 10000 - 2000 = 8000
        // ratio = 4000 / 8000 = 0.5
        assert!((ratio - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_should_compact() {
        let mut state = ContextState::new(10_000, 2_000);
        // effective_window = 8000
        state.current_tokens = 6_000; // 75%
        assert!(!state.should_compact());

        state.current_tokens = 7_000; // 87.5%
        assert!(state.should_compact());
    }

    // ---- 压缩级别测试 ----

    #[test]
    fn test_compression_levels() {
        assert_eq!(compression_level(0.30), CompressionLevel::None);
        assert_eq!(compression_level(0.49), CompressionLevel::None);
        assert_eq!(compression_level(0.50), CompressionLevel::TrimOldToolOutputs);
        assert_eq!(compression_level(0.64), CompressionLevel::TrimOldToolOutputs);
        assert_eq!(compression_level(0.65), CompressionLevel::CompressToolDetails);
        assert_eq!(compression_level(0.74), CompressionLevel::CompressToolDetails);
        assert_eq!(compression_level(0.75), CompressionLevel::SummarizeEarly);
        assert_eq!(compression_level(0.84), CompressionLevel::SummarizeEarly);
        assert_eq!(compression_level(0.85), CompressionLevel::FullCompress);
        assert_eq!(compression_level(0.95), CompressionLevel::FullCompress);
    }

    #[test]
    fn test_apply_compression_none() {
        let mut msgs = vec![
            CompactMessage { role: "user".to_string(), content: "hello".to_string(), estimated_tokens: 10 },
            CompactMessage { role: "assistant".to_string(), content: "hi".to_string(), estimated_tokens: 5 },
        ];
        let saved = apply_compression(&mut msgs, &CompressionLevel::None);
        assert_eq!(saved, 0);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn test_apply_compression_trim_old() {
        let mut msgs = vec![
            CompactMessage { role: "user".to_string(), content: "q1".to_string(), estimated_tokens: 10 },
            CompactMessage { role: "tool".to_string(), content: "a".repeat(500), estimated_tokens: 500 },
            CompactMessage { role: "user".to_string(), content: "q2".to_string(), estimated_tokens: 10 },
            CompactMessage { role: "tool".to_string(), content: "b".repeat(500), estimated_tokens: 500 },
        ];
        let saved = apply_compression(&mut msgs, &CompressionLevel::TrimOldToolOutputs);
        assert!(saved > 0);
        // 前半部分的 tool 消息被裁剪
        assert_eq!(msgs[1].estimated_tokens, 20);
    }

    #[test]
    fn test_apply_compression_summarize_early() {
        let mut msgs = vec![
            CompactMessage { role: "user".to_string(), content: "q1".to_string(), estimated_tokens: 100 },
            CompactMessage { role: "assistant".to_string(), content: "a1".to_string(), estimated_tokens: 200 },
            CompactMessage { role: "user".to_string(), content: "q2".to_string(), estimated_tokens: 100 },
            CompactMessage { role: "assistant".to_string(), content: "a2".to_string(), estimated_tokens: 200 },
            CompactMessage { role: "user".to_string(), content: "q3".to_string(), estimated_tokens: 100 },
            CompactMessage { role: "assistant".to_string(), content: "a3".to_string(), estimated_tokens: 200 },
        ];
        let before_len = msgs.len();
        let saved = apply_compression(&mut msgs, &CompressionLevel::SummarizeEarly);
        assert!(saved > 0);
        assert!(msgs.len() < before_len);
        assert!(msgs[0].content.contains("摘要"));
    }

    #[test]
    fn test_apply_compression_full() {
        let mut msgs = vec![
            CompactMessage { role: "user".to_string(), content: "q1".to_string(), estimated_tokens: 100 },
            CompactMessage { role: "assistant".to_string(), content: "a1".to_string(), estimated_tokens: 200 },
            CompactMessage { role: "user".to_string(), content: "q2".to_string(), estimated_tokens: 100 },
            CompactMessage { role: "assistant".to_string(), content: "a2".to_string(), estimated_tokens: 200 },
            CompactMessage { role: "user".to_string(), content: "q3".to_string(), estimated_tokens: 100 },
            CompactMessage { role: "assistant".to_string(), content: "a3".to_string(), estimated_tokens: 200 },
        ];
        let saved = apply_compression(&mut msgs, &CompressionLevel::FullCompress);
        assert!(saved > 0);
        assert!(msgs.len() <= 4); // keep=3 + 1 summary
    }

    // ---- 断路器测试 ----

    #[test]
    fn test_circuit_breaker_normal() {
        let mut cb = CircuitBreaker::new();
        let state = cb.check(0.5);
        assert_eq!(*state, CircuitState::Closed);
        assert!(!cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_trip() {
        let mut cb = CircuitBreaker::new();
        let state = cb.check(0.95);
        assert_eq!(*state, CircuitState::Open);
        assert!(cb.is_open());
        assert_eq!(cb.trip_count, 1);
    }

    #[test]
    fn test_circuit_breaker_recovery() {
        let mut cb = CircuitBreaker::new();
        cb.check(0.95); // Open
        assert_eq!(cb.state, CircuitState::Open);

        let state = cb.check(0.60); // HalfOpen
        assert_eq!(*state, CircuitState::HalfOpen);

        let state = cb.check(0.50); // Closed
        assert_eq!(*state, CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_retrip() {
        let mut cb = CircuitBreaker::new();
        cb.check(0.95); // Open
        cb.check(0.60); // HalfOpen
        cb.check(0.95); // Open again
        assert_eq!(cb.state, CircuitState::Open);
        assert_eq!(cb.trip_count, 2);
    }

    // ---- token 估算测试 ----

    #[test]
    fn test_estimate_tokens() {
        // 英文文本
        let tokens = estimate_tokens("hello world");
        assert!(tokens > 0 && tokens < 10);

        // 中文文本
        let tokens = estimate_tokens("你好世界");
        assert!(tokens >= 4 && tokens <= 8);
    }

    // ---- 压缩级别描述测试 ----

    #[test]
    fn test_compression_level_descriptions() {
        assert!(CompressionLevel::None.description().contains("无压缩"));
        assert!(CompressionLevel::TrimOldToolOutputs.description().contains("裁剪"));
        assert!(CompressionLevel::CompressToolDetails.description().contains("压缩"));
        assert!(CompressionLevel::SummarizeEarly.description().contains("摘要"));
        assert!(CompressionLevel::FullCompress.description().contains("全面"));
    }
}
