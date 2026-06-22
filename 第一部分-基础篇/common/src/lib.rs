//! 共享类型库 —— 对应 Claude Code 的核心类型定义
//!
//! Claude Code 的核心类型分布在多个模块中：
//! - src/Tool.ts: 工具接口定义
//! - src/query.ts: 对话循环状态
//! - src/services/tools/: 工具编排
//!
//! 我们的 common crate 遵循同样的模式，避免章节间重复定义。

pub mod types;
pub mod llm;
pub mod tools;

pub use types::*;
pub use llm::ClaudeClient;
pub use tools::{ToolHandler, ToolRouter, ToolRegistry, ReadFileTool, WriteFileTool, EditFileTool, BashTool, GlobTool, GrepTool};
