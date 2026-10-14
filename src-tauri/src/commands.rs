//! Chat + provider + agent + room + data commands exposed to the webview.
//!
//! Commands stay thin: they resolve shell context (DB path, credential-store
//! keys), run model I/O through `roundtable-core`, and persist through the
//! core SQLite store. API keys are fetched from the OS store in Rust for a
//! single call and never returned to JS.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use roundtable_core::{
    ChatMsg, Conversation, Message, NewProvider, OllamaStatus, Provider, OLLAMA_V1_BASE,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

use crate::secrets;

/// The built-in local provider id. Never stored in the providers table; the
/// shell resolves it to the Ollama OpenAI-compatible base on the fly.
pub const OLLAMA_ID: &str = "ollama";

/// Shared shell state: the DB path, one cancel flag per live stream, one
/// control block per room with a live driver, and the remote-mode server.
pub struct AppState {
    pub db_path: PathBuf,
    pub streams: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub room_controls: Arc<Mutex<HashMap<String, Arc<roundtable_core::rooms::RoomControl>>>>,
    pub remote: Mutex<RemoteServerState>,
}

/// The LAN remote-mode server (Phase 4). At most one runs at a time.
#[derive(Debug, Default)]
pub struct RemoteServerState {
    pub running: bool,
    pub bind: String,
    pub port: u16,
    pub shutdown: Option<Arc<AtomicBool>>,
}

fn open_store(state: &AppState) -> Result<roundtable_core::KbStore, String> {
    let store = roundtable_core::KbStore::open(&state.db_path).map_err(|e| e.to_string())?;
    store.migrate_chat().map_err(|e| e.to_string())?;
    Ok(store)
}

#[tauri::command(rename_all = "snake_case")]
pub fn app_info(app: tauri::AppHandle) -> roundtable_core::AppInfo {
    let package = app.package_info();
    roundtable_core::app_info(package.name.clone(), package.version.to_string())
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

/// A provider row plus whether a key is stored for it (never the key).
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub created_at: i64,
    pub has_key: bool,
}

impl ProviderInfo {
    fn of(provider: &Provider, has_key: bool) -> Self {
        Self {
            id: provider.id.clone(),
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            created_at: provider.created_at,
            has_key,
        }
    }
}

