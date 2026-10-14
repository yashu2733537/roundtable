//! Provider layer: OpenAI-compatible `/models` + `/chat/completions` SSE.
//!
//! Every provider is just a base URL (e.g. `https://api.openai.com/v1`,
//! `http://localhost:11434/v1`). Keys are passed per call and never stored
//! here — the shell reads them from the OS credential store.
//!
//! SSE parsing splits the byte stream at `\n`, which is always a character
//! boundary in UTF-8 (no multibyte sequence contains `0x0A`), so split
//! multi-byte characters across network chunks reassemble safely.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::RagError;

/// One chat turn in provider shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

impl ChatMsg {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
}

fn http_client(connect_secs: u64) -> Result<reqwest::Client, RagError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(connect_secs))
        .build()
        .map_err(|e| RagError::Embed(format!("Could not start the HTTP client: {e}")))
}

fn api_error(status: u16, body: &str, url: &str) -> RagError {
    // Try OpenAI's {error:{message}} shape first, fall back to raw text.
    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(message) = payload
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return RagError::Embed(format!("The endpoint refused the request (HTTP {status}): {message}"));
        }
    }
    RagError::Embed(format!(
        "{url} returned HTTP {status}: {}",
        truncate(body, 200)
    ))
}

/// List model ids from `GET {base}/models`. Sorted, deduplicated.
pub async fn list_models(base_url: &str, api_key: Option<&str>) -> Result<Vec<String>, RagError> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/models");
    let client = http_client(15)?;
    let mut request = client.get(&url);
    if let Some(key) = api_key {
        if !key.is_empty() {
            request = request.bearer_auth(key);
        }
    }
    let response = request.send().await.map_err(|e| {
        RagError::Embed(format!(
            "Could not reach {url} — check the base URL and that the server is running. ({e})"
        ))
    })?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        if status == 401 || status == 403 {
            return Err(RagError::Embed(
                "API key rejected (HTTP 401/403) — check the stored key for this provider.".to_string(),
            ));
        }
        return Err(api_error(status, &body, &url));
    }
    let payload: serde_json::Value = response.json().await.map_err(|_| {
        RagError::Embed(format!("{url} did not return an OpenAI-compatible /models payload."))
    })?;
    let mut models: Vec<String> = payload
        .get("data")
        .and_then(|d| d.as_array())
        .map(|data| {
            data.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(str::to_string))
                .collect()
        })
        .ok_or_else(|| {
            RagError::Embed(format!("{url} did not return an OpenAI-compatible /models payload."))
        })?;
    models.sort();
    models.dedup();
    Ok(models)
}

/// A function tool exposed to the model (OpenAI `tools` shape).
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// A model-requested function invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Per-turn generation knobs. Agents supply these; plain chats leave them empty.
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tools: Option<Vec<ToolDef>>,
}

/// Token counts reported by the provider (`usage` object in the stream).
/// `None` throughout means the endpoint did not report any.
#[derive(Debug, Clone, Default)]
pub struct ChatUsage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

/// The outcome of one streamed turn.
#[derive(Debug, Clone)]
pub struct ChatResult {
    pub content: String,
    pub usage: ChatUsage,
    pub tool_calls: Vec<ToolCall>,
}

