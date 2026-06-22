// =============================================================================
// 第十章：多 Agent 架构 —— 协作与分工
//
// 本文件实现 Claude Code 的多 Agent 架构，演示子 Agent 派生和协调。
//
// 对应 claude-code-book 第 10 章（多 Agent 架构）和第 11 章（Agent 协作）。
//
// 核心概念：
// - Agent 类型：explore（探索）、plan（规划）、general（通用）
// - Fork 模式：主 Agent 派生子 Agent 处理子任务
// - 协调器：管理子 Agent 生命周期和结果汇总
// - 任务隔离：子 Agent 有独立的工具集和上下文
//
// Claude Code 的多 Agent 在 src/agents/ 中：
//   MainAgent → spawnSubagent(type, prompt) → Subagent → 结果回传
//
// 运行方式：
//   cargo run -p ch10-multi-agent
// =============================================================================

use anyhow::Result;
use mini_claude_common::{
    BashTool, EditFileTool, GlobTool, GrepTool, Message, ReadFileTool,
    ToolRegistry, ToolRouter, WriteFileTool,
};
use std::time::{Duration, Instant};

// =============================================================================
// 第一部分：Agent 类型定义
//
// Claude Code 定义了三种内置 Agent 类型：
//   - explore: 快速探索代码库，只读操作，搜索和阅读文件
//   - plan: 制定执行计划，分析需求，规划步骤
//   - general: 通用 Agent，可执行任意操作
//
// 每种 Agent 有不同的工具集和系统提示。
// =============================================================================

/// Agent 类型 —— 对应 Claude Code 的内置 Agent 类型
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AgentType {
    /// 探索 Agent：只读，快速搜索和分析
    Explore,
    /// 规划 Agent：分析需求，制定计划
    Plan,
    /// 通用 Agent：可执行任意操作
    General,
}

impl AgentType {
    /// 获取 Agent 类型的描述
    pub fn description(&self) -> &str {
        match self {
            AgentType::Explore => "快速探索代码库，搜索和阅读文件",
            AgentType::Plan => "分析需求，制定执行计划",
            AgentType::General => "通用 Agent，可执行任意操作",
        }
    }

    /// 获取 Agent 的系统提示
    pub fn system_prompt(&self) -> &str {
        match self {
            AgentType::Explore => {
                "你是一个代码探索 Agent。你的任务是快速搜索和分析代码库。\
                 只使用只读工具（glob, grep, read_file）。\
                 返回简洁的发现报告。"
            }
            AgentType::Plan => {
                "你是一个规划 Agent。你的任务是分析需求并制定详细的执行计划。\
                 输出格式：1. 分析 2. 步骤列表 3. 风险评估。"
            }
            AgentType::General => {
                "你是一个通用 Agent。你可以使用所有工具来完成任务。\
                 先理解任务，再执行，最后验证结果。"
            }
        }
    }
}

// =============================================================================
// 第二部分：Agent 任务和结果
//
// Claude Code 中子 Agent 通过 Task 系统管理：
//   task { id, type, prompt, status, result }
// =============================================================================

/// Agent 任务
#[derive(Debug, Clone)]
pub struct AgentTask {
    /// 任务 ID
    pub id: String,
    /// Agent 类型
    pub agent_type: AgentType,
    /// 任务描述
    pub prompt: String,
    /// 父任务 ID（如果是子任务）
    pub parent_id: Option<String>,
}

/// Agent 执行结果
#[derive(Debug, Clone)]
pub struct AgentResult {
    /// 任务 ID
    pub task_id: String,
    /// 是否成功
    pub success: bool,
    /// 结果内容
    pub output: String,
    /// 执行耗时
    pub duration: Duration,
    /// 使用的工具调用次数
    pub tool_calls: usize,
}

// =============================================================================
// 第三部分：子 Agent
//
// 对应 Claude Code 的 Subagent 实现。
//
// 子 Agent 的特点：
//   - 独立的工具集（根据类型限制可用工具）
//   - 独立的消息历史
//   - 有超时限制
//   - 结果回传给父 Agent
// =============================================================================

/// 子 Agent
///
/// 对应 Claude Code 的 Subagent。
/// 每个子 Agent 有独立的工具集和执行环境。
pub struct SubAgent {
    pub task: AgentTask,
    pub tool_router: ToolRouter,
    pub messages: Vec<Message>,
}

