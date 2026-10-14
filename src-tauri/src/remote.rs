//! Remote mode: serve the room/agent runtime over the LAN (or Tailscale)
//! to the Android app. Token auth on every route except pairing; pairing
//! codes are single-use with expiry + attempt limits.
//!
//! Plaintext HTTP only: loopback and Tailscale binds are accepted (Tailscale
//! is end-to-end encrypted); anything else is refused at `remote_start`
//! with guidance. Tokens are never logged.
//!
//! Deliberately dependency-free (std only): a small router, minimal
//! HTTP/1.1 responses, and SSE for room runs.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use roundtable_core::rooms::{AgentCreds, DriveDeps, RoomControl, RoomEvent};

/// One outstanding pairing code.
#[derive(Debug, Default)]
pub struct PairingSlot {
    pub code: Option<String>,
    pub expires_at: Option<Instant>,
    pub attempts: u32,
}

pub const PAIRING_TTL: Duration = Duration::from_secs(600);
const MAX_ATTEMPTS: u32 = 5;

/// Mint a fresh single-use code, invalidating any previous one.
/// Returns the code plus its expiry as epoch millis.
pub fn new_pairing_code(slot: &Mutex<PairingSlot>) -> (String, i64) {
    let code = roundtable_core::pairing_code();
    let expires_at = Instant::now() + PAIRING_TTL;
    if let Ok(mut slot) = slot.lock() {
        slot.code = Some(code.clone());
        slot.expires_at = Some(expires_at);
        slot.attempts = 0;
    }
    let epoch_ms =
        std::time::SystemTime::now() + PAIRING_TTL;
    let epoch_ms = epoch_ms
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    (code, epoch_ms)
}

/// Only loopback and Tailscale (encrypted tunnel) binds. Everything else is
/// a plaintext-LAN exposure and is refused with guidance.
pub fn bind_allowed(bind: &str) -> Result<(), String> {
    let bind = bind.trim().trim_matches(|c| c == '[' || c == ']').to_lowercase();
    if bind == "127.0.0.1" || bind == "::1" || bind == "localhost" {
        return Ok(());
    }
    if bind.ends_with(".ts.net") {
        return Ok(());
    }
    if let Ok(ip) = bind.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        if o[0] == 100 && (o[1] & 0b1100_0000) == 64 {
            return Ok(()); // 100.64.0.0/10 (CGNAT/Tailscale range)
        }
    }
    Err("Refusing plaintext LAN exposure. Bind 127.0.0.1 (same machine), or serve through an encrypted tunnel — e.g. bind your Tailscale IP (100.x or *.ts.net) — or put a TLS-terminating proxy in front and bind 127.0.0.1 behind it.".to_string())
}

/// Shared context for the server thread.
#[derive(Clone)]
pub struct ServerCtx {
    pub db_path: PathBuf,
    pub controls: Arc<Mutex<HashMap<String, Arc<RoomControl>>>>,
}

pub fn serve(listener: TcpListener, ctx: ServerCtx, shutdown: Arc<AtomicBool>) {
    let _ = listener.set_nonblocking(true);
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let ctx = ctx.clone();
                std::thread::spawn(move || handle(stream, &ctx));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> Result<Request, String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut head = String::new();
    reader
        .read_line(&mut head)
        .map_err(|e| format!("Unreadable request: {e}"))?;
    let mut parts = head.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let raw_path = parts.next().unwrap_or("/").to_string();
    if method.is_empty() {
        return Err("Empty request.".to_string());
    }
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("Unreadable headers: {e}"))?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_lowercase(), value.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if len > 1_000_000 {
        return Err("Body too large.".to_string());
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|e| format!("Unreadable body: {e}"))?;
    }
    let path = raw_path.split('?').next().unwrap_or("/").to_string();
    Ok(Request { method, path, headers, body })
}

fn json_body(request: &Request) -> serde_json::Value {
    serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null)
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Error",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn respond_json(stream: &mut TcpStream, status: u16, value: &serde_json::Value) {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{\"error\":\"encode\"}".to_string());
    respond(stream, status, "application/json", body.as_bytes());
}

fn respond_err(stream: &mut TcpStream, status: u16, message: &str) {
    respond_json(stream, status, &serde_json::json!({ "error": message }));
}