/// Stream one chat completion, calling `on_delta` per text fragment.
/// Returns the full assistant text plus provider-reported usage. Stops early
/// when `cancel` is set (the partial text is still returned; the caller
/// decides what to persist).
pub async fn chat_stream(
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
    messages: &[ChatMsg],
    options: &ChatOptions,
    cancel: &AtomicBool,
    on_delta: &mut (dyn FnMut(String) + Send),
) -> Result<ChatResult, RagError> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/chat/completions");
    let client = http_client(20)?;
    let mut wire: Vec<ChatMsg> = Vec::with_capacity(messages.len() + 1);
    if let Some(system) = options.system_prompt.as_deref().filter(|s| !s.trim().is_empty()) {
        wire.push(ChatMsg {
            role: "system".to_string(),
            content: system.to_string(),
        });
    }
    wire.extend(messages.iter().cloned());
    let mut body = serde_json::json!({
        "model": model,
        "messages": wire,
        "stream": true,
        // Ask for usage in the final chunk; servers that do not understand
        // it ignore it.
        "stream_options": { "include_usage": true },
    });
    if let Some(temperature) = options.temperature {
        body["temperature"] = temperature.into();
    }
    if let Some(max_tokens) = options.max_tokens.filter(|m| *m > 0) {
        body["max_tokens"] = max_tokens.into();
    }
    if let Some(tools) = options.tools.as_ref() {
        let wire_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::Value::Array(wire_tools);
    }
    let mut request = client.post(&url).json(&body);
    if let Some(key) = api_key {
        if !key.is_empty() {
            request = request.bearer_auth(key);
        }
    }
    let mut response = request.send().await.map_err(|e| {
        if cancel.load(Ordering::Relaxed) {
            RagError::Input("cancelled".to_string())
        } else {
            RagError::Embed(format!(
                "Could not reach {url} — check the base URL and that the server is running. ({e})"
            ))
        }
    })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        if status == 401 || status == 403 {
            return Err(RagError::Embed(
                "API key rejected (HTTP 401/403) — check the stored key for this provider.".to_string(),
            ));
        }
        if status == 404 {
            return Err(RagError::Embed(format!(
                "{url} returned 404 — the base URL should usually end in /v1."
            )));
        }
        return Err(api_error(status, &body, &url));
    }

    let mut full = String::new();
    let mut usage = ChatUsage::default();
    let mut buffer: Vec<u8> = Vec::new();
    let mut finished = false;
    let mut stream_acc: Vec<ToolAcc> = Vec::new();
    let mut message_tools: Vec<ToolCall> = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match response.chunk().await {
            Err(e) => {
                return Err(RagError::Embed(format!("The stream broke off: {e}")));
            }
            Ok(None) => break,
            Ok(Some(bytes)) => {
                buffer.extend_from_slice(&bytes);
                while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                    let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                    // `\n` never appears inside a UTF-8 multibyte sequence,
                    // so this split is always a character boundary.
                    let line = String::from_utf8_lossy(&line_bytes);
                    let line = line.strip_suffix('\n').unwrap_or(&line);
                    let line = line.strip_suffix('\r').unwrap_or(line);
                    let event = parse_sse_line(line, &mut finished)?;
                    if let Some(delta) = event.delta {
                        full.push_str(&delta);
                        on_delta(delta);
                    }
                    for chunk in event.tool_deltas {
                        let idx = chunk.index;
                        if stream_acc.len() <= idx {
                            stream_acc.resize_with(idx + 1, ToolAcc::default);
                        }
                        let acc = &mut stream_acc[idx];
                        if let Some(id) = chunk.id {
                            if !id.is_empty() && acc.id.is_empty() {
                                acc.id = id;
                            }
                        }
                        if let Some(name) = chunk.name {
                            if !name.is_empty() && acc.name.is_empty() {
                                acc.name = name;
                            }
                        }
                        if let Some(args) = chunk.arguments {
                            acc.arguments.push_str(&args);
                        }
                    }
                    if let Some(tools) = event.message_tools {
                        message_tools.extend(tools);
                    }
                    if let Some(event_usage) = event.usage {
                        usage = event_usage;
                    }
                    if finished || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                }
                if finished {
                    break;
                }
            }
        }
    }
    let mut tool_calls = message_tools;
    for (i, acc) in stream_acc.into_iter().enumerate() {
        if acc.name.is_empty() {
            continue;
        }
        let id = if acc.id.is_empty() {
            format!("call_{i}")
        } else {
            acc.id
        };
        tool_calls.push(ToolCall {
            id,
            name: acc.name,
            arguments: acc.arguments,
        });
    }
    Ok(ChatResult {
        content: full,
        usage,
        tool_calls,
    })
}

/// One parsed SSE event: an optional text fragment plus optional token usage.
#[derive(Debug, Clone, Default)]
struct SseEvent {
    delta: Option<String>,
    usage: Option<ChatUsage>,
    tool_deltas: Vec<ToolChunk>,
    message_tools: Option<Vec<ToolCall>>,
}

