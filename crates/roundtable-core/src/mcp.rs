//! Minimal MCP (Model Context Protocol) client: stdio + streamable HTTP.
//!
//! Speaks just enough JSON-RPC for tool use — `initialize`, `tools/list`,
//! `tools/call` — with zero new dependencies. stdio servers are spawned
//! per operation (initialize → call → kill), so no client state is kept.
//! HTTP servers are called statelessly (best effort: servers that require a
//! session id will fail with a clear message).
//!
//! Desktop only. Nothing here runs without an explicit human-approved tool
//! call from the room runtime (see `rooms.rs`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::McpServer;

/// One tool offered by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub server_id: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Stable grant/assignment id for a tool: `mcp:<server_id>:<tool>`.
pub fn tool_id(server_id: &str, name: &str) -> String {
    format!("mcp:{server_id}:{name}")
}

fn init_payload(id: u64) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "roundtable", "version": "0.1.0" }
        }
    })
}

fn initialized_notification() -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
}

/// List the tools of one enabled server.
pub async fn list_tools(server: &McpServer) -> Result<Vec<McpToolInfo>, String> {
    let op = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let responses = roundtrip(server, &[init_payload(1), initialized_notification(), op]).await?;
    let result = response_result(&responses, 2, &server.name)?;
    parse_tool_list(&result, &server.id)
}

/// Call one tool. Returns human-readable text (non-text content is noted,
/// not dumped).
pub async fn call_tool(server: &McpServer, tool: &str, args: &Value) -> Result<String, String> {
    let op = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args }
    });
    let responses = roundtrip(server, &[init_payload(1), initialized_notification(), op]).await?;
    let result = response_result(&responses, 2, &server.name)?;
    Ok(result_text(&result))
}

async fn roundtrip(server: &McpServer, payloads: &[Value]) -> Result<Vec<Value>, String> {
    match server.transport.as_str() {
        "http" => http_roundtrip(server, payloads).await,
        _ => stdio_roundtrip(server, payloads),
    }
}

fn response_result(
    responses: &[Value],
    id: u64,
    server_name: &str,
) -> Result<Value, String> {
    let response = responses
        .iter()
        .find(|r| r.get("id").and_then(|i| i.as_u64()) == Some(id))
        .ok_or_else(|| format!("{server_name} never answered — is it an MCP server?"))?;
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(format!("{server_name} reported an error: {message}"));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| format!("{server_name} sent a reply without a result."))
}

fn parse_tool_list(result: &Value, server_id: &str) -> Result<Vec<McpToolInfo>, String> {
    let tools = result
        .get("tools")
        .and_then(|t| t.as_array())
        .ok_or_else(|| "That server listed tools in a shape Roundtable cannot read.".to_string())?;
    let mut out = Vec::new();
    for tool in tools {
        let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if name.trim().is_empty() {
            continue;
        }
        out.push(McpToolInfo {
            server_id: server_id.to_string(),
            name: name.to_string(),
            description: tool
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string(),
            input_schema: tool.get("inputSchema").cloned().unwrap_or(Value::Null),
        });
        if out.len() >= 100 {
            break;
        }
    }
    Ok(out)
}

/// Flatten a `tools/call` result into text.
fn result_text(result: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
        for item in content {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                Some(other) => parts.push(format!("[non-text content omitted: {other}]")),
                None => {}
            }
        }
    }
    if result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false) {
        parts.insert(0, "(the tool reported an error)".to_string());
    }
    let text = parts.join("\n");
    if text.trim().is_empty() {
        "(the tool returned no text)".to_string()
    } else if text.chars().count() > 20_000 {
        format!("{}…(truncated)", text.chars().take(20_000).collect::<String>())
    } else {
        text
    }
}

// -- stdio transport ---------------------------------------------------------

/// Run one process, feed it newline-delimited JSON-RPC, collect the replies.
/// Reader runs on a thread with an overall deadline; the child is always
/// reaped.
fn stdio_roundtrip(server: &McpServer, payloads: &[Value]) -> Result<Vec<Value>, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let command = server.command.as_deref().unwrap_or("").trim();
    if command.is_empty() {
        return Err(format!("{} has no command to run.", server.name));
    }
    let mut child = Command::new(command)
        .args(&server.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}", server.name))?;

    // Write every payload up front (tiny payloads never fill the pipe).
    if let Some(mut stdin) = child.stdin.take() {
        for payload in payloads {
            let line = serde_json::to_string(payload).unwrap_or_default();
            let _ = writeln!(stdin, "{line}");
        }
        let _ = stdin.flush();
    }

    let (sender, receiver) = mpsc::channel();
    let stdout = child.stdout.take();
    std::thread::spawn(move || {
        if let Some(stdout) = stdout {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        }
    });

    let wanted: Vec<u64> = payloads
        .iter()
        .filter_map(|p| p.get("id").and_then(|i| i.as_u64()))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut responses = Vec::new();
    loop {
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{} did not answer within 15s — is it an MCP server?",
                server.name
            ));
        }
        match receiver.recv_timeout(deadline - now) {
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} stopped answering mid-handshake.",
                    server.name
                ));
            }
            Ok(line) => {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    responses.push(value);
                }
                if wanted
                    .iter()
                    .all(|id| responses.iter().any(|r| r.get("id").and_then(|i| i.as_u64()) == Some(*id)))
                {
                    break;
                }
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(responses)
}

