// =============================================================================
// 第九章：MCP 客户端 —— 扩展工具生态
//
// 本文件实现 Model Context Protocol（MCP）客户端，连接外部工具服务器。
//
// 对应 claude-code-book 第 13 章（MCP 协议 —— 工具生态的标准化）。
//
// 核心概念：
// - JSON-RPC 2.0：MCP 的通信协议
// - 工具发现：通过 tools/list 获取服务器提供的工具
// - 三段式命名：server:tool（如 github:create_issue）
// - 工具代理：将 MCP 工具包装为本地 ToolHandler
//
// Claude Code 的 MCP 客户端在 src/services/mcp/ 中：
//   McpClient → 连接 MCP 服务器 → 发现工具 → 注册到 ToolRouter
//
// 运行方式：
//   cargo run -p ch09-mcp
// =============================================================================

use anyhow::Result;
use mini_claude_common::{
    BashTool, GrepTool, ReadFileTool, ToolHandler, ToolRegistry, ToolResult, ToolRouter, ToolSpec,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// =============================================================================
// 第一部分：JSON-RPC 2.0 协议类型
//
// MCP 基于 JSON-RPC 2.0 协议（https://www.jsonrpc.org/specification）。
//
// Claude Code 的 MCP 通信使用三种消息：
//   - Request: { jsonrpc: "2.0", id, method, params }
//   - Response: { jsonrpc: "2.0", id, result | error }
//   - Notification: { jsonrpc: "2.0", method, params }（无 id，无需响应）
// =============================================================================

/// JSON-RPC 2.0 请求
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 响应
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 错误
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// =============================================================================
// 第二部分：MCP 协议方法
//
// MCP 定义的标准方法：
//   - initialize: 初始化连接，协商协议版本和能力
//   - tools/list: 列出服务器提供的工具
//   - tools/call: 调用服务器上的工具
//   - resources/list: 列出服务器提供的资源
//   - resources/read: 读取资源内容
//
// 对应 Claude Code 的 MCP 协议实现。
// =============================================================================

/// MCP 初始化请求参数
#[derive(Debug, Serialize)]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

/// 客户端能力
#[derive(Debug, Serialize)]
pub struct ClientCapabilities {
    pub tools: Option<ToolsCapability>,
}

/// 工具能力
#[derive(Debug, Serialize)]
pub struct ToolsCapability {
    pub list_changed: bool,
}

/// 客户端信息
#[derive(Debug, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// MCP 工具定义 —— 来自 tools/list 响应
///
/// 对应 MCP 协议的 Tool 类型：
/// { name, description, inputSchema }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// MCP 工具调用结果
#[derive(Debug, Serialize, Deserialize)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    #[serde(default)]
    pub is_error: bool,
}

/// MCP 内容块
#[derive(Debug, Serialize, Deserialize)]
pub struct McpContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: Option<String>,
}

// =============================================================================
// 第三部分：MCP 客户端
//
// 对应 Claude Code 的 McpClient（src/services/mcp/client.ts）。
//
// Claude Code 的 MCP 客户端支持多种传输方式：
//   - stdio: 通过标准输入/输出通信（最常用）
//   - SSE: 通过 HTTP SSE 通信
//   - WebSocket: 通过 WebSocket 通信
//
// 我们实现一个 Mock MCP 客户端，演示完整的协议流程。
// =============================================================================

/// MCP 传输层 trait
///
/// Claude Code 支持多种传输方式，我们用 trait 抽象。
pub trait McpTransport: Send + Sync {
    /// 发送请求并接收响应
    fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse>;
}

/// MCP 客户端
///
/// 对应 Claude Code 的 MCP 客户端，负责：
/// 1. 连接 MCP 服务器
/// 2. 初始化协议
/// 3. 发现工具
/// 4. 调用工具
pub struct McpClient {
    transport: Box<dyn McpTransport>,
    server_name: String,
    tools: Vec<McpToolDefinition>,
    next_id: u64,
}

impl McpClient {
    pub fn new(transport: Box<dyn McpTransport>, server_name: String) -> Self {
        Self {
            transport,
            server_name,
            tools: Vec::new(),
            next_id: 1,
        }
    }

