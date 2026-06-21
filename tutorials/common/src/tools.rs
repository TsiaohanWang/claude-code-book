//! 工具系统 —— 对应 Claude Code 的工具注册和路由
//!
//! Claude Code 的工具系统在 src/Tool.ts 和 src/services/tools/ 中定义。
//! 每个工具实现 Tool 接口的五要素：名称、Schema、权限、执行、UI 渲染。
//! 我们简化为核心三要素：名称、Schema、执行。

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::types::{ToolResult, ToolSpec};

// ============================================================================
// 工具处理器 trait —— 对应 Claude Code 的 Tool 接口
// ============================================================================

/// 工具处理器 trait —— 对应 Claude Code 的 Tool<Input, Output, Progress>
///
/// Claude Code 中每个工具必须实现：
/// - name(): 工具名称
/// - inputSchema: Zod Schema（运行时验证 + API 文档）
/// - call(): 执行逻辑
/// - isReadOnly/isConcurrencySafe: 安全属性
/// - renderToolUseMessage/renderToolResultMessage: UI 渲染
///
/// 我们保留最核心的方法：name、spec、execute。
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;
    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult;
}

// ============================================================================
// 工具注册表 —— 对应 Claude Code 的 getAllBaseTools()
// ============================================================================

/// 工具注册表 —— 对应 Claude Code 的工具注册中心
///
/// Claude Code 的工具注册在 src/tools.ts 中，
/// 通过 getAllBaseTools() 返回所有可用工具。
pub struct ToolRegistry {
    handlers: HashMap<String, Box<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, handler: Box<dyn ToolHandler>) {
        let name = handler.name().to_string();
        self.handlers.insert(name, handler);
    }

    pub fn get(&self, name: &str) -> Option<&dyn ToolHandler> {
        self.handlers.get(name).map(|h| h.as_ref())
    }

    pub fn all_specs(&self) -> Vec<ToolSpec> {
        self.handlers.values().map(|h| h.spec()).collect()
    }

    pub fn len(&self) -> usize {
        self.handlers.len()
    }
}

// ============================================================================
// 工具路由器 —— 对应 Claude Code 的工具编排引擎
// ============================================================================

/// 工具路由器 —— 对应 Claude Code 的 runTools()
///
/// 负责将模型的工具调用分发到正确的处理器。
/// Claude Code 中还有并发分区、流式执行等高级功能，
/// 我们简化为顺序执行。
pub struct ToolRouter {
    registry: ToolRegistry,
    model_visible_specs: Vec<ToolSpec>,
}

impl ToolRouter {
    pub fn new(registry: ToolRegistry) -> Self {
        let model_visible_specs = registry.all_specs();
        Self {
            registry,
            model_visible_specs,
        }
    }

    pub fn model_visible_specs(&self) -> &[ToolSpec] {
        &self.model_visible_specs
    }

    pub fn execute(&self, call_id: &str, name: &str, input: &serde_json::Value) -> ToolResult {
        match self.registry.get(name) {
            Some(handler) => handler.execute(call_id, input),
            None => ToolResult {
                call_id: call_id.to_string(),
                output: format!("Unknown tool: {name}"),
                is_error: true,
                wall_time: Duration::ZERO,
            },
        }
    }
}

// ============================================================================
// 内置工具实现 —— 对应 Claude Code 的 66+ 内置工具
// ============================================================================

/// 读文件工具 —— 对应 Claude Code 的 FileReadTool
pub struct ReadFileTool;

impl ToolHandler for ReadFileTool {
    fn name(&self) -> &str { "read_file" }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read the contents of a file. Returns the file content with line numbers.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "The path to the file to read" }
                },
                "required": ["file_path"]
            }),
        }
    }

    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let start = Instant::now();

        match std::fs::read_to_string(path) {
            Ok(content) => {
                let lines: Vec<String> = content
                    .lines()
                    .enumerate()
                    .map(|(i, line)| format!("{:>4}: {}", i + 1, line))
                    .collect();
                let output = lines.join("\n");
                let truncated = if output.len() > 50000 {
                    format!("{}\n\n... (truncated)", &output[..50000])
                } else {
                    output
                };
                ToolResult {
                    call_id: call_id.to_string(),
                    output: truncated,
                    is_error: false,
                    wall_time: start.elapsed(),
                }
            }
            Err(e) => ToolResult {
                call_id: call_id.to_string(),
                output: format!("Error reading file: {e}"),
                is_error: true,
                wall_time: start.elapsed(),
            },
        }
    }
}

/// 写文件工具 —— 对应 Claude Code 的 FileWriteTool
pub struct WriteFileTool;

impl ToolHandler for WriteFileTool {
    fn name(&self) -> &str { "write_file" }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".to_string(),
            description: "Write content to a file. Creates the file if it doesn't exist, overwrites if it does.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "The path to the file to write" },
                    "content": { "type": "string", "description": "The content to write to the file" }
                },
                "required": ["file_path", "content"]
            }),
        }
    }

    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let start = Instant::now();

        match std::fs::write(path, content) {
            Ok(()) => ToolResult {
                call_id: call_id.to_string(),
                output: format!("Successfully wrote to {}", path),
                is_error: false,
                wall_time: start.elapsed(),
            },
            Err(e) => ToolResult {
                call_id: call_id.to_string(),
                output: format!("Error writing file: {e}"),
                is_error: true,
                wall_time: start.elapsed(),
            },
        }
    }
}

/// 编辑文件工具 —— 对应 Claude Code 的 FileEditTool
///
/// Claude Code 使用 search-and-replace 模式，而非行号或 AST。
/// 这是其"抗幻觉"设计的核心。
pub struct EditFileTool;

