//! # 第四章：权限系统 —— Claude Code 的安全守卫
//!
//! 本模块演示 Claude Code 的四阶段权限管线，忠实映射书中第 4 章内容。
//!
//! 对应 claude-code-book 第 4 章（权限系统 —— 安全的第一道防线）。
//!
//! 核心概念：
//! - 四阶段权限管线：validateInput → rule matching → checkPermissions → interactive
//! - 五种权限模式：Default / Plan / AcceptEdits / BypassPermissions / DontAsk
//! - 三种决策：Allow / Deny / Ask
//! - 规则匹配：基于工具名称的 allowlist / blocklist
//!
//! Claude Code 的权限系统位于：
//!   src/permissions/: 权限管线核心
//!   src/permissions/rules.ts: 规则匹配
//!   src/permissions/modes.ts: 权限模式
//!
//! 运行方式：
//! ```bash
//! cargo run -p ch04-permissions
//! ```

use anyhow::Result;
use mini_claude_common::{
    BashTool, EditFileTool, GlobTool, GrepTool, PermissionDecision, PermissionMode,
    ReadFileTool, ToolRegistry, ToolRouter, WriteFileTool,
};

// ============================================================================
// 第一部分：权限规则 —— 对应 Claude Code 的 permissions/rules.ts
//
// Claude Code 使用规则匹配来决定工具的权限。
// 规则分为两类：
//   - allowlist: 匹配的工具自动允许
//   - blocklist: 匹配的工具自动拒绝
//
// 规则基于工具名称匹配，支持通配符。
// ============================================================================

/// 权限规则
///
/// 对应 Claude Code src/permissions/rules.ts 中的 PermissionRule。
/// 每条规则包含：
///   - tool_pattern: 工具名称模式（支持通配符 *）
///   - decision: 匹配时的决策（Allow / Deny）
///   - description: 规则描述
#[derive(Debug, Clone)]
struct PermissionRule {
    tool_pattern: String,
    decision: PermissionDecision,
    description: String,
}

impl PermissionRule {
    fn matches(&self, tool_name: &str) -> bool {
        if self.tool_pattern == "*" {
            return true;
        }
        if let Some(prefix) = self.tool_pattern.strip_suffix('*') {
            return tool_name.starts_with(prefix);
        }
        if let Some(suffix) = self.tool_pattern.strip_prefix('*') {
            return tool_name.ends_with(suffix);
        }
        self.tool_pattern == tool_name
    }
}

// ============================================================================
// 第二部分：四阶段权限管线
//
// 对应 Claude Code 的权限管线实现。
//
// 阶段 1: validateInput —— 输入验证
//   - 检查工具名称是否已知
//   - 检查参数格式是否正确
//
// 阶段 2: rule matching —— 规则匹配
//   - 遍历规则列表，找到第一个匹配的规则
//   - 返回规则对应的决策
//
// 阶段 3: checkPermissions —— 权限模式检查
//   - 根据当前权限模式和工具类型做出决策
//   - 不同模式有不同的默认行为
//
// 阶段 4: interactive —— 交互确认
//   - 当前三阶段无法决定时，提示用户确认
//   - 在 mock 模式下自动批准
// ============================================================================

/// 权限管线
///
/// 对应 Claude Code 的 PermissionPipeline。
/// 按照四阶段顺序处理每个工具调用请求。
struct PermissionPipeline {
    mode: PermissionMode,
    rules: Vec<PermissionRule>,
    tool_registry: ToolRegistry,
}

/// 管线各阶段的处理结果
#[derive(Debug)]
enum PipelineStageResult {
    /// 已做出最终决策
    Decided(PermissionDecision),
    /// 需要进入下一阶段
    Continue,
}

impl PermissionPipeline {
    fn new(mode: PermissionMode) -> Self {
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Box::new(ReadFileTool));
        tool_registry.register(Box::new(WriteFileTool));
        tool_registry.register(Box::new(EditFileTool));
        tool_registry.register(Box::new(BashTool));
        tool_registry.register(Box::new(GlobTool));
        tool_registry.register(Box::new(GrepTool));

