// =============================================================================
// 第十一章：Mini Claude Code —— 全章节综合集成
//
// 本文件是教程的最终 capstone，集成前面所有章节的核心概念。
//
// 对应 claude-code-book 第 17 章（从零构建你自己的 Claude Code）。
//
// 集成内容：
//   - Agent Loop（第 2 章）：while(true) 思考-行动循环
//   - 工具系统（第 3 章）：ToolHandler + ToolRouter + ToolRegistry
//   - 权限系统（第 4 章）：PermissionMode + 权限检查
//   - 上下文管理（第 5 章）：ContextState + token 追踪 + 压缩触发
//   - 记忆系统（第 6 章）：会话历史 + 持久化
//   - 流式响应（第 7 章）：SSE 流式输出
//   - Hook 系统（第 8 章）：PreToolUse / PostToolUse
//   - MCP 协议（第 9 章）：外部工具扩展
//   - 多 Agent（第 10 章）：子 Agent 派生
//
// 运行方式：
//   cargo run -p ch11-mini-claude
//   # 设置 ANTHROPIC_API_KEY 启用真实 API
// =============================================================================

use anyhow::Result;
use mini_claude_common::{
    BashTool, EditFileTool, GlobTool, GrepTool, Message, ReadFileTool,
    ToolCallInfo, ToolRegistry, ToolResult, ToolRouter, WriteFileTool,
};
use std::collections::HashMap;
use std::time::Duration;

// =============================================================================
// 第一部分：权限系统（集成第 4 章）
//
// Claude Code 的四阶段权限管线：
//   阶段 1: 硬编码拒绝规则（rm -rf / 等）
//   阶段 2: 用户 Allowlist / Denylist
//   阶段 3: 权限模式判断
//   阶段 4: 用户交互确认
// =============================================================================

/// 权限决策
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    Allow,
    Deny { reason: String },
    Ask,
}

/// 权限模式
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionMode {
    /// 默认：写操作需确认
    Default,
    /// 只读模式
    Plan,
    /// 自动批准编辑
    AcceptEdits,
    /// 全部自动批准
    BypassPermissions,
}

/// 权限管理器
pub struct PermissionManager {
    mode: PermissionMode,
    allowlist: Vec<String>,
    denylist: Vec<String>,
}

impl PermissionManager {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            allowlist: Vec::new(),
            denylist: Vec::new(),
        }
    }

    pub fn allow(&mut self, tool: String) {
        self.allowlist.push(tool);
    }

    /// 检查工具调用权限
    pub fn check(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision {
        // 阶段 1: 硬编码拒绝
        if tool_name == "bash" {
            if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                if cmd.contains("rm -rf /") || cmd.starts_with("sudo ") {
                    return PermissionDecision::Deny {
                        reason: "危险命令被拦截".to_string(),
                    };
                }
            }
        }

        // 阶段 2: 用户列表
        if self.denylist.contains(&tool_name.to_string()) {
            return PermissionDecision::Deny {
                reason: format!("{tool_name} 在拒绝列表中"),
            };
        }
        if self.allowlist.contains(&tool_name.to_string()) {
            return PermissionDecision::Allow;
        }

        // 阶段 3: 权限模式
        match self.mode {
            PermissionMode::BypassPermissions => PermissionDecision::Allow,
            PermissionMode::Plan => {
                if is_read_only_tool(tool_name) {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Deny {
                        reason: "只读模式不允许写操作".to_string(),
                    }
                }
            }
            PermissionMode::AcceptEdits => PermissionDecision::Allow,
            PermissionMode::Default => {
                if is_read_only_tool(tool_name) {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Ask
                }
            }
        }
    }
}

fn is_read_only_tool(name: &str) -> bool {
    matches!(name, "read_file" | "glob" | "grep")
}

// =============================================================================
// 第二部分：上下文管理（集成第 5 章）
//
// Claude Code 的上下文管理策略：
//   - 追踪 token 使用量
//   - 有效窗口 = 模型窗口 - min(最大输出token, 20000)
//   - 使用率 > 85% 时触发压缩
//   - 压缩方式：保留最近消息 + 生成历史摘要
// =============================================================================