// -- HTTP transport ------------------------------------------------------------

/// Stateless POSTs. Servers needing a session handshake fail with guidance.
async fn http_roundtrip(server: &McpServer, payloads: &[Value]) -> Result<Vec<Value>, String> {
    use std::time::Duration;

    let url = server.url.as_deref().unwrap_or("").trim();
    if url.is_empty() {
        return Err(format!("{} has no URL.", server.name));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Web calls are unavailable: {e}"))?;
    let mut responses = Vec::new();
    for payload in payloads {
        // Notifications get no reply.
        if payload.get("id").is_none() {
            continue;
        }
        let response = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("{} did not answer: {e}", server.name))?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(format!(
                "{} needs an MCP session handshake, which Roundtable does not do — prefer its stdio command if it has one.",
                server.name
            ));
        }
        if !response.status().is_success() {
            return Err(format!(
                "{} answered HTTP {}.",
                server.name,
                response.status()
            ));
        }
        let is_stream = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|t| t.contains("text/event-stream"))
            .unwrap_or(false);
        if is_stream {
            let body = response.text().await.unwrap_or_default();
            let mut last: Option<Value> = None;
            for line in body.lines() {
                if let Some(data) = line.strip_prefix("data:") {
                    if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                        last = Some(value);
                    }
                }
            }
            responses.push(last.ok_or_else(|| {
                format!("{} streamed events Roundtable could not read.", server.name)
            })?);
        } else {
            responses.push(
                response
                    .json::<Value>()
                    .await
                    .map_err(|_| format!("{} answered with junk.", server.name))?,
            );
        }
    }
    Ok(responses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewMcpServer;

    /// A fake MCP server: ignores stdin, prints canned JSON-RPC replies.
    /// `responses` arrive in order; the client matches them by id.
    fn fake_server(name: &str, responses: &[&str]) -> (std::path::PathBuf, McpServer) {
        let dir = std::env::temp_dir().join(format!("roundtable-mcp-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake.sh");
        let mut body = String::from("#!/bin/sh\n");
        for response in responses {
            body.push_str(&format!("printf '%s\\n' '{response}'\n"));
        }
        std::fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        let server = McpServer {
            id: format!("srv-{name}"),
            name: "Fake".to_string(),
            transport: "stdio".to_string(),
            command: Some(script.to_string_lossy().to_string()),
            args: vec![],
            url: None,
            enabled: true,
            created_at: 0,
        };
        (dir, server)
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn stdio_lists_tools() {
        let init = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"fake","version":"1"}}}"#;
        let list = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo back","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}"#;
        let (dir, server) = fake_server("list", &[init, list]);
        let tools = list_tools(&server).await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, "Echo back");
        assert_eq!(tool_id(&server.id, "echo"), format!("mcp:{}:echo", server.id));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn stdio_calls_tool_and_flattens_text() {
        let init = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}"#;
        let call = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"hi back"},{"type":"image","data":"..."}]}}"#;
        let (dir, server) = fake_server("call", &[init, call]);
        let out = call_tool(&server, "echo", &serde_json::json!({"text": "hi"})).await.unwrap();
        assert!(out.contains("hi back"), "unexpected: {out}");
        assert!(out.contains("non-text content omitted"), "unexpected: {out}");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn stdio_surfaces_server_errors() {
        let init = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}"#;
        let err = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32602,"message":"Unknown tool"}}"#;
        let (dir, server) = fake_server("err", &[init, err]);
        let failure = call_tool(&server, "nope", &serde_json::json!({})).await.unwrap_err();
        assert!(failure.contains("Unknown tool"), "unexpected: {failure}");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn stdio_missing_command_fails_fast() {
        let server = McpServer {
            id: "x".to_string(),
            name: "Broken".to_string(),
            transport: "stdio".to_string(),
            command: Some("/nonexistent/roundtable-fake".to_string()),
            args: vec![],
            url: None,
            enabled: true,
            created_at: 0,
        };
        assert!(list_tools(&server).await.is_err());
    }

    #[test]
    fn new_mcp_server_shape_deserializes() {
        // The shell passes this shape straight from the UI.
        let input: NewMcpServer = serde_json::from_str(
            r#"{"name":"T","transport":"http","url":"https://x.example/mcp"}"#,
        )
        .unwrap();
        assert_eq!(input.transport, "http");
        assert!(input.args.is_empty());
    }
}
