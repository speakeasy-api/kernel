use crate::config::{ModelsConfig, ProviderConfig};
use crate::prompt_router::classify::{ClassificationError, LlmClient};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use super::types::{ContentBlock, Message, Role, ToolDefinition, Usage};

const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api";
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Delta {
        text: String,
    },
    ToolUseStart {
        index: u64,
        id: String,
        name: String,
    },
    ToolInputDelta {
        index: u64,
        partial_json: String,
    },
    ContentBlockStop {
        index: u64,
    },
    MessageUsage {
        usage: Usage,
    },
    Done {
        stop_reason: String,
    },
    DoneWithUsage {
        stop_reason: String,
        usage: Usage,
    },
    Error {
        message: String,
    },
}

/// Auth style determines how the API key is sent.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthStyle {
    /// `x-api-key` header + `anthropic-version` (native Anthropic)
    ApiKey,
    /// `Authorization: Bearer <key>` (OpenRouter, proxies)
    Bearer,
}

/// Full streaming request with multi-turn message history and optional tools.
pub struct StreamRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub model: &'a str,
    pub max_tokens: u32,
    pub tools: &'a [ToolDefinition],
}

/// Result of a non-streaming completion that includes usage data.
#[derive(Debug, Clone)]
pub struct CompletionResult {
    pub text: String,
    pub usage: Usage,
}

pub struct LlmClient2 {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    auth_style: AuthStyle,
}

impl LlmClient2 {
    /// Build from explicit values.
    pub fn new(api_key: String, base_url: String, auth_style: AuthStyle) -> Self {
        info!(base_url = %base_url, auth_style = ?auth_style, "creating LLM client");
        Self {
            http: reqwest::Client::new(),
            api_key,
            base_url,
            auth_style,
        }
    }

    /// Resolve provider from the project config's `models.providers` map.
    ///
    /// Looks up `provider_name` in the config. Falls back to env-var
    /// detection if the provider isn't configured.
    pub fn from_config(models_config: &ModelsConfig, provider_name: &str) -> Result<Self, String> {
        debug!(provider = %provider_name, "resolving LLM provider from config");
        if let Some(pc) = models_config.providers.get(provider_name) {
            return Self::from_provider_config(provider_name, pc);
        }
        // No explicit config — fall back to env detection
        Self::from_env()
    }

    /// Build from a single `ProviderConfig` entry.
    fn from_provider_config(name: &str, pc: &ProviderConfig) -> Result<Self, String> {
        debug!(name = %name, "building client from provider config");
        let env_var = pc.api_key_env.as_deref().unwrap_or(default_env_var(name));
        let api_key =
            std::env::var(env_var).map_err(|_| format!("{env_var} not set (provider: {name})"))?;

        let (base_url, auth_style) = match pc.base_url.as_deref() {
            Some(url) => (url.to_string(), infer_auth_style(url)),
            None => default_provider_settings(name),
        };

        Ok(Self::new(api_key, base_url, auth_style))
    }