    /// 初始化 MCP 连接
    ///
    /// 对应 MCP 协议的 initialize 方法。
    pub fn initialize(&mut self) -> Result<()> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "clientInfo": { "name": "mini-claude", "version": "0.1.0" }
            })),
        };

        let response = self.transport.send_request(&request)?;
        if let Some(error) = response.error {
            anyhow::bail!("MCP 初始化失败: {}", error.message);
        }

        tracing::info!(
            "MCP 服务器 {} 初始化成功",
            self.server_name
        );
        Ok(())
    }

    /// 发现服务器提供的工具
    ///
    /// 对应 MCP 协议的 tools/list 方法。
    pub fn discover_tools(&mut self) -> Result<Vec<McpToolDefinition>> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "tools/list".to_string(),
            params: None,
        };

        let response = self.transport.send_request(&request)?;
        if let Some(error) = response.error {
            anyhow::bail!("工具发现失败: {}", error.message);
        }

        let tools_value = response
            .result
            .and_then(|r| r.get("tools").cloned())
            .unwrap_or(serde_json::json!([]));

        let tools: Vec<McpToolDefinition> = serde_json::from_value(tools_value)?;
        self.tools = tools.clone();

        tracing::info!(
            "从 {} 发现 {} 个工具",
            self.server_name,
            tools.len()
        );

        Ok(tools)
    }

    /// 调用远程工具
    ///
    /// 对应 MCP 协议的 tools/call 方法。
    pub fn call_tool(&mut self, name: &str, arguments: &serde_json::Value) -> Result<McpToolResult> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: self.next_id(),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": name,
                "arguments": arguments,
            })),
        };

        let response = self.transport.send_request(&request)?;
        if let Some(error) = response.error {
            anyhow::bail!("工具调用失败: {}", error.message);
        }

        let result: McpToolResult = serde_json::from_value(
            response.result.unwrap_or(serde_json::json!({})),
        )?;

        Ok(result)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

// =============================================================================
// 第四部分：MCP 工具代理
//
// 将 MCP 远程工具包装为本地 ToolHandler，注册到 ToolRouter 中。
//
// Claude Code 的 MCP 工具代理在 src/services/mcp/tools.ts 中：
//   MCP 工具定义 → 包装为 Tool 接口 → 注册到工具系统
//
// 三段式命名规则：
//   server_name:tool_name（如 github:create_issue）
//   避免不同 MCP 服务器的工具名冲突。
// =============================================================================

/// MCP 工具代理 —— 将远程 MCP 工具包装为本地 ToolHandler
///
/// 对应 Claude Code 的 MCP 工具包装器。
/// 使用三段式命名: server_name:tool_name
pub struct McpToolProxy {
    server_name: String,
    tool_def: McpToolDefinition,
}

impl McpToolProxy {
    pub fn new(server_name: String, tool_def: McpToolDefinition) -> Self {
        Self {
            server_name,
            tool_def,
        }
    }

    /// 三段式名称: server:tool
    pub fn qualified_name(&self) -> String {
        format!("{}:{}", self.server_name, self.tool_def.name)
    }
}

impl ToolHandler for McpToolProxy {
    fn name(&self) -> &str {
        // 注意：这里返回原始名，qualified_name 用于注册
        // 实际 Claude Code 中使用 qualified_name
        &self.tool_def.name
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.qualified_name(),
            description: format!(
                "[MCP:{}] {}",
                self.server_name, self.tool_def.description
            ),
            input_schema: self.tool_def.input_schema.clone(),
        }
    }

    fn execute(&self, call_id: &str, _input: &serde_json::Value) -> ToolResult {
        // 实际实现需要通过 McpClient 调用远程工具
        // 这里返回提示信息
        ToolResult {
            call_id: call_id.to_string(),
            output: format!(
                "[MCP 代理] 工具 {} 需要通过 MCP 客户端调用远程服务器 {}",
                self.qualified_name(),
                self.server_name
            ),
            is_error: false,
            wall_time: Duration::ZERO,
        }
    }
}

// =============================================================================
// 第五部分：Mock MCP 传输层
//
// 模拟 MCP 服务器的响应，用于测试和演示。
// =============================================================================

/// Mock MCP 传输层 —— 模拟 MCP 服务器
pub struct MockMcpTransport {
    /// 服务器名称
    pub server_name: String,
    /// 提供的工具列表
    pub tools: Vec<McpToolDefinition>,
}

impl MockMcpTransport {
    pub fn github() -> Self {
        Self {
            server_name: "github".to_string(),
            tools: vec![
                McpToolDefinition {
                    name: "create_issue".to_string(),
                    description: "创建 GitHub Issue".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "title": { "type": "string", "description": "Issue 标题" },
                            "body": { "type": "string", "description": "Issue 内容" },
                            "labels": { "type": "array", "items": { "type": "string" }, "description": "标签列表" }
                        },
                        "required": ["title"]
                    }),
                },
                McpToolDefinition {
                    name: "search_repos".to_string(),
                    description: "搜索 GitHub 仓库".to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "搜索关键词" }
                        },
                        "required": ["query"]
                    }),
                },
            ],
        }
    }

    pub fn slack() -> Self {
        Self {
            server_name: "slack".to_string(),
            tools: vec![McpToolDefinition {
                name: "send_message".to_string(),
                description: "发送 Slack 消息".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "channel": { "type": "string", "description": "频道名" },
                        "text": { "type": "string", "description": "消息内容" }
                    },
                    "required": ["channel", "text"]
                }),
            }],
        }
    }
}