fn bearer_token(request: &Request) -> Option<String> {
    request
        .headers
        .get("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn open_store(db_path: &PathBuf) -> Result<roundtable_core::KbStore, String> {
    let store =
        roundtable_core::KbStore::open(db_path).map_err(|e| e.to_string())?;
    store.migrate_chat().map_err(|e| e.to_string())?;
    Ok(store)
}

/// Token check. Returns the device label. Never logs the token.
fn require_token(
    store: &roundtable_core::KbStore,
    request: &Request,
) -> Result<String, (u16, &'static str)> {
    match bearer_token(request) {
        Some(token) => match store.verify_remote_token(&token) {
            Ok(Some(label)) => Ok(label),
            _ => Err((401, "Bad or revoked token — pair again.")),
        },
        None => Err((401, "This server needs a pairing token.")),
    }
}

fn handle(mut stream: TcpStream, ctx: &ServerCtx) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let request = match read_request(&stream) {
        Ok(request) => request,
        Err(_) => {
            respond_err(&mut stream, 400, "Could not read that request.");
            return;
        }
    };
    let store = match open_store(&ctx.db_path) {
        Ok(store) => store,
        Err(e) => {
            respond_err(&mut stream, 500, &e);
            return;
        }
    };

    // Connectivity probe (no auth; reveals nothing sensitive).
    if request.path == "/" {
        respond_json(
            &mut stream,
            200,
            &serde_json::json!({ "name": "roundtable-remote", "version": "0.1.0" }),
        );
        return;
    }

    // Pairing (no auth; single-use code + attempt cap).
    if request.path == "/pair" && request.method == "POST" {
        pair(&mut stream, &store, &json_body(&request));
        return;
    }

    let _device = match require_token(&store, &request) {
        Ok(label) => label,
        Err((status, message)) => {
            respond_err(&mut stream, status, message);
            return;
        }
    };

    let segments: Vec<&str> = request.path.trim_matches('/').split('/').collect();
    match (request.method.as_str(), segments.as_slice()) {
        ("GET", ["agents"]) => match store.search_agents("", None) {
            Ok(agents) => respond_json(&mut stream, 200, &serde_json::to_value(&agents).unwrap_or_default()),
            Err(e) => respond_err(&mut stream, 500, &e.to_string()),
        },
        ("GET", ["rooms"]) => match store.list_rooms() {
            Ok(rooms) => respond_json(&mut stream, 200, &serde_json::to_value(&rooms).unwrap_or_default()),
            Err(e) => respond_err(&mut stream, 500, &e.to_string()),
        },
        ("GET", ["rooms", id, "messages"]) => match store.get_room_detail(id) {
            Ok(None) => respond_err(&mut stream, 404, "That room is gone."),
            Ok(Some(_)) => match store.list_room_messages(id) {
                Ok(messages) => respond_json(&mut stream, 200, &serde_json::to_value(&messages).unwrap_or_default()),
                Err(e) => respond_err(&mut stream, 500, &e.to_string()),
            },
            Err(e) => respond_err(&mut stream, 500, &e.to_string()),
        },
        ("GET", ["rooms", id, "state"]) => match room_state_snapshot(&store, id) {
            Ok(state) => respond_json(&mut stream, 200, &serde_json::to_value(&state).unwrap_or_default()),
            Err(e) => respond_err(&mut stream, 404, &e),
        },
        ("POST", ["rooms", id, "start"]) => {
            let content = json_body(&request)
                .get("content")
                .and_then(|c| c.as_str())
                .map(str::to_string);
            start_run(&mut stream, ctx, &store, id, content);
        }
        ("POST", ["rooms", id, "stop"]) => {
            let stopped = ctx
                .controls
                .lock()
                .ok()
                .and_then(|controls| controls.get(*id).cloned())
                .map(|control| {
                    control.stopped.store(true, Ordering::Relaxed);
                    control.paused.store(false, Ordering::Relaxed);
                    true
                })
                .unwrap_or(false);
            respond_json(&mut stream, 200, &serde_json::json!({ "stopped": stopped }));
        }
        _ => respond_err(&mut stream, 404, "Unknown endpoint."),
    }
}