    /// Auto-detect from environment variables.
    /// Checks OPENROUTER_API_KEY, then ANTHROPIC_API_KEY.
    pub fn from_env() -> Result<Self, String> {
        debug!("auto-detecting LLM provider from env");
        if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
            return Ok(Self::new(
                key,
                OPENROUTER_BASE_URL.to_string(),
                AuthStyle::Bearer,
            ));
        }
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            return Ok(Self::new(
                key,
                ANTHROPIC_BASE_URL.to_string(),
                AuthStyle::ApiKey,
            ));
        }
        Err("No LLM API key found. Set OPENROUTER_API_KEY or ANTHROPIC_API_KEY.".to_string())
    }

    fn headers(&self) -> Result<HeaderMap, String> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        match self.auth_style {
            AuthStyle::ApiKey => {
                headers.insert(
                    "x-api-key",
                    HeaderValue::from_str(&self.api_key)
                        .map_err(|_| "API key contains invalid header characters".to_string())?,
                );
                headers.insert(
                    "anthropic-version",
                    HeaderValue::from_static(ANTHROPIC_VERSION),
                );
            }
            AuthStyle::Bearer => {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                        .map_err(|_| "API key contains invalid header characters".to_string())?,
                );
            }
        }

        Ok(headers)
    }

    /// Normalize a model ID for the current provider.
    /// OpenRouter requires `anthropic/` prefix and full model IDs.
    fn normalize_model(&self, model: &str) -> String {
        let result = if self.auth_style == AuthStyle::ApiKey {
            // Native Anthropic — pass through as-is
            model.to_string()
        } else {
            // OpenRouter / Bearer providers need `anthropic/` prefix
            if model.contains('/') {
                model.to_string()
            } else {
                format!("anthropic/{model}")
            }
        };
        debug!(input = %model, output = %result, "normalizing model");
        result
    }

    /// Non-streaming completion for classification.
    pub async fn complete_async(&self, prompt: &str, model: &str) -> Result<String, String> {
        debug!(model = %model, "completing async");
        self.complete_with_usage(prompt, model)
            .await
            .map(|r| r.text)
    }

    /// Non-streaming completion that also returns usage metadata.
    pub async fn complete_with_usage(
        &self,
        prompt: &str,
        model: &str,
    ) -> Result<CompletionResult, String> {
        info!(model = %model, "completing with usage tracking");
        let model = self.normalize_model(model);
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 256,
            "messages": [
                { "role": "user", "content": prompt }
            ]
        });

        let resp = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("API request failed: {e}"))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        if !status.is_success() {
            let err = format_api_error(status, &text);
            error!(model = %model, status = %status, error = %err, "completion request failed");
            return Err(err);
        }

        let json: Value =
            serde_json::from_str(&text).map_err(|e| format!("Failed to parse response: {e}"))?;

        let content_text = json["content"]
            .as_array()
            .and_then(|blocks| blocks.first())
            .and_then(|block| block["text"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Unexpected response structure: {text}"))?;

        let usage: Usage = serde_json::from_value(json["usage"].clone()).unwrap_or_default();

        debug!(
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            "completion usage"
        );

        Ok(CompletionResult {
            text: content_text,
            usage,
        })
    }

    /// Non-streaming completion with system prompt and configurable max_tokens.
    /// Used by the compaction pipeline for deep compaction LLM calls.
    pub async fn complete_system_async(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        model: &str,
        max_tokens: u32,
    ) -> Result<String, String> {
        debug!(model = %model, max_tokens = max_tokens, "completing with system prompt");
        let model = self.normalize_model(model);
        let body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system_prompt,
            "messages": [
                { "role": "user", "content": user_prompt }
            ]
        });

        let resp = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("API request failed: {e}"))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        if !status.is_success() {
            let err = format_api_error(status, &text);
            error!(model = %model, status = %status, error = %err, "system completion request failed");
            return Err(err);
        }

        let json: Value =
            serde_json::from_str(&text).map_err(|e| format!("Failed to parse response: {e}"))?;

        json["content"]
            .as_array()
            .and_then(|blocks| blocks.first())
            .and_then(|block| block["text"].as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Unexpected response structure: {text}"))
    }

    /// Streaming call with full message history and optional tools.
    pub async fn stream_message_full(
        &self,
        req: &StreamRequest<'_>,
    ) -> Result<mpsc::Receiver<StreamChunk>, String> {
        info!(
            model = %req.model,
            max_tokens = req.max_tokens,
            tools_count = req.tools.len(),
            "starting streaming request"
        );
        let model = self.normalize_model(req.model);

        let mut messages_json: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                let content: Vec<Value> = m
                    .content
                    .iter()
                    .map(|b| serde_json::to_value(b).unwrap_or(Value::Null))
                    .collect();
                serde_json::json!({
                    "role": m.role,
                    "content": content,
                })
            })
            .collect();

        // Inject cache_control breakpoint on the last user message so the
        // entire conversation prefix is cached between agentic turns.
        inject_message_cache_breakpoints(&mut messages_json);

        // System prompt as a structured content block with cache_control so it
        // is cached across every turn of the agentic loop.
        let system_with_cache = serde_json::json!([{
            "type": "text",
            "text": req.system,
            "cache_control": {"type": "ephemeral"}
        }]);

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": req.max_tokens,
            "stream": true,
            "system": system_with_cache,
            "messages": messages_json,
        });

        if !req.tools.is_empty() {
            let mut tools_json: Vec<Value> = req
                .tools
                .iter()
                .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
                .collect();
            // Tag the last tool definition with cache_control so the full
            // system + tools prefix is cached (tools are stable across turns).
            if let Some(last_tool) = tools_json.last_mut() {
                if let Some(obj) = last_tool.as_object_mut() {
                    obj.insert(
                        "cache_control".into(),
                        serde_json::json!({"type": "ephemeral"}),
                    );
                }
            }
            body.as_object_mut()
                .unwrap()
                .insert("tools".into(), Value::Array(tools_json));
        }

        let resp = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Stream request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_else(|_| "unknown error".into());
            let err = format_api_error(status, &text);
            error!(status = %status, error = %err, "stream request failed");
            return Err(err);
        }

        debug!("stream channel established");
        let (tx, rx) = mpsc::channel::<StreamChunk>(64);
        let mut stream = resp.bytes_stream();

        tokio::spawn(async move {
            let mut buffer = String::new();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));

                        while let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].trim_end().to_string();
                            buffer = buffer[newline_pos + 1..].to_string();

                            if line.starts_with("data: ") {
                                let data = &line[6..];
                                if data == "[DONE]" {
                                    continue;
                                }
                                if let Ok(json) = serde_json::from_str::<Value>(data) {
                                    if let Some(chunk) = parse_sse_event(&json) {
                                        debug!("stream chunk received");
                                        if tx.send(chunk).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "stream error");
                        let _ = tx
                            .send(StreamChunk::Error {
                                message: e.to_string(),
                            })
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Streaming call to Messages API. Returns a receiver that yields StreamChunks.
    /// Convenience wrapper around `stream_message_full` for single user messages.
    pub async fn stream_message(
        &self,
        system: &str,
        user_msg: &str,
        model: &str,
    ) -> Result<mpsc::Receiver<StreamChunk>, String> {
        debug!(model = %model, "streaming single message");
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: user_msg.to_string(),
            }],
        }];

        let req = StreamRequest {
            system,
            messages: &messages,
            model,
            max_tokens: 8192,
            tools: &[],
        };

        self.stream_message_full(&req).await
    }
}