/// One streamed `delta.tool_calls[]` fragment, keyed by `index`.
#[derive(Debug, Clone, Default)]
struct ToolChunk {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

/// Streaming accumulator for one tool call by index.
#[derive(Debug, Clone, Default)]
struct ToolAcc {
    id: String,
    name: String,
    arguments: String,
}

fn parse_usage(value: &serde_json::Value) -> Option<ChatUsage> {
    let usage = value.get("usage")?;
    let number = |key: &str| usage.get(key).and_then(|n| n.as_u64()).map(|n| n as u32);
    // Only report when the endpoint actually sent numbers.
    if usage.get("prompt_tokens").is_none()
        && usage.get("completion_tokens").is_none()
        && usage.get("total_tokens").is_none()
    {
        return None;
    }
    Some(ChatUsage {
        prompt_tokens: number("prompt_tokens"),
        completion_tokens: number("completion_tokens"),
        total_tokens: number("total_tokens"),
    })
}

/// Handle one SSE line. Returns text to append and/or usage, or an empty
/// event for control lines. Sets `finished` on `[DONE]` or a non-streaming
/// complete payload.
fn parse_sse_line(line: &str, finished: &mut bool) -> Result<SseEvent, RagError> {
    // Ignore comments/heartbeats and non-data fields.
    let payload = match line.strip_prefix("data:") {
        Some(rest) => rest.trim(),
        None => return Ok(SseEvent::default()),
    };
    if payload.is_empty() {
        return Ok(SseEvent::default());
    }
    if payload == "[DONE]" {
        *finished = true;
        return Ok(SseEvent::default());
    }
    let value: serde_json::Value = serde_json::from_str(payload).map_err(|_| {
        RagError::Embed("The endpoint sent a stream event that is not valid JSON.".to_string())
    })?;
    if let Some(message) = value.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return Err(RagError::Embed(format!("The endpoint ended the stream with an error: {message}")));
    }
    let mut event = SseEvent::default();
    event.usage = parse_usage(&value);
    let choice = value.get("choices").and_then(|c| c.as_array()).and_then(|c| c.first());
    if let Some(choice) = choice {
        // Standard streaming fragment: content and/or tool calls.
        if let Some(delta) = choice.get("delta") {
            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                if !content.is_empty() {
                    event.delta = Some(content.to_string());
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for entry in calls {
                    let index = entry
                        .get("index")
                        .and_then(|i| i.as_u64())
                        .unwrap_or(0) as usize;
                    let id = entry
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    let function = entry.get("function");
                    let name = function
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    let arguments = function
                        .and_then(|f| f.get("arguments"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    // Only keep entries that carry something.
                    if id.is_some() || name.is_some() || arguments.is_some() {
                        event.tool_deltas.push(ToolChunk {
                            index,
                            id,
                            name,
                            arguments,
                        });
                    }
                }
            }
            if event.delta.is_some() || !event.tool_deltas.is_empty() {
                if choice.get("finish_reason").is_some()
                    && choice.get("finish_reason").and_then(|v| v.as_str()).is_some()
                {
                    // A finish reason alongside a delta still ends the turn,
                    // but the payload above is already captured.
                    *finished = true;
                } else if choice.get("finish_reason").is_some()
                    && !choice.get("finish_reason").map(|v| v.is_null()).unwrap_or(false)
                {
                    *finished = true;
                }
                return Ok(event);
            }
        }
        // Some servers ignore stream:true and return the full message.
        if let Some(message) = choice.get("message") {
            let mut saw_message = false;
            if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
                *finished = true;
                saw_message = true;
                if !content.is_empty() {
                    event.delta = Some(content.to_string());
                }
            }
            if let Some(calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
                *finished = true;
                saw_message = true;
                let mut tools = Vec::new();
                for (i, entry) in calls.iter().enumerate() {
                    let id = entry
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("call_{i}"));
                    let function = entry.get("function");
                    let name = function
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .or_else(|| {
                            entry
                                .get("name")
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                        });
                    let Some(name) = name else { continue };
                    if name.is_empty() {
                        continue;
                    }
                    let arguments = function
                        .and_then(|f| f.get("arguments"))
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => serde_json::to_string(other).unwrap_or_default(),
                        })
                        .or_else(|| {
                            entry.get("arguments").map(|v| match v {
                                serde_json::Value::String(s) => s.clone(),
                                other => serde_json::to_string(other).unwrap_or_default(),
                            })
                        })
                        .unwrap_or_default();
                    tools.push(ToolCall { id, name, arguments });
                }
                if !tools.is_empty() {
                    event.message_tools = Some(tools);
                }
            }
            if saw_message {
                if choice.get("finish_reason").is_some() {
                    *finished = true;
                }
                return Ok(event);
            }
        }
        if choice.get("finish_reason").is_some() {
            // `finish_reason: null` means "still streaming"; any real reason
            // ends the turn. `is_some` alone would also fire on explicit
            // null, so check for non-null.
            let is_null = choice
                .get("finish_reason")
                .map(|v| v.is_null())
                .unwrap_or(true);
            if !is_null {
                *finished = true;
            }
        }
    }
    Ok(event)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.trim().to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{}…", head.trim())
}

