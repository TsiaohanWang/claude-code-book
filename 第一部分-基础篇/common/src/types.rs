//! 核心类型定义 —— 对应 Claude Code 的消息和工具类型系统
//!
//! Claude Code 使用 Anthropic Messages API，消息格式为：
//! - role: "user" | "assistant" | "system"
//! - content: 可以是字符串或内容块数组（text, tool_use, tool_result）
//!
//! 与 Codex 的 Responses API 不同，Claude Code 的 tool_use 和 tool_result
//! 是消息内容块的一部分，而非独立的消息类型。

use serde::{Deserialize, Serialize};

// ============================================================================
// 消息类型 —— 对应 Claude Code 的消息系统
// ============================================================================

/// 对话消息 —— 对应 Anthropic Messages API 的 MessageParam
///
/// Claude Code 中消息存储在 `messages` 数组中，每条消息有：
/// - role: "user" | "assistant" | "system"
/// - content: 字符串或内容块数组
///
/// 我们简化为字符串内容，高级版本支持内容块。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

// ============================================================================
// 响应类型 —— 对应 Claude Code 的响应解析
// ============================================================================

/// 模型响应 —— 对应 Claude Code 从 API 响应中解析出的结构
///
/// Claude Code 的响应解析在 src/services/api/ 中，
/// 将 SSE 流事件转换为结构化的响应类型。
#[derive(Debug)]
pub enum ResponseItem {
    /// 模型返回了文本消息 —— 回合结束
    Message { content: String },
    /// 模型请求执行工具 —— 需要继续循环
    ToolUse { calls: Vec<ToolCallInfo> },
}

/// 工具调用信息 —— 对应 Anthropic API 的 tool_use 内容块
///
/// Claude Code 中每个 tool_use 块包含：
/// - id: 唯一标识符，用于将 tool_result 关联回这次调用
/// - name: 工具名称
/// - input: 工具输入参数（JSON 对象）
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

// ============================================================================
// 工具相关类型 —— 对应 Claude Code 的 Tool 接口
// ============================================================================

/// 工具规范 —— 对应 Claude Code 的工具定义
///
/// Claude Code 中每个工具通过 `Tool<Input, Output, Progress>` 接口定义，
/// 包含五要素：名称、Schema、权限、执行、UI 渲染。
/// 我们简化为核心字段。
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl ToolSpec {
    /// 转换为 Anthropic API 格式
    ///
    /// Anthropic API 的工具格式：
    /// { "name": "...", "description": "...", "input_schema": {...} }
    pub fn to_api_format(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.input_schema,
        })
    }
}

/// 工具执行结果 —— 对应 Claude Code 的 ToolResult
///
/// Claude Code 中工具结果通过 tool_result 内容块返回给模型，
/// 包含 output（字符串）和 is_error（布尔值）。
#[derive(Debug)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub is_error: bool,
    pub wall_time: std::time::Duration,
}

impl ToolResult {
    /// 格式化为模型可读的输出
    ///
    /// 对应 Claude Code 中工具结果的格式化逻辑：
    /// - 退出码
    /// - 执行时间
    /// - 输出内容（可能被截断）
    pub fn format_for_model(&self, max_bytes: usize) -> String {
        let duration_secs = (self.wall_time.as_secs_f32() * 10.0).round() / 10.0;
        let content = &self.output;
        let total_lines = content.lines().count();
        let truncated = content.len() > max_bytes;

        let content = if truncated {
            &content[..max_bytes.min(content.len())]
        } else {
            content
        };

        let mut sections = Vec::new();
        sections.push(format!("Exit code: {}", if self.is_error { 1 } else { 0 }));
        sections.push(format!("Wall time: {duration_secs} seconds"));
        if truncated {
            sections.push(format!("Total output lines: {total_lines}"));
        }
        sections.push("Output:".to_string());
        sections.push(content.to_string());

        sections.join("\n")
    }
}

// ============================================================================
// 权限相关类型 —— 对应 Claude Code 的权限系统
// ============================================================================

/// 权限决策 —— 对应 Claude Code 的权限管线输出
///
/// Claude Code 的四阶段权限管线最终产出三种决策：
/// - Allow: 允许执行
/// - Deny: 拒绝执行
/// - Ask: 需要用户确认
///
/// Codex 对比（codex-rs/execpolicy/src/decision.rs）：
/// - Allow → 允许执行（对应 Claude Code 的 Allow）
/// - Prompt → 需要用户确认（对应 Claude Code 的 Ask）
/// - Forbidden → 直接拒绝（对应 Claude Code 的 Deny）
///
/// 命名差异源于两个系统的不同设计理念：
/// - Claude Code 以用户交互为中心（Ask = 询问用户）
/// - Codex 以策略评估为中心（Prompt = 提示用户）
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

/// 权限模式 —— 对应 Claude Code 的六种权限模式
///
/// Claude Code 定义了六种权限模式（官方文档 code.claude.com/docs/en/permissions）：
/// - default: 无匹配规则时交互确认（标准行为）
/// - acceptEdits: 自动批准文件编辑和常见文件系统命令
/// - plan: 只读模式，Claude 可读取文件和运行只读命令但不编辑源文件
/// - auto: 自动批准工具调用，后台安全检查验证操作是否符合请求（研究预览）
/// - dontAsk: 无匹配规则时自动拒绝
/// - bypassPermissions: 跳过权限提示，但 deny 规则和 rm -rf / 仍生效
///
/// Codex 对比：Codex 使用 AskForApproval 枚举控制审批策略，
/// 包含 Never / OnFailure / Always / UnlessAllowListed 等选项。
#[derive(Debug, Clone)]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    Auto,
    DontAsk,
    BypassPermissions,
}

// ============================================================================
// 上下文相关类型 —— 对应 Claude Code 的上下文管理
// ============================================================================

/// 上下文状态 —— 对应 Claude Code 的上下文追踪
///
/// Claude Code 追踪 token 使用量以决定何时触发压缩。
/// 有效窗口 = 模型窗口 - min(最大输出token, 20000)
#[derive(Debug, Clone)]
pub struct ContextState {
    pub effective_window: usize,
    pub current_tokens: usize,
    pub turn_count: usize,
}

impl ContextState {
    pub fn new(model_window: usize, max_output_tokens: usize) -> Self {
        let reserved = max_output_tokens.min(20000);
        Self {
            effective_window: model_window - reserved,
            current_tokens: 0,
            turn_count: 0,
        }
    }

    pub fn usage_ratio(&self) -> f64 {
        self.current_tokens as f64 / self.effective_window as f64
    }

    pub fn should_compact(&self) -> bool {
        self.usage_ratio() > 0.85
    }
}