        Self {
            mode,
            rules: Vec::new(),
            tool_registry,
        }
    }

    /// 添加权限规则
    fn add_rule(&mut self, rule: PermissionRule) {
        self.rules.push(rule);
    }

    // ---- 阶段 1: validateInput ----
    /// 验证工具调用输入是否合法
    ///
    /// 对应 Claude Code 的 validateInput 阶段。
    /// 检查：
    ///   - 工具名称是否已注册
    ///   - 必需参数是否存在
    fn stage_validate_input(&self, tool_name: &str, input: &serde_json::Value) -> PipelineStageResult {
        // 检查工具是否存在
        if self.tool_registry.get(tool_name).is_none() {
            return PipelineStageResult::Decided(PermissionDecision::Deny);
        }

        // 检查 input 是否为有效 JSON 对象
        if !input.is_object() {
            return PipelineStageResult::Decided(PermissionDecision::Deny);
        }

        PipelineStageResult::Continue
    }

    // ---- 阶段 2: rule matching ----
    /// 规则匹配
    ///
    /// 对应 Claude Code 的 rule matching 阶段。
    /// 遍历规则列表，返回第一个匹配规则的决策。
    fn stage_rule_matching(&self, tool_name: &str) -> PipelineStageResult {
        for rule in &self.rules {
            if rule.matches(tool_name) {
                tracing::info!("规则匹配: {} -> {:?}", rule.description, rule.decision);
                return PipelineStageResult::Decided(rule.decision.clone());
            }
        }
        PipelineStageResult::Continue
    }

    // ---- 阶段 3: checkPermissions ----
    /// 权限模式检查
    ///
    /// 对应 Claude Code 的 checkPermissions 阶段。
    /// 根据当前权限模式和工具的读写属性做出决策。
    fn stage_check_permissions(&self, tool_name: &str) -> PipelineStageResult {
        let is_read_only = matches!(tool_name, "read_file" | "glob" | "grep");

        match &self.mode {
            // Default: 只读工具自动允许，写入工具需要确认
            PermissionMode::Default => {
                if is_read_only {
                    PipelineStageResult::Decided(PermissionDecision::Allow)
                } else {
                    PipelineStageResult::Continue // 需要交互确认
                }
            }
            // Plan: 所有工具只读，写入工具被拒绝
            PermissionMode::Plan => {
                if is_read_only {
                    PipelineStageResult::Decided(PermissionDecision::Allow)
                } else {
                    PipelineStageResult::Decided(PermissionDecision::Deny)
                }
            }
            // AcceptEdits: 编辑类工具自动允许，bash 需确认
            PermissionMode::AcceptEdits => {
                if matches!(tool_name, "bash") {
                    PipelineStageResult::Continue
                } else {
                    PipelineStageResult::Decided(PermissionDecision::Allow)
                }
            }
            // BypassPermissions: 全部自动批准
            PermissionMode::BypassPermissions => {
                PipelineStageResult::Decided(PermissionDecision::Allow)
            }
            // DontAsk: 无匹配规则时自动拒绝
            PermissionMode::DontAsk => {
                PipelineStageResult::Decided(PermissionDecision::Deny)
            }
        }
    }

    // ---- 阶段 4: interactive ----
    /// 交互确认
    ///
    /// 对应 Claude Code 的 interactive 阶段。
    /// 当前三阶段无法决定时，提示用户确认。
    /// Mock 模式下自动批准。
    fn stage_interactive(&self, tool_name: &str, _input: &serde_json::Value) -> PipelineStageResult {
        tracing::info!("交互确认: 工具 {tool_name} 需要用户批准");
        PipelineStageResult::Decided(PermissionDecision::Allow)
    }

    /// 执行完整的四阶段权限管线
    ///
    /// 按照顺序执行四个阶段，返回最终决策。
    fn check(&self, tool_name: &str, input: &serde_json::Value) -> PermissionDecision {
        // 阶段 1: 输入验证
        match self.stage_validate_input(tool_name, input) {
            PipelineStageResult::Decided(d) => return d,
            PipelineStageResult::Continue => {}
        }

        // 阶段 2: 规则匹配
        match self.stage_rule_matching(tool_name) {
            PipelineStageResult::Decided(d) => return d,
            PipelineStageResult::Continue => {}
        }

        // 阶段 3: 权限模式检查
        match self.stage_check_permissions(tool_name) {
            PipelineStageResult::Decided(d) => return d,
            PipelineStageResult::Continue => {}
        }

        // 阶段 4: 交互确认
        match self.stage_interactive(tool_name, input) {
            PipelineStageResult::Decided(d) => return d,
            PipelineStageResult::Continue => PermissionDecision::Deny,
        }
    }
}