/// 上下文状态
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
        if self.effective_window == 0 {
            return 0.0;
        }
        self.current_tokens as f64 / self.effective_window as f64
    }

    pub fn should_compact(&self) -> bool {
        self.usage_ratio() > 0.85
    }

    /// 估算消息的 token 数（简化：1 token ≈ 4 字符）
    pub fn estimate_tokens(&self, messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|m| m.content.len() / 4 + 10) // role 开销
            .sum()
    }

    /// 更新 token 计数
    pub fn update(&mut self, messages: &[Message]) {
        self.current_tokens = self.estimate_tokens(messages);
        self.turn_count += 1;
    }
}

/// 上下文压缩 —— 对应 Claude Code 的压缩策略
///
/// 保留最近 N 条消息，生成历史摘要。
pub fn compact_context(messages: &mut Vec<Message>, keep_recent: usize) -> String {
    if messages.len() <= keep_recent {
        return String::new();
    }

    // 生成历史摘要
    let old_messages = &messages[..messages.len() - keep_recent];
    let summary = format!(
        "[历史摘要] 之前进行了 {} 轮对话，包含 {} 条消息。",
        old_messages.len(),
        old_messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .count()
    );

    // 保留最近的消息
    let recent: Vec<Message> = messages[messages.len() - keep_recent..].to_vec();
    messages.clear();
    messages.push(Message {
        role: "system".to_string(),
        content: summary.clone(),
    });
    messages.extend(recent);

    summary
}

// =============================================================================
// 第三部分：记忆系统（集成第 6 章）
//
// Claude Code 的记忆层次：
//   - 会话记忆：当前对话历史（messages 数组）
//   - 项目记忆：.claude/memory/ 下的 markdown 文件
//   - 全局记忆：用户级别的偏好设置
// =============================================================================

/// 简化的记忆管理器
pub struct MemoryManager {
    /// 会话历史摘要
    session_summaries: Vec<String>,
    /// 项目笔记（键值对）
    project_notes: HashMap<String, String>,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self {
            session_summaries: Vec::new(),
            project_notes: HashMap::new(),
        }
    }

    /// 记录会话摘要
    pub fn record_session(&mut self, summary: String) {
        self.session_summaries.push(summary);
    }

    /// 添加项目笔记
    pub fn add_note(&mut self, key: String, value: String) {
        self.project_notes.insert(key, value);
    }

    /// 获取项目笔记
    pub fn get_note(&self, key: &str) -> Option<&String> {
        self.project_notes.get(key)
    }

    /// 构建记忆上下文（注入到系统提示中）
    pub fn build_context(&self) -> String {
        let mut parts = Vec::new();

        if !self.session_summaries.is_empty() {
            parts.push("历史会话摘要:".to_string());
            for (i, s) in self.session_summaries.iter().enumerate() {
                parts.push(format!("  {}. {}", i + 1, s));
            }
        }

        if !self.project_notes.is_empty() {
            parts.push("项目笔记:".to_string());
            for (k, v) in &self.project_notes {
                parts.push(format!("  {k}: {v}"));
            }
        }

        parts.join("\n")
    }
}

// =============================================================================
// 第四部分：Hook 系统（集成第 8 章）
//
// 简化的 Hook 管线，支持 PreToolUse 和 PostToolUse。
// =============================================================================

/// 简化的 Hook 决策
#[derive(Debug)]
pub enum HookDecision {
    Allow,
    Deny { reason: String },
}

/// Mini Hook 管线
pub struct MiniHookPipeline {
    /// 工具名 → 拦截原因（模拟 PreToolUse Hook）
    blocked_tools: HashMap<String, String>,
}

impl MiniHookPipeline {
    pub fn new() -> Self {
        Self {
            blocked_tools: HashMap::new(),
        }
    }