fn room_state_snapshot(
    store: &roundtable_core::KbStore,
    room_id: &str,
) -> Result<roundtable_core::rooms::RoomRunState, String> {
    let detail = store
        .get_room_detail(room_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That room is gone.".to_string())?;
    match store.get_room_run(room_id).map_err(|e| e.to_string())? {
        Some(run) => Ok(run),
        None => Ok(roundtable_core::rooms::RoomRunState {
            room_id: detail.room.id,
            status: "idle".to_string(),
            stop_reason: None,
            turn_count: 0,
            tokens_used: 0,
            updated_at: roundtable_core::kb::now_ms(),
        }),
    }
}

fn pair(
    stream: &mut TcpStream,
    store: &roundtable_core::KbStore,
    body: &serde_json::Value,
) {
    // The pairing slot lives in the Tauri-managed state; the server thread
    // reaches it through a process-wide slot set at startup.
    let decision = pairing_slot().lock().map(|mut slot| {
        if slot.attempts >= MAX_ATTEMPTS {
            slot.code = None;
            return Err((400, "Too many wrong codes — make a new one on the desktop."));
        }
        let code_ok = match (&slot.code, &slot.expires_at) {
            (Some(expected), Some(expires)) if Instant::now() <= *expires => {
                let given = body.get("code").and_then(|c| c.as_str()).unwrap_or("");
                constant_eq(expected, given)
            }
            _ => false,
        };
        if !code_ok {
            slot.attempts += 1;
            if slot.attempts >= MAX_ATTEMPTS {
                slot.code = None;
            }
            return Err((400, "Wrong or expired code — make a new one on the desktop."));
        }
        let label = body.get("label").and_then(|l| l.as_str()).unwrap_or("").trim();
        if label.is_empty() {
            return Err((400, "Name this device first."));
        }
        slot.code = None; // single use
        Ok(label.to_string())
    });
    let label = match decision {
        Ok(Ok(label)) => label,
        Ok(Err((status, message))) => {
            respond_err(stream, status, message);
            return;
        }
        Err(_) => {
            respond_err(stream, 500, "Pairing is unavailable.");
            return;
        }
    };
    match store.create_remote_token(&label) {
        Ok((info, token)) => respond_json(
            stream,
            200,
            &serde_json::json!({ "token": token, "id": info.id }),
        ),
        Err(e) => respond_err(stream, 500, &e.to_string()),
    }
}

fn constant_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// -- process-wide pairing slot (set once by the shell at startup) --------------

static PAIRING: std::sync::OnceLock<Mutex<PairingSlot>> = std::sync::OnceLock::new();

/// Called once from the shell's setup; the server thread uses it for `/pair`.
pub fn init_pairing_slot() -> &'static Mutex<PairingSlot> {
    PAIRING.get_or_init(|| Mutex::new(PairingSlot::default()))
}

fn pairing_slot() -> &'static Mutex<PairingSlot> {
    init_pairing_slot()
}

// -- room runs over SSE ----------------------------------------------------------

/// Resolve every member agent to in-memory creds (keys never touch the log).
fn resolve_room_creds(
    store: &roundtable_core::KbStore,
    detail: &roundtable_core::rooms::RoomDetail,
) -> Result<HashMap<String, AgentCreds>, String> {
    crate::commands::resolve_room_creds(store, detail)
}

fn start_run(
    stream: &mut TcpStream,
    ctx: &ServerCtx,
    store: &roundtable_core::KbStore,
    room_id: &str,
    content: Option<String>,
) {
    let detail = match store.get_room_detail(room_id).map_err(|e| e.to_string()) {
        Ok(Some(detail)) => detail,
        _ => {
            respond_err(stream, 404, "That room is gone.");
            return;
        }
    };
    if detail.members.is_empty() {
        respond_err(stream, 400, "Add at least one agent to the room first.");
        return;
    }
    {
        let live = ctx
            .controls
            .lock()
            .map(|controls| controls.contains_key(room_id))
            .unwrap_or(false);
        if live {
            let busy = matches!(
                store.get_room_run(room_id),
                Ok(Some(run)) if run.status == "running" || run.status == "paused"
            );
            if busy {
                respond_err(stream, 409, "That room already has a live run.");
                return;
            }
        }
    }
    let creds = match resolve_room_creds(store, &detail) {
        Ok(creds) => creds,
        Err(message) => {
            respond_err(stream, 400, &message);
            return;
        }
    };
    // Extras come from desktop settings (remote clients never send keys).
    let web_search_key = store
        .get_setting("tools.tavily_key")
        .ok()
        .flatten()
        .filter(|k| !k.trim().is_empty());
    let workspace_dir = store
        .get_setting("workspace.dir")
        .ok()
        .flatten()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from);

    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n";
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let _ = stream.flush();

    let control = Arc::new(RoomControl::default());
    if let Ok(mut controls) = ctx.controls.lock() {
        controls.insert(room_id.to_string(), Arc::clone(&control));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let db_path = ctx.db_path.clone();
    let room_id = room_id.to_string();
    let text = content
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);

    // The driver runs on the Tauri runtime; this thread blocks until it is
    // done so the SSE stream spans the whole run.
    let writer = match stream.try_clone() {
        Ok(writer) => Arc::new(Mutex::new(writer)),
        Err(_) => return,
    };
    let writer_task = Arc::clone(&writer);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    tauri::async_runtime::spawn(async move {
        let mut deps = DriveDeps {
            db_path,
            creds,
            control,
            cancel,
            emit: Box::new(move |event: RoomEvent| {
                let line = serde_json::to_string(&event).unwrap_or_default();
                if let Ok(mut writer) = writer_task.lock() {
                    let _ = writer.write_all(format!("data: {line}\n\n").as_bytes());
                    let _ = writer.flush();
                }
            }),
            web_search_key,
            workspace_dir,
        };
        let _final = roundtable_core::rooms::drive_room(&room_id, text, &mut deps).await;
        let _ = done_tx.send(());
    });
    // A dropped client ends the stream; the run keeps going (stop it via
    // POST /rooms/:id/stop).
    let _ = done_rx.recv_timeout(Duration::from_secs(3600));
}