/// Parse a single SSE JSON event into an optional StreamChunk.
fn parse_sse_event(json: &Value) -> Option<StreamChunk> {
    match json["type"].as_str()? {
        "message_start" => {
            if let Some(usage_val) = json.get("message").and_then(|m| m.get("usage")) {
                if let Ok(usage) = serde_json::from_value::<Usage>(usage_val.clone()) {
                    return Some(StreamChunk::MessageUsage { usage });
                }
            }
            None
        }
        "content_block_start" => {
            let index = json["index"].as_u64().unwrap_or(0);
            let cb = &json["content_block"];
            match cb["type"].as_str()? {
                "tool_use" => Some(StreamChunk::ToolUseStart {
                    index,
                    id: cb["id"].as_str().unwrap_or("").to_string(),
                    name: cb["name"].as_str().unwrap_or("").to_string(),
                }),
                _ => None,
            }
        }
        "content_block_delta" => {
            let index = json["index"].as_u64().unwrap_or(0);
            let delta = &json["delta"];
            match delta["type"].as_str()? {
                "text_delta" => {
                    let text = delta["text"].as_str().unwrap_or("").to_string();
                    Some(StreamChunk::Delta { text })
                }
                "input_json_delta" => {
                    let partial = delta["partial_json"].as_str().unwrap_or("").to_string();
                    Some(StreamChunk::ToolInputDelta {
                        index,
                        partial_json: partial,
                    })
                }
                _ => None,
            }
        }
        "content_block_stop" => {
            let index = json["index"].as_u64().unwrap_or(0);
            Some(StreamChunk::ContentBlockStop { index })
        }
        "message_delta" => {
            let stop = json["delta"]["stop_reason"].as_str().map(|r| r.to_string());
            let usage = json
                .get("usage")
                .and_then(|v| serde_json::from_value::<Usage>(v.clone()).ok());

            // Return both as a combined variant so no data is lost
            match (stop, usage) {
                (Some(stop_reason), Some(usage)) => {
                    Some(StreamChunk::DoneWithUsage { stop_reason, usage })
                }
                (Some(stop_reason), None) => Some(StreamChunk::Done { stop_reason }),
                (None, Some(usage)) => Some(StreamChunk::MessageUsage { usage }),
                (None, None) => None,
            }
        }
        "message_stop" => Some(StreamChunk::Done {
            stop_reason: "end_turn".to_string(),
        }),
        "error" => {
            let msg = json["error"]["message"].as_str().unwrap_or("unknown error");
            Some(StreamChunk::Error {
                message: msg.to_string(),
            })
        }
        _ => None,
    }
}