// ============================================================================
// 第三部分：五种权限模式演示
//
// 对应 Claude Code 的五种外部权限模式：
//   - Default: 无匹配规则时交互确认
//   - Plan: 只读模式，写入工具被拒绝
//   - AcceptEdits: 自动批准编辑操作
//   - BypassPermissions: 全部自动批准（危险！）
//   - DontAsk: 无匹配规则时自动拒绝
// ============================================================================

/// 演示五种权限模式的行为差异
fn demo_permission_modes() {
    println!("=== 五种权限模式对比 ===");
    println!();

    let test_tools = vec![
        ("read_file", serde_json::json!({"file_path": "/tmp/test.txt"})),
        ("write_file", serde_json::json!({"file_path": "/tmp/test.txt", "content": "hello"})),
        ("bash", serde_json::json!({"command": "echo hello"})),
        ("glob", serde_json::json!({"pattern": "*.rs"})),
        ("edit_file", serde_json::json!({"file_path": "/tmp/test.txt", "old_string": "a", "new_string": "b"})),
    ];

    let modes = vec![
        ("Default", PermissionMode::Default),
        ("Plan", PermissionMode::Plan),
        ("AcceptEdits", PermissionMode::AcceptEdits),
        ("BypassPermissions", PermissionMode::BypassPermissions),
        ("DontAsk", PermissionMode::DontAsk),
    ];

    // 打印表头
    println!("{:<20}", "工具");
    for (name, _) in &modes {
        print!("{:<16}", name);
    }
    println!();
    println!("{}", "-".repeat(100));

    // 测试每种模式
    for (tool_name, input) in &test_tools {
        print!("{:<20}", tool_name);
        for (_, mode) in &modes {
            let pipeline = PermissionPipeline::new(mode.clone());
            let decision = pipeline.check(tool_name, input);
            let label = match decision {
                PermissionDecision::Allow => "Allow",
                PermissionDecision::Deny => "Deny",
                PermissionDecision::Ask => "Ask",
            };
            print!("{:<16}", label);
        }
        println!();
    }
    println!();
}

// ============================================================================
// 第四部分：带规则的权限管线演示
//
// 展示如何使用自定义规则覆盖默认行为。
// 对应 Claude Code 的 .claude/settings.json 中的权限配置。
// ============================================================================

/// 演示带自定义规则的权限管线
fn demo_custom_rules() {
    println!("=== 自定义规则演示 ===");
    println!();

    // 创建 Default 模式的管线
    let mut pipeline = PermissionPipeline::new(PermissionMode::Default);

    // 添加规则：允许特定 bash 命令
    pipeline.add_rule(PermissionRule {
        tool_pattern: "read_*".to_string(),
        decision: PermissionDecision::Allow,
        description: "允许所有 read_ 开头的工具".to_string(),
    });

    // 添加规则：拒绝所有 write 操作
    pipeline.add_rule(PermissionRule {
        tool_pattern: "write_*".to_string(),
        decision: PermissionDecision::Deny,
        description: "拒绝所有 write_ 开头的工具".to_string(),
    });

    // 添加规则：允许 glob
    pipeline.add_rule(PermissionRule {
        tool_pattern: "glob".to_string(),
        decision: PermissionDecision::Allow,
        description: "允许 glob 工具".to_string(),
    });

    let test_cases = vec![
        ("read_file", serde_json::json!({"file_path": "/tmp/test"})),
        ("write_file", serde_json::json!({"file_path": "/tmp/test", "content": "x"})),
        ("glob", serde_json::json!({"pattern": "*.rs"})),
        ("bash", serde_json::json!({"command": "ls"})),
        ("nonexistent", serde_json::json!({})),
    ];

    for (tool_name, input) in &test_cases {
        let decision = pipeline.check(tool_name, input);
        let label = match decision {
            PermissionDecision::Allow => "Allow",
            PermissionDecision::Deny => "Deny",
            PermissionDecision::Ask => "Ask",
        };
        println!("  {tool_name:<20} => {label}");
    }
    println!();
}

// ============================================================================
// 第五部分：Mock 完整流程
//
// 将权限系统与工具调用结合，演示完整的权限检查流程。
// ============================================================================

