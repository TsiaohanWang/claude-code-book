// =============================================================================
// 第八章：Hook 系统 —— 工具执行的拦截与增强
//
// 本文件实现 Claude Code 的 Hook 系统，在工具执行前后注入自定义逻辑。
//
// 对应 claude-code-book 第 9 章（Hook 系统 —— 可扩展的执行管线）。
//
// 核心概念：
// - 5 种 Hook 类型：PreToolUse / PostToolUse / Notification / Stop / SubagentStop
// - Glob 模式匹配：对特定工具名应用 Hook
// - Hook 决策：Allow / Deny / Modify（修改输入或输出）
// - 多 Hook 链式执行：按优先级依次执行，任一 Deny 即拒绝
//
// Claude Code 的 Hook 系统在 src/hooks/ 中：
//   config.yaml 定义 hooks → PreToolUse hook 匹配工具名 → 执行脚本 → 解析决策
//
// 运行方式：
//   cargo run -p ch08-hooks
// =============================================================================

use anyhow::Result;
use mini_claude_common::{
    BashTool, EditFileTool, GlobTool, GrepTool, ReadFileTool, ToolCallInfo,
    ToolRegistry, ToolResult, ToolRouter, WriteFileTool,
};
use std::time::Duration;

// =============================================================================
// 第一部分：Hook 类型定义
//
// 对应 Claude Code 的 src/hooks/types.ts。
//
// Claude Code 定义了 5 种 Hook 类型：
//   - PreToolUse: 工具执行前（可拦截、修改输入）
//   - PostToolUse: 工具执行后（可修改输出、添加日志）
//   - Notification: 模型返回消息时（用于通知、日志）
//   - Stop: Agent 循环结束时（清理、统计）
//   - SubagentStop: 子 Agent 结束时（收集结果）
// =============================================================================

/// Hook 类型 —— 对应 Claude Code 的 5 种 Hook
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HookType {
    /// 工具执行前
    PreToolUse,
    /// 工具执行后
    PostToolUse,
    /// 消息通知
    Notification,
    /// Agent 循环停止
    Stop,
    /// 子 Agent 停止
    SubagentStop,
}

/// Hook 决策 —— 对应 Claude Code 的 Hook 执行结果
///
/// Claude Code 中 Hook 可以：
/// - Allow: 允许工具继续执行
/// - Deny: 拒绝工具执行（返回错误给模型）
/// - Modify: 修改工具输入或输出
#[derive(Debug, Clone)]
pub enum HookDecision {
    /// 允许执行
    Allow,
    /// 拒绝执行（附带拒绝原因）
    Deny { reason: String },
    /// 修改输入（PreToolUse 专用）
    ModifyInput { new_input: serde_json::Value },
    /// 修改输出（PostToolUse 专用）
    ModifyOutput { new_output: String },
}

/// Hook 上下文 —— 传递给 Hook 的执行上下文
#[derive(Debug, Clone)]
pub struct HookContext {
    /// 当前工具名称
    pub tool_name: String,
    /// 工具调用 ID
    pub call_id: String,
    /// 工具输入参数
    pub input: serde_json::Value,
    /// 工具输出（PostToolUse 时有值）
    pub output: Option<String>,
    /// 是否有错误
    pub is_error: bool,
}

/// Hook trait —— 对应 Claude Code 的 Hook 接口
///
/// Claude Code 中 Hook 通过配置文件定义，可以是：
/// - 命令行脚本（shell 命令）
/// - JavaScript 函数
/// - 内置 Hook
///
/// 我们用 Rust trait 实现，更类型安全。
pub trait Hook: Send + Sync {
    /// Hook 名称
    fn name(&self) -> &str;

    /// Hook 类型
    fn hook_type(&self) -> HookType;

    /// Glob 匹配模式 —— 对工具名进行匹配
    ///
    /// Claude Code 使用 glob 模式匹配工具名：
    /// - "bash" 精确匹配
    /// - "file_*" 匹配 file_read, file_write 等
    /// - "*" 匹配所有工具
    fn pattern(&self) -> &str;