impl SubAgent {
    /// 创建子 Agent，根据类型配置工具集
    pub fn new(task: AgentTask) -> Self {
        let mut registry = ToolRegistry::new();

        match task.agent_type {
            AgentType::Explore => {
                // 探索 Agent 只有只读工具
                registry.register(Box::new(ReadFileTool));
                registry.register(Box::new(GlobTool));
                registry.register(Box::new(GrepTool));
            }
            AgentType::Plan => {
                // 规划 Agent 有只读工具 + bash（用于分析）
                registry.register(Box::new(ReadFileTool));
                registry.register(Box::new(GlobTool));
                registry.register(Box::new(GrepTool));
                registry.register(Box::new(BashTool));
            }
            AgentType::General => {
                // 通用 Agent 有所有工具
                registry.register(Box::new(ReadFileTool));
                registry.register(Box::new(WriteFileTool));
                registry.register(Box::new(EditFileTool));
                registry.register(Box::new(BashTool));
                registry.register(Box::new(GlobTool));
                registry.register(Box::new(GrepTool));
            }
        }

        let messages = vec![Message {
            role: "system".to_string(),
            content: task.agent_type.system_prompt().to_string(),
        }];

        Self {
            task,
            tool_router: ToolRouter::new(registry),
            messages,
        }
    }

    /// Mock 执行子 Agent 任务
    ///
    /// 模拟 Agent Loop：调用模型 → 执行工具 → 返回结果。
    pub async fn execute_mock(&mut self) -> AgentResult {
        let start = Instant::now();
        let mut tool_calls = 0;

        // 模拟 Agent 行为（根据类型返回不同的 mock 响应）
        let output = match self.task.agent_type {
            AgentType::Explore => {
                // 模拟探索：执行 glob 搜索
                let result = self.tool_router.execute(
                    "explore_001",
                    "glob",
                    &serde_json::json!({"pattern": "*.rs"}),
                );
                tool_calls += 1;
                format!(
                    "代码探索完成。\n\
                     - 发现 Rust 源文件\n\
                     - 主要模块: lib.rs, main.rs\n\
                     - 工具数量: {} 个\n\
                     搜索结果: {}",
                    self.tool_router.model_visible_specs().len(),
                    truncate(&result.output, 100)
                )
            }
            AgentType::Plan => {
                // 模拟规划：分析任务并输出计划
                format!(
                    "执行计划（基于任务: {}）\n\
                     \n\
                     1. 分析阶段:\n\
                     - 理解任务需求\n\
                     - 识别相关文件\n\
                     \n\
                     2. 执行阶段:\n\
                     - 步骤 1: 搜索相关代码\n\
                     - 步骤 2: 理解现有实现\n\
                     - 步骤 3: 制定修改方案\n\
                     \n\
                     3. 风险评估:\n\
                     - 低风险: 只读操作\n\
                     - 中风险: 文件修改\n\
                     - 建议: 先用 explore Agent 了解情况",
                    truncate(&self.task.prompt, 50)
                )
            }
            AgentType::General => {
                // 模拟通用 Agent：执行 bash 命令
                let result = self.tool_router.execute(
                    "general_001",
                    "bash",
                    &serde_json::json!({"command": "echo 'Task completed'"}),
                );
                tool_calls += 1;
                format!(
                    "任务完成: {}\n执行结果: {}",
                    truncate(&self.task.prompt, 50),
                    truncate(&result.output, 100)
                )
            }
        };

        AgentResult {
            task_id: self.task.id.clone(),
            success: true,
            output,
            duration: start.elapsed(),
            tool_calls,
        }
    }
}

// =============================================================================
// 第四部分：Agent 协调器
//
// 对应 Claude Code 的 SubagentCoordinator。
//
// 协调器负责：
//   1. 接收主 Agent 的子任务请求
//   2. 创建并启动子 Agent
//   3. 管理子 Agent 生命周期
//   4. 收集和汇总结果
//   5. 处理子 Agent 错误
//
// Claude Code 使用 fork 模式：
//   MainAgent → fork(prompt, type) → Subagent → result
// =============================================================================

/// Agent 协调器 —— 管理子 Agent 的生命周期
pub struct AgentCoordinator {
    /// 已完成的结果
    results: Vec<AgentResult>,
    /// 活跃的子 Agent 数量
    active_count: usize,
}