impl McpTransport for MockMcpTransport {
    fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        match request.method.as_str() {
            "initialize" => Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": self.server_name, "version": "1.0.0" }
                })),
                error: None,
            }),
            "tools/list" => Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: Some(serde_json::json!({ "tools": self.tools })),
                error: None,
            }),
            "tools/call" => {
                let params = request.params.as_ref().unwrap();
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

                // 模拟工具执行
                let result_text = match tool_name {
                    "create_issue" => {
                        let title = arguments.get("title").and_then(|v| v.as_str()).unwrap_or("无标题");
                        format!("[GitHub] Issue 已创建: \"{title}\" (id: #42)")
                    }
                    "search_repos" => {
                        let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
                        format!("[GitHub] 搜索 \"{query}\" 找到 3 个仓库")
                    }
                    "send_message" => {
                        let channel = arguments.get("channel").and_then(|v| v.as_str()).unwrap_or("");
                        format!("[Slack] 消息已发送到 #{channel}")
                    }
                    _ => format!("[Mock] 未知工具: {tool_name}"),
                };

                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: Some(serde_json::json!({
                        "content": [{ "type": "text", "text": result_text }],
                        "isError": false
                    })),
                    error: None,
                })
            }
            _ => Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("未知方法: {}", request.method),
                    data: None,
                }),
            }),
        }
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

    println!("=== Ch09: MCP 客户端 ===");
    println!("对应: 第 13 章（MCP 协议 —— 工具生态的标准化）");
    println!();

    // ---- 连接 GitHub MCP 服务器 ----
    println!("--- 连接 GitHub MCP 服务器 ---");
    let github_transport = Box::new(MockMcpTransport::github());
    let mut github_client = McpClient::new(github_transport, "github".to_string());

    github_client.initialize()?;
    let github_tools = github_client.discover_tools()?;
    println!("发现 {} 个 GitHub 工具:", github_tools.len());
    for tool in &github_tools {
        println!("  - github:{}: {}", tool.name, tool.description);
    }
    println!();

    // 调用 GitHub 工具
    println!("--- 调用 GitHub 工具 ---");
    let result = github_client.call_tool(
        "create_issue",
        &serde_json::json!({"title": "修复登录 bug", "body": "用户无法登录", "labels": ["bug"]}),
    )?;
    for content in &result.content {
        if let Some(ref text) = content.text {
            println!("  {text}");
        }
    }
    println!();

    let result = github_client.call_tool(
        "search_repos",
        &serde_json::json!({"query": "rust mcp"}),
    )?;
    for content in &result.content {
        if let Some(ref text) = content.text {
            println!("  {text}");
        }
    }
    println!();

    // ---- 连接 Slack MCP 服务器 ----
    println!("--- 连接 Slack MCP 服务器 ---");
    let slack_transport = Box::new(MockMcpTransport::slack());
    let mut slack_client = McpClient::new(slack_transport, "slack".to_string());

    slack_client.initialize()?;
    let slack_tools = slack_client.discover_tools()?;
    println!("发现 {} 个 Slack 工具:", slack_tools.len());
    for tool in &slack_tools {
        println!("  - slack:{}: {}", tool.name, tool.description);
    }
    println!();

    // 调用 Slack 工具
    println!("--- 调用 Slack 工具 ---");
    let result = slack_client.call_tool(
        "send_message",
        &serde_json::json!({"channel": "general", "text": "Hello from MCP!"}),
    )?;
    for content in &result.content {
        if let Some(ref text) = content.text {
            println!("  {text}");
        }
    }
    println!();

    // ---- 演示三段式命名 ----
    println!("--- 三段式命名示例 ---");
    let all_tools: Vec<String> = github_tools
        .iter()
        .map(|t| format!("github:{}", t.name))
        .chain(slack_tools.iter().map(|t| format!("slack:{}", t.name)))
        .collect();
    for name in &all_tools {
        println!("  {name}");
    }
    println!();

    // ---- 注册 MCP 工具到 ToolRouter ----
    println!("--- 注册 MCP 工具到 ToolRouter ---");
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(BashTool));
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(GrepTool));

    // 将 GitHub 工具注册为代理
    for tool_def in &github_tools {
        let proxy = McpToolProxy::new("github".to_string(), tool_def.clone());
        println!("  注册: {}", proxy.qualified_name());
        registry.register(Box::new(proxy));
    }

    let router = ToolRouter::new(registry);
    println!("总工具数: {}", router.model_visible_specs().len());
    println!();

    println!("(MCP 演示完成。MCP 协议使 Claude Code 可以连接任意工具服务器。)");

    Ok(())
}

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- JSON-RPC 协议测试 ----

    /// 测试 JSON-RPC 请求序列化
    #[test]
    fn test_jsonrpc_request_serialization() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "tools/list".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"tools/list\""));
        assert!(json.contains("\"id\":1"));
    }

    /// 测试 JSON-RPC 响应反序列化
    #[test]
    fn test_jsonrpc_response_deserialization() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, 1);
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    /// 测试 JSON-RPC 错误响应
    #[test]
    fn test_jsonrpc_error_response() {
        let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, -32601);
    }

    // ---- MCP 工具定义测试 ----

    /// 测试 MCP 工具定义序列化/反序列化
    #[test]
    fn test_mcp_tool_definition() {
        let tool = McpToolDefinition {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "arg": { "type": "string" } }
            }),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let deserialized: McpToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test_tool");
    }

    // ---- Mock 传输层测试 ----

    /// 测试 Mock GitHub 传输层初始化
    #[test]
    fn test_mock_github_initialize() {
        let transport = MockMcpTransport::github();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "initialize".to_string(),
            params: Some(serde_json::json!({})),
        };
        let response = transport.send_request(&request).unwrap();
        assert!(response.error.is_none());
        assert!(response.result.is_some());
    }

    /// 测试 Mock 工具发现
    #[test]
    fn test_mock_tools_list() {
        let transport = MockMcpTransport::github();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 2,
            method: "tools/list".to_string(),
            params: None,
        };
        let response = transport.send_request(&request).unwrap();
        let tools = response.result.unwrap();
        let tools_arr = tools.get("tools").unwrap().as_array().unwrap();
        assert_eq!(tools_arr.len(), 2);
    }

    /// 测试 Mock 工具调用
    #[test]
    fn test_mock_tool_call() {
        let transport = MockMcpTransport::github();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 3,
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({
                "name": "create_issue",
                "arguments": { "title": "Test Issue" }
            })),
        };
        let response = transport.send_request(&request).unwrap();
        let result: McpToolResult = serde_json::from_value(response.result.unwrap()).unwrap();
        assert!(!result.is_error);
        assert!(!result.content.is_empty());
        assert!(result.content[0].text.as_ref().unwrap().contains("Test Issue"));
    }

    // ---- MCP 客户端测试 ----

    /// 测试 MCP 客户端完整流程
    #[test]
    fn test_mcp_client_full_flow() {
        let transport = Box::new(MockMcpTransport::github());
        let mut client = McpClient::new(transport, "github".to_string());

        // 初始化
        assert!(client.initialize().is_ok());

        // 发现工具
        let tools = client.discover_tools().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "create_issue");

        // 调用工具
        let result = client
            .call_tool("create_issue", &serde_json::json!({"title": "Hello"}))
            .unwrap();
        assert!(!result.is_error);
    }

    // ---- MCP 工具代理测试 ----

    /// 测试三段式命名
    #[test]
    fn test_qualified_name() {
        let tool_def = McpToolDefinition {
            name: "create_issue".to_string(),
            description: "Create issue".to_string(),
            input_schema: serde_json::json!({}),
        };
        let proxy = McpToolProxy::new("github".to_string(), tool_def);
        assert_eq!(proxy.qualified_name(), "github:create_issue");
    }

    /// 测试 MCP 工具代理 spec
    #[test]
    fn test_mcp_proxy_spec() {
        let tool_def = McpToolDefinition {
            name: "send_message".to_string(),
            description: "Send a message".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let proxy = McpToolProxy::new("slack".to_string(), tool_def);
        let spec = proxy.spec();
        assert_eq!(spec.name, "slack:send_message");
        assert!(spec.description.contains("[MCP:slack]"));
    }

    // ---- 跨服务器工具测试 ----

    /// 测试多个 MCP 服务器的工具不冲突
    #[test]
    fn test_multiple_servers_no_collision() {
        let mut github_client = McpClient::new(
            Box::new(MockMcpTransport::github()),
            "github".to_string(),
        );
        github_client.initialize().unwrap();
        let github_tools = github_client.discover_tools().unwrap();

        let mut slack_client = McpClient::new(
            Box::new(MockMcpTransport::slack()),
            "slack".to_string(),
        );
        slack_client.initialize().unwrap();
        let slack_tools = slack_client.discover_tools().unwrap();

        // 三段式命名确保不冲突
        let all_names: Vec<String> = github_tools
            .iter()
            .map(|t| format!("github:{}", t.name))
            .chain(slack_tools.iter().map(|t| format!("slack:{}", t.name)))
            .collect();

        assert_eq!(all_names.len(), 3);
        assert!(all_names.contains(&"github:create_issue".to_string()));
        assert!(all_names.contains(&"github:search_repos".to_string()));
        assert!(all_names.contains(&"slack:send_message".to_string()));
    }
}
