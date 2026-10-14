//! Multi-agent rooms (Phase 3).
//!
//! A room binds a set of agents to a scheduler (`directed`, `orchestrated`,
//! or `round_robin`). Agents talk through three tools — `send_message`,
//! `delegate`, and `finish` — delivered natively when the model supports
//! function calling and as JSON/TEXT fallbacks otherwise. Every turn, tool
//! result, and run-state change is persisted to SQLite and emitted as a
//! [`RoomEvent`] so the shell can stream progress to the UI.
//!
//! Shapes here mirror `src/lib/api.ts` (rooms section) field-for-field.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::agents::Agent;
use crate::kb::{now_ms, KbStore};
use crate::providers::{chat_stream, ChatMsg, ChatOptions, ToolCall, ToolDef};

// ---------------------------------------------------------------------------
// Shapes (JSON field-for-field with the TS interfaces)
// ---------------------------------------------------------------------------

/// A multi-agent room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub title: String,
    pub mode: String,
    pub lead_agent_id: Option<String>,
    pub max_turns: i64,
    pub token_budget: i64,
    pub timeout_ms: i64,
    pub loop_detection: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One seat in a room: which agent sits there and whom it may message.
/// `can_talk_to` holds agent ids plus `"room"` for room-wide broadcasts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMember {
    pub room_id: String,
    pub agent_id: String,
    pub seat: i64,
    pub can_talk_to: Vec<String>,
}

/// A room plus its members, as the room commands return it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomDetail {
    pub room: Room,
    pub members: Vec<RoomMember>,
}

/// One executed tool call attached to a room message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomToolCall {
    pub id: String,
    pub tool: String,
    pub args: serde_json::Value,
    #[serde(default)]
    pub result: Option<String>,
}

/// One row of room history: user text, agent turns, tool results (as
/// `system` messages), and stop notices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMessage {
    pub id: String,
    pub room_id: String,
    pub sender: String,
    pub sender_agent_id: Option<String>,
    pub recipient: Option<String>,
    pub recipient_agent_id: Option<String>,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<RoomToolCall>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub prompt_tokens: Option<i64>,
    #[serde(default)]
    pub completion_tokens: Option<i64>,
    #[serde(default)]
    pub total_tokens: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
    pub created_at: i64,
}

/// The persisted run record of a room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomRunState {
    pub room_id: String,
    pub status: String,
    pub stop_reason: Option<String>,
    pub turn_count: i64,
    pub tokens_used: i64,
    pub updated_at: i64,
}

/// Sparse update for a room: every field is optional.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub lead_agent_id: Option<Option<String>>,
    #[serde(default)]
    pub max_turns: Option<i64>,
    #[serde(default)]
    pub token_budget: Option<i64>,
    #[serde(default)]
    pub timeout_ms: Option<i64>,
    #[serde(default)]
    pub loop_detection: Option<bool>,
}

/// One seat assignment for `rooms_set_members`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMemberInput {
    pub agent_id: String,
    pub seat: i64,
    pub can_talk_to: Vec<String>,
}

/// Live pause/step/stop flags for a running driver. Owned by the shell's
/// room registry and shared with the driver task. `approvals` carries human
/// tool decisions by call id (`once` | `always` | `deny`); the shell fills it
/// via `rooms_approve` and the driver consumes entries as they arrive.
#[derive(Debug, Default)]
pub struct RoomControl {
    pub paused: AtomicBool,
    pub step_once: AtomicBool,
    pub stopped: AtomicBool,
    pub approvals: Mutex<HashMap<String, String>>,
}

/// Streamed progress events. The `"type"` tag matches the TS `RoomEvent`
/// union exactly.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoomEvent {
    Token { agent_id: String, delta: String },
    Message { message: RoomMessage },
    ToolCall { agent_id: String, call: RoomToolCall },
    ToolResult {
        agent_id: String,
        call_id: String,
        tool: String,
        result: String,
    },
    RunState { state: RoomRunState },
    /// A risky tool needs a human decision. The shell shows a dialog and
    /// answers via `rooms_approve`; the driver waits (pausable/stoppable).
    Approval {
        agent_id: String,
        call_id: String,
        tool: String,
        summary: String,
        command: Option<String>,
    },
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Tools + prompt augmentation
// ---------------------------------------------------------------------------

/// The three room tools in OpenAI function shape.
pub fn room_tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "send_message".to_string(),
            description: "Send a message to one teammate or broadcast it to the whole room."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "string",
                        "description": "Target agent id, or \"room\" to broadcast to everyone."
                    },
                    "content": { "type": "string", "description": "The message text." }
                },
                "required": ["to", "content"]
            }),
        },
        ToolDef {
            name: "delegate".to_string(),
            description: "Ask one teammate to do a subtask right now and return their answer."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Teammate agent id." },
                    "task": { "type": "string", "description": "What you need from them." }
                },
                "required": ["agent", "task"]
            }),
        },
        ToolDef {
            name: "finish".to_string(),
            description: "End your turn with the final answer for the room.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "result": { "type": "string", "description": "The final answer text." }
                },
                "required": ["result"]
            }),
        },
    ]
}

/// System-prompt appendix for room turns: who is in the room plus how to
/// act. `members` holds `(agent_id, display_name)` pairs. Native models get
/// the same text so ids and etiquette are always visible.
pub fn tool_system_augmentation(members: &[(String, String)]) -> String {
    let mut text =
        String::from("You are collaborating in a shared room. Teammates (name + id):\n");
    for (id, name) in members {
        text.push_str(&format!("- {name} (id: {id})\n"));
    }
    text.push_str(
        r###"To act, call exactly one tool per turn:
- send_message {"to": "<agent id>" or "room", "content": "..."} — talk to one teammate or broadcast to everyone.
- delegate {"agent": "<agent id>", "task": "..."} — have a teammate do a subtask now; you get their answer back.
- finish {"result": "..."} — end with the final answer.
You may only message teammates your room allows (violations are rejected — use "room" to reach everyone).
If you cannot use native tool calls, reply with exactly one JSON object like {"tool": "send_message", "args": {"to": "room", "content": "..."}} or a line like TOOL: finish {"result": "..."}."###,
    );
    text
}

/// Whether an approved risky call may run, and what the audit log records.
struct ApprovalUse {
    proceed: bool,
    /// once | always | grant (prior "always") | deny
    decision: &'static str,
}

/// Shorten to `max` chars, noting the cut. Never splits a character.
fn truncate_text(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}\n…(truncated)")
}

/// Jail a user-supplied path inside the workspace folder. Rejects absolute
/// paths and anything escaping with `..`. Pure and unit-tested.
fn workspace_path(root: &std::path::Path, user: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    let user = user.trim().trim_start_matches('/');
    if user.is_empty() {
        return Err("Give a path inside the workspace folder.".to_string());
    }
    if user == "." {
        return Ok(root.to_path_buf());
    }
    let mut clean = PathBuf::new();
    for component in std::path::Path::new(user).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err("Give a path inside the workspace folder.".to_string())
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !clean.pop() {
                    return Err("That path escapes the workspace folder.".to_string());
                }
            }
            Component::Normal(part) => clean.push(part),
        }
    }
    let full = root.join(clean);
    if !full.starts_with(root) {
        return Err("That path escapes the workspace folder.".to_string());
    }
    Ok(full)
}

/// `workspace/docs/a.md` instead of `/home/…/workspace/docs/a.md`.
fn display_rel(root: &std::path::Path, full: &std::path::Path) -> String {
    full.strip_prefix(root)
        .unwrap_or(full)
        .to_string_lossy()
        .to_string()
}

/// Run one program with no network and a 30s timeout. Prefers
/// `docker run --rm --network none` when a daemon with the image is present;
/// otherwise a stripped-environment local subprocess (documented
/// best-effort sandbox, not a security boundary). Blocking and synchronous
/// by design — callers run quick programs; output is capped at 20KB.
fn run_sandboxed(language: &str, code: &str) -> String {
    use std::io::Read;
    use std::process::{Command, Stdio};

    if language != "python" && language != "python3" && language != "sh" {
        return "run_code supports language \"python\" or \"sh\".".to_string();
    }
    let workdir = std::env::temp_dir().join(format!("roundtable-run-{}", uuid::Uuid::new_v4()));
    let setup = (|| -> Result<(), String> {
        std::fs::create_dir_all(&workdir).map_err(|e| format!("Could not stage the run: {e}"))?;
        let file = if language == "sh" { "main.sh" } else { "main.py" };
        std::fs::write(workdir.join(file), code).map_err(|e| format!("Could not stage the run: {e}"))?;
        Ok(())
    })();
    if let Err(message) = setup {
        return message;
    }
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&workdir);
    };

    // Docker first, but only when the image is already local — never trigger
    // a network pull from inside a tool call.
    let docker_ok = Command::new("docker")
        .arg("image")
        .arg("inspect")
        .arg(if language == "sh" { "alpine:3.19" } else { "python:3-slim" })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let child = if docker_ok {
        let mount = format!("{}:/work", workdir.display());
        let (image, run_args): (&str, Vec<&str>) = if language == "sh" {
            ("alpine:3.19", vec!["sh", "/work/main.sh"])
        } else {
            ("python:3-slim", vec!["python", "/work/main.py"])
        };
        let mut cmd_args = vec![
            "run", "--rm", "--network", "none", "--memory", "256m", "--cpus", "1", "-v",
            &mount, "-w", "/work", image,
        ];
        cmd_args.extend(run_args);
        match Command::new("docker")
            .args(&cmd_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => Some((child, "(docker) ")),
            Err(_) => None,
        }
    } else {
        None
    };
    let (mut child, prefix) = match child {
        Some(pair) => pair,
        None => {
            let (prog, arg): (&str, &str) = if language == "sh" {
                ("sh", "main.sh")
            } else if Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
                ("python3", "main.py")
            } else {
                ("python", "main.py")
            };
            let spawned = Command::new(prog)
                .arg(workdir.join(arg))
                .current_dir(&workdir)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &workdir)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            match spawned {
                Ok(child) => (child, "(local) "),
                Err(e) => {
                    cleanup();
                    return format!("Could not start the runner: {e}");
                }
            }
        }
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                break None;
            }
        }
    };
    let mut output = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut output);
    }
    if let Some(mut err) = child.stderr.take() {
        let mut err_text = String::new();
        let _ = err.read_to_string(&mut err_text);
        if !err_text.trim().is_empty() {
            output.push_str("\n[stderr]\n");
            output.push_str(&err_text);
        }
    }
    cleanup();
    match status {
        None => format!("{prefix}timed out after 30s (killed).\n{}", truncate_text(&output, 20_000)),
        Some(status) if status.success() => {
            format!("{prefix}exit 0.\n{}", truncate_text(&output, 20_000))
        }
        Some(status) => format!(
            "{prefix}failed with {status}.\n{}",
            truncate_text(&output, 20_000)
        ),
    }
}

/// Native tool definitions for the Phase 4 external tools, gated per agent:
/// file tools need the workspace permission, web_search the network one,
/// run_code the shell one. Agents without permissions get no extra defs.
pub fn phase4_tool_defs(agent: &Agent) -> Vec<ToolDef> {
    let mut defs = Vec::new();
    if agent.permissions.workspace {
        defs.push(ToolDef {
            name: "read_file".to_string(),
            description: "Read a text file inside the workspace folder.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path inside the workspace, e.g. \"docs/a.md\"." }
                },
                "required": ["path"]
            }),
        });
        defs.push(ToolDef {
            name: "write_file".to_string(),
            description: "Write (or overwrite) a text file inside the workspace folder. Asks the human first."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path inside the workspace." },
                    "content": { "type": "string", "description": "Full new file content." }
                },
                "required": ["path", "content"]
            }),
        });
        defs.push(ToolDef {
            name: "list_dir".to_string(),
            description: "List a folder inside the workspace.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Folder inside the workspace (\".\" for the root)." }
                },
                "required": ["path"]
            }),
        });
    }
    if agent.permissions.network {
        defs.push(ToolDef {
            name: "web_search".to_string(),
            description: "Search the web and get short snippets with sources.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The search query." }
                },
                "required": ["query"]
            }),
        });
    }
    if agent.permissions.shell {
        defs.push(ToolDef {
            name: "run_code".to_string(),
            description: "Run one Python or shell snippet in a sandbox (no network, 30s). Asks the human first."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "language": { "type": "string", "description": "\"python\" or \"sh\"." },
                    "code": { "type": "string", "description": "The program to run." }
                },
                "required": ["language", "code"]
            }),
        });
    }
    defs
}