/// Inject `cache_control` breakpoints into the serialised message array for
/// Anthropic prompt caching.
///
/// Adds `cache_control: {"type": "ephemeral"}` to the last content block of
/// the last user message so the entire conversation prefix is cached between
/// agentic turns.  This means on turn N+1 the API only processes the new
/// assistant + tool-result messages while reusing the cached prefix from turn N.
fn inject_message_cache_breakpoints(messages: &mut [Value]) {
    for msg in messages.iter_mut().rev() {
        if msg["role"].as_str() == Some("user") {
            if let Some(content) = msg["content"].as_array_mut() {
                if let Some(last_block) = content.last_mut() {
                    if let Some(obj) = last_block.as_object_mut() {
                        obj.insert(
                            "cache_control".into(),
                            serde_json::json!({"type": "ephemeral"}),
                        );
                    }
                }
            }
            break;
        }
    }
}

impl LlmClient for LlmClient2 {
    fn complete(&self, prompt: &str, model: &str) -> Result<String, ClassificationError> {
        debug!(model = %model, "sync LLM complete");
        // dispatch() runs inside spawn_blocking, so block_on is safe here.
        let handle = tokio::runtime::Handle::current();
        handle
            .block_on(self.complete_async(prompt, model))
            .map_err(|msg| ClassificationError { message: msg })
    }
}

// ---- Provider defaults ----

fn default_env_var(provider_name: &str) -> &str {
    match provider_name {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        _ => "ANTHROPIC_API_KEY",
    }
}

fn default_provider_settings(provider_name: &str) -> (String, AuthStyle) {
    match provider_name {
        "openrouter" => (OPENROUTER_BASE_URL.to_string(), AuthStyle::Bearer),
        _ => (ANTHROPIC_BASE_URL.to_string(), AuthStyle::ApiKey),
    }
}

/// Guess auth style from the base URL.
fn infer_auth_style(url: &str) -> AuthStyle {
    if url.contains("anthropic.com") {
        AuthStyle::ApiKey
    } else {
        AuthStyle::Bearer
    }
}