// ---------------------------------------------------------------------------
// Ollama
// ---------------------------------------------------------------------------

/// Native Ollama API root (separate from the OpenAI-compatible `/v1`).
pub const OLLAMA_API: &str = "http://127.0.0.1:11434";
/// OpenAI-compatible base for an Ollama server.
pub const OLLAMA_V1_BASE: &str = "http://localhost:11434/v1";

/// Installed Ollama models, e.g. `llama3.1:8b`.
#[derive(Debug, Clone, Serialize)]
pub struct OllamaStatus {
    pub running: bool,
    pub models: Vec<String>,
}

/// Probe the local Ollama daemon. Any failure means "not running" — the UI
/// turns that into the install guide + Retry, so this never errors.
pub async fn ollama_status() -> OllamaStatus {
    let url = format!("{OLLAMA_API}/api/tags");
    let client = match http_client(2) {
        Ok(c) => c,
        Err(_) => {
            return OllamaStatus {
                running: false,
                models: Vec::new(),
            }
        }
    };
    let response = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => {
            return OllamaStatus {
                running: false,
                models: Vec::new(),
            }
        }
    };
    if !response.status().is_success() {
        return OllamaStatus {
            running: false,
            models: Vec::new(),
        };
    }
    let mut models: Vec<String> = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| v.get("models").and_then(|m| m.as_array()).cloned())
        .map(|list| {
            list.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    OllamaStatus {
        running: true,
        models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn sse_line_parses_delta() {
        let mut done = false;
        let out = parse_sse_line(
            r#"data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#,
            &mut done,
        )
        .unwrap();
        assert_eq!(out.delta.as_deref(), Some("Hello"));
        assert!(out.usage.is_none());
        assert!(!done);
    }

    #[test]
    fn sse_line_handles_done_and_control() {
        let mut done = false;
        let control = parse_sse_line(": ping", &mut done).unwrap();
        assert!(control.delta.is_none() && control.usage.is_none());
        let end = parse_sse_line("data: [DONE]", &mut done).unwrap();
        assert!(end.delta.is_none() && end.usage.is_none());
        assert!(done);
    }

    #[test]
    fn sse_line_handles_full_message_fallback() {
        let mut done = false;
        let out = parse_sse_line(
            r#"data: {"choices":[{"message":{"content":"Full answer"},"finish_reason":"stop"}]}"#,
            &mut done,
        )
        .unwrap();
        assert_eq!(out.delta.as_deref(), Some("Full answer"));
        assert!(done);
    }

    #[test]
    fn sse_line_captures_usage() {
        let mut done = false;
        let out = parse_sse_line(
            r#"data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":34,"total_tokens":46}}"#,
            &mut done,
        )
        .unwrap();
        assert!(out.delta.is_none());
        let usage = out.usage.expect("usage captured");
        assert_eq!(usage.prompt_tokens, Some(12));
        assert_eq!(usage.completion_tokens, Some(34));
        assert_eq!(usage.total_tokens, Some(46));
        assert!(!done);
    }

    #[test]
    fn sse_line_accumulates_streamed_tool_calls() {
        let mut done = false;
        let first = parse_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"send_message","arguments":"{\"to\""}}]},"finish_reason":null}]}"#,
            &mut done,
        )
        .unwrap();
        assert_eq!(first.tool_deltas.len(), 1);
        assert_eq!(first.tool_deltas[0].index, 0);
        assert_eq!(first.tool_deltas[0].id.as_deref(), Some("call_1"));
        assert_eq!(
            first.tool_deltas[0].name.as_deref(),
            Some("send_message")
        );
        assert!(!done);

        let second = parse_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"_rest"}}]},"finish_reason":null}]}"#,
            &mut done,
        )
        .unwrap();
        assert_eq!(second.tool_deltas.len(), 1);
        assert_eq!(second.tool_deltas[0].arguments.as_deref(), Some("_rest"));
        assert!(!done);
    }

    #[test]
    fn sse_line_handles_message_tool_calls_fallback() {
        let mut done = false;
        let out = parse_sse_line(
            r#"data: {"choices":[{"message":{"content":"","tool_calls":[{"id":"call_9","type":"function","function":{"name":"finish","arguments":"{\"result\":\"done\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut done,
        )
        .unwrap();
        let tools = out.message_tools.expect("tool calls parsed");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id, "call_9");
        assert_eq!(tools[0].name, "finish");
        assert!(tools[0].arguments.contains("done"));
        assert!(done);
    }

    /// End-to-end stream against a hand-rolled server that splits a
    /// multi-byte emoji across TCP chunks — proves UTF-8 reassembly.
    #[tokio::test]
    async fn chat_stream_reassembles_split_utf8() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let mut read = 0usize;
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap();
                if n == 0 {
                    break;
                }
                read += n;
                let head = String::from_utf8_lossy(&buf[..read]);
                if let Some(pos) = head.find("\r\n\r\n") {
                    let headers = &head[..pos];
                    let len = headers
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if read >= pos + 4 + len {
                        break;
                    }
                }
            }
            // "Hi 🚀 there" — the rocket (4 bytes in UTF-8) is split across
            // TCP segments on purpose. Each JSON line stays whole; only the
            // transport chunks cut mid-character.
            let ev1 = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi 🚀\"}}]}\n\n";
            let ev2 = "data: {\"choices\":[{\"delta\":{\"content\":\" there\"}}]}\n\ndata: [DONE]\n\n";
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                ev1.len() + ev2.len()
            );
            sock.write_all(head.as_bytes()).await.unwrap();
            let rocket_at = ev1.find('🚀').unwrap();
            let cut = rocket_at + 2; // middle of the 4-byte sequence
            sock.write_all(&ev1.as_bytes()[..cut]).await.unwrap();
            sock.write_all(&ev1.as_bytes()[cut..]).await.unwrap();
            sock.write_all(ev2.as_bytes()).await.unwrap();
        });

        let cancel = AtomicBool::new(false);
        let mut pieces = Vec::new();
        let result = chat_stream(
            &format!("http://{addr}/v1"),
            None,
            "mock",
            &[ChatMsg::user("hi")],
            &ChatOptions::default(),
            &cancel,
            &mut |d| pieces.push(d),
        )
        .await
        .unwrap();
        server.await.unwrap();
        let full = result.content;
        assert!(full.contains('🚀'), "emoji lost: {full:?}");
        assert_eq!(full, "Hi 🚀 there");
        assert_eq!(pieces.join(""), full);
    }

    /// The request body carries the system prompt first plus generation
    /// knobs and a usage request.
    #[tokio::test]
    async fn chat_stream_sends_system_and_options() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let mut read = 0usize;
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap();
                if n == 0 {
                    break;
                }
                read += n;
                let head = String::from_utf8_lossy(&buf[..read]);
                if let Some(pos) = head.find("\r\n\r\n") {
                    let headers = &head[..pos];
                    let len = headers
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if read >= pos + 4 + len {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&buf[..read]).to_string();
            let body = "data: [DONE]\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let cancel = AtomicBool::new(false);
        let options = ChatOptions {
            system_prompt: Some("Be brief.".to_string()),
            temperature: Some(0.2),
            max_tokens: Some(100),
            tools: None,
        };
        let mut pieces = Vec::new();
        let result = chat_stream(
            &format!("http://{addr}/v1"),
            None,
            "mock",
            &[ChatMsg::user("hi")],
            &options,
            &cancel,
            &mut |d| pieces.push(d),
        )
        .await
        .unwrap();
        assert_eq!(result.content, "");
        let request = server.await.unwrap();
        let body_start = request.find("\r\n\r\n").map(|p| p + 4).unwrap_or(0);
        let body: serde_json::Value =
            serde_json::from_str(&request[body_start..]).expect("request is JSON");
        let messages = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].get("role").and_then(|r| r.as_str()), Some("system"));
        assert_eq!(
            messages[0].get("content").and_then(|c| c.as_str()),
            Some("Be brief.")
        );
        assert_eq!(body.get("max_tokens").and_then(|m| m.as_u64()), Some(100));
        let temperature = body.get("temperature").and_then(|t| t.as_f64()).unwrap_or(-1.0);
        assert!(
            (temperature - 0.2).abs() < 1e-6,
            "temperature was {temperature}"
        );
        assert_eq!(
            body.get("stream_options")
                .and_then(|s| s.get("include_usage"))
                .and_then(|u| u.as_bool()),
            Some(true)
        );
        // No tools requested → the key stays out so strict servers accept it.
        assert!(body.get("tools").is_none(), "tools must be omitted when None");
    }

    /// When tools are supplied they ride the OpenAI `tools` wire shape.
    #[tokio::test]
    async fn chat_stream_sends_tools_when_present() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let mut read = 0usize;
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap();
                if n == 0 {
                    break;
                }
                read += n;
                let head = String::from_utf8_lossy(&buf[..read]);
                if let Some(pos) = head.find("\r\n\r\n") {
                    let headers = &head[..pos];
                    let len = headers
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if read >= pos + 4 + len {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&buf[..read]).to_string();
            let body = "data: [DONE]\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let cancel = AtomicBool::new(false);
        let options = ChatOptions {
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Some(vec![ToolDef {
                name: "finish".to_string(),
                description: "End the turn.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "result": { "type": "string" } },
                    "required": ["result"]
                }),
            }]),
        };
        let mut pieces = Vec::new();
        chat_stream(
            &format!("http://{addr}/v1"),
            None,
            "mock",
            &[ChatMsg::user("hi")],
            &options,
            &cancel,
            &mut |d| pieces.push(d),
        )
        .await
        .unwrap();
        let request = server.await.unwrap();
        let body_start = request.find("\r\n\r\n").map(|p| p + 4).unwrap_or(0);
        let body: serde_json::Value =
            serde_json::from_str(&request[body_start..]).expect("request is JSON");
        let tools = body.get("tools").and_then(|t| t.as_array()).expect("tools sent");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].get("type").and_then(|t| t.as_str()), Some("function"));
        let function = tools[0].get("function").expect("function wrapper");
        assert_eq!(function.get("name").and_then(|n| n.as_str()), Some("finish"));
    }

    /// End-to-end: streamed `delta.tool_calls[]` fragments reassemble into
    /// one call on the result.
    #[tokio::test]
    async fn chat_stream_accumulates_tool_calls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let mut read = 0usize;
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap();
                if n == 0 {
                    break;
                }
                read += n;
                let head = String::from_utf8_lossy(&buf[..read]);
                if let Some(pos) = head.find("\r\n\r\n") {
                    let headers = &head[..pos];
                    let len = headers
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if read >= pos + 4 + len {
                        break;
                    }
                }
            }
            let ev1 = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"finish\",\"arguments\":\"{\\\"res\"}}]},\"finish_reason\":null}]}\n\n";
            let ev2 = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ult\\\":\\\"ok\\\"}\"}}]},\"finish_reason\":null}]}\n\n";
            let ev3 = "data: {\"choices\":[{\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
            let body = format!("{ev1}{ev2}{ev3}");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });

        let cancel = AtomicBool::new(false);
        let mut pieces = Vec::new();
        let result = chat_stream(
            &format!("http://{addr}/v1"),
            None,
            "mock",
            &[ChatMsg::user("hi")],
            &ChatOptions::default(),
            &cancel,
            &mut |d| pieces.push(d),
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].id, "call_1");
        assert_eq!(result.tool_calls[0].name, "finish");
        assert!(result.tool_calls[0].arguments.contains("ok"), "args reassembled: {}", result.tool_calls[0].arguments);
    }

    #[tokio::test]
    async fn list_models_parses_openai_shape() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let mut read = 0usize;
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap();
                if n == 0 {
                    break;
                }
                read += n;
                let head = String::from_utf8_lossy(&buf[..read]);
                if head.contains("\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"object":"list","data":[{"id":"b-model","object":"model"},{"id":"a-model","object":"model"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });

        let models = list_models(&format!("http://{addr}/v1"), None).await.unwrap();
        server.await.unwrap();
        assert_eq!(models, vec!["a-model".to_string(), "b-model".to_string()]);
    }
}