/// Resolve a provider id to (base_url, key). The built-in Ollama id maps to
/// the local server with no key.
fn resolve_provider(
    store: &roundtable_core::KbStore,
    provider_id: &str,
) -> Result<(String, Option<String>), String> {
    if provider_id == OLLAMA_ID {
        return Ok((OLLAMA_V1_BASE.to_string(), None));
    }
    let provider = store
        .get_provider(provider_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That provider is gone — pick another one.".to_string())?;
    let key = secrets::get_key(&provider.id)?;
    Ok((provider.base_url, key))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn providers_list(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ProviderInfo>, String> {
    let store = open_store(&state)?;
    let providers = store.list_providers().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(providers.len());
    for provider in &providers {
        out.push(ProviderInfo::of(provider, secrets::has_key(&provider.id)?));
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct ProviderInput {
    pub name: String,
    pub base_url: String,
    /// None = leave the stored key untouched; Some (possibly empty) = set/clear.
    pub key: Option<String>,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn providers_add(
    state: tauri::State<'_, AppState>,
    input: ProviderInput,
) -> Result<ProviderInfo, String> {
    let store = open_store(&state)?;
    let provider = store
        .add_provider(&NewProvider {
            name: input.name,
            base_url: input.base_url,
        })
        .map_err(|e| e.to_string())?;
    if let Some(key) = input.key {
        secrets::set_key(&provider.id, &key)?;
    }
    Ok(ProviderInfo::of(&provider, secrets::has_key(&provider.id)?))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn providers_update(
    state: tauri::State<'_, AppState>,
    id: String,
    input: ProviderInput,
) -> Result<ProviderInfo, String> {
    let store = open_store(&state)?;
    let provider = store
        .update_provider(&id, &input.name, &input.base_url)
        .map_err(|e| e.to_string())?;
    if let Some(key) = input.key {
        secrets::set_key(&provider.id, &key)?;
    }
    Ok(ProviderInfo::of(&provider, secrets::has_key(&provider.id)?))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn providers_remove(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    if id == OLLAMA_ID {
        return Err("The built-in Ollama entry cannot be removed.".to_string());
    }
    let store = open_store(&state)?;
    if !store.delete_provider(&id).map_err(|e| e.to_string())? {
        return Err("That provider is already gone.".to_string());
    }
    // Best effort: the row is gone either way.
    let _ = secrets::set_key(&id, "");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn provider_key_has(id: String) -> Result<bool, String> {
    secrets::has_key(&id)
}

/// Hit `GET {base}/models` with the stored key. Proves base URL + key work.
#[tauri::command(rename_all = "snake_case")]
pub async fn providers_test(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    let store = open_store(&state)?;
    let (base_url, key) = resolve_provider(&store, &id)?;
    let models = roundtable_core::list_models(&base_url, key.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "OK — {base_url} answered with {} model{}.",
        models.len(),
        if models.len() == 1 { "" } else { "s" }
    ))
}

/// Model ids for one provider, for the picker. Ollama falls back to its
/// native tag list when `/v1/models` is unavailable.
#[tauri::command(rename_all = "snake_case")]
pub async fn provider_models(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Vec<String>, String> {
    let store = open_store(&state)?;
    let (base_url, key) = resolve_provider(&store, &id)?;
    match roundtable_core::list_models(&base_url, key.as_deref()).await {
        Ok(models) => Ok(models),
        Err(e) => {
            if id == OLLAMA_ID {
                Ok(roundtable_core::ollama_status().await.models)
            } else {
                Err(e.to_string())
            }
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn ollama_status() -> OllamaStatus {
    roundtable_core::ollama_status().await
}

// ---------------------------------------------------------------------------
// Conversations + messages
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn conversations_list(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Conversation>, String> {
    open_store(&state)?
        .list_conversations()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversations_search(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<Vec<Conversation>, String> {
    open_store(&state)?
        .search_conversations(&query)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversations_create(
    state: tauri::State<'_, AppState>,
    title: String,
    provider_id: Option<String>,
    model: String,
) -> Result<Conversation, String> {
    let title = title.trim();
    let title = if title.is_empty() { "New chat" } else { title };
    open_store(&state)?
        .create_conversation(title, provider_id.as_deref(), &model)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversations_rename(
    state: tauri::State<'_, AppState>,
    id: String,
    title: String,
) -> Result<Conversation, String> {
    open_store(&state)?
        .rename_conversation(&id, &title)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversations_delete(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    if open_store(&state)?
        .delete_conversation(&id)
        .map_err(|e| e.to_string())?
    {
        Ok(())
    } else {
        Err("That conversation is already gone.".to_string())
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversation_get(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Option<Conversation>, String> {
    open_store(&state)?
        .get_conversation(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversations_set_model(
    state: tauri::State<'_, AppState>,
    id: String,
    provider_id: Option<String>,
    model: String,
) -> Result<Conversation, String> {
    if let Some(pid) = provider_id.as_deref() {
        if !pid.trim().is_empty() && pid != OLLAMA_ID {
            let store = open_store(&state)?;
            if store
                .get_provider(pid)
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Err("That provider is gone — pick another one.".to_string());
            }
        }
    }
    open_store(&state)?
        .set_conversation_model(&id, provider_id.as_deref(), &model)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn messages_list(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<Message>, String> {
    open_store(&state)?
        .list_messages(&conversation_id)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Streaming chat
// ---------------------------------------------------------------------------

/// Events pushed to the webview over a Tauri channel during a stream.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Token { delta: String },
    Done {
        content: String,
        message_id: String,
        stopped: bool,
        agent_id: Option<String>,
    },
    Error { message: String },
}

fn register_stream(state: &AppState, stream_id: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut streams) = state.streams.lock() {
        streams.insert(stream_id.to_string(), Arc::clone(&flag));
    }
    flag
}

/// One resolved model call: where to send it, with what knobs, as whom.
struct TurnPlan {
    base_url: String,
    key: Option<String>,
    model: String,
    provider_id: String,
    agent_id: Option<String>,
    options: roundtable_core::ChatOptions,
}

/// Resolve the effective provider/model/system prompt for a turn:
/// an explicit @mention override wins, then the bound agent (when present
/// and enabled), then the conversation's own provider/model.
fn plan_turn(
    store: &roundtable_core::KbStore,
    conv: &Conversation,
    override_agent: Option<&str>,
) -> Result<TurnPlan, String> {
    let override_agent = override_agent.filter(|s| !s.trim().is_empty());
    if let Some(agent_id) = override_agent {
        let agent = store
            .get_agent(agent_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "That agent is gone.".to_string())?;
        if !agent.enabled {
            return Err(format!("{} is disabled — enable it to chat with it.", agent.name));
        }
        return plan_from_agent(store, &agent);
    }
    if let Some(agent_id) = conv.agent_id.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Some(agent) = store.get_agent(agent_id).map_err(|e| e.to_string())? {
            if agent.enabled {
                return plan_from_agent(store, &agent);
            }
        }
        // Missing or disabled bound agent: fall through to the
        // conversation's own provider/model.
    }
    let provider_id = conv.provider_id.as_deref().unwrap_or("").trim();
    if provider_id.is_empty() {
        return Err("Pick a provider and model for this conversation first.".to_string());
    }
    if conv.model.trim().is_empty() {
        return Err("Pick a model for this conversation first.".to_string());
    }
    let (base_url, key) = resolve_provider(store, provider_id)?;
    Ok(TurnPlan {
        base_url,
        key,
        model: conv.model.trim().to_string(),
        provider_id: provider_id.to_string(),
        agent_id: None,
        options: roundtable_core::ChatOptions::default(),
    })
}

fn plan_from_agent(
    store: &roundtable_core::KbStore,
    agent: &roundtable_core::Agent,
) -> Result<TurnPlan, String> {
    let provider_id = agent.provider_id.as_deref().unwrap_or("").trim();
    if provider_id.is_empty() {
        return Err(format!(
            "{} has no provider — edit the agent and pick one.",
            agent.name
        ));
    }
    if agent.model.trim().is_empty() {
        return Err(format!(
            "{} has no model — edit the agent and pick one.",
            agent.name
        ));
    }
    let (base_url, key) = resolve_provider(store, provider_id)?;
    Ok(TurnPlan {
        base_url,
        key,
        model: agent.model.trim().to_string(),
        provider_id: provider_id.to_string(),
        agent_id: Some(agent.id.clone()),
        options: roundtable_core::ChatOptions {
            system_prompt: Some(agent.system_prompt.clone()),
            temperature: Some(agent.temperature),
            max_tokens: if agent.max_tokens > 0 {
                Some(agent.max_tokens)
            } else {
                None
            },
            tools: None,
        },
    })
}

/// The exact wire payload for the run record: system first, then history.
fn wire_messages(options: &roundtable_core::ChatOptions, history: &[ChatMsg]) -> Vec<ChatMsg> {
    let mut wire = Vec::with_capacity(history.len() + 1);
    if let Some(system) = options.system_prompt.as_deref().filter(|s| !s.trim().is_empty()) {
        wire.push(ChatMsg {
            role: "system".to_string(),
            content: system.to_string(),
        });
    }
    wire.extend(history.iter().cloned());
    wire
}

async fn run_stream(
    db_path: PathBuf,
    conversation_id: String,
    override_agent: Option<String>,
    cancel: Arc<AtomicBool>,
    channel: Channel<StreamEvent>,
) {
    let result = run_stream_inner(
        &db_path,
        &conversation_id,
        override_agent.as_deref(),
        &cancel,
        channel.clone(),
    )
    .await;
    if let Err(message) = result {
        if message != "cancelled" {
            let _ = channel.send(StreamEvent::Error { message });
        } else {
            let _ = channel.send(StreamEvent::Done {
                content: String::new(),
                message_id: String::new(),
                stopped: true,
                agent_id: None,
            });
        }
    }
}

async fn run_stream_inner(
    db_path: &PathBuf,
    conversation_id: &str,
    override_agent: Option<&str>,
    cancel: &AtomicBool,
    channel: Channel<StreamEvent>,
) -> Result<(), String> {
    let store =
        roundtable_core::KbStore::open(db_path).map_err(|e| e.to_string())?;
    store.migrate_chat().map_err(|e| e.to_string())?;

    let conv = store
        .get_conversation(conversation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That conversation is gone.".to_string())?;
    let plan = plan_turn(&store, &conv, override_agent)?;
    let history: Vec<ChatMsg> = store
        .list_messages(conversation_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|m| ChatMsg {
            role: m.role,
            content: m.content,
        })
        .collect();
    if history.is_empty() || history.last().map(|m| m.role.as_str()) != Some("user") {
        return Err("Nothing to send — write a message first.".to_string());
    }
    let wire = wire_messages(&plan.options, &history);
    let request_json =
        serde_json::to_string(&wire).unwrap_or_else(|_| "[]".to_string());

    let started = std::time::Instant::now();
    let mut accumulated = String::new();
    let stream_result = roundtable_core::chat_stream(
        &plan.base_url,
        plan.key.as_deref(),
        &plan.model,
        &history,
        &plan.options,
        cancel,
        &mut |delta: String| {
            accumulated.push_str(&delta);
            let _ = channel.send(StreamEvent::Token { delta });
        },
    )
    .await;
    let latency_ms = started.elapsed().as_millis() as i64;

    let (content, usage) = match stream_result {
        Ok(result) => (result.content, result.usage),
        Err(e) => {
            // Record the attempt so the Inspector can show what was sent.
            let _ = store.save_run(&roundtable_core::RunInfo {
                conversation_id: conversation_id.to_string(),
                model: plan.model.clone(),
                provider_id: Some(plan.provider_id.clone()),
                agent_id: plan.agent_id.clone(),
                request_json,
                prompt_tokens: None,
                completion_tokens: None,
                total_tokens: None,
                latency_ms,
                created_at: roundtable_core::kb::now_ms(),
            });
            return Err(e.to_string());
        }
    };

    let stopped = cancel.load(Ordering::Relaxed);
    let message_id = if accumulated.trim().is_empty() && stopped {
        String::new()
    } else if accumulated.trim().is_empty() {
        return Err("The model returned an empty reply.".to_string());
    } else {
        let saved = store
            .add_message(conversation_id, "assistant", &content, plan.agent_id.as_deref())
            .map_err(|e| e.to_string())?;
        store
            .touch_conversation(conversation_id)
            .map_err(|e| e.to_string())?;
        saved.id
    };
    let _ = store.save_run(&roundtable_core::RunInfo {
        conversation_id: conversation_id.to_string(),
        model: plan.model.clone(),
        provider_id: Some(plan.provider_id.clone()),
        agent_id: plan.agent_id.clone(),
        request_json,
        prompt_tokens: usage.prompt_tokens.map(i64::from),
        completion_tokens: usage.completion_tokens.map(i64::from),
        total_tokens: usage.total_tokens.map(i64::from),
        latency_ms,
        created_at: roundtable_core::kb::now_ms(),
    });
    let _ = channel.send(StreamEvent::Done {
        content,
        message_id,
        stopped,
        agent_id: plan.agent_id,
    });
    Ok(())
}

fn auto_title(store: &roundtable_core::KbStore, conv: &Conversation, first_text: &str) {
    if conv.title == "New chat" {
        let short: String = first_text.trim().chars().take(40).collect();
        if !short.is_empty() {
            let _ = store.rename_conversation(&conv.id, &short);
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_send(
    state: tauri::State<'_, AppState>,
    channel: Channel<StreamEvent>,
    conversation_id: String,
    content: String,
    stream_id: String,
    agent_id: Option<String>,
) -> Result<String, String> {
    if content.trim().is_empty() {
        return Err("Write a message first.".to_string());
    }
    let store = open_store(&state)?;
    let conv = store
        .get_conversation(&conversation_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That conversation is gone.".to_string())?;
    store
        .add_message(&conversation_id, "user", content.trim(), None)
        .map_err(|e| e.to_string())?;
    store
        .touch_conversation(&conversation_id)
        .map_err(|e| e.to_string())?;
    auto_title(&store, &conv, content.trim());

    let cancel = register_stream(&state, &stream_id);
    tauri::async_runtime::spawn(run_stream(
        state.db_path.clone(),
        conversation_id,
        agent_id,
        cancel,
        channel,
    ));
    Ok(stream_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_regenerate(
    state: tauri::State<'_, AppState>,
    channel: Channel<StreamEvent>,
    conversation_id: String,
    stream_id: String,
    agent_id: Option<String>,
) -> Result<String, String> {
    let store = open_store(&state)?;
    let history = store
        .list_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    if history.is_empty() {
        return Err("Nothing to regenerate — no messages yet.".to_string());
    }
    store
        .drop_trailing_assistant(&conversation_id)
        .map_err(|e| e.to_string())?;
    let remaining = store
        .list_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    if remaining.last().map(|m| m.role.as_str()) != Some("user") {
        return Err("Nothing to regenerate — the last message is not yours.".to_string());
    }
    store
        .touch_conversation(&conversation_id)
        .map_err(|e| e.to_string())?;

    let cancel = register_stream(&state, &stream_id);
    tauri::async_runtime::spawn(run_stream(
        state.db_path.clone(),
        conversation_id,
        agent_id,
        cancel,
        channel,
    ));
    Ok(stream_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_edit_resend(
    state: tauri::State<'_, AppState>,
    channel: Channel<StreamEvent>,
    conversation_id: String,
    message_id: String,
    content: String,
    stream_id: String,
    agent_id: Option<String>,
) -> Result<String, String> {
    if content.trim().is_empty() {
        return Err("The message cannot be empty.".to_string());
    }
    let store = open_store(&state)?;
    let history = store
        .list_messages(&conversation_id)
        .map_err(|e| e.to_string())?;
    let target = history
        .iter()
        .find(|m| m.id == message_id)
        .ok_or_else(|| "That message is gone.".to_string())?;
    if target.role != "user" {
        return Err("Only your own messages can be edited and resent.".to_string());
    }
    store
        .truncate_after(&conversation_id, &message_id)
        .map_err(|e| e.to_string())?;
    store
        .update_message(&message_id, content.trim())
        .map_err(|e| e.to_string())?;
    store
        .touch_conversation(&conversation_id)
        .map_err(|e| e.to_string())?;

    let cancel = register_stream(&state, &stream_id);
    tauri::async_runtime::spawn(run_stream(
        state.db_path.clone(),
        conversation_id,
        agent_id,
        cancel,
        channel,
    ));
    Ok(stream_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn chat_cancel(state: tauri::State<'_, AppState>, stream_id: String) -> Result<(), String> {
    if let Ok(streams) = state.streams.lock() {
        if let Some(flag) = streams.get(&stream_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_search(
    state: tauri::State<'_, AppState>,
    query: String,
    enabled: Option<bool>,
) -> Result<Vec<roundtable_core::Agent>, String> {
    open_store(&state)?
        .search_agents(&query, enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_get(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Option<roundtable_core::Agent>, String> {
    open_store(&state)?
        .get_agent(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_create(
    state: tauri::State<'_, AppState>,
    draft: roundtable_core::NewAgent,
) -> Result<roundtable_core::Agent, String> {
    open_store(&state)?
        .add_agent(&draft)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_update(
    state: tauri::State<'_, AppState>,
    id: String,
    draft: roundtable_core::NewAgent,
) -> Result<roundtable_core::Agent, String> {
    open_store(&state)?
        .update_agent(&id, &draft)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_delete(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    if open_store(&state)?
        .delete_agent(&id)
        .map_err(|e| e.to_string())?
    {
        Ok(())
    } else {
        Err("That agent is already gone.".to_string())
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_duplicate(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<roundtable_core::Agent, String> {
    open_store(&state)?
        .duplicate_agent(&id)
        .map_err(|e| e.to_string())
}

/// Export one agent as JSON (no keys involved — agents never hold secrets).
#[tauri::command(rename_all = "snake_case")]
pub async fn agents_export(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    let agent = open_store(&state)?
        .get_agent(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That agent is gone.".to_string())?;
    serde_json::to_string_pretty(&agent)
        .map_err(|e| format!("Could not serialize the agent: {e}"))
}

/// Import one agent from JSON. Gets a fresh id; provider links are kept as-is
/// (a missing provider surfaces as an actionable error at chat time).
#[tauri::command(rename_all = "snake_case")]
pub async fn agents_import(
    state: tauri::State<'_, AppState>,
    json: String,
) -> Result<roundtable_core::Agent, String> {
    let draft: roundtable_core::NewAgent = serde_json::from_str(&json).map_err(|_| {
        "That file is not a Roundtable agent — pick the JSON file created by Export.".to_string()
    })?;
    open_store(&state)?
        .add_agent(&draft)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agent_templates() -> Vec<roundtable_core::AgentTemplate> {
    roundtable_core::agent_templates()
}

/// One-off test turn for a builder draft. Nothing is persisted.
#[tauri::command(rename_all = "snake_case")]
pub async fn agent_test(
    state: tauri::State<'_, AppState>,
    channel: Channel<StreamEvent>,
    draft: roundtable_core::NewAgent,
    content: String,
    stream_id: String,
) -> Result<String, String> {
    if content.trim().is_empty() {
        return Err("Write a message first.".to_string());
    }
    let checked = roundtable_core::validate_agent(&draft).map_err(|e| e.to_string())?;
    let provider_id = checked.provider_id.as_deref().unwrap_or("").trim();
    if provider_id.is_empty() {
        return Err("Pick a provider for the agent before testing.".to_string());
    }
    if checked.model.trim().is_empty() {
        return Err("Pick a model for the agent before testing.".to_string());
    }
    let store = open_store(&state)?;
    // A synthetic agent row lets the shared planner resolve everything
    // (including the built-in Ollama id) without touching the database.
    let agent = roundtable_core::Agent {
        id: String::new(),
        name: checked.name.clone(),
        avatar_color: checked.avatar_color.clone(),
        avatar_icon: checked.avatar_icon.clone(),
        role: checked.role.clone(),
        system_prompt: checked.system_prompt.clone(),
        provider_id: checked.provider_id.clone(),
        model: checked.model.clone(),
        temperature: checked.temperature,
        max_tokens: checked.max_tokens,
        enabled: true,
        tool_protocol: checked.tool_protocol.clone(),
        permissions: checked.permissions,
        tools: checked.tools.clone(),
        created_at: 0,
        updated_at: 0,
    };
    let mut plan = plan_from_agent(&store, &agent)?;
    plan.agent_id = None;
    let history = vec![ChatMsg::user(content.trim())];

    let cancel = register_stream(&state, &stream_id);
    tauri::async_runtime::spawn(async move {
        let mut accumulated = String::new();
        let result = roundtable_core::chat_stream(
            &plan.base_url,
            plan.key.as_deref(),
            &plan.model,
            &history,
            &plan.options,
            &cancel,
            &mut |delta: String| {
                accumulated.push_str(&delta);
                let _ = channel.send(StreamEvent::Token { delta });
            },
        )
        .await;
        match result {
            Ok(output) => {
                let _ = channel.send(StreamEvent::Done {
                    content: output.content,
                    message_id: String::new(),
                    stopped: cancel.load(Ordering::Relaxed),
                    agent_id: None,
                });
            }
            Err(e) => {
                let message = e.to_string();
                if message != "cancelled" {
                    let _ = channel.send(StreamEvent::Error { message });
                } else {
                    let _ = channel.send(StreamEvent::Done {
                        content: accumulated,
                        message_id: String::new(),
                        stopped: true,
                        agent_id: None,
                    });
                }
            }
        }
    });
    Ok(stream_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conversations_set_agent(
    state: tauri::State<'_, AppState>,
    id: String,
    agent_id: Option<String>,
) -> Result<Conversation, String> {
    open_store(&state)?
        .set_conversation_agent(&id, agent_id.as_deref())
        .map_err(|e| e.to_string())
}

/// The last model call of a conversation, for the Inspector.
#[tauri::command(rename_all = "snake_case")]
pub async fn run_info(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Option<roundtable_core::RunInfo>, String> {
    open_store(&state)?
        .get_run(&conversation_id)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Rooms (Phase 3: multi-agent rooms)
// ---------------------------------------------------------------------------

use roundtable_core::rooms::{
    drive_room, AgentCreds, DriveDeps, RoomControl, RoomEvent, RoomRunState,
};

fn room_control_of(state: &AppState, room_id: &str) -> Option<Arc<RoomControl>> {
    state
        .room_controls
        .lock()
        .ok()?
        .get(room_id)
        .cloned()
}

/// The persisted run record, or a synthetic idle one for rooms that never
/// ran. Errors when the room itself is gone.
fn room_run_snapshot(
    store: &roundtable_core::KbStore,
    room_id: &str,
) -> Result<RoomRunState, String> {
    let detail = store
        .get_room_detail(room_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That room is gone.".to_string())?;
    match store.get_room_run(room_id).map_err(|e| e.to_string())? {
        Some(run) => Ok(run),
        None => Ok(RoomRunState {
            room_id: detail.room.id,
            status: "idle".to_string(),
            stop_reason: None,
            turn_count: 0,
            tokens_used: 0,
            updated_at: roundtable_core::kb::now_ms(),
        }),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_list(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<roundtable_core::rooms::Room>, String> {
    open_store(&state)?
        .list_rooms()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_create(
    state: tauri::State<'_, AppState>,
    title: String,
    member_ids: Vec<String>,
    mode: String,
    lead_agent_id: Option<String>,
) -> Result<roundtable_core::rooms::RoomDetail, String> {
    open_store(&state)?
        .create_room(&title, &member_ids, &mode, lead_agent_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_get(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Option<roundtable_core::rooms::RoomDetail>, String> {
    open_store(&state)?
        .get_room_detail(&id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_update(
    state: tauri::State<'_, AppState>,
    id: String,
    patch: roundtable_core::rooms::RoomPatch,
) -> Result<roundtable_core::rooms::RoomDetail, String> {
    open_store(&state)?
        .update_room(&id, &patch)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_delete(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    if open_store(&state)?
        .delete_room(&id)
        .map_err(|e| e.to_string())?
    {
        Ok(())
    } else {
        Err("That room is already gone.".to_string())
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_set_members(
    state: tauri::State<'_, AppState>,
    id: String,
    members: Vec<roundtable_core::rooms::RoomMemberInput>,
) -> Result<roundtable_core::rooms::RoomDetail, String> {
    open_store(&state)?
        .set_room_members(&id, &members)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn room_messages_list(
    state: tauri::State<'_, AppState>,
    room_id: String,
) -> Result<Vec<roundtable_core::rooms::RoomMessage>, String> {
    let store = open_store(&state)?;
    store
        .get_room_detail(&room_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That room is gone.".to_string())?;
    store
        .list_room_messages(&room_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_start(
    state: tauri::State<'_, AppState>,
    channel: Channel<RoomEvent>,
    room_id: String,
    content: Option<String>,
    stream_id: String,
) -> Result<String, String> {
    let store = open_store(&state)?;
    // A finished driver leaves its registry entry behind; only a live run
    // blocks a fresh start. The whole pre-spawn section is synchronous, so
    // check-and-register cannot interleave with another start.
    if room_control_of(&state, &room_id).is_some() {
        let live = matches!(
            store.get_room_run(&room_id).map_err(|e| e.to_string())?,
            Some(run) if run.status == "running" || run.status == "paused"
        );
        if live {
            return Err(
                "That room already has a live run — pause, step, or stop it first."
                    .to_string(),
            );
        }
    }
    let detail = store
        .get_room_detail(&room_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That room is gone.".to_string())?;
    if detail.members.is_empty() {
        return Err("Add at least one agent to the room first.".to_string());
    }
    let creds = resolve_room_creds(&store, &detail)?;

    let text = content
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let control = Arc::new(RoomControl::default());
    state
        .room_controls
        .lock()
        .map_err(|_| "Internal state is unavailable.".to_string())?
        .insert(room_id.clone(), Arc::clone(&control));
    let cancel = register_stream(&state, &stream_id);
    let db_path = state.db_path.clone();
    // Desktop extras: the web-search key and workspace folder come from the
    // OS store / settings here, never from the webview.
    let web_search_key = secrets::web_search_key().ok().flatten();
    let workspace_dir = store
        .get_setting("workspace.dir")
        .ok()
        .flatten()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from);
    tauri::async_runtime::spawn(async move {
        let mut deps = DriveDeps {
            db_path,
            creds,
            control,
            cancel,
            emit: Box::new(move |event: RoomEvent| {
                let _ = channel.send(event);
            }),
            web_search_key,
            workspace_dir,
        };
        drive_room(&room_id, text, &mut deps).await;
    });
    Ok(stream_id)
}

/// Resolve every room member to in-memory provider credentials (keys travel
/// in memory only). Shared by local runs and the remote server.
pub(crate) fn resolve_room_creds(
    store: &roundtable_core::KbStore,
    detail: &roundtable_core::rooms::RoomDetail,
) -> Result<HashMap<String, AgentCreds>, String> {
    let mut creds = HashMap::new();
    for member in &detail.members {
        let agent = store
            .get_agent(&member.agent_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                "One of the room members is gone — pick the agents again.".to_string()
            })?;
        if !agent.enabled {
            return Err(format!(
                "{} is disabled — enable it or remove it from the room.",
                agent.name
            ));
        }
        let provider_id = agent.provider_id.as_deref().unwrap_or("").trim();
        if provider_id.is_empty() {
            return Err(format!(
                "{} has no provider — edit the agent and pick one.",
                agent.name
            ));
        }
        if agent.model.trim().is_empty() {
            return Err(format!(
                "{} has no model — edit the agent and pick one.",
                agent.name
            ));
        }
        let (base_url, key) = resolve_provider(store, provider_id)?;
        creds.insert(
            agent.id.clone(),
            AgentCreds {
                agent_id: agent.id.clone(),
                base_url,
                key,
                model: agent.model.trim().to_string(),
                temperature: agent.temperature,
                max_tokens: agent.max_tokens,
            },
        );
    }
    Ok(creds)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_pause(
    state: tauri::State<'_, AppState>,
    room_id: String,
) -> Result<RoomRunState, String> {
    if let Some(control) = room_control_of(&state, &room_id) {
        control.paused.store(true, Ordering::Relaxed);
    }
    room_run_snapshot(&open_store(&state)?, &room_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_resume(
    state: tauri::State<'_, AppState>,
    room_id: String,
) -> Result<RoomRunState, String> {
    if let Some(control) = room_control_of(&state, &room_id) {
        control.step_once.store(false, Ordering::Relaxed);
        control.paused.store(false, Ordering::Relaxed);
    }
    room_run_snapshot(&open_store(&state)?, &room_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_step(
    state: tauri::State<'_, AppState>,
    room_id: String,
) -> Result<RoomRunState, String> {
    match room_control_of(&state, &room_id) {
        Some(control) => {
            // Run exactly one more turn, then pause again.
            control.step_once.store(true, Ordering::Relaxed);
            control.paused.store(false, Ordering::Relaxed);
            room_run_snapshot(&open_store(&state)?, &room_id)
        }
        None => Err("Nothing running — Send a message to start the room.".to_string()),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_stop(
    state: tauri::State<'_, AppState>,
    room_id: String,
) -> Result<RoomRunState, String> {
    let store = open_store(&state)?;
    match room_control_of(&state, &room_id) {
        Some(control) => {
            control.stopped.store(true, Ordering::Relaxed);
            control.paused.store(false, Ordering::Relaxed);
            if let Ok(mut registry) = state.room_controls.lock() {
                registry.remove(&room_id);
            }
        }
        None => {
            // No live driver: heal an orphaned running/paused row so the
            // room is startable again.
            if let Some(run) = store.get_room_run(&room_id).map_err(|e| e.to_string())? {
                if run.status == "running" || run.status == "paused" {
                    let healed = RoomRunState {
                        status: "stopped".to_string(),
                        stop_reason: Some("stopped_by_user".to_string()),
                        updated_at: roundtable_core::kb::now_ms(),
                        ..run
                    };
                    store.save_room_run(&healed).map_err(|e| e.to_string())?;
                    return Ok(healed);
                }
            }
        }
    }
    room_run_snapshot(&store, &room_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_run_state(
    state: tauri::State<'_, AppState>,
    room_id: String,
) -> Result<RoomRunState, String> {
    room_run_snapshot(&open_store(&state)?, &room_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_set_protocol(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    protocol: String,
) -> Result<roundtable_core::Agent, String> {
    open_store(&state)?
        .set_agent_protocol(&agent_id, &protocol)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Phase 4: external tools, permissions, audit, remote
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn mcp_servers(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<roundtable_core::McpServer>, String> {
    open_store(&state)?
        .list_mcp_servers()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mcp_add_server(
    state: tauri::State<'_, AppState>,
    input: roundtable_core::NewMcpServer,
) -> Result<roundtable_core::McpServer, String> {
    open_store(&state)?
        .add_mcp_server(&input)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mcp_remove_server(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    if open_store(&state)?
        .delete_mcp_server(&id)
        .map_err(|e| e.to_string())?
    {
        Ok(())
    } else {
        Err("That MCP server is already gone.".to_string())
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mcp_set_enabled(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<roundtable_core::McpServer, String> {
    open_store(&state)?
        .set_mcp_server_enabled(&id, enabled)
        .map_err(|e| e.to_string())
}

/// Live tool listing for one server (spawns the server briefly).
#[tauri::command(rename_all = "snake_case")]
pub async fn mcp_tools(
    state: tauri::State<'_, AppState>,
    server_id: String,
) -> Result<Vec<roundtable_core::McpToolInfo>, String> {
    let server = open_store(&state)?
        .get_mcp_server(&server_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That MCP server is gone.".to_string())?;
    roundtable_core::mcp_list_tools(&server)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_tools(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> Result<Vec<String>, String> {
    Ok(open_store(&state)?
        .get_agent(&agent_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That agent is gone.".to_string())?
        .tools)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_set_tools(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    tools: Vec<String>,
) -> Result<Vec<String>, String> {
    // Only assign tools from servers we know; typos fail loudly here
    // instead of silently never matching.
    let store = open_store(&state)?;
    store
        .get_agent(&agent_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That agent is gone.".to_string())?;
    let servers = store.list_mcp_servers().map_err(|e| e.to_string())?;
    for tool in &tools {
        match tool.split_once(':') {
            Some((server_id, name)) if !name.trim().is_empty() => {
                if !servers.iter().any(|s| s.id == server_id) {
                    return Err(format!(
                        "Unknown MCP server in tool id \"{tool}\" — remove it or add the server first."
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "Bad tool id \"{tool}\" — use the \"server-id:tool-name\" shape from the tools list."
                ))
            }
        }
    }
    store
        .set_agent_tools(&agent_id, &tools)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_permissions(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> Result<roundtable_core::AgentPermissions, String> {
    Ok(open_store(&state)?
        .get_agent(&agent_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That agent is gone.".to_string())?
        .permissions)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn agents_set_permissions(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    perms: roundtable_core::AgentPermissions,
) -> Result<roundtable_core::AgentPermissions, String> {
    open_store(&state)?
        .set_agent_permissions(&agent_id, &perms)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn workspace_dir(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(open_store(&state)?
        .get_setting("workspace.dir")
        .map_err(|e| e.to_string())?
        .filter(|d| !d.trim().is_empty()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn workspace_pick_dir(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};
    let picked = app
        .dialog()
        .file()
        .set_title("Pick the workspace folder agents may use")
        .blocking_pick_folder();
    match picked {
        None => Ok(None),
        Some(FilePath::Path(path)) => {
            if !path.is_dir() {
                return Err("That is not a folder — pick a folder.".to_string());
            }
            let dir = path.to_string_lossy().into_owned();
            open_store(&state)?
                .set_setting("workspace.dir", &dir)
                .map_err(|e| e.to_string())?;
            Ok(Some(dir))
        }
        Some(other) => Err(format!("Could not use that location ({other:?}).")),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn workspace_clear_dir(state: tauri::State<'_, AppState>) -> Result<(), String> {
    open_store(&state)?
        .set_setting("workspace.dir", "")
        .map_err(|e| e.to_string())
}

/// Web-search (Tavily-compatible) key. Empty clears it. Stored in the OS
/// credential store, never returned to the webview.
#[tauri::command(rename_all = "snake_case")]
pub async fn tools_set_search_key(key: String) -> Result<(), String> {
    secrets::set_web_search_key(key.trim())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tools_search_key_has() -> Result<bool, String> {
    secrets::has_web_search_key()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn audit_list(
    state: tauri::State<'_, AppState>,
    room_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<roundtable_core::AuditRow>, String> {
    open_store(&state)?
        .list_audit(
            room_id.as_deref().filter(|s| !s.trim().is_empty()),
            limit.unwrap_or(100),
        )
        .map_err(|e| e.to_string())
}

/// Answer a pending tool approval. The decision is broadcast to every live
/// driver; call ids are room-scoped so only the asking run can consume it.
#[tauri::command(rename_all = "snake_case")]
pub async fn rooms_approve(
    state: tauri::State<'_, AppState>,
    call_id: String,
    decision: String,
) -> Result<(), String> {
    match decision.as_str() {
        "once" | "always" | "deny" => {}
        _ => return Err("Decision must be once, always, or deny.".to_string()),
    }
    let registry = state
        .room_controls
        .lock()
        .map_err(|_| "Internal state is unavailable.".to_string())?;
    if registry.is_empty() {
        return Err("That approval is no longer pending — the run may have ended.".to_string());
    }
    for control in registry.values() {
        if let Ok(mut approvals) = control.approvals.lock() {
            approvals.insert(call_id.clone(), decision.clone());
        }
    }
    Ok(())
}

// -- remote mode ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct RemoteStatus {
    pub running: bool,
    pub bind: String,
    pub port: u16,
    pub tls: bool,
    pub token_count: usize,
}

fn remote_status_of(
    state: &AppState,
    store: &roundtable_core::KbStore,
) -> Result<RemoteStatus, String> {
    let remote = state
        .remote
        .lock()
        .map_err(|_| "Internal state is unavailable.".to_string())?;
    let token_count = store
        .list_remote_tokens()
        .map_err(|e| e.to_string())?
        .len();
    Ok(RemoteStatus {
        running: remote.running,
        bind: remote.bind.clone(),
        port: remote.port,
        tls: false, // plaintext HTTP; encryption comes from loopback or the tunnel
        token_count,
    })
}

fn stop_remote_locked(state: &AppState) {
    if let Ok(mut remote) = state.remote.lock() {
        if let Some(flag) = remote.shutdown.take() {
            flag.store(true, Ordering::Relaxed);
        }
        remote.running = false;
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_status(state: tauri::State<'_, AppState>) -> Result<RemoteStatus, String> {
    let store = open_store(&state)?;
    remote_status_of(&state, &store)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_start(
    state: tauri::State<'_, AppState>,
    bind: String,
    port: u16,
) -> Result<RemoteStatus, String> {
    stop_remote_locked(&state);
    let bind = if bind.trim().is_empty() { "127.0.0.1".to_string() } else { bind.trim().to_string() };
    if port == 0 {
        return Err("Pick a port between 1 and 65535.".to_string());
    }
    crate::remote::bind_allowed(&bind)?;
    let listener = std::net::TcpListener::bind((bind.as_str(), port))
        .map_err(|e| format!("Could not listen on {bind}:{port}: {e}"))?;
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let shutdown = Arc::new(AtomicBool::new(false));
    let ctx = crate::remote::ServerCtx {
        db_path: state.db_path.clone(),
        controls: Arc::clone(&state.room_controls),
    };
    let flag = Arc::clone(&shutdown);
    std::thread::spawn(move || crate::remote::serve(listener, ctx, flag));
    {
        let mut remote = state
            .remote
            .lock()
            .map_err(|_| "Internal state is unavailable.".to_string())?;
        remote.running = true;
        remote.bind = bind;
        remote.port = actual_port;
        remote.shutdown = Some(shutdown);
    }
    let store = open_store(&state)?;
    remote_status_of(&state, &store)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_stop(state: tauri::State<'_, AppState>) -> Result<RemoteStatus, String> {
    stop_remote_locked(&state);
    let store = open_store(&state)?;
    remote_status_of(&state, &store)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_pairing_code(
    state: tauri::State<'_, AppState>,
) -> Result<HashMap<String, serde_json::Value>, String> {
    {
        let remote = state
            .remote
            .lock()
            .map_err(|_| "Internal state is unavailable.".to_string())?;
        if !remote.running {
            return Err("Start the server first — codes are useless without it.".to_string());
        }
    }
    let (code, expires_at) = crate::remote::new_pairing_code(crate::remote::init_pairing_slot());
    Ok(HashMap::from([
        ("code".to_string(), serde_json::Value::String(code)),
        ("expires_at".to_string(), serde_json::Value::Number(expires_at.into())),
    ]))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_tokens(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<roundtable_core::RemoteTokenInfo>, String> {
    open_store(&state)?
        .list_remote_tokens()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_revoke(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    if open_store(&state)?
        .revoke_remote_token(&id)
        .map_err(|e| e.to_string())?
    {
        Ok(())
    } else {
        Err("That device is already unpaired.".to_string())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteClientStatus {
    pub configured: bool,
    pub url: Option<String>,
    pub has_token: bool,
}

fn remote_url_of(store: &roundtable_core::KbStore) -> Result<Option<String>, String> {
    Ok(store
        .get_setting("remote.url")
        .map_err(|e| e.to_string())?
        .filter(|u| !u.trim().is_empty()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_client_status(
    state: tauri::State<'_, AppState>,
) -> Result<RemoteClientStatus, String> {
    let store = open_store(&state)?;
    let url = remote_url_of(&store)?;
    let has_token = secrets::has_remote_token()?;
    Ok(RemoteClientStatus {
        configured: url.is_some() && has_token,
        url,
        has_token,
    })
}

/// Pair with a desktop server: exchange a single-use code for a long-lived
/// token, stored in the OS credential store. Nothing sensitive is logged.
#[tauri::command(rename_all = "snake_case")]
pub async fn remote_connect(
    state: tauri::State<'_, AppState>,
    url: String,
    code: String,
    label: String,
) -> Result<(), String> {
    let url = url.trim().trim_end_matches('/').to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("The server URL must start with http:// or https://.".to_string());
    }
    if code.trim().is_empty() {
        return Err("Enter the pairing code shown on the desktop.".to_string());
    }
    if label.trim().is_empty() {
        return Err("Name this device first.".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Web calls are unavailable: {e}"))?;
    // Probe first so "no server there" reads clearly.
    client
        .get(format!("{url}/"))
        .send()
        .await
        .map_err(|_| format!("No Roundtable server answers at {url} — check the address."))?;
    let response = client
        .post(format!("{url}/pair"))
        .json(&serde_json::json!({ "code": code.trim(), "label": label.trim() }))
        .send()
        .await
        .map_err(|_| format!("No Roundtable server answers at {url} — check the address."))?;
    if !response.status().is_success() {
        let message: String = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| "Pairing was refused.".to_string());
        return Err(message);
    }
    let token: String = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "The server answered with junk.".to_string())?
        .get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| "The server answered with junk.".to_string())?;
    secrets::set_remote_token(&token)?;
    open_store(&state)?
        .set_setting("remote.url", &url)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_disconnect(state: tauri::State<'_, AppState>) -> Result<(), String> {
    secrets::set_remote_token("")?;
    open_store(&state)?
        .set_setting("remote.url", "")
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Settings + data
// ---------------------------------------------------------------------------

const SETTING_KEYS: &[&str] = &[
    "default.provider",
    "default.model",
    "embeddings.mode",
    "embeddings.base_url",
    "embeddings.model",
    "workspace.dir",
    "remote.url",
];

#[tauri::command(rename_all = "snake_case")]
pub async fn settings_get(
    state: tauri::State<'_, AppState>,
) -> Result<HashMap<String, String>, String> {
    let store = open_store(&state)?;
    let mut map = HashMap::new();
    for key in SETTING_KEYS {
        if let Some(value) = store.get_setting(key).map_err(|e| e.to_string())? {
            map.insert((*key).to_string(), value);
        }
    }
    Ok(map)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn settings_set(
    state: tauri::State<'_, AppState>,
    pairs: HashMap<String, String>,
) -> Result<(), String> {
    for key in pairs.keys() {
        if !SETTING_KEYS.contains(&key.as_str()) {
            return Err(format!("Unknown setting: {key}"));
        }
    }
    let store = open_store(&state)?;
    for (key, value) in &pairs {
        store.set_setting(key, value).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Full JSON snapshot (provider keys excluded — they stay in the OS store).
#[tauri::command(rename_all = "snake_case")]
pub async fn data_export(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let bundle = open_store(&state)?
        .export_data()
        .map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&bundle)
        .map_err(|e| format!("Could not serialize the export: {e}"))
}

/// Replace all local data with a snapshot. Returns a one-line summary.
#[tauri::command(rename_all = "snake_case")]
pub async fn data_import(state: tauri::State<'_, AppState>, json: String) -> Result<String, String> {
    let bundle: roundtable_core::ExportBundle =
        serde_json::from_str(&json).map_err(|_| {
            "That file is not a Roundtable export — pick the JSON file created by Export.".to_string()
        })?;
    let (convs, msgs) = open_store(&state)?
        .import_data(&bundle)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Imported {convs} conversation{} with {msgs} message{}. Provider keys are not part of exports — re-enter them under Providers.",
        if convs == 1 { "" } else { "s" },
        if msgs == 1 { "" } else { "s" },
    ))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn data_dir(state: tauri::State<'_, AppState>) -> Result<String, String> {
    Ok(state.db_path.to_string_lossy().into_owned())
}

/// Reveal the data folder in the OS file manager.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_data_dir(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let dir = state
        .db_path
        .parent()
        .ok_or_else(|| "Could not locate the data folder.".to_string())?;
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(dir)
            .spawn()
            .map_err(|e| format!("Could not open the data folder: {e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| format!("Could not open the data folder: {e}"))?;
    }
    #[cfg(target_os = "android")]
    {
        let _ = dir;
        return Err("Opening the data folder is not supported on Android.".to_string());
    }
    Ok(())
}