/// Extract a human-readable message from an API error response.
fn format_api_error(status: reqwest::StatusCode, body: &str) -> String {
    // Try to extract the message from JSON error body
    if let Ok(json) = serde_json::from_str::<Value>(body) {
        // Anthropic: {"type":"error","error":{"type":"...","message":"..."}}
        // OpenRouter: {"error":{"message":"...","code":400}}
        if let Some(msg) = json["error"]["message"].as_str() {
            return msg.to_string();
        }
        if let Some(msg) = json["message"].as_str() {
            return msg.to_string();
        }
    }
    format!("Request failed ({status})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_delta() {
        let json: Value = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        )
        .unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::Delta { text }) => assert_eq!(text, "Hello"),
            other => panic!("Expected Delta, got {:?}", other),
        }
    }

    #[test]
    fn parse_tool_use_start() {
        let json: Value = serde_json::from_str(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu_abc","name":"read_file"}}"#,
        )
        .unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::ToolUseStart { index, id, name }) => {
                assert_eq!(index, 1);
                assert_eq!(id, "tu_abc");
                assert_eq!(name, "read_file");
            }
            other => panic!("Expected ToolUseStart, got {:?}", other),
        }
    }

    #[test]
    fn parse_input_json_delta() {
        let json: Value = serde_json::from_str(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\""}}"#,
        )
        .unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::ToolInputDelta {
                index,
                partial_json,
            }) => {
                assert_eq!(index, 1);
                assert_eq!(partial_json, r#"{"path""#);
            }
            other => panic!("Expected ToolInputDelta, got {:?}", other),
        }
    }

    #[test]
    fn parse_content_block_stop() {
        let json: Value =
            serde_json::from_str(r#"{"type":"content_block_stop","index":0}"#).unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::ContentBlockStop { index }) => assert_eq!(index, 0),
            other => panic!("Expected ContentBlockStop, got {:?}", other),
        }
    }

    #[test]
    fn parse_message_start_usage() {
        let json: Value = serde_json::from_str(
            r#"{"type":"message_start","message":{"usage":{"input_tokens":100,"output_tokens":0}}}"#,
        )
        .unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::MessageUsage { usage }) => {
                assert_eq!(usage.input_tokens, 100);
                assert_eq!(usage.output_tokens, 0);
            }
            other => panic!("Expected MessageUsage, got {:?}", other),
        }
    }

    #[test]
    fn parse_message_delta_done_with_usage() {
        let json: Value = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#,
        )
        .unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::DoneWithUsage { stop_reason, usage }) => {
                assert_eq!(stop_reason, "end_turn");
                assert_eq!(usage.output_tokens, 42);
            }
            other => panic!("Expected DoneWithUsage, got {:?}", other),
        }
    }

    #[test]
    fn parse_message_delta_done_without_usage() {
        let json: Value =
            serde_json::from_str(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
                .unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::Done { stop_reason }) => assert_eq!(stop_reason, "end_turn"),
            other => panic!("Expected Done, got {:?}", other),
        }
    }

    #[test]
    fn parse_message_stop() {
        let json: Value = serde_json::from_str(r#"{"type":"message_stop"}"#).unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::Done { stop_reason }) => assert_eq!(stop_reason, "end_turn"),
            other => panic!("Expected Done, got {:?}", other),
        }
    }

    #[test]
    fn parse_error() {
        let json: Value =
            serde_json::from_str(r#"{"type":"error","error":{"message":"rate limited"}}"#).unwrap();
        match parse_sse_event(&json) {
            Some(StreamChunk::Error { message }) => assert_eq!(message, "rate limited"),
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn parse_unknown_event_returns_none() {
        let json: Value = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert!(parse_sse_event(&json).is_none());
    }

    #[test]
    fn text_content_block_start_ignored() {
        let json: Value = serde_json::from_str(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        )
        .unwrap();
        assert!(parse_sse_event(&json).is_none());
    }

    // ---- Cache breakpoint injection tests ----

    #[test]
    fn cache_breakpoints_marks_last_user_message() {
        let mut messages = vec![
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "hello"}]
            }),
            serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "hi"}]
            }),
            serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "tu_1", "content": "r1"},
                    {"type": "tool_result", "tool_use_id": "tu_2", "content": "r2"}
                ]
            }),
        ];
        inject_message_cache_breakpoints(&mut messages);

        // First user message should NOT have cache_control
        assert!(messages[0]["content"][0].get("cache_control").is_none());

        // Last user message: only the last content block gets cache_control
        assert!(messages[2]["content"][0].get("cache_control").is_none());
        assert_eq!(
            messages[2]["content"][1]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn cache_breakpoints_single_user_message() {
        let mut messages = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "hello"}]
        })];
        inject_message_cache_breakpoints(&mut messages);
        assert_eq!(
            messages[0]["content"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn cache_breakpoints_empty_messages_no_panic() {
        let mut messages: Vec<Value> = vec![];
        inject_message_cache_breakpoints(&mut messages);
    }

    #[test]
    fn cache_breakpoints_skips_trailing_assistant() {
        let mut messages = vec![
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "hello"}]
            }),
            serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "bye"}]
            }),
        ];
        inject_message_cache_breakpoints(&mut messages);

        // The user message should get the breakpoint (it's the last *user* msg)
        assert_eq!(
            messages[0]["content"][0]["cache_control"],
            serde_json::json!({"type": "ephemeral"})
        );
        // Assistant message should NOT have cache_control
        assert!(messages[1]["content"][0].get("cache_control").is_none());
    }
}
