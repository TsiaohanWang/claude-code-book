//! LLM 客户端 —— 对应 Claude Code 的 API 通信层
//!
//! Claude Code 使用 Anthropic Messages API（/v1/messages 端点），
//! 支持流式响应（SSE）和工具调用。
//!
//! 与 Codex 使用 OpenAI Responses API 不同，Claude Code 使用：
//! - 端点: POST /v1/messages
//! - 消息格式: { role, content } 其中 content 可以是字符串或内容块数组
//! - 工具格式: { name, description, input_schema }
//! - 流式事件: message_start, content_block_start, content_block_delta,
//!             content_block_stop, message_delta, message_stop

use anyhow::{Context, Result};
use eventsource_stream::{Event, EventStream};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::types::{Message, ResponseItem, ToolCallInfo, ToolSpec};

// ============================================================================
// Anthropic API 类型
// ============================================================================

/// Anthropic Messages API 请求体
#[derive(Debug, Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    stream: bool,
}

/// API 消息格式
#[derive(Debug, Serialize, Deserialize)]
struct ApiMessage {
    role: String,
    content: serde_json::Value,
}

/// 流式响应事件
#[derive(Debug)]
pub enum StreamEvent {
    /// 文本增量
    TextDelta(String),
    /// 工具调用开始
    ToolUseStart { id: String, name: String },
    /// 工具调用输入增量
    ToolUseDelta { id: String, input_delta: String },
    /// 工具调用结束
    ToolUseStop { id: String },
    /// 消息完成
    MessageDone,
}

// ============================================================================
// Claude 客户端
// ============================================================================

/// Claude API 客户端 —— 对应 Claude Code 的 API 通信层
///
/// Claude Code 的 API 通信在 src/services/api/ 中，
/// 负责构建请求、解析流式响应、处理错误。
pub struct ClaudeClient {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    max_tokens: u32,
}

impl ClaudeClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            model,
            max_tokens: 4096,
        }
    }

    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// 构建请求头
    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(&self.api_key).unwrap());
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers
    }

    /// 发送非流式请求 —— 对应 Claude Code 的非流式 API 调用
    pub async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        system: Option<&str>,
    ) -> Result<ResponseItem> {
        let api_messages: Vec<ApiMessage> = messages
            .iter()
            .map(|m| ApiMessage {
                role: m.role.clone(),
                content: serde_json::Value::String(m.content.clone()),
            })
            .collect();

        let tools_json: Option<Vec<serde_json::Value>> = if tools.is_empty() {
            None
        } else {
            Some(tools.iter().map(|t| t.to_api_format()).collect())
        };

        let body = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            messages: api_messages,
            system: system.map(|s| s.to_string()),
            tools: tools_json,
            stream: false,
        };

        let url = format!("{}/v1/messages", self.base_url);
        info!("发送请求到: {}", url);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await
            .context("发送请求失败")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("API 请求失败 ({}): {}", status, body);
        }

        let resp: serde_json::Value = response.json().await?;
        self.parse_response(&resp)
    }

    /// 发送流式请求 —— 对应 Claude Code 的流式 API 调用
    ///
    /// Claude Code 使用 SSE 流式接收响应，每个事件包含：
    /// - message_start: 消息开始
    /// - content_block_start: 内容块开始（text 或 tool_use）
    /// - content_block_delta: 内容增量
    /// - content_block_stop: 内容块结束
    /// - message_delta: 消息级别更新
    /// - message_stop: 消息结束
    pub async fn send_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        system: Option<&str>,
    ) -> Result<Vec<StreamEvent>> {
        let api_messages: Vec<ApiMessage> = messages
            .iter()
            .map(|m| ApiMessage {
                role: m.role.clone(),
                content: serde_json::Value::String(m.content.clone()),
            })
            .collect();

        let tools_json: Option<Vec<serde_json::Value>> = if tools.is_empty() {
            None
        } else {
            Some(tools.iter().map(|t| t.to_api_format()).collect())
        };

        let body = MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            messages: api_messages,
            system: system.map(|s| s.to_string()),
            tools: tools_json,
            stream: true,
        };

        let url = format!("{}/v1/messages", self.base_url);
        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await
            .context("发送请求失败")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("API 请求失败 ({}): {}", status, body);
        }

        let byte_stream = response.bytes_stream();
        let mut event_stream = EventStream::new(byte_stream);
        let mut events = Vec::new();

        while let Some(event_result) = event_stream.next().await {
            match event_result {
                Ok(event) => {
                    if let Some(stream_event) = self.parse_sse_event(&event)? {
                        events.push(stream_event);
                    }
                }
                Err(e) => {
                    warn!("SSE 流错误: {:?}", e);
                    break;
                }
            }
        }

        Ok(events)
    }

    /// 解析非流式响应
    fn parse_response(&self, resp: &serde_json::Value) -> Result<ResponseItem> {
        let content = resp
            .get("content")
            .and_then(|c| c.as_array())
            .context("响应缺少 content 字段")?;

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(text.to_string());
                    }
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));
                    tool_calls.push(ToolCallInfo { id, name, input });
                }
                _ => {}
            }
        }

        if !tool_calls.is_empty() {
            Ok(ResponseItem::ToolUse { calls: tool_calls })
        } else {
            Ok(ResponseItem::Message {
                content: text_parts.join(""),
            })
        }
    }

    /// 解析 SSE 事件
    fn parse_sse_event(&self, event: &Event) -> Result<Option<StreamEvent>> {
        let event_type = event.event.as_str();

        match event_type {
            "content_block_start" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                let block = data.get("content_block").context("缺少 content_block")?;
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("tool_use") => {
                        let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        Ok(Some(StreamEvent::ToolUseStart { id, name }))
                    }
                    _ => Ok(None),
                }
            }
            "content_block_delta" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                let delta = data.get("delta").context("缺少 delta")?;
                match delta.get("type").and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        let text = delta.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        Ok(Some(StreamEvent::TextDelta(text.to_string())))
                    }
                    Some("input_json_delta") => {
                        let partial = delta.get("partial_json").and_then(|p| p.as_str()).unwrap_or("");
                        let id = data.get("index").map(|i| i.to_string()).unwrap_or_default();
                        Ok(Some(StreamEvent::ToolUseDelta {
                            id,
                            input_delta: partial.to_string(),
                        }))
                    }
                    _ => Ok(None),
                }
            }
            "content_block_stop" => {
                let data: serde_json::Value = serde_json::from_str(&event.data)?;
                let id = data.get("index").map(|i| i.to_string()).unwrap_or_default();
                Ok(Some(StreamEvent::ToolUseStop { id }))
            }
            "message_stop" => Ok(Some(StreamEvent::MessageDone)),
            _ => {
                debug!("忽略事件类型: {}", event_type);
                Ok(None)
            }
        }
    }
}