impl ToolHandler for EditFileTool {
    fn name(&self) -> &str { "edit_file" }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".to_string(),
            description: "Edit a file by replacing an exact string match with new content. The old_string must match exactly.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "The path to the file to edit" },
                    "old_string": { "type": "string", "description": "The exact string to find and replace" },
                    "new_string": { "type": "string", "description": "The string to replace it with" }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
        }
    }

    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult {
        let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let old_string = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
        let new_string = input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
        let start = Instant::now();

        match std::fs::read_to_string(path) {
            Ok(content) => {
                if !content.contains(old_string) {
                    return ToolResult {
                        call_id: call_id.to_string(),
                        output: format!("Error: old_string not found in {path}"),
                        is_error: true,
                        wall_time: start.elapsed(),
                    };
                }
                let count = content.matches(old_string).count();
                if count > 1 {
                    return ToolResult {
                        call_id: call_id.to_string(),
                        output: format!("Error: old_string found {count} times in {path}. Must be unique."),
                        is_error: true,
                        wall_time: start.elapsed(),
                    };
                }
                let new_content = content.replace(old_string, new_string);
                match std::fs::write(path, &new_content) {
                    Ok(()) => ToolResult {
                        call_id: call_id.to_string(),
                        output: format!("Successfully edited {path}"),
                        is_error: false,
                        wall_time: start.elapsed(),
                    },
                    Err(e) => ToolResult {
                        call_id: call_id.to_string(),
                        output: format!("Error writing file: {e}"),
                        is_error: true,
                        wall_time: start.elapsed(),
                    },
                }
            }
            Err(e) => ToolResult {
                call_id: call_id.to_string(),
                output: format!("Error reading file: {e}"),
                is_error: true,
                wall_time: start.elapsed(),
            },
        }
    }
}

/// Bash 工具 —— 对应 Claude Code 的 BashTool
///
/// Claude Code 的 BashTool 是最复杂的工具，包含：
/// - AST 解析和 23 项静态安全检查
/// - 沙箱隔离（macOS Seatbelt / Linux 命名空间）
/// - 命令分类（搜索/读取 vs 写入）
/// - 超时控制
///
/// 我们简化为基本的命令执行。
pub struct BashTool;

impl ToolHandler for BashTool {
    fn name(&self) -> &str { "bash" }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".to_string(),
            description: "Execute a shell command. Use for system commands and terminal operations.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The shell command to execute" }
                },
                "required": ["command"]
            }),
        }
    }

    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let start = Instant::now();

        match Command::new("sh").arg("-c").arg(command).output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                let mut content = String::new();
                content.push_str(&stdout);
                if !stderr.is_empty() {
                    if !content.is_empty() && !content.ends_with('\n') {
                        content.push('\n');
                    }
                    content.push_str(&stderr);
                }
                if content.is_empty() {
                    content = "(no output)".to_string();
                }

                ToolResult {
                    call_id: call_id.to_string(),
                    output: content,
                    is_error: exit_code != 0,
                    wall_time: start.elapsed(),
                }
            }
            Err(e) => ToolResult {
                call_id: call_id.to_string(),
                output: format!("Error executing command: {e}"),
                is_error: true,
                wall_time: start.elapsed(),
            },
        }
    }
}

/// Glob 工具 —— 对应 Claude Code 的 GlobTool
pub struct GlobTool;

impl ToolHandler for GlobTool {
    fn name(&self) -> &str { "glob" }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern. Returns matching file paths.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match files" },
                    "path": { "type": "string", "description": "Directory to search in (default: current directory)" }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("**/*");
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let start = Instant::now();

        let full_pattern = format!("{}/{}", path.trim_end_matches('/'), pattern);
        match glob::glob(&full_pattern) {
            Ok(paths) => {
                let results: Vec<String> = paths
                    .filter_map(|p| p.ok())
                    .filter(|p| p.is_file())
                    .map(|p| p.display().to_string())
                    .take(100)
                    .collect();
                ToolResult {
                    call_id: call_id.to_string(),
                    output: results.join("\n"),
                    is_error: false,
                    wall_time: start.elapsed(),
                }
            }
            Err(e) => ToolResult {
                call_id: call_id.to_string(),
                output: format!("Error: {e}"),
                is_error: true,
                wall_time: start.elapsed(),
            },
        }
    }
}

/// Grep 工具 —— 对应 Claude Code 的 GrepTool
pub struct GrepTool;

impl ToolHandler for GrepTool {
    fn name(&self) -> &str { "grep" }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".to_string(),
            description: "Search file contents using a regex pattern. Returns matching lines.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "Directory to search in" },
                    "include": { "type": "string", "description": "File pattern to include (e.g., '*.ts')" }
                },
                "required": ["pattern"]
            }),
        }
    }

    fn execute(&self, call_id: &str, input: &serde_json::Value) -> ToolResult {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let start = Instant::now();

        let mut cmd = Command::new("grep");
        cmd.arg("-r").arg("-n").arg(pattern).arg(path);
        if let Some(include) = input.get("include").and_then(|v| v.as_str()) {
            cmd.arg("--include").arg(include);
        }

        match cmd.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let results: String = stdout.lines().take(100).collect::<Vec<_>>().join("\n");
                ToolResult {
                    call_id: call_id.to_string(),
                    output: results,
                    is_error: false,
                    wall_time: start.elapsed(),
                }
            }
            Err(e) => ToolResult {
                call_id: call_id.to_string(),
                output: format!("Error: {e}"),
                is_error: true,
                wall_time: start.elapsed(),
            },
        }
    }
}