/// Appendix for agents with external access, so JSON/text-protocol models
/// see the same tools. Empty for plain-chat agents (keeps prompts stable).
pub fn external_tools_augmentation(agent: &Agent) -> String {
    let mut lines = Vec::new();
    if agent.permissions.workspace {
        lines.push("- read_file {\"path\": \"...\"} — read a workspace file; list_dir {\"path\": \".\"} — list a folder; write_file {\"path\": \"...\", \"content\": \"...\"} — overwrite a file (asks the human first).");
    }
    if agent.permissions.network {
        lines.push("- web_search {\"query\": \"...\"} — search the web for facts with sources.");
    }
    if agent.permissions.shell {
        lines.push("- run_code {\"language\": \"python\" or \"sh\", \"code\": \"...\"} — run one snippet sandboxed, no network, 30s (asks the human first).");
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("Extra tools you may call:\n{}", lines.join("\n"))
}

/// One MCP tool pinned to a native-safe wire name for a single drive.
/// Grant ids stay stable (`mcp:<server_id>:<tool>`); wire names only need
/// to be unique within the run.
#[derive(Debug, Clone)]
struct McpWireTool {
    wire: String,
    server_id: String,
    server_name: String,
    name: String,
    description: String,
    schema: serde_json::Value,
}

/// `mcp__<id8>__<tool>` — native function names allow `[a-zA-Z0-9_-]`.
fn mcp_wire_name(server_id: &str, tool: &str) -> String {
    let id8: String = server_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    let clean: String = tool
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    format!("mcp__{id8}__{clean}")
}

/// Load the assigned MCP tools for this drive. Only servers with at least
/// one assigned tool among the room's agents are contacted; failures skip
/// the server (the UI's live listing explains why).
async fn load_mcp_catalog(
    servers: Vec<crate::db::McpServer>,
    agents: &HashMap<String, Agent>,
    members: &[RoomMember],
) -> Vec<McpWireTool> {
    let mut wanted: std::collections::HashSet<String> = std::collections::HashSet::new();
    for member in members {
        if let Some(agent) = agents.get(&member.agent_id) {
            for assignment in &agent.tools {
                if let Some((server, _)) = assignment.split_once(':') {
                    wanted.insert(server.trim().to_string());
                }
            }
        }
    }
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut catalog = Vec::new();
    for server in servers.iter().filter(|s| s.enabled && wanted.contains(&s.id)) {
        let tools = match crate::mcp::list_tools(server).await {
            Ok(tools) => tools,
            Err(_) => continue,
        };
        for tool in tools {
            let wire = mcp_wire_name(&server.id, &tool.name);
            if catalog.iter().any(|e: &McpWireTool| e.wire == wire) {
                continue;
            }
            catalog.push(McpWireTool {
                wire,
                server_id: server.id.clone(),
                server_name: server.name.clone(),
                name: tool.name,
                description: tool.description,
                schema: tool.input_schema,
            });
        }
    }
    catalog
}

/// Native defs for the agent's assigned MCP tools that are live this run.
fn mcp_tool_defs(catalog: &[McpWireTool], agent: &Agent) -> Vec<ToolDef> {
    let mut defs = Vec::new();
    for entry in catalog {
        if !agent
            .tools
            .iter()
            .any(|t| t == &format!("{}:{}", entry.server_id, entry.name))
        {
            continue;
        }
        let schema = if entry.schema.is_object() {
            entry.schema.clone()
        } else {
            serde_json::json!({"type": "object"})
        };
        defs.push(ToolDef {
            name: entry.wire.clone(),
            description: format!("{} → {}: {}", entry.server_name, entry.name, entry.description),
            parameters: schema,
        });
    }
    defs
}

/// Appendix so JSON/text-protocol models see their assigned MCP tools.
fn mcp_augmentation(catalog: &[McpWireTool], agent: &Agent) -> String {
    let mut lines = Vec::new();
    for entry in catalog {
        if agent
            .tools
            .iter()
            .any(|t| t == &format!("{}:{}", entry.server_id, entry.name))
        {
            lines.push(format!(
                "- {} {{\"args as JSON\"}} — {} → {}: {} (asks the human first)",
                entry.wire, entry.server_name, entry.name, entry.description
            ));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("Assigned external tools:\n{}", lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Context building (pure)
// ---------------------------------------------------------------------------

/// Build the provider message history for one agent: the newest visible
/// messages, oldest first, capped at 40.
///
/// Visible means public (`recipient` null), broadcast to the room, sent by
/// the agent itself, directed at the agent, or written by the user/system.
/// The agent's own turns read back as `assistant`, system rows as `system`,
/// and everything else as `user` with a `"[Name]: "` speaker prefix.
pub fn build_context(
    log: &[RoomMessage],
    agent_id: &str,
    agent_name_by_id: &HashMap<String, String>,
) -> Vec<ChatMsg> {
    let mut out = Vec::new();
    for msg in log.iter().rev() {
        if out.len() >= 40 {
            break;
        }
        if !is_visible_to(msg, agent_id) {
            continue;
        }
        out.push(to_chat_msg(msg, agent_id, agent_name_by_id));
    }
    out.reverse();
    out
}

fn is_visible_to(msg: &RoomMessage, agent_id: &str) -> bool {
    if msg.sender == "user" || msg.sender == "system" {
        return true;
    }
    if msg.recipient.is_none() {
        return true;
    }
    if msg.recipient.as_deref() == Some("room") {
        return true;
    }
    if msg.sender_agent_id.as_deref() == Some(agent_id) {
        return true;
    }
    if msg.recipient.as_deref() == Some("agent")
        && msg.recipient_agent_id.as_deref() == Some(agent_id)
    {
        return true;
    }
    false
}

fn display_name(msg: &RoomMessage, names: &HashMap<String, String>) -> String {
    if msg.sender == "user" {
        return "User".to_string();
    }
    if let Some(id) = msg.sender_agent_id.as_deref() {
        if let Some(name) = names.get(id) {
            return name.clone();
        }
        return "Agent".to_string();
    }
    "System".to_string()
}

fn to_chat_msg(
    msg: &RoomMessage,
    agent_id: &str,
    names: &HashMap<String, String>,
) -> ChatMsg {
    if msg.sender == "system" {
        return ChatMsg {
            role: "system".to_string(),
            content: msg.content.clone(),
        };
    }
    if msg.sender == "agent" && msg.sender_agent_id.as_deref() == Some(agent_id) {
        return ChatMsg {
            role: "assistant".to_string(),
            content: msg.content.clone(),
        };
    }
    let name = display_name(msg, names);
    ChatMsg {
        role: "user".to_string(),
        content: format!("[{name}]: {}", msg.content),
    }
}

// ---------------------------------------------------------------------------
// Tool-call parsing (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PendingCall {
    tool: String,
    args: serde_json::Value,
}

fn finish_call(result: &str) -> PendingCall {
    PendingCall {
        tool: "finish".to_string(),
        args: serde_json::json!({ "result": result }),
    }
}

/// Parse `{"tool": "<name>", "args": {...}}`. `args` defaults to null.
fn parse_tool_object(text: &str) -> Option<PendingCall> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = value.as_object()?;
    let tool = obj.get("tool")?.as_str()?;
    if tool.trim().is_empty() {
        return None;
    }
    Some(PendingCall {
        tool: tool.to_string(),
        args: obj.get("args").cloned().unwrap_or(serde_json::Value::Null),
    })
}

/// Every balanced `{...}` span, longest first. String-aware so braces
/// inside JSON strings do not split spans.
fn brace_spans_longest_first(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => stack.push(i),
            b'}' => {
                if let Some(start) = stack.pop() {
                    spans.push((start, i));
                }
            }
            _ => {}
        }
    }
    spans.sort_by_key(|(s, e)| e.saturating_sub(*s));
    spans.reverse();
    spans
}

/// First longest `{...}` substring that parses as a tool object, plus the
/// text around it (the "left over" prose).
fn parse_substring_call(text: &str) -> Option<(PendingCall, String)> {
    for (start, end) in brace_spans_longest_first(text) {
        if let Some(call) = parse_tool_object(&text[start..=end]) {
            let mut leftover = text[..start].trim().to_string();
            let tail = text[end + 1..].trim();
            if !tail.is_empty() {
                if !leftover.is_empty() {
                    leftover.push('\n');
                }
                leftover.push_str(tail);
            }
            return Some((call, leftover));
        }
    }
    None
}

/// TEXT protocol: `TOOL: <name> <json-args>` lines. Anything left over
/// becomes `finish(result=text)`.
fn parse_text_tool_calls(content: &str) -> Vec<PendingCall> {
    let mut calls = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        let rest = if trimmed.len() >= 5 && trimmed[..5].eq_ignore_ascii_case("tool:") {
            Some(trimmed[5..].trim())
        } else {
            None
        };
        match rest {
            Some(rest) if !rest.is_empty() => {
                let mut parts = rest.splitn(2, char::is_whitespace);
                let name = parts.next().unwrap_or("").trim().to_string();
                let args_text = parts.next().unwrap_or("").trim();
                if name.is_empty() {
                    prose.push(line);
                    continue;
                }
                let args = if args_text.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str(args_text)
                        .unwrap_or_else(|_| serde_json::Value::String(args_text.to_string()))
                };
                calls.push(PendingCall { tool: name, args });
            }
            _ => prose.push(line),
        }
    }
    let leftover = prose.join("\n").trim().to_string();
    if !leftover.is_empty() {
        calls.push(finish_call(&leftover));
    } else if calls.is_empty() {
        calls.push(finish_call(content.trim()));
    }
    calls
}

/// Native turns: real tool calls win; otherwise a JSON tool object in the
/// text counts as one call (lenient); anything left over is
/// `finish(result=text)`.
fn calls_for_native(content: &str, raw: &[ToolCall]) -> Vec<PendingCall> {
    if !raw.is_empty() {
        return raw
            .iter()
            .map(|c| PendingCall {
                tool: c.name.clone(),
                args: serde_json::from_str(&c.arguments).unwrap_or_else(|_| {
                    serde_json::Value::String(c.arguments.clone())
                }),
            })
            .collect();
    }
    let trimmed = content.trim();
    if let Some(call) = parse_tool_object(trimmed) {
        return vec![call];
    }
    if let Some((call, _)) = parse_substring_call(trimmed) {
        return vec![call];
    }
    vec![finish_call(trimmed)]
}

/// JSON turns: whole content as `{"tool","args"}`; else the largest `{...}`
/// substring (plus leftover prose as finish); else the TEXT rules.
fn calls_for_json(content: &str) -> Vec<PendingCall> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return vec![finish_call("")];
    }
    if let Some(call) = parse_tool_object(trimmed) {
        return vec![call];
    }
    if let Some((call, leftover)) = parse_substring_call(trimmed) {
        let mut out = vec![call];
        if !leftover.trim().is_empty() {
            out.push(finish_call(leftover.trim()));
        }
        return out;
    }
    parse_text_tool_calls(content)
}

/// Pull the result text out of `finish` args in any shape.
fn finish_text_of(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => match map.get("result") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) if other.is_null() => String::new(),
            Some(other) => serde_json::to_string(other).unwrap_or_default(),
            None => serde_json::to_string(args).unwrap_or_default(),
        },
        serde_json::Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Loop detection + token estimates (pure)
// ---------------------------------------------------------------------------

/// True when the last six directed agent→agent messages strictly alternate
/// between the same pair (A→B, B→A, …) — a ping-pong loop.
pub fn detect_loop(log: &[RoomMessage]) -> bool {
    let directed: Vec<&RoomMessage> = log
        .iter()
        .filter(|m| m.sender == "agent" && m.recipient.as_deref() == Some("agent"))
        .collect();
    if directed.len() < 6 {
        return false;
    }
    let last6 = &directed[directed.len() - 6..];
    let (Some(a), Some(b)) = (
        last6[0].sender_agent_id.as_deref(),
        last6[0].recipient_agent_id.as_deref(),
    ) else {
        return false;
    };
    if a == b {
        return false;
    }
    for (i, m) in last6.iter().enumerate() {
        let (expect_from, expect_to) = if i % 2 == 0 { (a, b) } else { (b, a) };
        if m.sender_agent_id.as_deref() != Some(expect_from) {
            return false;
        }
        if m.recipient_agent_id.as_deref() != Some(expect_to) {
            return false;
        }
    }
    true
}

/// Rough token estimate: characters divided by four.
pub fn estimate_tokens(text: &str) -> i64 {
    text.chars().count() as i64 / 4
}

/// Does this provider error look like a function-calling refusal (unknown
/// `tools` parameter)? Used for the auto → JSON fallback.
fn is_tools_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("tool") || lower.contains("unknown parameter") || lower.contains("400")
}