    /// 执行 Hook
    fn execute(&self, ctx: &HookContext) -> HookDecision;

    /// 检查工具名是否匹配 pattern
    fn matches(&self, tool_name: &str) -> bool {
        let pat = self.pattern();
        if pat == "*" {
            return true;
        }
        if pat.contains('*') || pat.contains('?') {
            return glob_match(pat, tool_name);
        }
        pat == tool_name
    }
}

/// 简单的 glob 匹配实现
///
/// 对应 Claude Code 使用的 minimatch 库。
/// 支持 *（匹配任意字符）和 ?（匹配单个字符）。
fn glob_match(pattern: &str, text: &str) -> bool {
    let regex_pattern = pattern
        .replace('.', r"\.")
        .replace('*', ".*")
        .replace('?', ".");
    let regex_pattern = format!("^{regex_pattern}$");
    regex::Regex::new(&regex_pattern)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

// =============================================================================
// 第二部分：内置 Hook 实现
//
// 对应 Claude Code 的内置安全 Hook。
// =============================================================================

/// 安全 Hook：拦截危险的 bash 命令
///
/// 对应 Claude Code 的安全检查：
/// - 拦截 rm -rf /
/// - 拦截 sudo 命令
/// - 拦截格式化磁盘命令
pub struct SafetyHook;

impl Hook for SafetyHook {
    fn name(&self) -> &str { "safety_check" }
    fn hook_type(&self) -> HookType { HookType::PreToolUse }
    fn pattern(&self) -> &str { "bash" }

    fn execute(&self, ctx: &HookContext) -> HookDecision {
        let cmd = ctx
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 检查危险命令
        if cmd.contains("rm -rf /") || cmd.contains("rm -rf /*") {
            return HookDecision::Deny {
                reason: "危险命令: rm -rf / 被拦截".to_string(),
            };
        }
        if cmd.starts_with("sudo ") {
            return HookDecision::Deny {
                reason: "不允许使用 sudo 命令".to_string(),
            };
        }
        if cmd.contains("mkfs") || cmd.contains("dd if=") {
            return HookDecision::Deny {
                reason: "危险命令: 磁盘操作被拦截".to_string(),
            };
        }

        HookDecision::Allow
    }
}

/// 日志 Hook：记录所有工具调用
///
/// 对应 Claude Code 的遥测和日志系统。
pub struct LoggingHook;

impl Hook for LoggingHook {
    fn name(&self) -> &str { "logging" }
    fn hook_type(&self) -> HookType { HookType::PostToolUse }
    fn pattern(&self) -> &str { "*" }

    fn execute(&self, ctx: &HookContext) -> HookDecision {
        let status = if ctx.is_error { "ERROR" } else { "OK" };
        tracing::info!(
            "[Hook 日志] {}({}) = {} (call_id: {})",
            ctx.tool_name,
            truncate(&ctx.input.to_string(), 50),
            status,
            ctx.call_id
        );
        HookDecision::Allow
    }
}

/// 输入增强 Hook：为 bash 命令添加安全前缀
///
/// 演示 PreToolUse 的 ModifyInput 能力。
/// 自动为某些命令添加超时限制。
pub struct TimeoutHook;

impl Hook for TimeoutHook {
    fn name(&self) -> &str { "timeout_inject" }
    fn hook_type(&self) -> HookType { HookType::PreToolUse }
    fn pattern(&self) -> &str { "bash" }

    fn execute(&self, ctx: &HookContext) -> HookDecision {
        let cmd = ctx
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 为 sleep 命令添加超时
        if cmd.starts_with("sleep ") {
            let wrapped = format!("timeout 5 {cmd}");
            return HookDecision::ModifyInput {
                new_input: serde_json::json!({"command": wrapped}),
            };
        }

        HookDecision::Allow
    }
}

/// 输出格式化 Hook：为输出添加时间戳
///
/// 演示 PostToolUse 的 ModifyOutput 能力。
pub struct TimestampHook;

impl Hook for TimestampHook {
    fn name(&self) -> &str { "timestamp" }
    fn hook_type(&self) -> HookType { HookType::PostToolUse }
    fn pattern(&self) -> &str { "bash" }

    fn execute(&self, ctx: &HookContext) -> HookDecision {
        if let Some(ref output) = ctx.output {
            let timestamped = format!("[{}] {}", chrono::Local::now().format("%H:%M:%S"), output);
            HookDecision::ModifyOutput {
                new_output: timestamped,
            }
        } else {
            HookDecision::Allow
        }
    }
}

// =============================================================================
// 第三部分：Hook 执行管线
//
// 对应 Claude Code 的 Hook 执行引擎（src/hooks/execute.ts）。
//
// Claude Code 的 Hook 执行流程：
//   1. 收集所有匹配当前工具的 Hook
//   2. 按优先级排序
//   3. 依次执行，传递上下文
//   4. 任一 Hook 返回 Deny → 立即拒绝
//   5. 返回 Modify → 使用修改后的输入/输出
//   6. 所有 Allow → 继续执行
// =============================================================================

/// Hook 管线 —— 管理和执行 Hook 链
pub struct HookPipeline {
    hooks: Vec<Box<dyn Hook>>,
}

impl HookPipeline {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// 注册一个 Hook
    pub fn register(&mut self, hook: Box<dyn Hook>) {
        self.hooks.push(hook);
    }

    /// 执行 PreToolUse Hook 链
    ///
    /// 返回 None 表示允许，Some(Deny) 表示拒绝。
    /// 如果返回 ModifyInput，调用者应使用修改后的输入。
    pub fn run_pre_tool_use(&self, ctx: &HookContext) -> HookDecision {
        for hook in &self.hooks {
            if hook.hook_type() != HookType::PreToolUse {
                continue;
            }
            if !hook.matches(&ctx.tool_name) {
                continue;
            }

            tracing::debug!("执行 PreToolUse Hook: {}", hook.name());
            match hook.execute(ctx) {
                HookDecision::Allow => continue,
                HookDecision::Deny { reason } => {
                    tracing::warn!("Hook {} 拒绝工具 {}: {}", hook.name(), ctx.tool_name, reason);
                    return HookDecision::Deny { reason };
                }
                HookDecision::ModifyInput { new_input } => {
                    tracing::info!("Hook {} 修改了 {} 的输入", hook.name(), ctx.tool_name);
                    return HookDecision::ModifyInput { new_input };
                }
                _ => continue,
            }
        }
        HookDecision::Allow
    }

    /// 执行 PostToolUse Hook 链
    pub fn run_post_tool_use(&self, ctx: &HookContext) -> HookDecision {
        let mut current_output = ctx.output.clone();

        for hook in &self.hooks {
            if hook.hook_type() != HookType::PostToolUse {
                continue;
            }
            if !hook.matches(&ctx.tool_name) {
                continue;
            }

            let modified_ctx = HookContext {
                output: current_output.clone(),
                ..ctx.clone()
            };

            match hook.execute(&modified_ctx) {
                HookDecision::ModifyOutput { new_output } => {
                    current_output = Some(new_output);
                }
                HookDecision::Deny { reason } => {
                    return HookDecision::Deny { reason };
                }
                _ => {}
            }
        }

        match current_output {
            Some(output) => HookDecision::ModifyOutput { new_output: output },
            None => HookDecision::Allow,
        }
    }

    /// 获取已注册的 Hook 列表（用于调试）
    pub fn list_hooks(&self) -> Vec<(&str, HookType, &str)> {
        self.hooks
            .iter()
            .map(|h| (h.name(), h.hook_type(), h.pattern()))
            .collect()
    }
}

// =============================================================================
// 第四部分：带 Hook 的工具路由器
//
// 将 Hook 管线集成到 ToolRouter 中，增强工具执行。
// =============================================================================

/// 带 Hook 的工具执行
///
/// 在工具执行前后运行 Hook 管线。
pub fn execute_with_hooks(
    router: &ToolRouter,
    pipeline: &HookPipeline,
    call: &ToolCallInfo,
) -> ToolResult {
    let ctx = HookContext {
        tool_name: call.name.clone(),
        call_id: call.id.clone(),
        input: call.input.clone(),
        output: None,
        is_error: false,
    };

    // 运行 PreToolUse Hook
    match pipeline.run_pre_tool_use(&ctx) {
        HookDecision::Deny { reason } => {
            return ToolResult {
                call_id: call.id.clone(),
                output: format!("工具被 Hook 拒绝: {reason}"),
                is_error: true,
                wall_time: Duration::ZERO,
            };
        }
        HookDecision::ModifyInput { new_input } => {
            // 使用修改后的输入执行工具
            let result = router.execute(&call.id, &call.name, &new_input);
            return run_post_hooks(pipeline, &call, &result);
        }
        _ => {}
    }

    // 正常执行工具
    let result = router.execute(&call.id, &call.name, &call.input);
    run_post_hooks(pipeline, &call, &result)
}

fn run_post_hooks(pipeline: &HookPipeline, call: &ToolCallInfo, result: &ToolResult) -> ToolResult {
    let post_ctx = HookContext {
        tool_name: call.name.clone(),
        call_id: call.id.clone(),
        input: call.input.clone(),
        output: Some(result.output.clone()),
        is_error: result.is_error,
    };

    match pipeline.run_post_tool_use(&post_ctx) {
        HookDecision::ModifyOutput { new_output } => ToolResult {
            call_id: result.call_id.clone(),
            output: new_output,
            is_error: result.is_error,
            wall_time: result.wall_time,
        },
        _ => ToolResult {
            call_id: result.call_id.clone(),
            output: result.output.clone(),
            is_error: result.is_error,
            wall_time: result.wall_time,
        },
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

    println!("=== Ch08: Hook 系统 ===");
    println!("对应: 第 9 章（Hook 系统 —— 可扩展的执行管线）");
    println!();

    // 创建工具路由器
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(BashTool));
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(EditFileTool));
    registry.register(Box::new(GlobTool));
    registry.register(Box::new(GrepTool));
    let router = ToolRouter::new(registry);

    // 创建 Hook 管线
    let mut pipeline = HookPipeline::new();
    pipeline.register(Box::new(SafetyHook));
    pipeline.register(Box::new(TimeoutHook));
    pipeline.register(Box::new(LoggingHook));
    pipeline.register(Box::new(TimestampHook));

    println!("已注册 Hook:");
    for (name, hook_type, pattern) in pipeline.list_hooks() {
        println!("  - [{hook_type:?}] {name} (pattern: {pattern})");
    }
    println!();

    // 演示 1：安全 Hook 拦截危险命令
    println!("--- 演示 1: 安全 Hook 拦截 ---");
    let dangerous_call = ToolCallInfo {
        id: "call_001".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({"command": "rm -rf /"}),
    };
    let result = execute_with_hooks(&router, &pipeline, &dangerous_call);
    println!(
        "  rm -rf / → {}",
        if result.is_error { "已拦截" } else { "通过" }
    );
    println!("  原因: {}", truncate(&result.output, 80));
    println!();

    // 演示 2：正常命令通过
    println!("--- 演示 2: 正常命令通过 ---");
    let normal_call = ToolCallInfo {
        id: "call_002".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({"command": "echo 'Hello Hooks!'"}),
    };
    let result = execute_with_hooks(&router, &pipeline, &normal_call);
    println!(
        "  echo → {}",
        if result.is_error { "错误" } else { "成功" }
    );
    println!("  输出: {}", truncate(&result.output, 80));
    println!();

    // 演示 3：输入修改 Hook
    println!("--- 演示 3: 超时注入 Hook ---");
    let sleep_call = ToolCallInfo {
        id: "call_003".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({"command": "sleep 1"}),
    };
    let result = execute_with_hooks(&router, &pipeline, &sleep_call);
    println!("  sleep 1 → 自动添加 timeout 5 前缀");
    println!("  结果: {}", truncate(&result.output, 80));
    println!();

    // 演示 4：sudo 拦截
    println!("--- 演示 4: sudo 拦截 ---");
    let sudo_call = ToolCallInfo {
        id: "call_004".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({"command": "sudo apt-get install something"}),
    };
    let result = execute_with_hooks(&router, &pipeline, &sudo_call);
    println!(
        "  sudo → {}",
        if result.is_error { "已拦截" } else { "通过" }
    );
    println!("  原因: {}", truncate(&result.output, 80));
    println!();

    println!("(演示完成。Hook 系统通过拦截和增强机制保障工具执行安全。)");

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
        registry.register(Box::new(BashTool));
        registry.register(Box::new(ReadFileTool));
        registry.register(Box::new(WriteFileTool));
        ToolRouter::new(registry)
    }

    fn test_pipeline() -> HookPipeline {
        let mut pipeline = HookPipeline::new();
        pipeline.register(Box::new(SafetyHook));
        pipeline.register(Box::new(TimeoutHook));
        pipeline.register(Box::new(LoggingHook));
        pipeline.register(Box::new(TimestampHook));
        pipeline
    }

    // ---- Hook 类型和匹配测试 ----

    /// 测试 Hook 类型正确性
    #[test]
    fn test_hook_types() {
        let safety = SafetyHook;
        assert_eq!(safety.hook_type(), HookType::PreToolUse);
        assert_eq!(safety.pattern(), "bash");

        let logging = LoggingHook;
        assert_eq!(logging.hook_type(), HookType::PostToolUse);
        assert_eq!(logging.pattern(), "*");
    }

    /// 测试 Glob 模式匹配
    #[test]
    fn test_glob_pattern_matching() {
        let safety = SafetyHook;
        assert!(safety.matches("bash"));
        assert!(!safety.matches("read_file"));

        let logging = LoggingHook;
        assert!(logging.matches("bash"));
        assert!(logging.matches("read_file"));
        assert!(logging.matches("any_tool"));
    }

    /// 测试 glob_match 函数
    #[test]
    fn test_glob_match() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("bash", "bash"));
        assert!(!glob_match("bash", "read_file"));
        assert!(glob_match("file_*", "file_read"));
        assert!(glob_match("file_*", "file_write"));
        assert!(!glob_match("file_*", "bash"));
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.py"));
    }

    // ---- 安全 Hook 测试 ----

    /// 测试安全 Hook 拦截 rm -rf /
    #[test]
    fn test_safety_hook_blocks_rm_rf() {
        let hook = SafetyHook;
        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "rm -rf /"}),
            output: None,
            is_error: false,
        };
        match hook.execute(&ctx) {
            HookDecision::Deny { reason } => assert!(reason.contains("危险命令")),
            _ => panic!("应拒绝 rm -rf /"),
        }
    }

    /// 测试安全 Hook 拦截 sudo
    #[test]
    fn test_safety_hook_blocks_sudo() {
        let hook = SafetyHook;
        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "sudo rm -rf something"}),
            output: None,
            is_error: false,
        };
        match hook.execute(&ctx) {
            HookDecision::Deny { reason } => assert!(reason.contains("sudo")),
            _ => panic!("应拒绝 sudo"),
        }
    }

    /// 测试安全 Hook 放行正常命令
    #[test]
    fn test_safety_hook_allows_normal() {
        let hook = SafetyHook;
        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "echo hello"}),
            output: None,
            is_error: false,
        };
        assert!(matches!(hook.execute(&ctx), HookDecision::Allow));
    }

    // ---- 超时注入 Hook 测试 ----

    /// 测试超时 Hook 为 sleep 命令添加 timeout
    #[test]
    fn test_timeout_hook_modifies_sleep() {
        let hook = TimeoutHook;
        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "sleep 100"}),
            output: None,
            is_error: false,
        };
        match hook.execute(&ctx) {
            HookDecision::ModifyInput { new_input } => {
                let cmd = new_input.get("command").unwrap().as_str().unwrap();
                assert!(cmd.starts_with("timeout 5"));
                assert!(cmd.contains("sleep 100"));
            }
            _ => panic!("应修改 sleep 命令"),
        }
    }

    /// 测试超时 Hook 不修改非 sleep 命令
    #[test]
    fn test_timeout_hook_ignores_others() {
        let hook = TimeoutHook;
        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "echo hello"}),
            output: None,
            is_error: false,
        };
        assert!(matches!(hook.execute(&ctx), HookDecision::Allow));
    }

    // ---- Hook 管线测试 ----

    /// 测试管线注册和列出
    #[test]
    fn test_pipeline_registration() {
        let pipeline = test_pipeline();
        let hooks = pipeline.list_hooks();
        assert_eq!(hooks.len(), 4);
    }

    /// 测试管线 PreToolUse 链
    #[test]
    fn test_pipeline_pre_tool_use() {
        let pipeline = test_pipeline();

        // 危险命令应被拦截
        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "rm -rf /"}),
            output: None,
            is_error: false,
        };
        assert!(matches!(
            pipeline.run_pre_tool_use(&ctx),
            HookDecision::Deny { .. }
        ));

        // sleep 命令应被修改
        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "sleep 5"}),
            output: None,
            is_error: false,
        };
        assert!(matches!(
            pipeline.run_pre_tool_use(&ctx),
            HookDecision::ModifyInput { .. }
        ));

        // 正常命令应通过
        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "echo ok"}),
            output: None,
            is_error: false,
        };
        assert!(matches!(
            pipeline.run_pre_tool_use(&ctx),
            HookDecision::Allow
        ));
    }

    /// 测试管线 PostToolUse 链
    #[test]
    fn test_pipeline_post_tool_use() {
        let pipeline = test_pipeline();

        let ctx = HookContext {
            tool_name: "bash".to_string(),
            call_id: "test".to_string(),
            input: serde_json::json!({"command": "echo ok"}),
            output: Some("ok\n".to_string()),
            is_error: false,
        };
        // TimestampHook 应修改输出，添加时间戳
        match pipeline.run_post_tool_use(&ctx) {
            HookDecision::ModifyOutput { new_output } => {
                assert!(new_output.contains("ok"));
                // 时间戳格式 [HH:MM:SS]
                assert!(new_output.contains('['));
            }
            other => panic!("应修改输出，实际: {other:?}"),
        }
    }

    // ---- 集成测试 ----

    /// 测试带 Hook 的工具执行：拦截危险命令
    #[test]
    fn test_execute_with_hooks_safety() {
        let router = test_router();
        let pipeline = test_pipeline();

        let call = ToolCallInfo {
            id: "test_001".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "rm -rf /"}),
        };

        let result = execute_with_hooks(&router, &pipeline, &call);
        assert!(result.is_error);
        assert!(result.output.contains("Hook 拒绝"));
    }

    /// 测试带 Hook 的工具执行：正常命令
    #[test]
    fn test_execute_with_hooks_normal() {
        let router = test_router();
        let pipeline = test_pipeline();

        let call = ToolCallInfo {
            id: "test_002".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "echo hello"}),
        };

        let result = execute_with_hooks(&router, &pipeline, &call);
        assert!(!result.is_error);
        assert!(result.output.contains("hello"));
    }

    /// 测试 truncate 辅助函数
    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello...");
    }
}