/// 演示完整的权限检查流程（mock 模式）
fn demo_full_pipeline() {
    println!("=== 完整权限检查流程 ===");
    println!();

    // 使用 AcceptEdits 模式 + 自定义规则
    let mut pipeline = PermissionPipeline::new(PermissionMode::AcceptEdits);
    pipeline.add_rule(PermissionRule {
        tool_pattern: "bash".to_string(),
        decision: PermissionDecision::Allow,
        description: "自动批准 bash".to_string(),
    });

    let router = {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ReadFileTool));
        registry.register(Box::new(WriteFileTool));
        registry.register(Box::new(EditFileTool));
        registry.register(Box::new(BashTool));
        registry.register(Box::new(GlobTool));
        registry.register(Box::new(GrepTool));
        ToolRouter::new(registry)
    };

    // 模拟模型返回的工具调用序列
    let mock_calls = vec![
        ("read_file", serde_json::json!({"file_path": "/tmp/test.txt"})),
        ("bash", serde_json::json!({"command": "echo 'permission check'"})),
        ("write_file", serde_json::json!({"file_path": "/tmp/test.txt", "content": "data"})),
        ("glob", serde_json::json!({"pattern": "*.rs"})),
    ];

    for (tool_name, input) in &mock_calls {
        println!("  工具调用: {tool_name}");

        // 阶段 1-4: 权限检查
        let decision = pipeline.check(tool_name, input);
        let label = match decision {
            PermissionDecision::Allow => "Allow",
            PermissionDecision::Deny => "Deny",
            PermissionDecision::Ask => "Ask",
        };
        println!("    权限决策: {label}");

        // 如果允许，执行工具
        if decision == PermissionDecision::Allow {
            let result = router.execute("call_001", tool_name, input);
            if result.is_error {
                println!("    执行结果: [错误] {}", truncate(&result.output, 80));
            } else {
                println!("    执行结果: {}", truncate(&result.output, 80));
            }
        } else {
            println!("    执行结果: 跳过（权限拒绝）");
        }
        println!();
    }
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

    println!("=== Ch04: 权限系统 ===");
    println!("对应 claude-code-book 第 4 章");
    println!();

    demo_permission_modes();
    demo_custom_rules();
    demo_full_pipeline();

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

    fn make_pipeline(mode: PermissionMode) -> PermissionPipeline {
        PermissionPipeline::new(mode)
    }

    // ---- 阶段 1 测试：validateInput ----

    #[test]
    fn test_validate_known_tool() {
        let pipeline = make_pipeline(PermissionMode::BypassPermissions);
        let decision = pipeline.check("bash", &serde_json::json!({"command": "ls"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn test_validate_unknown_tool() {
        let pipeline = make_pipeline(PermissionMode::BypassPermissions);
        let decision = pipeline.check("nonexistent_tool", &serde_json::json!({}));
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[test]
    fn test_validate_invalid_input() {
        let pipeline = make_pipeline(PermissionMode::BypassPermissions);
        let decision = pipeline.check("bash", &serde_json::json!("not_an_object"));
        assert_eq!(decision, PermissionDecision::Deny);
    }

    // ---- 阶段 2 测试：rule matching ----

    #[test]
    fn test_rule_exact_match() {
        let mut pipeline = make_pipeline(PermissionMode::Default);
        pipeline.add_rule(PermissionRule {
            tool_pattern: "bash".to_string(),
            decision: PermissionDecision::Deny,
            description: "deny bash".to_string(),
        });
        let decision = pipeline.check("bash", &serde_json::json!({"command": "ls"}));
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[test]
    fn test_rule_wildcard_suffix() {
        let mut pipeline = make_pipeline(PermissionMode::Default);
        pipeline.add_rule(PermissionRule {
            tool_pattern: "read_*".to_string(),
            decision: PermissionDecision::Allow,
            description: "allow read_*".to_string(),
        });
        let decision = pipeline.check("read_file", &serde_json::json!({"file_path": "/tmp/x"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn test_rule_wildcard_prefix() {
        let mut pipeline = make_pipeline(PermissionMode::Default);
        pipeline.add_rule(PermissionRule {
            tool_pattern: "*_file".to_string(),
            decision: PermissionDecision::Deny,
            description: "deny *_file".to_string(),
        });
        let decision = pipeline.check("read_file", &serde_json::json!({"file_path": "/tmp/x"}));
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[test]
    fn test_rule_global_wildcard() {
        let mut pipeline = make_pipeline(PermissionMode::Default);
        pipeline.add_rule(PermissionRule {
            tool_pattern: "*".to_string(),
            decision: PermissionDecision::Allow,
            description: "allow all".to_string(),
        });
        let decision = pipeline.check("bash", &serde_json::json!({"command": "rm -rf /"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn test_rule_priority_first_match() {
        let mut pipeline = make_pipeline(PermissionMode::Default);
        pipeline.add_rule(PermissionRule {
            tool_pattern: "bash".to_string(),
            decision: PermissionDecision::Allow,
            description: "first rule".to_string(),
        });
        pipeline.add_rule(PermissionRule {
            tool_pattern: "bash".to_string(),
            decision: PermissionDecision::Deny,
            description: "second rule (should not match)".to_string(),
        });
        let decision = pipeline.check("bash", &serde_json::json!({"command": "ls"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }

    // ---- 阶段 3 测试：checkPermissions ----

    #[test]
    fn test_default_mode_read_only() {
        let pipeline = make_pipeline(PermissionMode::Default);
        let decision = pipeline.check("read_file", &serde_json::json!({"file_path": "/tmp/x"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn test_default_mode_write_asks() {
        let pipeline = make_pipeline(PermissionMode::Default);
        // write_file 没有匹配规则，Default 模式下写入工具需要交互确认
        // 但阶段 4 mock 自动批准，所以最终是 Allow
        let decision = pipeline.check("write_file", &serde_json::json!({"file_path": "/tmp/x", "content": "y"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn test_plan_mode_allows_read() {
        let pipeline = make_pipeline(PermissionMode::Plan);
        let decision = pipeline.check("glob", &serde_json::json!({"pattern": "*.rs"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn test_plan_mode_denies_write() {
        let pipeline = make_pipeline(PermissionMode::Plan);
        let decision = pipeline.check("write_file", &serde_json::json!({"file_path": "/tmp/x", "content": "y"}));
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[test]
    fn test_plan_mode_denies_bash() {
        let pipeline = make_pipeline(PermissionMode::Plan);
        let decision = pipeline.check("bash", &serde_json::json!({"command": "ls"}));
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[test]
    fn test_accept_edits_allows_edit() {
        let pipeline = make_pipeline(PermissionMode::AcceptEdits);
        let decision = pipeline.check("edit_file", &serde_json::json!({"file_path": "/tmp/x", "old_string": "a", "new_string": "b"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[test]
    fn test_bypass_permissions_allows_all() {
        let pipeline = make_pipeline(PermissionMode::BypassPermissions);
        assert_eq!(pipeline.check("bash", &serde_json::json!({"command": "rm -rf /"})), PermissionDecision::Allow);
        assert_eq!(pipeline.check("write_file", &serde_json::json!({"file_path": "/tmp/x", "content": "y"})), PermissionDecision::Allow);
    }

    #[test]
    fn test_dont_ask_denies_unknown() {
        let pipeline = make_pipeline(PermissionMode::DontAsk);
        let decision = pipeline.check("bash", &serde_json::json!({"command": "ls"}));
        assert_eq!(decision, PermissionDecision::Deny);
    }

    // ---- PermissionRule.matches 测试 ----

    #[test]
    fn test_rule_matches_exact() {
        let rule = PermissionRule {
            tool_pattern: "bash".to_string(),
            decision: PermissionDecision::Allow,
            description: "".to_string(),
        };
        assert!(rule.matches("bash"));
        assert!(!rule.matches("bash_extra"));
    }

    #[test]
    fn test_rule_matches_glob() {
        let rule = PermissionRule {
            tool_pattern: "read_*".to_string(),
            decision: PermissionDecision::Allow,
            description: "".to_string(),
        };
        assert!(rule.matches("read_file"));
        assert!(rule.matches("read_dir"));
        assert!(!rule.matches("write_file"));
    }

    // ---- 阶段 4 测试：interactive (mock 自动批准) ----

    #[test]
    fn test_interactive_mock_auto_approve() {
        let pipeline = make_pipeline(PermissionMode::Default);
        // bash 在 Default 模式下不匹配规则，阶段 3 返回 Continue，
        // 阶段 4 mock 自动批准
        let decision = pipeline.check("bash", &serde_json::json!({"command": "echo hello"}));
        assert_eq!(decision, PermissionDecision::Allow);
    }
}