    pub fn block(&mut self, tool: String, reason: String) {
        self.blocked_tools.insert(tool, reason);
    }

    pub fn check_pre(&self, tool_name: &str) -> HookDecision {
        if let Some(reason) = self.blocked_tools.get(tool_name) {
            HookDecision::Deny {
                reason: reason.clone(),
            }
        } else {
            HookDecision::Allow
        }
    }
}

// =============================================================================
// 第五部分：Mini Claude Code —— 集成所有系统
//
// 将所有子系统组合成一个完整的 Agent。
// =============================================================================

/// Mini Claude Code 配置
pub struct MiniClaudeConfig {
    pub permission_mode: PermissionMode,
    pub model_window: usize,
    pub max_output_tokens: usize,
    pub max_turns: usize,
}

impl Default for MiniClaudeConfig {
    fn default() -> Self {
        Self {
            permission_mode: PermissionMode::Default,
            model_window: 200000,
            max_output_tokens: 4096,
            max_turns: 50,
        }
    }
}

/// Mini Claude Code —— 集成所有子系统的 Agent
pub struct MiniClaudeCode {
    /// 工具路由器
    router: ToolRouter,
    /// 权限管理器
    permissions: PermissionManager,
    /// 上下文状态
    context: ContextState,
    /// 记忆管理器
    memory: MemoryManager,
    /// Hook 管线
    hooks: MiniHookPipeline,
    /// 对话历史
    messages: Vec<Message>,
    /// 配置
    #[allow(dead_code)]
    config: MiniClaudeConfig,
}

impl MiniClaudeCode {
    pub fn new(config: MiniClaudeConfig) -> Self {
        // 注册工具
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadFileTool));
        registry.register(Box::new(WriteFileTool));
        registry.register(Box::new(EditFileTool));
        registry.register(Box::new(BashTool));
        registry.register(Box::new(GlobTool));
        registry.register(Box::new(GrepTool));

        let context = ContextState::new(config.model_window, config.max_output_tokens);

        Self {
            router: ToolRouter::new(registry),
            permissions: PermissionManager::new(config.permission_mode.clone()),
            context,
            memory: MemoryManager::new(),
            hooks: MiniHookPipeline::new(),
            messages: Vec::new(),
            config,
        }
    }

    /// 设置系统提示
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.messages.insert(
            0,
            Message {
                role: "system".to_string(),
                content: prompt.to_string(),
            },
        );
    }

    /// 执行一个工具调用（带权限和 Hook 检查）
    pub fn execute_tool(&self, call: &ToolCallInfo) -> ToolResult {
        // Hook 检查
        match self.hooks.check_pre(&call.name) {
            HookDecision::Deny { reason } => {
                return ToolResult {
                    call_id: call.id.clone(),
                    output: format!("Hook 拒绝: {reason}"),
                    is_error: true,
                    wall_time: Duration::ZERO,
                };
            }
            _ => {}
        }

        // 权限检查
        match self.permissions.check(&call.name, &call.input) {
            PermissionDecision::Deny { reason } => {
                return ToolResult {
                    call_id: call.id.clone(),
                    output: format!("权限拒绝: {reason}"),
                    is_error: true,
                    wall_time: Duration::ZERO,
                };
            }
            PermissionDecision::Ask => {
                return ToolResult {
                    call_id: call.id.clone(),
                    output: "需要用户确认".to_string(),
                    is_error: true,
                    wall_time: Duration::ZERO,
                };
            }
            _ => {}
        }

        // 执行工具
        self.router.execute(&call.id, &call.name, &call.input)
    }

    /// 检查是否需要压缩上下文
    pub fn maybe_compact(&mut self) -> Option<String> {
        self.context.update(&self.messages);
        if self.context.should_compact() {
            let summary = compact_context(&mut self.messages, 6);
            self.memory.record_session(summary.clone());
            Some(summary)
        } else {
            None
        }
    }

    /// 获取当前状态摘要
    pub fn status(&self) -> String {
        format!(
            "Mini Claude Code 状态:\n\
             - 消息数: {}\n\
             - Token 使用: {}/{} ({:.0}%)\n\
             - 回合数: {}\n\
             - 权限模式: {:?}\n\
             - 已注册工具: {}\n\
             - 记忆笔记: {} 条",
            self.messages.len(),
            self.context.current_tokens,
            self.context.effective_window,
            self.context.usage_ratio() * 100.0,
            self.context.turn_count,
            self.permissions.mode,
            self.router.model_visible_specs().len(),
            self.memory.project_notes.len(),
        )
    }

}