impl AgentCoordinator {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            active_count: 0,
        }
    }

    /// Fork 一个子 Agent
    ///
    /// 对应 Claude Code 的 spawnSubagent(type, prompt)。
    /// 创建子 Agent 并在后台执行。
    pub async fn fork(
        &mut self,
        agent_type: AgentType,
        prompt: &str,
        parent_id: Option<&str>,
    ) -> String {
        let task_id = format!("task_{}", self.results.len() + 1);
        let task = AgentTask {
            id: task_id.clone(),
            agent_type: agent_type.clone(),
            prompt: prompt.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
        };

        tracing::info!(
            "Fork 子 Agent: {} (type: {:?}, task: {})",
            task_id,
            agent_type,
            truncate(prompt, 50)
        );

        let mut sub_agent = SubAgent::new(task);
        self.active_count += 1;

        let result = sub_agent.execute_mock().await;
        self.active_count -= 1;
        self.results.push(result);

        task_id
    }

    /// 并行 Fork 多个子 Agent
    ///
    /// 对应 Claude Code 的并发子 Agent 派生。
    pub async fn fork_parallel(
        &mut self,
        tasks: Vec<(AgentType, String)>,
    ) -> Vec<String> {
        let mut handles = Vec::new();

        for (agent_type, prompt) in tasks {
            let task_id = format!("task_{}", self.results.len() + handles.len() + 1);
            let task = AgentTask {
                id: task_id.clone(),
                agent_type,
                prompt,
                parent_id: None,
            };

            let handle = tokio::spawn(async move {
                let mut sub_agent = SubAgent::new(task);
                sub_agent.execute_mock().await
            });
            handles.push((task_id, handle));
        }

        let mut task_ids = Vec::new();
        for (task_id, handle) in handles {
            if let Ok(result) = handle.await {
                self.results.push(result);
            }
            task_ids.push(task_id);
        }

        task_ids
    }

    /// 获取任务结果
    pub fn get_result(&self, task_id: &str) -> Option<&AgentResult> {
        self.results.iter().find(|r| r.task_id == task_id)
    }

    /// 获取所有结果
    pub fn all_results(&self) -> &[AgentResult] {
        &self.results
    }

    /// 生成汇总报告
    pub fn summary(&self) -> String {
        let total = self.results.len();
        let success = self.results.iter().filter(|r| r.success).count();
        let total_tools: usize = self.results.iter().map(|r| r.tool_calls).sum();
        let total_duration: Duration = self.results.iter().map(|r| r.duration).sum();

        format!(
            "Agent 协调汇总:\n\
             - 总任务: {total}\n\
             - 成功: {success}\n\
             - 工具调用: {total_tools}\n\
             - 总耗时: {total_duration:?}"
        )
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

    println!("=== Ch10: 多 Agent 架构 ===");
    println!("对应: 第 10 章（多 Agent 架构）+ 第 11 章（Agent 协作）");
    println!();

    let mut coordinator = AgentCoordinator::new();

    // ---- 演示 1: 串行 Fork ----
    println!("--- 演示 1: 串行 Fork ---");
    println!("主 Agent 派生 explore 子 Agent 探索代码库...\n");

    let explore_id = coordinator
        .fork(
            AgentType::Explore,
            "搜索 src/ 目录下的所有 Rust 源文件",
            None,
        )
        .await;

    if let Some(result) = coordinator.get_result(&explore_id) {
        println!("[Explore Agent 结果]");
        println!("  状态: {}", if result.success { "成功" } else { "失败" });
        println!("  耗时: {:?}", result.duration);
        println!("  输出:\n{}", indent(&result.output, 4));
    }
    println!();

    // ---- 演示 2: Plan Agent ----
    println!("--- 演示 2: Plan Agent ---");
    println!("主 Agent 派生 plan 子 Agent 制定计划...\n");

    let plan_id = coordinator
        .fork(
            AgentType::Plan,
            "为项目添加单元测试支持",
            Some(&explore_id),
        )
        .await;

    if let Some(result) = coordinator.get_result(&plan_id) {
        println!("[Plan Agent 结果]");
        println!("  状态: {}", if result.success { "成功" } else { "失败" });
        println!("  输出:\n{}", indent(&result.output, 4));
    }
    println!();

    // ---- 演示 3: 并行 Fork ----
    println!("--- 演示 3: 并行 Fork ---");
    println!("主 Agent 同时派生多个子 Agent...\n");

    let parallel_ids = coordinator
        .fork_parallel(vec![
            (
                AgentType::Explore,
                "搜索所有测试文件".to_string(),
            ),
            (
                AgentType::General,
                "运行 cargo build 检查编译".to_string(),
            ),
        ])
        .await;

    for id in &parallel_ids {
        if let Some(result) = coordinator.get_result(id) {
            println!(
                "[{}] 状态: {}, 耗时: {:?}",
                result.task_id,
                if result.success { "成功" } else { "失败" },
                result.duration
            );
            println!("  输出: {}", truncate(&result.output, 80));
        }
    }
    println!();

    // ---- 汇总报告 ----
    println!("--- 汇总报告 ---");
    println!("{}", coordinator.summary());
    println!();

    // ---- Agent 类型对比 ----
    println!("--- Agent 类型对比 ---");
    for agent_type in [AgentType::Explore, AgentType::Plan, AgentType::General] {
        println!(
            "  {:?}: {}",
            agent_type,
            agent_type.description()
        );
    }
    println!();

    println!("(多 Agent 演示完成。Fork 模式使 Claude Code 可以并行处理复杂任务。)");

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

fn indent(s: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    s.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Agent 类型测试 ----

    /// 测试 Agent 类型描述
    #[test]
    fn test_agent_type_descriptions() {
        assert!(!AgentType::Explore.description().is_empty());
        assert!(!AgentType::Plan.description().is_empty());
        assert!(!AgentType::General.description().is_empty());
    }

    /// 测试 Agent 系统提示
    #[test]
    fn test_agent_system_prompts() {
        assert!(AgentType::Explore.system_prompt().contains("只读"));
        assert!(AgentType::Plan.system_prompt().contains("计划"));
        assert!(AgentType::General.system_prompt().contains("所有工具"));
    }

    // ---- 子 Agent 测试 ----

    /// 测试 Explore Agent 工具集（只读）
    #[test]
    fn test_explore_agent_tools() {
        let task = AgentTask {
            id: "test_explore".to_string(),
            agent_type: AgentType::Explore,
            prompt: "test".to_string(),
            parent_id: None,
        };
        let agent = SubAgent::new(task);
        let names: Vec<String> = agent
            .tool_router
            .model_visible_specs()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"glob".to_string()));
        assert!(names.contains(&"grep".to_string()));
        assert!(!names.contains(&"write_file".to_string()));
        assert!(!names.contains(&"bash".to_string()));
    }

    /// 测试 Plan Agent 工具集（只读 + bash）
    #[test]
    fn test_plan_agent_tools() {
        let task = AgentTask {
            id: "test_plan".to_string(),
            agent_type: AgentType::Plan,
            prompt: "test".to_string(),
            parent_id: None,
        };
        let agent = SubAgent::new(task);
        let names: Vec<String> = agent
            .tool_router
            .model_visible_specs()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"bash".to_string()));
        assert!(!names.contains(&"write_file".to_string()));
    }

    /// 测试 General Agent 工具集（所有工具）
    #[test]
    fn test_general_agent_tools() {
        let task = AgentTask {
            id: "test_general".to_string(),
            agent_type: AgentType::General,
            prompt: "test".to_string(),
            parent_id: None,
        };
        let agent = SubAgent::new(task);
        let names: Vec<String> = agent
            .tool_router
            .model_visible_specs()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"write_file".to_string()));
        assert!(names.contains(&"bash".to_string()));
        assert!(names.contains(&"glob".to_string()));
    }

    /// 测试子 Agent Mock 执行
    #[tokio::test]
    async fn test_sub_agent_mock_execute() {
        let task = AgentTask {
            id: "test_exec".to_string(),
            agent_type: AgentType::Explore,
            prompt: "搜索 Rust 文件".to_string(),
            parent_id: None,
        };
        let mut agent = SubAgent::new(task);
        let result = agent.execute_mock().await;
        assert!(result.success);
        assert!(!result.output.is_empty());
        assert_eq!(result.task_id, "test_exec");
    }

    // ---- 协调器测试 ----

    /// 测试协调器 Fork
    #[tokio::test]
    async fn test_coordinator_fork() {
        let mut coordinator = AgentCoordinator::new();
        let task_id = coordinator
            .fork(AgentType::Explore, "test task", None)
            .await;

        assert!(!task_id.is_empty());
        let result = coordinator.get_result(&task_id);
        assert!(result.is_some());
        assert!(result.unwrap().success);
    }

    /// 测试协调器并行 Fork
    #[tokio::test]
    async fn test_coordinator_parallel_fork() {
        let mut coordinator = AgentCoordinator::new();
        let ids = coordinator
            .fork_parallel(vec![
                (AgentType::Explore, "task 1".to_string()),
                (AgentType::Plan, "task 2".to_string()),
            ])
            .await;

        assert_eq!(ids.len(), 2);
        assert_eq!(coordinator.all_results().len(), 2);
    }

    /// 测试协调器汇总报告
    #[tokio::test]
    async fn test_coordinator_summary() {
        let mut coordinator = AgentCoordinator::new();
        coordinator
            .fork(AgentType::Explore, "task", None)
            .await;

        let summary = coordinator.summary();
        assert!(summary.contains("总任务: 1"));
        assert!(summary.contains("成功: 1"));
    }

    /// 测试父子任务关联
    #[tokio::test]
    async fn test_parent_child_relationship() {
        let mut coordinator = AgentCoordinator::new();
        let parent_id = coordinator
            .fork(AgentType::Explore, "parent task", None)
            .await;

        let child_id = coordinator
            .fork(AgentType::Plan, "child task", Some(&parent_id))
            .await;

        let child = coordinator.get_result(&child_id).unwrap();
        // 子任务的 task_id 有效
        assert!(!child.task_id.is_empty());
    }

    /// 测试 truncate 辅助函数
    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    /// 测试 indent 辅助函数
    #[test]
    fn test_indent() {
        let result = indent("line1\nline2", 2);
        assert_eq!(result, "  line1\n  line2");
    }
}