/// Earliest `@Name` mention in `content` matching an enabled member
/// (case-insensitive). Returns the agent id, if any.
fn find_mentioned_agent(content: &str, candidates: &[(String, String)]) -> Option<String> {
    let lower = content.to_lowercase();
    let mut best: Option<(usize, String)> = None;
    for (id, name) in candidates {
        let needle = format!("@{}", name.to_lowercase());
        if let Some(pos) = lower.find(needle.as_str()) {
            let replace = match &best {
                Some((best_pos, _)) if *best_pos <= pos => false,
                _ => true,
            };
            if replace {
                best = Some((pos, id.clone()));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Human-readable stop notice persisted as a system message.
fn human_stop_text(reason: &str, max_turns: i64) -> String {
    match reason {
        "turn_limit" => format!("Turn limit reached ({max_turns} turns)."),
        "token_budget" => "Token budget exceeded.".to_string(),
        "timeout" => "Time budget exceeded.".to_string(),
        "loop_detected" => {
            "Stopped: two agents repeated the same exchange — looks like a loop.".to_string()
        }
        "stopped_by_user" => "Stopped by you.".to_string(),
        "no_agent_mentioned" => "Mention an agent with @Name to direct a reply.".to_string(),
        "reply_complete" => "Replied.".to_string(),
        "lead_finished" => "Lead synthesized the final answer.".to_string(),
        "finished_by_agent" => "An agent called finish.".to_string(),
        _ => "Stopped.".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Credentials for one room member, resolved by the shell (keys travel in
/// memory only — never the database, logs, or exports).
#[derive(Debug, Clone)]
pub struct AgentCreds {
    pub agent_id: String,
    pub base_url: String,
    pub key: Option<String>,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

/// Everything [`drive_room`] needs from the shell. The emitter may borrow
/// from the caller's scope (handy for tests); the shell passes an owned
/// channel sender.
pub struct DriveDeps<'a> {
    pub db_path: PathBuf,
    pub creds: HashMap<String, AgentCreds>,
    pub control: Arc<RoomControl>,
    pub cancel: Arc<AtomicBool>,
    pub emit: Box<dyn FnMut(RoomEvent) + Send + 'a>,
    /// Tavily (or compatible) key for the web_search tool. `None` means the
    /// tool reports itself unconfigured instead of failing.
    pub web_search_key: Option<String>,
    /// User-picked workspace folder for the file tools. `None` disables them.
    pub workspace_dir: Option<PathBuf>,
}

struct TurnOutcome {
    finished: bool,
    finish_result: Option<String>,
    last_content: String,
}

struct ToolExec {
    result: String,
    finished: bool,
    finish_text: Option<String>,
}

impl ToolExec {
    fn ok(result: String) -> Self {
        Self {
            result,
            finished: false,
            finish_text: None,
        }
    }

    fn err(result: String) -> Self {
        Self {
            result,
            finished: false,
            finish_text: None,
        }
    }
}

/// A fatal turn failure: the whole run stops with this status/reason and
/// the system notice is persisted for the user.
struct FatalStop {
    status: String,
    reason: String,
    system_text: String,
}

impl FatalStop {
    fn provider(detail: impl Into<String>) -> Self {
        Self {
            status: "stopped".to_string(),
            reason: "provider_error".to_string(),
            system_text: format!("The model call failed: {}", detail.into()),
        }
    }

    fn db(err: impl ToString) -> Self {
        Self {
            status: "stopped".to_string(),
            reason: "internal_error".to_string(),
            system_text: format!("Something went wrong with the room database: {}", err.to_string()),
        }
    }
}

struct Driver<'a, 'b> {
    db_path: &'a Path,
    creds: &'a HashMap<String, AgentCreds>,
    control: &'a RoomControl,
    cancel: &'a AtomicBool,
    emit: &'a mut Box<dyn FnMut(RoomEvent) + Send + 'b>,
    room: Room,
    members: Vec<RoomMember>,
    workspace_dir: Option<PathBuf>,
    web_search_key: Option<String>,
    mcp_catalog: Vec<McpWireTool>,    agents: HashMap<String, Agent>,
    names: HashMap<String, String>,
    turn_count: i64,
    tokens_used: i64,
    started: Instant,
    call_seq: u64,
}

impl<'a, 'b> Driver<'a, 'b> {
    async fn load(room_id: &str, deps: &'a mut DriveDeps<'b>) -> Result<Self, String> {
        let store = KbStore::open(&deps.db_path).map_err(|e| e.to_string())?;
        store.migrate_chat().map_err(|e| e.to_string())?;
        let detail = store
            .get_room_detail(room_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "That room is gone.".to_string())?;
        let mut agents = HashMap::new();
        let mut names = HashMap::new();
        for member in &detail.members {
            if let Some(agent) = store
                .get_agent(&member.agent_id)
                .map_err(|e| e.to_string())?
            {
                names.insert(agent.id.clone(), agent.name.clone());
                agents.insert(agent.id.clone(), agent);
            }
        }
        if let Some(lead) = detail.room.lead_agent_id.as_deref() {
            if agents.get(lead).is_none() {
                if let Some(agent) = store.get_agent(lead).map_err(|e| e.to_string())? {
                    names.insert(agent.id.clone(), agent.name.clone());
                    agents.insert(agent.id.clone(), agent);
                }
            }
        }
        let servers = store.list_mcp_servers().unwrap_or_default();
        let mcp_catalog = load_mcp_catalog(servers, &agents, &detail.members).await;
        Ok(Self {
            db_path: &deps.db_path,
            creds: &deps.creds,
            control: &deps.control,
            cancel: &deps.cancel,
            emit: &mut deps.emit,
            room: detail.room,
            members: detail.members,
            workspace_dir: deps.workspace_dir.clone(),
            web_search_key: deps.web_search_key.clone(),
            mcp_catalog,
            agents,
            names,
            turn_count: 0,
            tokens_used: 0,
            started: Instant::now(),
            call_seq: 0,
        })
    }

    fn store(&self) -> Result<KbStore, FatalStop> {
        let store = KbStore::open(self.db_path).map_err(FatalStop::db)?;
        store.migrate_chat().map_err(FatalStop::db)?;
        Ok(store)
    }

    fn open_store(&self) -> Result<KbStore, String> {
        let store = KbStore::open(self.db_path).map_err(|e| e.to_string())?;
        store.migrate_chat().map_err(|e| e.to_string())?;
        Ok(store)
    }

    fn agent_name(&self, agent_id: &str) -> String {
        self.names
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| "The agent".to_string())
    }

    fn is_member(&self, agent_id: &str) -> bool {
        self.members.iter().any(|m| m.agent_id == agent_id)
    }

    fn can_talk(&self, from_id: &str, to: &str) -> bool {
        self.members
            .iter()
            .find(|m| m.agent_id == from_id)
            .map(|m| m.can_talk_to.iter().any(|t| t == to))
            .unwrap_or(false)
    }

    fn member_list(&self) -> Vec<(String, String)> {
        self.members
            .iter()
            .map(|m| {
                (
                    m.agent_id.clone(),
                    self.names
                        .get(&m.agent_id)
                        .cloned()
                        .unwrap_or_else(|| m.agent_id.clone()),
                )
            })
            .collect()
    }

    fn stop_requested(&self) -> bool {
        self.control.stopped.load(Ordering::Relaxed) || self.cancel.load(Ordering::Relaxed)
    }

    fn list_log(&self) -> Result<Vec<RoomMessage>, FatalStop> {
        Ok(self
            .store()?
            .list_room_messages(&self.room.id)
            .map_err(FatalStop::db)?)
    }

    fn current_state(&self, status: &str, stop_reason: Option<&str>) -> RoomRunState {
        RoomRunState {
            room_id: self.room.id.clone(),
            status: status.to_string(),
            stop_reason: stop_reason.map(str::to_string),
            turn_count: self.turn_count,
            tokens_used: self.tokens_used,
            updated_at: now_ms(),
        }
    }

    fn save_state(&self, state: &RoomRunState) -> Result<(), FatalStop> {
        self.store()?
            .save_room_run(state)
            .map_err(FatalStop::db)
    }

    fn emit_state(&mut self, status: &str) {
        let state = self.current_state(status, None);
        (self.emit)(RoomEvent::RunState { state });
    }

    fn persist_status(&self, status: &str) -> Result<(), FatalStop> {
        self.save_state(&self.current_state(status, None))
    }

    fn emit_msg(&mut self, message: RoomMessage) {
        (self.emit)(RoomEvent::Message { message });
    }

    fn append_user(&self, text: &str) -> Result<RoomMessage, FatalStop> {
        Ok(self
            .store()?
            .add_room_message(
                &self.room.id,
                "user",
                None,
                None,
                None,
                text,
                &[],
                "",
                None,
                None,
                None,
                None,
            )
            .map_err(FatalStop::db)?)
    }

    fn append_system(&self, text: &str) -> Result<RoomMessage, FatalStop> {
        Ok(self
            .store()?
            .add_room_message(
                &self.room.id,
                "system",
                None,
                None,
                None,
                text,
                &[],
                "",
                None,
                None,
                None,
                None,
            )
            .map_err(FatalStop::db)?)
    }

    /// Guardrails, checked at the top of every scheduler iteration.
    fn check_guardrails(&self, log: &[RoomMessage]) -> Option<(String, String)> {
        if self.turn_count >= self.room.max_turns {
            return Some(("stopped".to_string(), "turn_limit".to_string()));
        }
        if self.tokens_used > self.room.token_budget {
            return Some(("stopped".to_string(), "token_budget".to_string()));
        }
        if self.started.elapsed().as_millis() as i64 > self.room.timeout_ms {
            return Some(("stopped".to_string(), "timeout".to_string()));
        }
        if self.room.loop_detection && detect_loop(log) {
            return Some(("stopped".to_string(), "loop_detected".to_string()));
        }
        None
    }

    async fn wait_if_paused(&mut self) {
        while self.control.paused.load(Ordering::Relaxed)
            && !self.control.step_once.load(Ordering::Relaxed)
        {
            if self.stop_requested() {
                break;
            }
            self.emit_state("paused");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn after_turn(&mut self) {
        if self.control.step_once.load(Ordering::Relaxed) {
            self.control.step_once.store(false, Ordering::Relaxed);
            self.control.paused.store(true, Ordering::Relaxed);
            let _ = self.persist_status("paused");
            self.emit_state("paused");
        }
    }

    fn resolve_directed_target(&self, log: &[RoomMessage]) -> Option<String> {
        let latest = log.iter().rev().find(|m| m.sender == "user")?;
        let candidates: Vec<(String, String)> = self
            .members
            .iter()
            .filter_map(|m| {
                let agent = self.agents.get(&m.agent_id)?;
                if !agent.enabled {
                    return None;
                }
                Some((agent.id.clone(), agent.name.clone()))
            })
            .collect();
        find_mentioned_agent(&latest.content, &candidates)
    }

    fn next_round_robin(&self) -> Option<String> {
        if self.members.is_empty() {
            return None;
        }
        for offset in 0..self.members.len() {
            let idx = (self.turn_count as usize + offset) % self.members.len();
            let id = &self.members[idx].agent_id;
            if self.agents.get(id).map(|a| a.enabled).unwrap_or(false) {
                return Some(id.clone());
            }
        }
        None
    }

    fn next_call_id(&mut self) -> String {
        self.call_seq += 1;
        let room8: String = self
            .room
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(8)
            .collect();
        format!("call-{room8}-{}-{}", self.turn_count, self.call_seq)
    }

    #[allow(clippy::too_many_lines)]
    async fn run_agent_turn(
        &mut self,
        agent_id: &str,
        depth: u32,
    ) -> Result<TurnOutcome, FatalStop> {
        let agent = self
            .agents
            .get(agent_id)
            .cloned()
            .ok_or_else(|| FatalStop::provider("That agent is gone — set the room members again."))?;
        let creds = self.creds.get(agent_id).cloned().ok_or_else(|| {
            FatalStop::provider(format!(
                "{} has no provider or model configured — edit the agent and pick both.",
                agent.name
            ))
        })?;
        let log = self.list_log()?;
        let context = build_context(&log, agent_id, &self.names);
        let mut system = format!(
            "{}\n\n{}",
            agent.system_prompt,
            tool_system_augmentation(&self.member_list())
        );
        let extra = external_tools_augmentation(&agent);
        if !extra.is_empty() {
            system.push_str("\n\n");
            system.push_str(&extra);
        }
        let mcp_extra = mcp_augmentation(&self.mcp_catalog, &agent);
        if !mcp_extra.is_empty() {
            system.push_str("\n\n");
            system.push_str(&mcp_extra);
        }
        let prompt_chars: usize =
            system.chars().count() + context.iter().map(|m| m.content.chars().count()).sum::<usize>();

        let mut options = ChatOptions {
            system_prompt: Some(system.clone()),
            temperature: Some(creds.temperature),
            max_tokens: if creds.max_tokens > 0 {
                Some(creds.max_tokens)
            } else {
                None
            },
            tools: None,
        };
        let explicit = agent.tool_protocol.clone();
        let mut protocol = if explicit == "auto" {
            "native".to_string()
        } else {
            explicit.clone()
        };
        if protocol == "native" {
            let mut defs = room_tool_defs();
            defs.extend(phase4_tool_defs(&agent));
            defs.extend(mcp_tool_defs(&self.mcp_catalog, &agent));
            options.tools = Some(defs);
        }

        let started = Instant::now();
        let agent_label = agent_id.to_string();
        let emit = &mut *self.emit;
        let mut on_delta = |delta: String| {
            emit(RoomEvent::Token {
                agent_id: agent_label.clone(),
                delta,
            });
        };
        let mut raw = match chat_stream(
            &creds.base_url,
            creds.key.as_deref(),
            &creds.model,
            &context,
            &options,
            self.cancel,
            &mut on_delta,
        )
        .await
        {
            Ok(raw) => raw,
            Err(first) => {
                let message = first.to_string();
                if protocol == "native" && explicit == "auto" && is_tools_error(&message) {
                    // The endpoint choked on function calling: remember JSON
                    // for this agent and retry the turn without tools.
                    let _ = self
                        .open_store()
                        .and_then(|store| {
                            store
                                .set_agent_protocol(agent_id, "json")
                                .map_err(|e| e.to_string())
                        });
                    if let Some(entry) = self.agents.get_mut(agent_id) {
                        entry.tool_protocol = "json".to_string();
                    }
                    protocol = "json".to_string();
                    options.tools = None;
                    let emit_retry = &mut *self.emit;
                    let mut on_retry = |delta: String| {
                        emit_retry(RoomEvent::Token {
                            agent_id: agent_id.to_string(),
                            delta,
                        });
                    };
                    match chat_stream(
                        &creds.base_url,
                        creds.key.as_deref(),
                        &creds.model,
                        &context,
                        &options,
                        self.cancel,
                        &mut on_retry,
                    )
                    .await
                    {
                        Ok(raw) => raw,
                        Err(second) => return Err(FatalStop::provider(second.to_string())),
                    }
                } else {
                    return Err(FatalStop::provider(message));
                }
            }
        };
        // Silence an unused-assignment lint if the retry path never ran.
        let _ = &protocol;
        let latency_ms = started.elapsed().as_millis() as i64;

        let pending: Vec<PendingCall> = match protocol.as_str() {
            "native" => calls_for_native(&raw.content, &raw.tool_calls),
            "json" => calls_for_json(&raw.content),
            _ => parse_text_tool_calls(&raw.content),
        };

        let prompt_est = prompt_chars as i64 / 4;
        let completion_est = estimate_tokens(&raw.content);
        let prompt_tokens = raw
            .usage
            .prompt_tokens
            .map(i64::from)
            .unwrap_or(prompt_est);
        let completion_tokens = raw
            .usage
            .completion_tokens
            .map(i64::from)
            .unwrap_or(completion_est);
        let total_tokens = raw
            .usage
            .total_tokens
            .map(i64::from)
            .unwrap_or(prompt_tokens + completion_tokens);
        self.tokens_used += total_tokens;
        self.turn_count += 1;
        let content = std::mem::take(&mut raw.content);

        // Persist the requesting message first (calls pending), so the UI
        // can anchor tool_call/tool_result events to it.
        let pending_calls: Vec<RoomToolCall> = pending
            .iter()
            .map(|p| RoomToolCall {
                id: String::new(),
                tool: p.tool.clone(),
                args: p.args.clone(),
                result: None,
            })
            .collect();
        let requesting = self
            .store()?
            .add_room_message(
                &self.room.id,
                "agent",
                Some(agent_id),
                None,
                None,
                &content,
                &pending_calls,
                &creds.model,
                Some(prompt_tokens),
                Some(completion_tokens),
                Some(total_tokens),
                Some(latency_ms),
            )
            .map_err(FatalStop::db)?;
        self.emit_msg(requesting.clone());

        let mut finished = false;
        let mut finish_result: Option<String> = None;
        let mut calls: Vec<RoomToolCall> = Vec::with_capacity(pending.len());
        for p in &pending {
            let id = self.next_call_id();
            let call = RoomToolCall {
                id: id.clone(),
                tool: p.tool.clone(),
                args: p.args.clone(),
                result: None,
            };
            (self.emit)(RoomEvent::ToolCall {
                agent_id: agent_id.to_string(),
                call: call.clone(),
            });
            let exec = Box::pin(self.execute_tool(agent_id, depth, &p.tool, &p.args)).await?;
            (self.emit)(RoomEvent::ToolResult {
                agent_id: agent_id.to_string(),
                call_id: id.clone(),
                tool: p.tool.clone(),
                result: exec.result.clone(),
            });
            let system_msg = self
                .store()?
                .add_room_message(
                    &self.room.id,
                    "system",
                    None,
                    None,
                    None,
                    &format!("[tool:{}] {}", p.tool, exec.result),
                    &[],
                    "",
                    None,
                    None,
                    None,
                    None,
                )
                .map_err(FatalStop::db)?;
            self.emit_msg(system_msg);
            if exec.finished && finish_result.is_none() {
                finish_result = exec.finish_text.clone();
            }
            finished = finished || exec.finished;
            calls.push(RoomToolCall {
                id,
                tool: p.tool.clone(),
                args: p.args.clone(),
                result: Some(exec.result),
            });
        }
        self.store()?
            .set_room_message_tool_calls(&requesting.id, &calls)
            .map_err(FatalStop::db)?;

        let state = self.current_state("running", None);
        let _ = self.save_state(&state);
        (self.emit)(RoomEvent::RunState { state });

        Ok(TurnOutcome {
            finished,
            finish_result,
            last_content: content,
        })
    }

    /// One executed tool call, audited. Room builtins run freely; external
    /// tools check the agent's permissions first, and risky ones ask the
    /// human through `approval_gate`. Every call lands in the audit log.
    async fn execute_tool(
        &mut self,
        actor_id: &str,
        depth: u32,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<ToolExec, FatalStop> {
        let args_text = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
        let (exec, decision): (ToolExec, &'static str) = match tool {
            "send_message" | "delegate" | "finish" => (
                Box::pin(self.exec_builtin(actor_id, depth, tool, args)).await?,
                "allow",
            ),
            "read_file" | "write_file" | "list_dir" => {
                self.exec_workspace(actor_id, tool, args).await?
            }
            "web_search" => self.exec_web_search(actor_id, args).await?,
            "run_code" => self.exec_run_code(actor_id, args).await?,
            t if t.starts_with("mcp__") => self.exec_mcp_tool(actor_id, t, args).await?,
            other => (
                ToolExec::err(format!(
                    "Unknown tool \"{other}\" — use send_message, delegate, finish, or an assigned external tool."
                )),
                "deny",
            ),
        };
        if let Ok(store) = self.open_store() {
            let _ = store.log_audit(
                Some(&self.room.id),
                Some(actor_id),
                tool,
                &truncate_text(&args_text, 2000),
                decision,
                &exec.result,
            );
        }
        Ok(exec)
    }

    /// Ask the human about a risky call. Returns whether to proceed and the
    /// decision for the audit log. A prior "always" grant skips the dialog;
    /// stopping mid-wait stops the whole run.
    async fn approval_gate(
        &mut self,
        actor_id: &str,
        grant_id: &str,
        tool: &str,
        summary: String,
        command: Option<String>,
    ) -> Result<ApprovalUse, FatalStop> {
        if let Ok(store) = self.open_store() {
            if let Ok(true) = store.has_tool_grant(actor_id, grant_id) {
                return Ok(ApprovalUse { proceed: true, decision: "grant" });
            }
        }
        let call_id = self.next_call_id();
        (self.emit)(RoomEvent::Approval {
            agent_id: actor_id.to_string(),
            call_id: call_id.clone(),
            tool: tool.to_string(),
            summary,
            command,
        });
        loop {
            if self.stop_requested() {
                return Err(FatalStop {
                    status: "stopped".to_string(),
                    reason: "stopped_by_user".to_string(),
                    system_text: "Stopped by you.".to_string(),
                });
            }
            let decision = self
                .control
                .approvals
                .lock()
                .map(|mut grants| grants.remove(&call_id))
                .unwrap_or(None);
            match decision.as_deref() {
                Some("once") => return Ok(ApprovalUse { proceed: true, decision: "once" }),
                Some("always") => {
                    if let Ok(store) = self.open_store() {
                        let _ = store.add_tool_grant(actor_id, grant_id);
                    }
                    return Ok(ApprovalUse { proceed: true, decision: "always" });
                }
                Some(_) => return Ok(ApprovalUse { proceed: false, decision: "deny" }),
                None => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }

    /// Workspace-folder tools. Reads and listings run freely once permitted;
    /// writes ask the human. All paths are jailed to the workspace folder.
    async fn exec_workspace(
        &mut self,
        actor_id: &str,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<(ToolExec, &'static str), FatalStop> {
        let agent_label = self.agent_name(actor_id);
        let permitted = self
            .agents
            .get(actor_id)
            .map(|a| a.permissions.workspace)
            .unwrap_or(false);
        if !permitted {
            return Ok((
                ToolExec::err(format!(
                    "{agent_label} has no workspace permission — enable it under the agent's Tools & permissions."
                )),
                "deny-permission",
            ));
        }
        let root = match self.workspace_dir.clone() {
            Some(dir) => dir,
            None => {
                return Ok((
                    ToolExec::err(
                        "No workspace folder is set — pick one in Settings → Tools.".to_string(),
                    ),
                    "deny",
                ))
            }
        };
        let path_arg = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        match tool {
            "read_file" => match workspace_path(&root, path_arg) {
                Err(message) => Ok((ToolExec::err(message), "deny")),
                Ok(full) => match std::fs::read_to_string(&full) {
                    Err(e) => Ok((
                        ToolExec::err(format!("Could not read {}: {e}", display_rel(&root, &full))),
                        "deny",
                    )),
                    Ok(text) => {
                        let capped = truncate_text(&text, 50_000);
                        Ok((
                            ToolExec::ok(format!(
                                "{} ({} chars):\n{}",
                                display_rel(&root, &full),
                                text.chars().count(),
                                capped
                            )),
                            "allow",
                        ))
                    }
                },
            },
            "list_dir" => {
                let rel = if path_arg.trim().is_empty() { "." } else { path_arg };
                match workspace_path(&root, rel) {
                    Err(message) => Ok((ToolExec::err(message), "deny")),
                    Ok(full) => match std::fs::read_dir(&full) {
                        Err(e) => Ok((
                            ToolExec::err(format!(
                                "Could not list {}: {e}",
                                display_rel(&root, &full)
                            )),
                            "deny",
                        )),
                        Ok(entries) => {
                            let mut names: Vec<String> = entries
                                .filter_map(|e| e.ok())
                                .map(|e| {
                                    let name = e.file_name().to_string_lossy().to_string();
                                    if e.path().is_dir() {
                                        format!("{name}/")
                                    } else {
                                        name
                                    }
                                })
                                .collect();
                            names.sort();
                            let total = names.len();
                            names.truncate(200);
                            let mut out = names.join("\n");
                            if total > 200 {
                                out.push_str(&format!("\n…({total} entries, showing 200)"));
                            }
                            if out.is_empty() {
                                out = "(empty folder)".to_string();
                            }
                            Ok((ToolExec::ok(out), "allow"))
                        }
                    },
                }
            }
            "write_file" => {
                let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let full = match workspace_path(&root, path_arg) {
                    Err(message) => return Ok((ToolExec::err(message), "deny")),
                    Ok(full) => full,
                };
                if content.len() > 200_000 {
                    return Ok((
                        ToolExec::err("That write is too big (max 200,000 chars).".to_string()),
                        "deny",
                    ));
                }
                let rel = display_rel(&root, &full);
                let use_ = self
                    .approval_gate(
                        actor_id,
                        "write_file",
                        tool,
                        format!(
                            "{agent_label} wants to write {} chars to {}.",
                            content.chars().count(),
                            rel
                        ),
                        Some(rel.clone()),
                    )
                    .await?;
                if !use_.proceed {
                    return Ok((
                        ToolExec::err("The human denied this write.".to_string()),
                        use_.decision,
                    ));
                }
                if let Some(parent) = full.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return Ok((
                            ToolExec::err(format!("Could not create folders for {rel}: {e}")),
                            "deny",
                        ));
                    }
                }
                match std::fs::write(&full, content) {
                    Err(e) => Ok((ToolExec::err(format!("Could not write {rel}: {e}")), "deny")),
                    Ok(()) => Ok((
                        ToolExec::ok(format!("Wrote {} chars to {}.", content.chars().count(), rel)),
                        use_.decision,
                    )),
                }
            }
            other => Ok((
                ToolExec::err(format!("Unknown workspace tool \"{other}\".")),
                "deny",
            )),
        }
    }

    /// Web search via a Tavily-compatible endpoint. Needs the network
    /// permission and a key (the shell holds it; core only spends it).
    async fn exec_web_search(
        &mut self,
        actor_id: &str,
        args: &serde_json::Value,
    ) -> Result<(ToolExec, &'static str), FatalStop> {
        let agent_label = self.agent_name(actor_id);
        let permitted = self
            .agents
            .get(actor_id)
            .map(|a| a.permissions.network)
            .unwrap_or(false);
        if !permitted {
            return Ok((
                ToolExec::err(format!(
                    "{agent_label} has no network permission — enable it under the agent's Tools & permissions."
                )),
                "deny-permission",
            ));
        }
        let key = match self.web_search_key.clone() {
            Some(key) if !key.trim().is_empty() => key,
            _ => {
                return Ok((
                    ToolExec::err(
                        "No web-search key is configured — add one in Settings → Tools.".to_string(),
                    ),
                    "deny",
                ))
            }
        };
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if query.is_empty() {
            return Ok((ToolExec::err("web_search needs a \"query\" string.".to_string()), "deny"));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| FatalStop::provider(format!("Web search could not start: {e}")))?;
        let response = client
            .post("https://api.tavily.com/search")
            .json(&serde_json::json!({
                "api_key": key,
                "query": query,
                "max_results": 5,
            }))
            .send()
            .await
            .map_err(|e| FatalStop::provider(format!("Web search failed: {e}")))?;
        if !response.status().is_success() {
            return Ok((
                ToolExec::err(format!("Web search failed (HTTP {}).", response.status())),
                "deny",
            ));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| FatalStop::provider(format!("Web search returned junk: {e}")))?;
        let results = body
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if results.is_empty() {
            return Ok((ToolExec::ok("No results.".to_string()), "allow"));
        }
        let mut out = String::new();
        for (i, item) in results.iter().take(5).enumerate() {
            let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("(no title)");
            let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let snippet = item.get("content").and_then(|c| c.as_str()).unwrap_or("");
            out.push_str(&format!(
                "{}. {} — {}\n{}\n",
                i + 1,
                title,
                url,
                truncate_text(snippet, 500)
            ));
        }
        Ok((ToolExec::ok(out), "allow"))
    }

    /// Sandboxed code runner. Needs the shell permission plus a human
    /// approval (unless granted "always"). Prefers Docker when available,
    /// otherwise a stripped-environment subprocess with a 30s timeout.
    async fn exec_run_code(
        &mut self,
        actor_id: &str,
        args: &serde_json::Value,
    ) -> Result<(ToolExec, &'static str), FatalStop> {
        let agent_label = self.agent_name(actor_id);
        let permitted = self
            .agents
            .get(actor_id)
            .map(|a| a.permissions.shell)
            .unwrap_or(false);
        if !permitted {
            return Ok((
                ToolExec::err(format!(
                    "{agent_label} has no shell permission — enable it under the agent's Tools & permissions."
                )),
                "deny-permission",
            ));
        }
        let language = args.get("language").and_then(|v| v.as_str()).unwrap_or("python");
        if language != "python" && language != "python3" && language != "sh" {
            return Ok((
                ToolExec::err("run_code supports language \"python\" or \"sh\".".to_string()),
                "deny",
            ));
        }
        let code = args.get("code").and_then(|v| v.as_str()).unwrap_or("");
        if code.trim().is_empty() {
            return Ok((ToolExec::err("run_code needs \"code\" text.".to_string()), "deny"));
        }
        if code.len() > 20_000 {
            return Ok((
                ToolExec::err("That program is too long (max 20,000 chars).".to_string()),
                "deny",
            ));
        }
        let use_ = self
            .approval_gate(
                actor_id,
                "run_code",
                "run_code",
                format!(
                    "{agent_label} wants to run {language} code ({} chars).",
                    code.chars().count()
                ),
                Some(truncate_text(code, 500)),
            )
            .await?;
        if !use_.proceed {
            return Ok((ToolExec::err("The human denied this run.".to_string()), use_.decision));
        }
        Ok((ToolExec::ok(run_sandboxed(language, code)), use_.decision))
    }

    /// Assigned MCP tools. Always ask the human (unless granted "always"):
    /// external side effects are never implicit.
    async fn exec_mcp_tool(
        &mut self,
        actor_id: &str,
        wire: &str,
        args: &serde_json::Value,
    ) -> Result<(ToolExec, &'static str), FatalStop> {
        let entry = match self.mcp_catalog.iter().find(|e| e.wire == wire).cloned() {
            Some(entry) => entry,
            None => {
                return Ok((
                    ToolExec::err(format!(
                        "That external tool is not available in this run — the server may be off or unlisted."
                    )),
                    "deny",
                ))
            }
        };
        let agent_label = self.agent_name(actor_id);
        let assigned = self
            .agents
            .get(actor_id)
            .map(|a| {
                a.tools
                    .iter()
                    .any(|t| t == &format!("{}:{}", entry.server_id, entry.name))
            })
            .unwrap_or(false);
        if !assigned {
            return Ok((
                ToolExec::err(format!(
                    "{} is not assigned to {} — assign it under the agent's Tools.",
                    entry.name, agent_label
                )),
                "deny-permission",
            ));
        }
        let args_text = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
        let use_ = self
            .approval_gate(
                actor_id,
                &crate::mcp::tool_id(&entry.server_id, &entry.name),
                wire,
                format!(
                    "{agent_label} wants to call {} → {}.",
                    entry.server_name, entry.name
                ),
                Some(truncate_text(&args_text, 500)),
            )
            .await?;
        if !use_.proceed {
            return Ok((ToolExec::err("The human denied this call.".to_string()), use_.decision));
        }
        let server = self
            .open_store()
            .map_err(FatalStop::db)?
            .get_mcp_server(&entry.server_id)
            .map_err(FatalStop::db)?
            .ok_or_else(|| FatalStop::provider(format!("{} was removed — assign the agent's tools again.", entry.server_name)))?;
        if !server.enabled {
            return Ok((
                ToolExec::err(format!("{} is turned off.", entry.server_name)),
                use_.decision,
            ));
        }
        match crate::mcp::call_tool(&server, &entry.name, args).await {
            Ok(text) => Ok((ToolExec::ok(text), use_.decision)),
            Err(message) => Ok((ToolExec::err(message), use_.decision)),
        }
    }

    async fn exec_builtin(
        &mut self,
        actor_id: &str,
        depth: u32,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<ToolExec, FatalStop> {
        match tool {
            "send_message" => {
                let to = args
                    .get("to")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if to.is_empty() {
                    return Ok(ToolExec::err(
                        "send_message needs a \"to\" — an agent id or \"room\".".to_string(),
                    ));
                }
                if content.trim().is_empty() {
                    return Ok(ToolExec::err("send_message needs \"content\" text.".to_string()));
                }
                if to == "room" {
                    if !self.can_talk(actor_id, "room") {
                        return Ok(ToolExec::err(format!(
                            "{} is not allowed to broadcast to the room.",
                            self.agent_name(actor_id)
                        )));
                    }
                    let msg = self
                        .store()?
                        .add_room_message(
                            &self.room.id,
                            "agent",
                            Some(actor_id),
                            Some("room"),
                            None,
                            &content,
                            &[],
                            "",
                            None,
                            None,
                            None,
                            None,
                        )
                        .map_err(FatalStop::db)?;
                    self.emit_msg(msg);
                    Ok(ToolExec::ok("Sent to the room.".to_string()))
                } else {
                    if !self.is_member(&to) {
                        return Ok(ToolExec::err(format!(
                            "There is no agent \"{to}\" in this room — check the id."
                        )));
                    }
                    if !self.can_talk(actor_id, &to) {
                        return Ok(ToolExec::err(format!(
                            "{} is not allowed to message {} directly.",
                            self.agent_name(actor_id),
                            self.agent_name(&to)
                        )));
                    }
                    let target_name = self.agent_name(&to);
                    let msg = self
                        .store()?
                        .add_room_message(
                            &self.room.id,
                            "agent",
                            Some(actor_id),
                            Some("agent"),
                            Some(&to),
                            &content,
                            &[],
                            "",
                            None,
                            None,
                            None,
                            None,
                        )
                        .map_err(FatalStop::db)?;
                    self.emit_msg(msg);
                    Ok(ToolExec::ok(format!("Sent to {target_name}.")))
                }
            }
            "delegate" => {
                if depth >= 3 {
                    return Ok(ToolExec::err(
                        "Delegation is nested too deep (max 3) — finish with what you have."
                            .to_string(),
                    ));
                }
                let target = args
                    .get("agent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let task = args
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if target.is_empty() {
                    return Ok(ToolExec::err(
                        "delegate needs an \"agent\" id.".to_string(),
                    ));
                }
                if task.trim().is_empty() {
                    return Ok(ToolExec::err("delegate needs a \"task\" description.".to_string()));
                }
                if !self.is_member(&target) {
                    return Ok(ToolExec::err(format!(
                        "There is no agent \"{target}\" in this room — check the id."
                    )));
                }
                if !self.can_talk(actor_id, &target) {
                    return Ok(ToolExec::err(format!(
                        "{} is not allowed to delegate to {}.",
                        self.agent_name(actor_id),
                        self.agent_name(&target)
                    )));
                }
                if self.turn_count >= self.room.max_turns
                    || self.tokens_used > self.room.token_budget
                {
                    return Ok(ToolExec::err(
                        "The room hit its budget — wrap up with finish.".to_string(),
                    ));
                }
                // The task itself is a visible directed message so the
                // transcript shows what was asked.
                let task_msg = self
                    .store()?
                    .add_room_message(
                        &self.room.id,
                        "agent",
                        Some(actor_id),
                        Some("agent"),
                        Some(&target),
                        &task,
                        &[],
                        "",
                        None,
                        None,
                        None,
                        None,
                    )
                    .map_err(FatalStop::db)?;
                self.emit_msg(task_msg);
                match Box::pin(self.run_agent_turn(&target, depth + 1)).await {
                    Ok(outcome) => {
                        let answer = outcome
                            .finish_result
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| {
                                if outcome.last_content.trim().is_empty() {
                                    "(the worker gave no answer)".to_string()
                                } else {
                                    outcome.last_content
                                }
                            });
                        Ok(ToolExec::ok(answer))
                    }
                    Err(fatal) => Err(fatal),
                }
            }
            "finish" => {
                let text = finish_text_of(args);
                if !text.trim().is_empty() {
                    let msg = self
                        .store()?
                        .add_room_message(
                            &self.room.id,
                            "agent",
                            Some(actor_id),
                            None,
                            None,
                            &text,
                            &[],
                            "",
                            None,
                            None,
                            None,
                            None,
                        )
                        .map_err(FatalStop::db)?;
                    self.emit_msg(msg);
                    Ok(ToolExec {
                        result: text.clone(),
                        finished: true,
                        finish_text: Some(text),
                    })
                } else {
                    Ok(ToolExec {
                        result: "(finished)".to_string(),
                        finished: true,
                        finish_text: Some(String::new()),
                    })
                }
            }
            other => Ok(ToolExec::err(format!(
                "Unknown tool \"{other}\" — use send_message, delegate, or finish."
            ))),
        }
    }

    fn finish_run(
        &mut self,
        status: &str,
        reason: &str,
        system_text: Option<String>,
    ) -> RoomRunState {
        let text =
            system_text.unwrap_or_else(|| human_stop_text(reason, self.room.max_turns));
        if let Ok(store) = self.open_store() {
            if let Ok(msg) = store.add_room_message(
                &self.room.id,
                "system",
                None,
                None,
                None,
                &text,
                &[],
                "",
                None,
                None,
                None,
                None,
            ) {
                self.emit_msg(msg);
            }
            let state = RoomRunState {
                room_id: self.room.id.clone(),
                status: status.to_string(),
                stop_reason: Some(reason.to_string()),
                turn_count: self.turn_count,
                tokens_used: self.tokens_used,
                updated_at: now_ms(),
            };
            let _ = store.save_room_run(&state);
            let _ = store.touch_room(&self.room.id);
            (self.emit)(RoomEvent::RunState {
                state: state.clone(),
            });
            return state;
        }
        let state = self.current_state(status, Some(reason));
        (self.emit)(RoomEvent::RunState {
            state: state.clone(),
        });
        state
    }
}

/// Run a room: append `user_text` as a user message when it is `Some` and
/// non-empty, then drive the room's scheduler until it stops. Every message
/// and the final run state are persisted; progress flows through
/// `deps.emit`. Returns the final [`RoomRunState`].
pub async fn drive_room(
    room_id: &str,
    user_text: Option<String>,
    deps: &mut DriveDeps<'_>,
) -> RoomRunState {
    let mut driver = match Driver::load(room_id, deps).await {
        Ok(driver) => driver,
        Err(message) => {
            (deps.emit)(RoomEvent::Error { message });
            return RoomRunState {
                room_id: room_id.to_string(),
                status: "stopped".to_string(),
                stop_reason: Some("room_missing".to_string()),
                turn_count: 0,
                tokens_used: 0,
                updated_at: now_ms(),
            };
        }
    };

    if let Some(text) = user_text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        match driver.append_user(text) {
            Ok(msg) => driver.emit_msg(msg),
            Err(fatal) => {
                return driver.finish_run(&fatal.status, &fatal.reason, Some(fatal.system_text));
            }
        }
    }

    // A fresh run: budgets apply per drive.
    driver.turn_count = 0;
    driver.tokens_used = 0;
    driver.started = Instant::now();
    if driver.persist_status("running").is_err() {
        let state = driver.current_state("running", None);
        (driver.emit)(RoomEvent::RunState { state: state.clone() });
    } else {
        driver.emit_state("running");
    }

    let (status, reason, system_text): (String, String, Option<String>) = loop {
        if driver.stop_requested() {
            break (
                "stopped".to_string(),
                "stopped_by_user".to_string(),
                None,
            );
        }
        driver.wait_if_paused().await;
        if driver.stop_requested() {
            break (
                "stopped".to_string(),
                "stopped_by_user".to_string(),
                None,
            );
        }
        let log = match driver.list_log() {
            Ok(log) => log,
            Err(fatal) => break (fatal.status, fatal.reason, Some(fatal.system_text)),
        };
        if let Some((status, reason)) = driver.check_guardrails(&log) {
            break (status, reason, None);
        }

        match driver.room.mode.as_str() {
            "directed" => {
                match driver.resolve_directed_target(&log) {
                    None => {
                        if let Err(fatal) = driver
                            .append_system("Mention an agent with @Name to direct a reply.")
                            .map(|msg| driver.emit_msg(msg))
                        {
                            break (fatal.status, fatal.reason, Some(fatal.system_text));
                        }
                        break (
                            "stopped".to_string(),
                            "no_agent_mentioned".to_string(),
                            None,
                        );
                    }
                    Some(target) => match driver.run_agent_turn(&target, 0).await {
                        Ok(_) => {
                            driver.after_turn();
                            break ("done".to_string(), "reply_complete".to_string(), None);
                        }
                        Err(fatal) => break (fatal.status, fatal.reason, Some(fatal.system_text)),
                    },
                }
            }
            "orchestrated" => {
                let lead = driver.room.lead_agent_id.clone().unwrap_or_default();
                let lead_ok = !lead.is_empty()
                    && driver.is_member(&lead)
                    && driver.agents.get(&lead).map(|a| a.enabled).unwrap_or(false);
                if !lead_ok {
                    let text =
                        "The lead agent is gone or disabled — open the room and pick another lead.";
                    if let Err(fatal) = driver
                        .append_system(text)
                        .map(|msg| driver.emit_msg(msg))
                    {
                        break (fatal.status, fatal.reason, Some(fatal.system_text));
                    }
                    break ("stopped".to_string(), "lead_missing".to_string(), Some(text.to_string()));
                }
                match driver.run_agent_turn(&lead, 0).await {
                    Ok(outcome) => {
                        driver.after_turn();
                        if outcome.finished {
                            break ("done".to_string(), "lead_finished".to_string(), None);
                        }
                    }
                    Err(fatal) => break (fatal.status, fatal.reason, Some(fatal.system_text)),
                }
            }
            _ => {
                // round_robin (and any future mode defaults to rotation).
                match driver.next_round_robin() {
                    None => {
                        let text = "The room has no enabled members to speak.";
                        if let Err(fatal) = driver
                            .append_system(text)
                            .map(|msg| driver.emit_msg(msg))
                        {
                            break (fatal.status, fatal.reason, Some(fatal.system_text));
                        }
                        break (
                            "stopped".to_string(),
                            "provider_error".to_string(),
                            Some(text.to_string()),
                        );
                    }
                    Some(next) => match driver.run_agent_turn(&next, 0).await {
                        Ok(outcome) => {
                            driver.after_turn();
                            if outcome.finished {
                                break (
                                    "done".to_string(),
                                    "finished_by_agent".to_string(),
                                    None,
                                );
                            }
                        }
                        Err(fatal) => break (fatal.status, fatal.reason, Some(fatal.system_text)),
                    },
                }
            }
        }
    };

    driver.finish_run(&status, &reason, system_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_msg(
        sender: &str,
        from: Option<&str>,
        recipient: Option<&str>,
        to: Option<&str>,
        content: &str,
    ) -> RoomMessage {
        RoomMessage {
            id: "m".to_string(),
            room_id: "r".to_string(),
            sender: sender.to_string(),
            sender_agent_id: from.map(str::to_string),
            recipient: recipient.map(str::to_string),
            recipient_agent_id: to.map(str::to_string),
            content: content.to_string(),
            tool_calls: vec![],
            model: String::new(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            latency_ms: None,
            created_at: 0,
        }
    }

    fn names() -> HashMap<String, String> {
        HashMap::from([
            ("a".to_string(), "Alice".to_string()),
            ("b".to_string(), "Bob".to_string()),
        ])
    }

    #[test]
    fn context_filters_maps_and_caps() {
        let mut log = vec![
            test_msg("user", None, None, None, "hello all"),
            test_msg("agent", Some("a"), None, None, "Alice public"),
            test_msg("agent", Some("a"), Some("agent"), Some("b"), "Alice to Bob"),
            test_msg("agent", Some("b"), Some("agent"), Some("a"), "Bob to Alice"),
            test_msg("system", None, None, None, "sys note"),
            test_msg("agent", Some("b"), Some("room"), None, "Bob broadcast"),
        ];
        // Bob's view: everything here involves Bob, the room, or the user.
        let ctx = build_context(&log, "b", &names());
        assert_eq!(ctx.len(), 6);
        assert_eq!(ctx[0].role, "user");
        assert_eq!(ctx[0].content, "[User]: hello all");
        assert_eq!(ctx[1].role, "user");
        assert!(ctx[1].content.starts_with("[Alice]: "));
        assert_eq!(ctx[2].role, "user");
        assert_eq!(ctx[3].role, "assistant");
        assert_eq!(ctx[3].content, "Bob to Alice");
        assert_eq!(ctx[4].role, "system");
        assert_eq!(ctx[5].role, "assistant");

        // Alice never sees Bob's private note to someone else.
        log.push(test_msg("agent", Some("b"), Some("agent"), Some("zzz"), "secret"));
        let ctx = build_context(&log, "a", &names());
        assert!(!ctx.iter().any(|m| m.content.contains("secret")));

        // Cap: 50 visible messages keep only the newest 40.
        let mut big = Vec::new();
        for i in 0..50 {
            big.push(test_msg("user", None, None, None, &format!("msg {i}")));
        }
        let ctx = build_context(&big, "a", &names());
        assert_eq!(ctx.len(), 40);
        assert!(ctx[0].content.contains("msg 10"));
        assert!(ctx[39].content.contains("msg 49"));
    }

    #[test]
    fn loop_detection_needs_six_alternating() {
        let pair = |from: &str, to: &str| test_msg("agent", Some(from), Some("agent"), Some(to), "x");
        // Only five directed messages: no loop.
        let log: Vec<_> = (0..5)
            .map(|i| if i % 2 == 0 { pair("a", "b") } else { pair("b", "a") })
            .collect();
        assert!(!detect_loop(&log));
        // Six alternating between the same pair: loop.
        let mut log = log;
        log.push(pair("b", "a"));
        assert!(detect_loop(&log));
        // Same count but one message breaks the alternation.
        let mut broken = log.clone();
        broken[3] = pair("a", "b");
        assert!(!detect_loop(&broken));
        // Broadcasts and public notes never count.
        let noise = vec![
            test_msg("agent", Some("a"), Some("room"), None, "hi all"),
            test_msg("agent", Some("a"), None, None, "public"),
        ];
        assert!(!detect_loop(&noise));
    }

    #[test]
    fn json_and_text_parsing() {
        // Whole-object parse.
        let calls = calls_for_json(r#"{"tool": "finish", "args": {"result": "done"}}"#);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "finish");

        // Largest-substring parse keeps leftover prose as finish.
        let calls = calls_for_json(
            "My take: {\"tool\": \"send_message\", \"args\": {\"to\": \"room\", \"content\": \"hi\"}} thanks",
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool, "send_message");
        assert_eq!(calls[1].tool, "finish");
        assert!(finish_text_of(&calls[1].args).contains("thanks"));

        // TEXT lines, multiple calls plus prose.
        let calls = parse_text_tool_calls(
            "TOOL: send_message {\"to\": \"room\", \"content\": \"hi\"}\nTOOL: finish {\"result\": \"bye\"}\nsome prose",
        );
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].tool, "send_message");
        assert_eq!(calls[1].tool, "finish");
        assert_eq!(calls[2].tool, "finish");

        // Bare prose is finish.
        let calls = parse_text_tool_calls("just talking");
        assert_eq!(calls.len(), 1);
        assert_eq!(finish_text_of(&calls[0].args), "just talking");

        // Native lenient: JSON text with no tool calls counts as one call.
        let calls = calls_for_native(
            r#"{"tool": "send_message", "args": {"to": "room", "content": "hi"}}"#,
            &[],
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "send_message");

        // Native plain prose is finish.
        let calls = calls_for_native("hello everyone", &[]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "finish");
    }

    #[test]
    fn token_estimate_and_tool_errors() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens(""), 0);
        assert!(is_tools_error("The endpoint refused the request (HTTP 400): Unknown parameter: tools"));
        assert!(is_tools_error("unknown parameter \"tool_choice\""));
        assert!(!is_tools_error("API key rejected (HTTP 401/403)"));
    }

    #[test]
    fn mention_resolution_is_case_insensitive() {
        let candidates = vec![
            ("id-lead".to_string(), "Lead".to_string()),
            ("id-bob".to_string(), "Bob".to_string()),
        ];
        assert_eq!(
            find_mentioned_agent("hey @bob can you help?", &candidates).as_deref(),
            Some("id-bob")
        );
        assert_eq!(find_mentioned_agent("hello all", &candidates), None);
        // Earliest mention wins.
        assert_eq!(
            find_mentioned_agent("@Lead then @Bob", &candidates).as_deref(),
            Some("id-lead")
        );
    }

    // -- integration: drive rooms against scripted mock servers --------------

    fn tool_sse(id: &str, name: &str, args: serde_json::Value) -> String {
        let args_str = serde_json::to_string(&args).unwrap();
        let chunk = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": id,
                        "function": {"name": name, "arguments": args_str}
                    }]
                },
                "finish_reason": null
            }]
        });
        let end = serde_json::json!({"choices": [{"finish_reason": "tool_calls"}]});
        format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n")
    }

    fn text_sse(text: &str) -> String {
        let chunk = serde_json::json!({
            "choices": [{"delta": {"content": text}, "finish_reason": null}]
        });
        let end = serde_json::json!({"choices": [{"finish_reason": "stop"}]});
        format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n")
    }

    fn http_response(body: &str, content_type: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// Scripted OpenAI-compatible server: pops the next SSE body per model,
    /// falling back to `default_body` when a queue runs dry.
    async fn spawn_scripted(
        routes: HashMap<String, VecDeque<String>>,
        default_body: String,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let routes = Arc::new(Mutex::new(routes));
        let handle = tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 0];
                let mut tmp = [0u8; 8192];
                let body_text = loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    if n == 0 {
                        break String::new();
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    let head = String::from_utf8_lossy(&buf);
                    if let Some(pos) = head.find("\r\n\r\n") {
                        let headers = &head[..pos];
                        let len = headers
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                            .and_then(|l| l.split(':').nth(1))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= pos + 4 + len {
                            break head[pos + 4..pos + 4 + len].to_string();
                        }
                    }
                };
                let model = serde_json::from_str::<serde_json::Value>(&body_text)
                    .ok()
                    .and_then(|v| {
                        v.get("model")
                            .and_then(|m| m.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                let sse = {
                    let mut locked = routes.lock().unwrap();
                    locked
                        .get_mut(&model)
                        .and_then(|q| q.pop_front())
                        .unwrap_or_else(|| default_body.clone())
                };
                sock.write_all(&http_response(&sse, "text/event-stream"))
                    .await
                    .unwrap();
            }
        });
        (format!("http://{addr}/v1"), handle)
    }

    fn temp_store(name: &str) -> (PathBuf, KbStore) {
        let path =
            std::env::temp_dir().join(format!("roundtable-rooms-test-{name}-{}", uuid::Uuid::new_v4()));
        let store = KbStore::open(&path).unwrap();
        store.migrate_chat().unwrap();
        (path, store)
    }

    fn cleanup(path: &Path) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    fn new_agent(name: &str, model: &str, protocol: &str) -> crate::NewAgent {
        crate::NewAgent {
            name: name.to_string(),
            avatar_color: "#2c5fd0".to_string(),
            avatar_icon: "T".to_string(),
            role: "teammate".to_string(),
            system_prompt: "You are a helpful teammate.".to_string(),
            provider_id: None,
            model: model.to_string(),
            temperature: 0.5,
            max_tokens: 100,
            enabled: true,
            tool_protocol: protocol.to_string(),
            permissions: crate::AgentPermissions::default(),
            tools: Vec::new(),
        }
    }

    fn creds_for(agent_id: &str, base_url: &str, model: &str) -> AgentCreds {
        AgentCreds {
            agent_id: agent_id.to_string(),
            base_url: base_url.to_string(),
            key: None,
            model: model.to_string(),
            temperature: 0.5,
            max_tokens: 100,
        }
    }

    fn has_event(events: &[RoomEvent], kind: &str) -> bool {
        events.iter().any(|e| match e {
            RoomEvent::Token { .. } => kind == "token",
            RoomEvent::Message { .. } => kind == "message",
            RoomEvent::ToolCall { .. } => kind == "tool_call",
            RoomEvent::ToolResult { .. } => kind == "tool_result",
            RoomEvent::RunState { .. } => kind == "run_state",
            RoomEvent::Approval { .. } => kind == "approval",
            RoomEvent::Error { .. } => kind == "error",
        })
    }

    fn seed_approvals(control: &RoomControl, room_id: &str, decision: &str) {
        // Approval call ids are sequential per drive (call-<room8>-<turn>-<seq>);
        // pre-seed a range so tests don't depend on exact numbering.
        let room8: String = room_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(8)
            .collect();
        let mut grants = control.approvals.lock().unwrap();
        for turn in 0..4 {
            for seq in 1..8 {
                grants.insert(format!("call-{room8}-{turn}-{seq}"), decision.to_string());
            }
        }
    }

    fn tool_result_text(events: &[RoomEvent], tool: &str) -> Option<String> {
        events.iter().find_map(|e| match e {
            RoomEvent::ToolResult { tool: t, result, .. } if t == tool => Some(result.clone()),
            _ => None,
        })
    }

    #[tokio::test]
    async fn orchestrated_lead_delegates_then_finishes() {
        let (path, store) = temp_store("orchestrated");
        let lead = store.add_agent(&new_agent("Lead", "lead-model", "auto")).unwrap();
        let worker = store.add_agent(&new_agent("Worker", "worker-model", "auto")).unwrap();
        let detail = store
            .create_room("Team", &[lead.id.clone(), worker.id.clone()], "orchestrated", Some(&lead.id))
            .unwrap();

        let delegate_args = serde_json::json!({"agent": worker.id, "task": "summarize this"});
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert(
            "lead-model".to_string(),
            VecDeque::from([
                tool_sse("c1", "delegate", delegate_args),
                tool_sse("c2", "finish", serde_json::json!({"result": "final answer"})),
            ]),
        );
        routes.insert(
            "worker-model".to_string(),
            VecDeque::from([tool_sse(
                "c3",
                "finish",
                serde_json::json!({"result": "worker done"}),
            )]),
        );
        let finish_default = tool_sse("dx", "finish", serde_json::json!({"result": "extra"}));
        let (base, server) = spawn_scripted(routes, finish_default).await;

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([
                    (lead.id.clone(), creds_for(&lead.id, &base, "lead-model")),
                    (worker.id.clone(), creds_for(&worker.id, &base, "worker-model")),
                ]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("please summarize".into()), &mut deps).await
        };
        server.abort();

        assert_eq!(final_state.status, "done");
        assert_eq!(final_state.stop_reason.as_deref(), Some("lead_finished"));
        assert_eq!(final_state.turn_count, 3);
        assert!(has_event(&events, "tool_call"));
        assert!(has_event(&events, "tool_result"));
        assert!(has_event(&events, "message"));
        assert!(has_event(&events, "run_state"));

        let messages = store.list_room_messages(&detail.room.id).unwrap();
        let texts: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        assert!(texts.contains(&"please summarize"));
        assert!(texts.contains(&"final answer"));
        assert!(texts.contains(&"worker done"));
        assert!(texts.iter().any(|t| t.contains("Lead synthesized")));
        // The delegate tool result carries the worker's answer.
        assert!(texts.iter().any(|t| t.contains("[tool:delegate]") && t.contains("worker done")));
        // Requesting messages carry tool_calls JSON with results.
        assert!(messages.iter().any(|m| m.tool_calls.iter().any(|c| c.result.is_some())));
        cleanup(&path);
    }

    #[tokio::test]
    async fn directed_needs_an_at_mention() {
        let (path, store) = temp_store("directed-none");
        let agent = store.add_agent(&new_agent("Coder", "m", "auto")).unwrap();
        let detail = store
            .create_room("Pair", &[agent.id.clone()], "directed", None)
            .unwrap();

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::new(),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("hello there".into()), &mut deps).await
        };

        assert_eq!(final_state.status, "stopped");
        assert_eq!(final_state.stop_reason.as_deref(), Some("no_agent_mentioned"));
        assert_eq!(final_state.turn_count, 0);
        let messages = store.list_room_messages(&detail.room.id).unwrap();
        assert!(messages.iter().any(|m| m.content.contains("@Name")));
        assert!(!has_event(&events, "tool_call"));
        cleanup(&path);
    }

    #[tokio::test]
    async fn directed_runs_exactly_one_mentioned_turn() {
        let (path, store) = temp_store("directed-one");
        let coder = store.add_agent(&new_agent("Coder", "coder-model", "auto")).unwrap();
        let other = store.add_agent(&new_agent("Other", "other-model", "auto")).unwrap();
        let detail = store
            .create_room(
                "Pair",
                &[coder.id.clone(), other.id.clone()],
                "directed",
                None,
            )
            .unwrap();

        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert(
            "coder-model".to_string(),
            VecDeque::from([tool_sse(
                "c1",
                "send_message",
                serde_json::json!({"to": "room", "content": "on it"}),
            )]),
        );
        let finish_default = tool_sse("dx", "finish", serde_json::json!({"result": "extra"}));
        let (base, server) = spawn_scripted(routes, finish_default).await;

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([
                    (coder.id.clone(), creds_for(&coder.id, &base, "coder-model")),
                    (other.id.clone(), creds_for(&other.id, &base, "other-model")),
                ]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            // Lowercase mention still resolves (case-insensitive).
            drive_room(&detail.room.id, Some("hey @coder help".into()), &mut deps).await
        };
        server.abort();

        assert_eq!(final_state.status, "done");
        assert_eq!(final_state.stop_reason.as_deref(), Some("reply_complete"));
        assert_eq!(final_state.turn_count, 1);
        let messages = store.list_room_messages(&detail.room.id).unwrap();
        assert!(messages.iter().any(|m| m.content == "on it"));
        cleanup(&path);
    }

    #[tokio::test]
    async fn round_robin_hits_turn_limit() {
        let (path, store) = temp_store("turn-limit");
        let a = store.add_agent(&new_agent("Ann", "a-model", "auto")).unwrap();
        let b = store.add_agent(&new_agent("Ben", "b-model", "auto")).unwrap();
        let mut detail = store
            .create_room("R", &[a.id.clone(), b.id.clone()], "round_robin", None)
            .unwrap();
        detail.room.max_turns = 2;
        store
            .update_room(
                &detail.room.id,
                &RoomPatch {
                    max_turns: Some(2),
                    ..Default::default()
                },
            )
            .unwrap();

        let send_a = tool_sse("c", "send_message", serde_json::json!({"to": "room", "content": "a speaks"}));
        let send_b = tool_sse("c", "send_message", serde_json::json!({"to": "room", "content": "b speaks"}));
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("a-model".to_string(), VecDeque::from([send_a.clone(), send_a]));
        routes.insert("b-model".to_string(), VecDeque::from([send_b.clone(), send_b]));
        let finish_default = tool_sse("dx", "finish", serde_json::json!({"result": "extra"}));
        let (base, server) = spawn_scripted(routes, finish_default).await;

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([
                    (a.id.clone(), creds_for(&a.id, &base, "a-model")),
                    (b.id.clone(), creds_for(&b.id, &base, "b-model")),
                ]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        assert_eq!(final_state.status, "stopped");
        assert_eq!(final_state.stop_reason.as_deref(), Some("turn_limit"));
        assert_eq!(final_state.turn_count, 2);
        let messages = store.list_room_messages(&detail.room.id).unwrap();
        assert!(messages.iter().any(|m| m.content.contains("Turn limit reached (2 turns)")));
        cleanup(&path);
    }

    #[tokio::test]
    async fn loop_detection_stops_ping_pong() {
        let (path, store) = temp_store("loop");
        let a = store.add_agent(&new_agent("Ann", "a-model", "auto")).unwrap();
        let b = store.add_agent(&new_agent("Ben", "b-model", "auto")).unwrap();
        let detail = store
            .create_room("R", &[a.id.clone(), b.id.clone()], "round_robin", None)
            .unwrap();

        let to_b = tool_sse(
            "c",
            "send_message",
            serde_json::json!({"to": &b.id, "content": "ping"}),
        );
        let to_a = tool_sse(
            "c",
            "send_message",
            serde_json::json!({"to": &a.id, "content": "pong"}),
        );
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert(
            "a-model".to_string(),
            VecDeque::from([to_b.clone(), to_b.clone(), to_b.clone(), to_b]),
        );
        routes.insert(
            "b-model".to_string(),
            VecDeque::from([to_a.clone(), to_a.clone(), to_a.clone(), to_a]),
        );
        let finish_default = tool_sse("dx", "finish", serde_json::json!({"result": "extra"}));
        let (base, server) = spawn_scripted(routes, finish_default).await;

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([
                    (a.id.clone(), creds_for(&a.id, &base, "a-model")),
                    (b.id.clone(), creds_for(&b.id, &base, "b-model")),
                ]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        assert_eq!(final_state.stop_reason.as_deref(), Some("loop_detected"));
        assert_eq!(final_state.status, "stopped");
        assert_eq!(final_state.turn_count, 6);
        cleanup(&path);
    }

    #[tokio::test]
    async fn json_protocol_text_finish_flows() {
        let (path, store) = temp_store("json-flow");
        let a = store.add_agent(&new_agent("Ann", "a-model", "json")).unwrap();
        let detail = store
            .create_room("R", &[a.id.clone()], "round_robin", None)
            .unwrap();

        let body = text_sse(r#"{"tool": "finish", "args": {"result": "json done"}}"#);
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("a-model".to_string(), VecDeque::from([body]));
        let (base, server) = spawn_scripted(routes, text_sse("late")).await;

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(a.id.clone(), creds_for(&a.id, &base, "a-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        assert_eq!(final_state.status, "done");
        assert_eq!(final_state.stop_reason.as_deref(), Some("finished_by_agent"));
        let messages = store.list_room_messages(&detail.room.id).unwrap();
        assert!(messages.iter().any(|m| m.content == "json done"));
        cleanup(&path);
    }

    #[tokio::test]
    async fn text_protocol_tool_lines_flow() {
        let (path, store) = temp_store("text-flow");
        let a = store.add_agent(&new_agent("Ann", "a-model", "text")).unwrap();
        let detail = store
            .create_room("R", &[a.id.clone()], "round_robin", None)
            .unwrap();

        let body = text_sse("TOOL: finish {\"result\": \"text done\"}");
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("a-model".to_string(), VecDeque::from([body]));
        let (base, server) = spawn_scripted(routes, text_sse("late")).await;

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(a.id.clone(), creds_for(&a.id, &base, "a-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        assert_eq!(final_state.status, "done");
        let messages = store.list_room_messages(&detail.room.id).unwrap();
        assert!(messages.iter().any(|m| m.content == "text done"));
        cleanup(&path);
    }

    /// Auto protocol against an endpoint that rejects `tools`: the driver
    /// persists `json` for the agent and retries the turn as JSON.
    #[tokio::test]
    async fn auto_falls_back_to_json_on_tools_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut first = true;
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 0];
                let mut tmp = [0u8; 8192];
                let body_text = loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    if n == 0 {
                        break String::new();
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    let head = String::from_utf8_lossy(&buf);
                    if let Some(pos) = head.find("\r\n\r\n") {
                        let headers = &head[..pos];
                        let len = headers
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                            .and_then(|l| l.split(':').nth(1))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= pos + 4 + len {
                            break head[pos + 4..pos + 4 + len].to_string();
                        }
                    }
                };
                if first && body_text.contains("\"tools\"") {
                    first = false;
                    let error = r#"{"error":{"message":"Unknown parameter: 'tools'"}}"#;
                    let response = format!(
                        "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{error}",
                        error.len()
                    );
                    sock.write_all(response.as_bytes()).await.unwrap();
                } else {
                    let sse = text_sse(r#"{"tool": "finish", "args": {"result": "fallback done"}}"#);
                    sock.write_all(&http_response(&sse, "text/event-stream"))
                        .await
                        .unwrap();
                }
            }
        });
        let base = format!("http://{addr}/v1");

        let (path, store) = temp_store("fallback");
        let a = store.add_agent(&new_agent("Ann", "a-model", "auto")).unwrap();
        let detail = store
            .create_room("R", &[a.id.clone()], "round_robin", None)
            .unwrap();

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(a.id.clone(), creds_for(&a.id, &base, "a-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        assert_eq!(final_state.status, "done");
        let updated = store.get_agent(&a.id).unwrap().unwrap();
        assert_eq!(updated.tool_protocol, "json");
        let messages = store.list_room_messages(&detail.room.id).unwrap();
        assert!(messages.iter().any(|m| m.content == "fallback done"));
        cleanup(&path);
    }

    #[tokio::test]
    async fn stop_flag_halts_the_run() {
        let (path, store) = temp_store("stop-flag");
        let a = store.add_agent(&new_agent("Ann", "a-model", "auto")).unwrap();
        let b = store.add_agent(&new_agent("Ben", "b-model", "auto")).unwrap();
        let detail = store
            .create_room("R", &[a.id.clone(), b.id.clone()], "round_robin", None)
            .unwrap();

        // Never finish: without the stop flag this would run to turn_limit.
        let send = tool_sse("c", "send_message", serde_json::json!({"to": "room", "content": "chat"}));
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert(
            "a-model".to_string(),
            VecDeque::from([send.clone(), send.clone(), send.clone()]),
        );
        routes.insert(
            "b-model".to_string(),
            VecDeque::from([send.clone(), send.clone(), send.clone()]),
        );
        let (base, server) = spawn_scripted(routes, send.clone()).await;

        let control = Arc::new(RoomControl::default());
        control.stopped.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([
                    (a.id.clone(), creds_for(&a.id, &base, "a-model")),
                    (b.id.clone(), creds_for(&b.id, &base, "b-model")),
                ]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        assert_eq!(final_state.status, "stopped");
        assert_eq!(final_state.stop_reason.as_deref(), Some("stopped_by_user"));
        assert_eq!(final_state.turn_count, 0);
        cleanup(&path);
    }

    #[test]
    fn workspace_path_jails_escapes() {
        let root = std::path::Path::new("/work");
        assert_eq!(
            workspace_path(root, "docs/a.md").unwrap(),
            std::path::PathBuf::from("/work/docs/a.md")
        );
        assert_eq!(workspace_path(root, ".").unwrap(), std::path::PathBuf::from("/work"));
        assert_eq!(workspace_path(root, "./x").unwrap(), std::path::PathBuf::from("/work/x"));
        assert!(workspace_path(root, "../etc").is_err());
        assert!(workspace_path(root, "a/../../etc").is_err());
        // A leading slash is treated as workspace-relative, never absolute.
        assert_eq!(
            workspace_path(root, "/abs").unwrap(),
            std::path::PathBuf::from("/work/abs")
        );
        assert!(workspace_path(root, "").is_err());
    }

    #[test]
    fn phase4_tool_defs_gate_on_permissions() {
        let mut agent = new_agent_struct();
        assert!(phase4_tool_defs(&agent).is_empty());
        assert!(external_tools_augmentation(&agent).is_empty());
        agent.permissions.shell = true;
        let defs = phase4_tool_defs(&agent);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "run_code");
        assert!(external_tools_augmentation(&agent).contains("run_code"));
        agent.permissions.workspace = true;
        agent.permissions.network = true;
        assert_eq!(phase4_tool_defs(&agent).len(), 5);
    }

    fn new_agent_struct() -> crate::Agent {
        crate::Agent {
            id: "a".to_string(),
            name: "A".to_string(),
            avatar_color: "#000000".to_string(),
            avatar_icon: "A".to_string(),
            role: "".to_string(),
            system_prompt: "hi".to_string(),
            provider_id: None,
            model: "m".to_string(),
            temperature: 0.5,
            max_tokens: 100,
            enabled: true,
            tool_protocol: "auto".to_string(),
            permissions: crate::AgentPermissions::default(),
            tools: Vec::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn run_sandboxed_runs_sh_and_caps_output() {
        let out = run_sandboxed("sh", "echo hello");
        assert!(out.contains("hello"), "unexpected: {out}");
        assert!(out.contains("exit 0"));
    }

    #[test]
    fn run_sandboxed_rejects_timeout_and_missing() {
        let out = run_sandboxed("sh", "sleep 60");
        assert!(out.contains("timed out"), "unexpected: {out}");
        let out = run_sandboxed("cobol", "PRINT 'hi'.");
        assert!(out.contains("supports"), "unexpected: {out}");
    }

    fn shell_agent(store: &KbStore, name: &str, model: &str) -> crate::Agent {
        let agent = store.add_agent(&new_agent(name, model, "auto")).unwrap();
        store
            .set_agent_permissions(
                &agent.id,
                &crate::AgentPermissions { workspace: false, network: false, shell: true },
            )
            .unwrap();
        store.get_agent(&agent.id).unwrap().unwrap()
    }

    #[tokio::test]
    async fn approval_once_runs_code_and_audits() {
        let (path, store) = temp_store("approve-once");
        let agent = shell_agent(&store, "Shell", "s-model");
        let detail = store
            .create_room("R", &[agent.id.clone()], "round_robin", None)
            .unwrap();

        let run = tool_sse("c", "run_code", serde_json::json!({"language": "sh", "code": "echo approved"}));
        let fin = tool_sse("c2", "finish", serde_json::json!({"result": "ran it"}));
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("s-model".to_string(), VecDeque::from([run, fin.clone()]));
        let (base, server) = spawn_scripted(routes, fin.clone()).await;

        let control = Arc::new(RoomControl::default());
        seed_approvals(&control, &detail.room.id, "once");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(agent.id.clone(), creds_for(&agent.id, &base, "s-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        assert!(has_event(&events, "approval"));
        let result = tool_result_text(&events, "run_code").expect("run_code result");
        assert!(result.contains("approved"), "unexpected: {result}");
        assert_eq!(final_state.stop_reason.as_deref(), Some("finished_by_agent"));
        let audit = store.list_audit(Some(&detail.room.id), 100).unwrap();
        let row = audit.iter().find(|r| r.tool == "run_code").expect("audit row");
        assert_eq!(row.decision, "once");
        cleanup(&path);
    }

    #[tokio::test]
    async fn approval_always_persists_grant() {
        let (path, store) = temp_store("approve-always");
        let agent = shell_agent(&store, "Shell", "s-model");
        let detail = store
            .create_room("R", &[agent.id.clone()], "round_robin", None)
            .unwrap();

        let run = tool_sse("c", "run_code", serde_json::json!({"language": "sh", "code": "echo one"}));
        let fin = tool_sse("c2", "finish", serde_json::json!({"result": "done"}));
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("s-model".to_string(), VecDeque::from([run.clone(), fin.clone(), run, fin.clone()]));
        let (base, server) = spawn_scripted(routes, fin.clone()).await;

        // First drive: human picks "always".
        let control = Arc::new(RoomControl::default());
        seed_approvals(&control, &detail.room.id, "always");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(agent.id.clone(), creds_for(&agent.id, &base, "s-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        assert!(store.has_tool_grant(&agent.id, "run_code").unwrap());

        // Second drive: no seeded decision — the grant skips the dialog.
        let control2 = Arc::new(RoomControl::default());
        let mut events2: Vec<RoomEvent> = Vec::new();
        {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(agent.id.clone(), creds_for(&agent.id, &base, "s-model"))]),
                control: Arc::clone(&control2),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events2.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("again".into()), &mut deps).await
        };
        server.abort();

        assert!(!has_event(&events2, "approval"));
        let result = tool_result_text(&events2, "run_code").expect("run_code result");
        assert!(result.contains("one"), "unexpected: {result}");
        cleanup(&path);
    }

    #[tokio::test]
    async fn permission_gates_deny_without_dialog() {
        let (path, store) = temp_store("perm-gates");
        // No permissions at all: every external tool is refused, none ask.
        let agent = store.add_agent(&new_agent("Plain", "p-model", "auto")).unwrap();
        let detail = store
            .create_room("R", &[agent.id.clone()], "round_robin", None)
            .unwrap();

        let run = tool_sse("c", "run_code", serde_json::json!({"language": "sh", "code": "echo x"}));
        let fin = tool_sse("c2", "finish", serde_json::json!({"result": "done"}));
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("p-model".to_string(), VecDeque::from([run, fin.clone()]));
        let (base, server) = spawn_scripted(routes, fin.clone()).await;

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(agent.id.clone(), creds_for(&agent.id, &base, "p-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        assert!(!has_event(&events, "approval"));
        let result = tool_result_text(&events, "run_code").expect("run_code result");
        assert!(result.contains("no shell permission"), "unexpected: {result}");
        let audit = store.list_audit(Some(&detail.room.id), 100).unwrap();
        assert_eq!(audit.iter().find(|r| r.tool == "run_code").unwrap().decision, "deny-permission");
        cleanup(&path);
    }

    #[test]
    fn mcp_wire_names_are_native_safe() {
        assert_eq!(mcp_wire_name("abc-123_XYZ", "my tool.v2"), "mcp__abc123XY__my_tool_v2");
    }

    /// A stdin-aware fake MCP server: answers initialize/list/call by method,
    /// silent on notifications. Lives only for this test.
    fn smart_mcp_server(dir: &std::path::Path) -> String {
        let script = dir.join("smart.sh");
        let body = concat!(
            "#!/bin/sh\n",
            "while IFS= read -r line; do\n",
            "  case \"$line\" in\n",
            "    *'\"method\":\"initialize\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{}}}' ;;\n",
            "    *'\"method\":\"tools/list\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"echo\",\"description\":\"Echo\",\"inputSchema\":{\"type\":\"object\"}}]}}' ;;\n",
            "    *'\"method\":\"tools/call\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"mcp-echo-ok\"}]}}' ;;\n",
            "  esac\n",
            "done\n",
        );
        std::fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        script.to_string_lossy().to_string()
    }

    #[tokio::test]
    async fn mcp_assigned_tool_calls_through_approval() {
        let (path, store) = temp_store("mcp-e2e");
        let workdir = std::env::temp_dir().join(format!("roundtable-mcp-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workdir).unwrap();
        let command = smart_mcp_server(&workdir);
        let server = store
            .add_mcp_server(&crate::db::NewMcpServer {
                name: "Echo".to_string(),
                transport: "stdio".to_string(),
                command: Some(command),
                args: vec![],
                url: None,
            })
            .unwrap();
        let agent = store.add_agent(&new_agent("Mcp", "m-model", "auto")).unwrap();
        store
            .set_agent_tools(&agent.id, &[format!("{}:echo", server.id)])
            .unwrap();
        let detail = store
            .create_room("R", &[agent.id.clone()], "round_robin", None)
            .unwrap();

        let wire = mcp_wire_name(&server.id, "echo");
        let call = tool_sse("c", &wire, serde_json::json!({}));
        let fin = tool_sse("c2", "finish", serde_json::json!({"result": "done"}));
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("m-model".to_string(), VecDeque::from([call, fin.clone()]));
        let (base, server_handle) = spawn_scripted(routes, fin.clone()).await;

        let control = Arc::new(RoomControl::default());
        seed_approvals(&control, &detail.room.id, "once");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        let final_state = {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(agent.id.clone(), creds_for(&agent.id, &base, "m-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server_handle.abort();

        assert!(has_event(&events, "approval"));
        let result = tool_result_text(&events, &wire).expect("mcp result");
        assert!(result.contains("mcp-echo-ok"), "unexpected: {result}");
        assert_eq!(final_state.stop_reason.as_deref(), Some("finished_by_agent"));
        let audit = store.list_audit(Some(&detail.room.id), 100).unwrap();
        let row = audit.iter().find(|r| r.tool == wire).expect("audit row");
        assert_eq!(row.decision, "once");
        let _ = std::fs::remove_dir_all(&workdir);
        cleanup(&path);
    }

    #[tokio::test]
    async fn mcp_unassigned_tool_is_refused() {
        let (path, store) = temp_store("mcp-unassigned");
        let workdir = std::env::temp_dir().join(format!("roundtable-mcp-un-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workdir).unwrap();
        let command = smart_mcp_server(&workdir);
        let server = store
            .add_mcp_server(&crate::db::NewMcpServer {
                name: "Echo".to_string(),
                transport: "stdio".to_string(),
                command: Some(command),
                args: vec![],
                url: None,
            })
            .unwrap();
        // Agent has NO tool assignment: the catalog stays empty.
        let agent = store.add_agent(&new_agent("Mcp", "m-model", "auto")).unwrap();
        let detail = store
            .create_room("R", &[agent.id.clone()], "round_robin", None)
            .unwrap();

        let wire = mcp_wire_name(&server.id, "echo");
        let call = tool_sse("c", &wire, serde_json::json!({}));
        let fin = tool_sse("c2", "finish", serde_json::json!({"result": "done"}));
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("m-model".to_string(), VecDeque::from([call, fin.clone()]));
        let (base, server_handle) = spawn_scripted(routes, fin.clone()).await;

        let control = Arc::new(RoomControl::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(agent.id.clone(), creds_for(&agent.id, &base, "m-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: None,
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server_handle.abort();

        // Unknown wire (empty catalog) and crucially no approval dialog.
        assert!(!has_event(&events, "approval"));
        let result = tool_result_text(&events, &wire).expect("mcp result");
        assert!(result.contains("not available"), "unexpected: {result}");
        let _ = std::fs::remove_dir_all(&workdir);
        cleanup(&path);
    }

    #[tokio::test]
    async fn workspace_write_deny_leaves_no_file() {
        let (path, store) = temp_store("ws-deny");
        let agent = store.add_agent(&new_agent("W", "w-model", "auto")).unwrap();
        store
            .set_agent_permissions(
                &agent.id,
                &crate::AgentPermissions { workspace: true, network: false, shell: false },
            )
            .unwrap();
        let detail = store
            .create_room("R", &[agent.id.clone()], "round_robin", None)
            .unwrap();
        let workdir = std::env::temp_dir().join(format!("roundtable-ws-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workdir).unwrap();

        let write = tool_sse(
            "c",
            "write_file",
            serde_json::json!({"path": "note.md", "content": "secret"}),
        );
        let fin = tool_sse("c2", "finish", serde_json::json!({"result": "done"}));
        let mut routes: HashMap<String, VecDeque<String>> = HashMap::new();
        routes.insert("w-model".to_string(), VecDeque::from([write, fin.clone()]));
        let (base, server) = spawn_scripted(routes, fin.clone()).await;

        let control = Arc::new(RoomControl::default());
        seed_approvals(&control, &detail.room.id, "deny");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut events: Vec<RoomEvent> = Vec::new();
        {
            let mut deps = DriveDeps {
                db_path: path.clone(),
                creds: HashMap::from([(agent.id.clone(), creds_for(&agent.id, &base, "w-model"))]),
                control: Arc::clone(&control),
                cancel: Arc::clone(&cancel),
                emit: Box::new(|ev: RoomEvent| events.push(ev)),
                web_search_key: None,
                workspace_dir: Some(workdir.clone()),
            };
            drive_room(&detail.room.id, Some("go".into()), &mut deps).await
        };
        server.abort();

        let result = tool_result_text(&events, "write_file").expect("write result");
        assert!(result.contains("denied"), "unexpected: {result}");
        assert!(!workdir.join("note.md").exists());
        let _ = std::fs::remove_dir_all(&workdir);
        cleanup(&path);
    }
}