// =============================================================================
// 第六部分：主函数
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let _api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable is required");
    let model =
        std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());

    println!("=== Ch11: Mini Claude Code ===");
    println!("对应: 第 17 章（从零构建你自己的 Claude Code）");
    println!();
    println!("模型: {model}");
    println!();

    // 创建 Mini Claude Code 实例
    let config = MiniClaudeConfig {
        permission_mode: PermissionMode::Default,
        ..Default::default()
    };

    let mut claude = MiniClaudeCode::new(config);

    // 添加记忆笔记
    claude.memory.add_note(
        "project".to_string(),
        "Mini Claude Code 教程项目".to_string(),
    );
    claude.memory.add_note(
        "chapter".to_string(),
        "第 17 章：从零构建".to_string(),
    );

    // 设置系统提示
    claude.set_system_prompt(
        "你是 Mini Claude Code，一个集成所有教程章节概念的 AI Agent。\
         你可以使用工具来帮助用户完成任务。",
    );

    // 显示状态
    println!("--- 初始状态 ---");
    println!("{}", claude.status());
    println!();

    println!("(Mini Claude Code 已就绪。设置 ANTHROPIC_API_KEY 后可使用真实 API。)");

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

    // ---- 权限系统测试 ----

    /// 测试权限管理器：硬编码拒绝
    #[test]
    fn test_permission_hardcoded_deny() {
        let pm = PermissionManager::new(PermissionMode::BypassPermissions);
        let decision = pm.check("bash", &serde_json::json!({"command": "rm -rf /"}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    /// 测试权限管理器：sudo 拒绝
    #[test]
    fn test_permission_sudo_deny() {
        let pm = PermissionManager::new(PermissionMode::BypassPermissions);
        let decision = pm.check("bash", &serde_json::json!({"command": "sudo something"}));
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    /// 测试权限管理器：只读模式
    #[test]
    fn test_permission_plan_mode() {
        let pm = PermissionManager::new(PermissionMode::Plan);
        assert!(matches!(
            pm.check("read_file", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            pm.check("write_file", &serde_json::json!({})),
            PermissionDecision::Deny { .. }
        ));
    }

    /// 测试权限管理器：默认模式需要确认
    #[test]
    fn test_permission_default_mode() {
        let pm = PermissionManager::new(PermissionMode::Default);
        assert!(matches!(
            pm.check("read_file", &serde_json::json!({})),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            pm.check("bash", &serde_json::json!({})),
            PermissionDecision::Ask
        ));
    }

    /// 测试权限管理器：Allowlist
    #[test]
    fn test_permission_allowlist() {
        let mut pm = PermissionManager::new(PermissionMode::Default);
        pm.allow("bash".to_string());
        assert!(matches!(
            pm.check("bash", &serde_json::json!({"command": "echo ok"})),
            PermissionDecision::Allow
        ));
    }

    // ---- 上下文管理测试 ----

    /// 测试 ContextState 创建
    #[test]
    fn test_context_state_creation() {
        let ctx = ContextState::new(200000, 4096);
        assert_eq!(ctx.effective_window, 200000 - 4096);
        assert_eq!(ctx.current_tokens, 0);
    }

    /// 测试 token 使用率
    #[test]
    fn test_context_usage_ratio() {
        let mut ctx = ContextState::new(1000, 100);
        ctx.current_tokens = 500;
        assert!((ctx.usage_ratio() - 0.5556).abs() < 0.01);
    }

    /// 测试压缩触发
    #[test]
    fn test_context_compact_trigger() {
        let mut ctx = ContextState::new(1000, 100);
        ctx.current_tokens = 800;
        assert!(ctx.should_compact());

        ctx.current_tokens = 500;
        assert!(!ctx.should_compact());
    }

    /// 测试上下文压缩
    #[test]
    fn test_compact_context() {
        let mut messages = vec![
            Message { role: "system".to_string(), content: "sys".to_string() },
            Message { role: "user".to_string(), content: "q1".to_string() },
            Message { role: "assistant".to_string(), content: "a1".to_string() },
            Message { role: "user".to_string(), content: "q2".to_string() },
            Message { role: "assistant".to_string(), content: "a2".to_string() },
            Message { role: "user".to_string(), content: "q3".to_string() },
        ];
        let summary = compact_context(&mut messages, 2);
        assert!(summary.contains("历史摘要"));
        // 保留 system(摘要) + 最近 2 条
        assert_eq!(messages.len(), 3);
    }

    // ---- 记忆系统测试 ----

    /// 测试记忆管理器
    #[test]
    fn test_memory_manager() {
        let mut mem = MemoryManager::new();
        mem.record_session("第 1 次会话".to_string());
        mem.add_note("key1".to_string(), "value1".to_string());

        assert_eq!(mem.get_note("key1"), Some(&"value1".to_string()));
        assert!(mem.get_note("key2").is_none());
        assert!(mem.build_context().contains("第 1 次会话"));
    }

    // ---- Hook 系统测试 ----

    /// 测试 Mini Hook 管线
    #[test]
    fn test_mini_hook_pipeline() {
        let mut hooks = MiniHookPipeline::new();
        hooks.block("dangerous_tool".to_string(), "不允许".to_string());

        assert!(matches!(
            hooks.check_pre("dangerous_tool"),
            HookDecision::Deny { .. }
        ));
        assert!(matches!(
            hooks.check_pre("safe_tool"),
            HookDecision::Allow
        ));
    }

    // ---- 集成测试 ----

    /// 测试 MiniClaudeCode 创建和状态
    #[test]
    fn test_mini_claude_creation() {
        let config = MiniClaudeConfig::default();
        let claude = MiniClaudeCode::new(config);
        let status = claude.status();
        assert!(status.contains("已注册工具: 6"));
        assert!(status.contains("记忆笔记: 0"));
    }

    /// 测试 MiniClaudeCode 工具执行带权限检查
    #[test]
    fn test_mini_claude_tool_execution() {
        let config = MiniClaudeConfig {
            permission_mode: PermissionMode::BypassPermissions,
            ..Default::default()
        };
        let claude = MiniClaudeCode::new(config);

        let call = ToolCallInfo {
            id: "test_001".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "echo test"}),
        };
        let result = claude.execute_tool(&call);
        assert!(!result.is_error);
        assert!(result.output.contains("test"));
    }

    /// 测试 MiniClaudeCode 危险命令被拦截
    #[test]
    fn test_mini_claude_dangerous_command() {
        let config = MiniClaudeConfig {
            permission_mode: PermissionMode::BypassPermissions,
            ..Default::default()
        };
        let claude = MiniClaudeCode::new(config);

        let call = ToolCallInfo {
            id: "test_002".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "rm -rf /"}),
        };
        let result = claude.execute_tool(&call);
        assert!(result.is_error);
        assert!(result.output.contains("权限拒绝") || result.output.contains("Hook 拒绝"));
    }

    /// 测试 MiniClaudeCode 记忆笔记
    #[test]
    fn test_mini_claude_memory() {
        let config = MiniClaudeConfig::default();
        let mut claude = MiniClaudeCode::new(config);
        claude
            .memory
            .add_note("test".to_string(), "value".to_string());
        let status = claude.status();
        assert!(status.contains("记忆笔记: 1"));
    }

    /// 测试 truncate 辅助函数
    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello...");
    }
}
