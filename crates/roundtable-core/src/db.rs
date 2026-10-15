//! Chat + agent + room + tool persistence: providers, conversations,
//! messages, agents, runs, rooms, MCP servers, audit log, remote tokens,
//! settings.
//!
//! Schema lives behind `PRAGMA user_version`: version 1 holds the knowledge
//! tables (see [`crate::kb`]), version 2 adds the chat tables, version 3
//! adds agents, message/conversation agent links, and per-conversation run
//! records, version 4 adds multi-agent rooms (rooms, members, room log,
//! room runs) plus the agent tool-protocol column, version 5 adds external
//! tools (MCP servers, agent permissions/tool assignments), the tool audit
//! log, and remote-mode tokens. No networking here; the shell passes keys
//! straight to [`crate::providers`].

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::agents::{validate_agent, validate_protocol, Agent, AgentPermissions, NewAgent};
use crate::kb::{now_ms, KbStore};
use crate::rooms::{
    Room, RoomDetail, RoomMember, RoomMemberInput, RoomMessage, RoomPatch, RoomRunState,
    RoomToolCall,
};
use crate::RagError;

fn db_err(e: rusqlite::Error) -> RagError {
    RagError::Db(e.to_string())
}

/// An OpenAI-compatible endpoint. The key lives in the OS credential store,
/// never in this row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProvider {
    pub name: String,
    pub base_url: String,
}

/// One chat thread. `model` is free-typed so manual ids survive.
/// `agent_id` binds the thread to a single agent (Phase 2); `None` means the
/// conversation's own provider/model are used directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub provider_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    /// Which agent wrote this turn (`None` = plain conversation model).
    #[serde(default)]
    pub agent_id: Option<String>,
    pub created_at: i64,
}

/// The last completed (or attempted) model call per conversation, for the
/// Inspector: exact request payload, usage, latency, and model used.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub conversation_id: String,
    pub model: String,
    pub provider_id: Option<String>,
    pub agent_id: Option<String>,
    pub request_json: String,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub latency_ms: i64,
    pub created_at: i64,
}

/// An external tool provider (Phase 4, desktop only, off by default).
/// `stdio` servers are spawned locally; `http` servers are called over HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMcpServer {
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
}

/// One recorded tool execution: what ran, under which policy, and how it
/// ended. Arguments are stored; results are truncated to a short summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRow {
    pub id: i64,
    pub created_at: i64,
    pub room_id: Option<String>,
    pub agent_id: Option<String>,
    pub tool: String,
    pub args_json: String,
    pub decision: String,
    pub result_summary: String,
}

/// A paired remote device. The plaintext token is never stored — only its
/// hash (see [`hash_token`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTokenInfo {
    pub id: String,
    pub label: String,
    pub created_at: i64,
}

/// SHA-256 hex of a remote token with a domain separator. std-only
/// implementation (no new dependencies for one hash).
fn hash_token(token: &str) -> String {
    sha256_hex(format!("roundtable-remote-v1:{token}").as_bytes())
}

// -- SHA-256 (compact, std-only; used only for remote-token hashes) ----------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

fn sha256_hex(message: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = message.to_vec();
    let bit_len = (message.len() as u64).wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(SHA256_K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|w| format!("{w:08x}")).collect()
}

/// Whole-database snapshot for export/import. Provider keys are excluded by
/// design — they stay in the OS credential store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportBundle {
    pub app: String,
    pub version: u32,
    pub exported_at: i64,
    pub providers: Vec<Provider>,
    pub conversations: Vec<Conversation>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub rooms: Vec<Room>,
    #[serde(default)]
    pub room_members: Vec<RoomMember>,
    #[serde(default)]
    pub room_messages: Vec<RoomMessage>,
    pub settings: Vec<(String, String)>,
}

const EXPORT_SETTINGS: &[&str] = &["default.provider", "default.model"];

impl KbStore {
    /// Bring the database up to the full schema (v2-v5).
    /// Idempotent — safe to call on every open.
    pub fn migrate_chat(&self) -> Result<(), RagError> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_err)?;
        if version < 2 {
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS providers (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        base_url TEXT NOT NULL,
                        created_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS conversations (
                        id TEXT PRIMARY KEY,
                        title TEXT NOT NULL,
                        provider_id TEXT,
                        model TEXT NOT NULL DEFAULT '',
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS messages (
                        id TEXT PRIMARY KEY,
                        conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                        role TEXT NOT NULL,
                        content TEXT NOT NULL,
                        created_at INTEGER NOT NULL
                    );
                    CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id);
                    CREATE INDEX IF NOT EXISTS idx_conv_updated ON conversations(updated_at);
                    PRAGMA user_version = 2;",
                )
                .map_err(db_err)?;
        }
        if version < 3 {
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS agents (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        avatar_color TEXT NOT NULL DEFAULT '#2c5fd0',
                        avatar_icon TEXT NOT NULL DEFAULT '?',
                        role TEXT NOT NULL DEFAULT '',
                        system_prompt TEXT NOT NULL,
                        provider_id TEXT,
                        model TEXT NOT NULL DEFAULT '',
                        temperature REAL NOT NULL DEFAULT 0.7,
                        max_tokens INTEGER NOT NULL DEFAULT 0,
                        enabled INTEGER NOT NULL DEFAULT 1,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    ALTER TABLE conversations ADD COLUMN agent_id TEXT;
                    ALTER TABLE messages ADD COLUMN agent_id TEXT;
                    CREATE TABLE IF NOT EXISTS runs (
                        conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
                        model TEXT NOT NULL DEFAULT '',
                        provider_id TEXT,
                        agent_id TEXT,
                        request_json TEXT NOT NULL DEFAULT '[]',
                        prompt_tokens INTEGER,
                        completion_tokens INTEGER,
                        total_tokens INTEGER,
                        latency_ms INTEGER NOT NULL DEFAULT 0,
                        created_at INTEGER NOT NULL
                    );
                    PRAGMA user_version = 3;",
                )
                .map_err(db_err)?;
        }
        if version < 4 {
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS rooms (
                        id TEXT PRIMARY KEY,
                        title TEXT NOT NULL,
                        mode TEXT NOT NULL DEFAULT 'orchestrated',
                        lead_agent_id TEXT,
                        max_turns INTEGER NOT NULL DEFAULT 12,
                        token_budget INTEGER NOT NULL DEFAULT 40000,
                        timeout_ms INTEGER NOT NULL DEFAULT 180000,
                        loop_detection INTEGER NOT NULL DEFAULT 1,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS room_members (
                        room_id TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
                        agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                        seat INTEGER NOT NULL DEFAULT 0,
                        can_talk_to TEXT NOT NULL DEFAULT '[]',
                        PRIMARY KEY (room_id, agent_id)
                    );
                    CREATE TABLE IF NOT EXISTS room_messages (
                        id TEXT PRIMARY KEY,
                        room_id TEXT NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
                        sender TEXT NOT NULL,
                        sender_agent_id TEXT,
                        recipient TEXT,
                        recipient_agent_id TEXT,
                        content TEXT NOT NULL,
                        tool_calls TEXT NOT NULL DEFAULT '[]',
                        model TEXT NOT NULL DEFAULT '',
                        prompt_tokens INTEGER,
                        completion_tokens INTEGER,
                        total_tokens INTEGER,
                        latency_ms INTEGER,
                        created_at INTEGER NOT NULL
                    );
                    CREATE INDEX IF NOT EXISTS idx_room_messages_room ON room_messages(room_id);
                    CREATE TABLE IF NOT EXISTS room_runs (
                        room_id TEXT PRIMARY KEY REFERENCES rooms(id) ON DELETE CASCADE,
                        status TEXT NOT NULL DEFAULT 'idle',
                        stop_reason TEXT,
                        turn_count INTEGER NOT NULL DEFAULT 0,
                        tokens_used INTEGER NOT NULL DEFAULT 0,
                        started_at INTEGER,
                        updated_at INTEGER NOT NULL
                    );
                    ALTER TABLE agents ADD COLUMN tool_protocol TEXT NOT NULL DEFAULT 'auto';
                    PRAGMA user_version = 4;",
                )
                .map_err(db_err)?;
        }
        if version < 5 {
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS mcp_servers (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        transport TEXT NOT NULL DEFAULT 'stdio',
                        command TEXT,
                        args TEXT NOT NULL DEFAULT '[]',
                        url TEXT,
                        enabled INTEGER NOT NULL DEFAULT 1,
                        created_at INTEGER NOT NULL
                    );
                    ALTER TABLE agents ADD COLUMN permissions TEXT NOT NULL
                        DEFAULT '{\"workspace\":false,\"network\":false,\"shell\":false}';
                    ALTER TABLE agents ADD COLUMN tools TEXT NOT NULL DEFAULT '[]';
                    CREATE TABLE IF NOT EXISTS tool_audit (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        created_at INTEGER NOT NULL,
                        room_id TEXT,
                        agent_id TEXT,
                        tool TEXT NOT NULL,
                        args_json TEXT NOT NULL DEFAULT '{}',
                        decision TEXT NOT NULL DEFAULT 'allow',
                        result_summary TEXT NOT NULL DEFAULT ''
                    );
                    CREATE INDEX IF NOT EXISTS idx_audit_room ON tool_audit(room_id);
                    CREATE INDEX IF NOT EXISTS idx_audit_time ON tool_audit(created_at);
                    CREATE TABLE IF NOT EXISTS remote_tokens (
                        id TEXT PRIMARY KEY,
                        label TEXT NOT NULL,
                        token_hash TEXT NOT NULL,
                        created_at INTEGER NOT NULL
                    );
                    CREATE TABLE IF NOT EXISTS agent_tool_grants (
                        agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                        tool TEXT NOT NULL,
                        granted_at INTEGER NOT NULL,
                        PRIMARY KEY (agent_id, tool)
                    );
                    PRAGMA user_version = 5;",
                )
                .map_err(db_err)?;
        }
        Ok(())
    }

    // -- providers ----------------------------------------------------------

    pub fn add_provider(&self, input: &NewProvider) -> Result<Provider, RagError> {
        let name = input.name.trim();
        let base_url = input.base_url.trim().trim_end_matches('/');
        if name.is_empty() {
            return Err(RagError::Input("Give the provider a name.".to_string()));
        }
        if base_url.is_empty() {
            return Err(RagError::Input("Give the provider a base URL.".to_string()));
        }
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(RagError::Input(
                "The base URL must start with http:// or https://.".to_string(),
            ));
        }
        let provider = Provider {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            base_url: base_url.to_string(),
            created_at: now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO providers(id, name, base_url, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![provider.id, provider.name, provider.base_url, provider.created_at],
            )
            .map_err(db_err)?;
        Ok(provider)
    }

    pub fn update_provider(
        &self,
        id: &str,
        name: &str,
        base_url: &str,
    ) -> Result<Provider, RagError> {
        let name = name.trim();
        let base_url = base_url.trim().trim_end_matches('/');
        if name.is_empty() {
            return Err(RagError::Input("Give the provider a name.".to_string()));
        }
        if base_url.is_empty() || (!base_url.starts_with("http://") && !base_url.starts_with("https://")) {
            return Err(RagError::Input(
                "The base URL must start with http:// or https://.".to_string(),
            ));
        }
        let changed = self
            .conn
            .execute(
                "UPDATE providers SET name = ?1, base_url = ?2 WHERE id = ?3",
                params![name, base_url, id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That provider is gone.".to_string()));
        }
        self.get_provider(id)?
            .ok_or_else(|| RagError::Input("That provider is gone.".to_string()))
    }

    pub fn get_provider(&self, id: &str) -> Result<Option<Provider>, RagError> {
        self.conn
            .query_row(
                "SELECT id, name, base_url, created_at FROM providers WHERE id = ?1",
                [id],
                row_to_provider,
            )
            .optional()
            .map_err(db_err)
    }

    pub fn list_providers(&self) -> Result<Vec<Provider>, RagError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, base_url, created_at FROM providers ORDER BY created_at")
            .map_err(db_err)?;
        let rows = stmt.query_map([], row_to_provider).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn delete_provider(&self, id: &str) -> Result<bool, RagError> {
        let removed = self
            .conn
            .execute("DELETE FROM providers WHERE id = ?1", [id])
            .map_err(db_err)?;
        // Conversations keep their model text; the provider link clears.
        self.conn
            .execute(
                "UPDATE conversations SET provider_id = NULL WHERE provider_id = ?1",
                [id],
            )
            .map_err(db_err)?;
        // Agents fall back to "no provider" (an actionable error at chat
        // time) instead of dangling at a ghost id.
        self.conn
            .execute("UPDATE agents SET provider_id = NULL WHERE provider_id = ?1", [id])
            .map_err(db_err)?;
        Ok(removed > 0)
    }

    // -- conversations -------------------------------------------------------

    pub fn create_conversation(
        &self,
        title: &str,
        provider_id: Option<&str>,
        model: &str,
    ) -> Result<Conversation, RagError> {
        let now = now_ms();
        let conv = Conversation {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.trim().to_string(),
            provider_id: provider_id
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            model: model.trim().to_string(),
            agent_id: None,
            created_at: now,
            updated_at: now,
        };
        if conv.title.is_empty() {
            return Err(RagError::Input("Give the conversation a title.".to_string()));
        }
        self.conn
            .execute(
                "INSERT INTO conversations(id, title, provider_id, model, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    conv.id,
                    conv.title,
                    conv.provider_id,
                    conv.model,
                    conv.created_at,
                    conv.updated_at
                ],
            )
            .map_err(db_err)?;
        Ok(conv)
    }

    pub fn list_conversations(&self) -> Result<Vec<Conversation>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, provider_id, model, agent_id, created_at, updated_at
                 FROM conversations ORDER BY updated_at DESC",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([], row_to_conversation).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Match on conversation titles and message bodies (case-insensitive).
    pub fn search_conversations(&self, query: &str) -> Result<Vec<Conversation>, RagError> {
        let query = query.trim();
        if query.is_empty() {
            return self.list_conversations();
        }
        let pattern = format!("%{query}%");
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT c.id, c.title, c.provider_id, c.model, c.agent_id, c.created_at, c.updated_at
                 FROM conversations c LEFT JOIN messages m ON m.conversation_id = c.id
                 WHERE c.title LIKE ?1 ESCAPE '\\' OR m.content LIKE ?1 ESCAPE '\\'
                 ORDER BY c.updated_at DESC",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([pattern], row_to_conversation).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn rename_conversation(&self, id: &str, title: &str) -> Result<Conversation, RagError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(RagError::Input("The title cannot be empty.".to_string()));
        }
        let changed = self
            .conn
            .execute(
                "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title, now_ms(), id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That conversation is gone.".to_string()));
        }
        self.get_conversation(id)?
            .ok_or_else(|| RagError::Input("That conversation is gone.".to_string()))
    }

    pub fn get_conversation(&self, id: &str) -> Result<Option<Conversation>, RagError> {
        self.conn
            .query_row(
                "SELECT id, title, provider_id, model, agent_id, created_at, updated_at
                 FROM conversations WHERE id = ?1",
                [id],
                row_to_conversation,
            )
            .optional()
            .map_err(db_err)
    }

    pub fn set_conversation_model(
        &self,
        id: &str,
        provider_id: Option<&str>,
        model: &str,
    ) -> Result<Conversation, RagError> {
        let changed = self
            .conn
            .execute(
                "UPDATE conversations SET provider_id = ?1, model = ?2, updated_at = ?3 WHERE id = ?4",
                params![
                    provider_id.filter(|s| !s.trim().is_empty()),
                    model.trim(),
                    now_ms(),
                    id
                ],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That conversation is gone.".to_string()));
        }
        self.get_conversation(id)?
            .ok_or_else(|| RagError::Input("That conversation is gone.".to_string()))
    }

    /// Bind a conversation to one agent (`None` = plain provider/model chat).
    /// The conversation keeps its own provider/model as a fallback for when
    /// the agent is missing or disabled.
    pub fn set_conversation_agent(
        &self,
        id: &str,
        agent_id: Option<&str>,
    ) -> Result<Conversation, RagError> {
        let agent_id = agent_id.filter(|s| !s.trim().is_empty());
        if let Some(agent) = agent_id {
            if self.get_agent(agent)?.is_none() {
                return Err(RagError::Input("That agent is gone.".to_string()));
            }
        }
        let changed = if agent_id.is_some() {
            self.conn
                .execute(
                    "UPDATE conversations SET agent_id = ?1, updated_at = ?2 WHERE id = ?3",
                    params![agent_id, now_ms(), id],
                )
                .map_err(db_err)?
        } else {
            self.conn
                .execute(
                    "UPDATE conversations SET agent_id = NULL, updated_at = ?1 WHERE id = ?2",
                    params![now_ms(), id],
                )
                .map_err(db_err)?
        };
        if changed == 0 {
            return Err(RagError::Input("That conversation is gone.".to_string()));
        }
        self.get_conversation(id)?
            .ok_or_else(|| RagError::Input("That conversation is gone.".to_string()))
    }

    pub fn touch_conversation(&self, id: &str) -> Result<(), RagError> {
        self.conn
            .execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                params![now_ms(), id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn delete_conversation(&self, id: &str) -> Result<bool, RagError> {
        let removed = self
            .conn
            .execute("DELETE FROM conversations WHERE id = ?1", [id])
            .map_err(db_err)?;
        Ok(removed > 0)
    }

    // -- messages -------------------------------------------------------------

    pub fn add_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
        agent_id: Option<&str>,
    ) -> Result<Message, RagError> {
        if role != "user" && role != "assistant" {
            return Err(RagError::Input("Message role must be user or assistant.".to_string()));
        }
        let msg = Message {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            agent_id: agent_id
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            created_at: now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO messages(id, conversation_id, role, content, agent_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    msg.id,
                    msg.conversation_id,
                    msg.role,
                    msg.content,
                    msg.agent_id,
                    msg.created_at
                ],
            )
            .map_err(db_err)?;
        Ok(msg)
    }

    pub fn list_messages(&self, conversation_id: &str) -> Result<Vec<Message>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, conversation_id, role, content, agent_id, created_at
                 FROM messages WHERE conversation_id = ?1 ORDER BY created_at, rowid",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([conversation_id], row_to_message).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Delete every message after `keep_through_id` (exclusive), keeping that
    /// message itself. Used by edit-and-resend. Returns the deleted count.
    pub fn truncate_after(&self, conversation_id: &str, keep_through_id: &str) -> Result<usize, RagError> {
        let messages = self.list_messages(conversation_id)?;
        let pos = messages
            .iter()
            .position(|m| m.id == keep_through_id)
            .ok_or_else(|| RagError::Input("That message is gone.".to_string()))?;
        let mut deleted = 0usize;
        for msg in messages.iter().skip(pos + 1) {
            self.conn
                .execute("DELETE FROM messages WHERE id = ?1", [&msg.id])
                .map_err(db_err)?;
            deleted += 1;
        }
        Ok(deleted)
    }

    /// Drop the trailing assistant message, if any. Used by regenerate.
    /// Returns true when a message was removed.
    pub fn drop_trailing_assistant(&self, conversation_id: &str) -> Result<bool, RagError> {
        let removed = self
            .conn
            .execute(
                "DELETE FROM messages WHERE id = (
                    SELECT id FROM messages
                    WHERE conversation_id = ?1
                    ORDER BY created_at DESC, rowid DESC LIMIT 1
                 ) AND role = 'assistant'",
                [conversation_id],
            )
            .map_err(db_err)?;
        Ok(removed > 0)
    }

    pub fn update_message(&self, id: &str, content: &str) -> Result<(), RagError> {
        self.conn
            .execute("UPDATE messages SET content = ?1 WHERE id = ?2", params![content, id])
            .map_err(db_err)?;
        Ok(())
    }

    // -- agents ---------------------------------------------------------------

    pub fn add_agent(&self, input: &NewAgent) -> Result<Agent, RagError> {
        let checked = validate_agent(input)?;
        let now = now_ms();
        let agent = Agent {
            id: uuid::Uuid::new_v4().to_string(),
            name: checked.name,
            avatar_color: checked.avatar_color,
            avatar_icon: checked.avatar_icon,
            role: checked.role,
            system_prompt: checked.system_prompt,
            provider_id: checked.provider_id,
            model: checked.model,
            temperature: checked.temperature,
            max_tokens: checked.max_tokens,
            enabled: checked.enabled,
            tool_protocol: checked.tool_protocol,
            permissions: checked.permissions,
            tools: checked.tools,
            created_at: now,
            updated_at: now,
        };
        self.conn
            .execute(
                "INSERT INTO agents(id, name, avatar_color, avatar_icon, role, system_prompt,
                                    provider_id, model, temperature, max_tokens, enabled,
                                    tool_protocol, permissions, tools, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    agent.id,
                    agent.name,
                    agent.avatar_color,
                    agent.avatar_icon,
                    agent.role,
                    agent.system_prompt,
                    agent.provider_id,
                    agent.model,
                    agent.temperature,
                    agent.max_tokens,
                    if agent.enabled { 1 } else { 0 },
                    agent.tool_protocol,
                    serde_json::to_string(&agent.permissions).unwrap_or_else(|_| "{}".to_string()),
                    serde_json::to_string(&agent.tools).unwrap_or_else(|_| "[]".to_string()),
                    agent.created_at,
                    agent.updated_at,
                ],
            )
            .map_err(db_err)?;
        Ok(agent)
    }

    pub fn update_agent(&self, id: &str, input: &NewAgent) -> Result<Agent, RagError> {
        let checked = validate_agent(input)?;
        let changed = self
            .conn
            .execute(
                "UPDATE agents SET name = ?1, avatar_color = ?2, avatar_icon = ?3, role = ?4,
                                  system_prompt = ?5, provider_id = ?6, model = ?7,
                                  temperature = ?8, max_tokens = ?9, enabled = ?10,
                                  tool_protocol = ?11, updated_at = ?12
                 WHERE id = ?13",
                params![
                    checked.name,
                    checked.avatar_color,
                    checked.avatar_icon,
                    checked.role,
                    checked.system_prompt,
                    checked.provider_id,
                    checked.model,
                    checked.temperature,
                    checked.max_tokens,
                    if checked.enabled { 1 } else { 0 },
                    checked.tool_protocol,
                    now_ms(),
                    id,
                ],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That agent is gone.".to_string()));
        }
        self.get_agent(id)?
            .ok_or_else(|| RagError::Input("That agent is gone.".to_string()))
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<Agent>, RagError> {
        self.conn
            .query_row(
                "SELECT id, name, avatar_color, avatar_icon, role, system_prompt,
                        provider_id, model, temperature, max_tokens, enabled,
                        tool_protocol, permissions, tools, created_at, updated_at
                 FROM agents WHERE id = ?1",
                [id],
                row_to_agent,
            )
            .optional()
            .map_err(db_err)
    }

    pub fn list_agents(&self) -> Result<Vec<Agent>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, avatar_color, avatar_icon, role, system_prompt,
                        provider_id, model, temperature, max_tokens, enabled,
                        tool_protocol, permissions, tools, created_at, updated_at
                 FROM agents ORDER BY name",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([], row_to_agent).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Match on names and role descriptions, optionally restricted to one
    /// enabled state.
    pub fn search_agents(
        &self,
        query: &str,
        enabled: Option<bool>,
    ) -> Result<Vec<Agent>, RagError> {
        let query = query.trim();
        let mut sql = "SELECT id, name, avatar_color, avatar_icon, role, system_prompt,
                              provider_id, model, temperature, max_tokens, enabled,
                              tool_protocol, permissions, tools, created_at, updated_at
                       FROM agents".to_string();
        let has_query = !query.is_empty();
        let mut clauses: Vec<String> = Vec::new();
        if has_query {
            clauses.push("(name LIKE ?1 ESCAPE '\\' OR role LIKE ?1 ESCAPE '\\')".to_string());
        }
        if enabled.is_some() {
            let index = if has_query { 2 } else { 1 };
            clauses.push(format!("enabled = ?{index}"));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY name");
        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let pattern = format!("%{query}%");
        let flag: i64 = if enabled == Some(true) { 1 } else { 0 };
        let rows = if has_query && enabled.is_some() {
            stmt.query_map(params![pattern, flag], row_to_agent)
                .map_err(db_err)?
        } else if has_query {
            stmt.query_map([pattern], row_to_agent).map_err(db_err)?
        } else if enabled.is_some() {
            stmt.query_map([flag], row_to_agent).map_err(db_err)?
        } else {
            stmt.query_map([], row_to_agent).map_err(db_err)?
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Delete an agent. Bound conversations fall back to their own
    /// provider/model instead of dangling; room seats are removed and room
    /// leads pointing at the agent are cleared.
    pub fn delete_agent(&self, id: &str) -> Result<bool, RagError> {
        let removed = self
            .conn
            .execute("DELETE FROM agents WHERE id = ?1", [id])
            .map_err(db_err)?;
        self.conn
            .execute(
                "UPDATE conversations SET agent_id = NULL WHERE agent_id = ?1",
                [id],
            )
            .map_err(db_err)?;
        self.conn
            .execute("DELETE FROM room_members WHERE agent_id = ?1", [id])
            .map_err(db_err)?;
        self.conn
            .execute(
                "UPDATE rooms SET lead_agent_id = NULL WHERE lead_agent_id = ?1",
                [id],
            )
            .map_err(db_err)?;
        Ok(removed > 0)
    }

    /// Change only an agent's tool-call protocol (auto/native/json/text).
    pub fn set_agent_protocol(&self, id: &str, protocol: &str) -> Result<Agent, RagError> {
        let protocol = validate_protocol(protocol)?;
        let changed = self
            .conn
            .execute(
                "UPDATE agents SET tool_protocol = ?1, updated_at = ?2 WHERE id = ?3",
                params![protocol, now_ms(), id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That agent is gone.".to_string()));
        }
        self.get_agent(id)?
            .ok_or_else(|| RagError::Input("That agent is gone.".to_string()))
    }

    /// Copy an agent under a fresh id with " (copy)" appended to the name.
    pub fn duplicate_agent(&self, id: &str) -> Result<Agent, RagError> {
        let agent = self
            .get_agent(id)?
            .ok_or_else(|| RagError::Input("That agent is gone.".to_string()))?;
        let mut draft = agent_to_new(&agent);
        draft.name = format!("{} (copy)", agent.name);
        self.add_agent(&draft)
    }

    // -- rooms (Phase 3: multi-agent rooms) ------------------------------------

    fn validate_room_mode(mode: &str) -> Result<String, RagError> {
        let mode = mode.trim();
        match mode {
            "directed" | "orchestrated" | "round_robin" => Ok(mode.to_string()),
            _ => Err(RagError::Input(
                "Pick a room mode: directed, orchestrated, or round_robin.".to_string(),
            )),
        }
    }

    pub fn create_room(
        &self,
        title: &str,
        member_ids: &[String],
        mode: &str,
        lead_agent_id: Option<&str>,
    ) -> Result<RoomDetail, RagError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(RagError::Input("Give the room a title.".to_string()));
        }
        let mode = Self::validate_room_mode(mode)?;
        // Dedupe seats, keep first-seen order.
        let mut seen = std::collections::HashSet::new();
        let mut members: Vec<String> = Vec::new();
        for id in member_ids {
            let id = id.trim();
            if !id.is_empty() && seen.insert(id.to_string()) {
                members.push(id.to_string());
            }
        }
        if members.is_empty() {
            return Err(RagError::Input("Add at least one agent to the room.".to_string()));
        }
        for id in &members {
            if self.get_agent(id)?.is_none() {
                return Err(RagError::Input(
                    "One of the room members is gone — pick the agents again.".to_string(),
                ));
            }
        }
        let lead = lead_agent_id
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string);
        if mode == "orchestrated" {
            match &lead {
                Some(id) if members.iter().any(|m| m == id) => {}
                _ => {
                    return Err(RagError::Input(
                        "Orchestrated rooms need a lead agent that is a member.".to_string(),
                    ))
                }
            }
        } else if lead.is_some() {
            return Err(RagError::Input(
                "Only orchestrated rooms have a lead agent.".to_string(),
            ));
        }
        let now = now_ms();
        let room = Room {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.to_string(),
            mode,
            lead_agent_id: lead,
            max_turns: 12,
            token_budget: 40000,
            timeout_ms: 180000,
            loop_detection: true,
            created_at: now,
            updated_at: now,
        };
        self.conn
            .execute(
                "INSERT INTO rooms(id, title, mode, lead_agent_id, max_turns, token_budget,
                                   timeout_ms, loop_detection, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    room.id,
                    room.title,
                    room.mode,
                    room.lead_agent_id,
                    room.max_turns,
                    room.token_budget,
                    room.timeout_ms,
                    if room.loop_detection { 1 } else { 0 },
                    room.created_at,
                    room.updated_at,
                ],
            )
            .map_err(db_err)?;
        // By default everyone may message everyone plus the whole room.
        for (seat, agent_id) in members.iter().enumerate() {
            let mut talk: Vec<String> =
                members.iter().filter(|m| *m != agent_id).cloned().collect();
            talk.push("room".to_string());
            let talk_json = serde_json::to_string(&talk).unwrap_or_else(|_| "[]".to_string());
            self.conn
                .execute(
                    "INSERT INTO room_members(room_id, agent_id, seat, can_talk_to)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![room.id, agent_id, seat as i64, talk_json],
                )
                .map_err(db_err)?;
        }
        self.conn
            .execute(
                "INSERT INTO room_runs(room_id, status, stop_reason, turn_count, tokens_used,
                                      started_at, updated_at)
                 VALUES (?1, 'idle', NULL, 0, 0, NULL, ?2)",
                params![room.id, now],
            )
            .map_err(db_err)?;
        self.get_room_detail(&room.id)?
            .ok_or_else(|| RagError::Input("That room is gone.".to_string()))
    }

    pub fn list_rooms(&self) -> Result<Vec<Room>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, mode, lead_agent_id, max_turns, token_budget,
                        timeout_ms, loop_detection, created_at, updated_at
                 FROM rooms ORDER BY updated_at DESC",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([], row_to_room).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    fn get_room(&self, id: &str) -> Result<Option<Room>, RagError> {
        self.conn
            .query_row(
                "SELECT id, title, mode, lead_agent_id, max_turns, token_budget,
                        timeout_ms, loop_detection, created_at, updated_at
                 FROM rooms WHERE id = ?1",
                [id],
                row_to_room,
            )
            .optional()
            .map_err(db_err)
    }

    fn list_room_members(&self, room_id: &str) -> Result<Vec<RoomMember>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT room_id, agent_id, seat, can_talk_to
                 FROM room_members WHERE room_id = ?1 ORDER BY seat, rowid",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([room_id], row_to_member)
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    fn list_all_room_members(&self) -> Result<Vec<RoomMember>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT room_id, agent_id, seat, can_talk_to
                 FROM room_members ORDER BY room_id, seat, rowid",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([], row_to_member).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn get_room_detail(&self, id: &str) -> Result<Option<RoomDetail>, RagError> {
        let room = match self.get_room(id)? {
            Some(room) => room,
            None => return Ok(None),
        };
        Ok(Some(RoomDetail {
            members: self.list_room_members(id)?,
            room,
        }))
    }

    pub fn update_room(&self, id: &str, patch: &RoomPatch) -> Result<RoomDetail, RagError> {
        let room = self
            .get_room(id)?
            .ok_or_else(|| RagError::Input("That room is gone.".to_string()))?;
        let members = self.list_room_members(id)?;
        let member_ids: Vec<&str> = members.iter().map(|m| m.agent_id.as_str()).collect();

        let title = match &patch.title {
            None => room.title.clone(),
            Some(t) if t.trim().is_empty() => {
                return Err(RagError::Input("Give the room a title.".to_string()))
            }
            Some(t) => t.trim().to_string(),
        };
        let final_mode = match &patch.mode {
            None => room.mode.clone(),
            Some(m) => Self::validate_room_mode(m)?,
        };
        let mut final_lead = room.lead_agent_id.clone();
        match &patch.lead_agent_id {
            None => {}
            Some(None) => {
                if final_mode == "orchestrated" {
                    return Err(RagError::Input(
                        "Orchestrated rooms need a lead agent that is a member.".to_string(),
                    ));
                }
                final_lead = None;
            }
            Some(Some(agent_id)) => {
                if final_mode != "orchestrated" {
                    return Err(RagError::Input(
                        "Only orchestrated rooms have a lead agent.".to_string(),
                    ));
                }
                if !member_ids.iter().any(|m| *m == agent_id) {
                    return Err(RagError::Input(
                        "The lead agent must be a room member.".to_string(),
                    ));
                }
                final_lead = Some(agent_id.clone());
            }
        }
        if final_mode == "orchestrated" {
            match &final_lead {
                Some(lead) if member_ids.iter().any(|m| *m == lead) => {}
                _ => {
                    return Err(RagError::Input(
                        "Orchestrated rooms need a lead agent that is a member.".to_string(),
                    ))
                }
            }
        } else {
            // Switching away drops the lead; it has no meaning elsewhere.
            final_lead = None;
        }
        let positive = |name: &str, value: i64| {
            if value <= 0 {
                Err(RagError::Input(format!("{name} must be above zero.")))
            } else {
                Ok(value)
            }
        };
        let max_turns = match patch.max_turns {
            None => room.max_turns,
            Some(v) => positive("Max turns", v)?,
        };
        let token_budget = match patch.token_budget {
            None => room.token_budget,
            Some(v) => positive("Token budget", v)?,
        };
        let timeout_ms = match patch.timeout_ms {
            None => room.timeout_ms,
            Some(v) => positive("Timeout", v)?,
        };
        let loop_detection = patch.loop_detection.unwrap_or(room.loop_detection);

        self.conn
            .execute(
                "UPDATE rooms SET title = ?1, mode = ?2, lead_agent_id = ?3, max_turns = ?4,
                                  token_budget = ?5, timeout_ms = ?6, loop_detection = ?7,
                                  updated_at = ?8
                 WHERE id = ?9",
                params![
                    title,
                    final_mode,
                    final_lead,
                    max_turns,
                    token_budget,
                    timeout_ms,
                    if loop_detection { 1 } else { 0 },
                    now_ms(),
                    id,
                ],
            )
            .map_err(db_err)?;
        self.get_room_detail(id)?
            .ok_or_else(|| RagError::Input("That room is gone.".to_string()))
    }

    /// Replace the whole roster. Seats come from the input order.
    pub fn set_room_members(
        &self,
        id: &str,
        members: &[RoomMemberInput],
    ) -> Result<RoomDetail, RagError> {
        let room = self
            .get_room(id)?
            .ok_or_else(|| RagError::Input("That room is gone.".to_string()))?;
        if members.is_empty() {
            return Err(RagError::Input("Add at least one agent to the room.".to_string()));
        }
        let mut seen = std::collections::HashSet::new();
        for member in members {
            if !seen.insert(member.agent_id.clone()) {
                return Err(RagError::Input(
                    "Each agent can only sit in a room once.".to_string(),
                ));
            }
            if self.get_agent(&member.agent_id)?.is_none() {
                return Err(RagError::Input(
                    "One of the room members is gone — pick the agents again.".to_string(),
                ));
            }
        }
        for member in members {
            for target in &member.can_talk_to {
                if target != "room" && !seen.contains(target) {
                    return Err(RagError::Input(format!(
                        "The allowlist names \"{target}\", which is not in this room."
                    )));
                }
            }
        }
        if let Some(lead) = room.lead_agent_id.as_deref() {
            if !seen.contains(lead) {
                return Err(RagError::Input(
                    "The lead agent must stay a member — pick a new lead first.".to_string(),
                ));
            }
        }
        self.conn
            .execute("DELETE FROM room_members WHERE room_id = ?1", [id])
            .map_err(db_err)?;
        for member in members {
            let talk_json =
                serde_json::to_string(&member.can_talk_to).unwrap_or_else(|_| "[]".to_string());
            self.conn
                .execute(
                    "INSERT INTO room_members(room_id, agent_id, seat, can_talk_to)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![id, member.agent_id, member.seat, talk_json],
                )
                .map_err(db_err)?;
        }
        self.touch_room(id)?;
        self.get_room_detail(id)?
            .ok_or_else(|| RagError::Input("That room is gone.".to_string()))
    }

    /// Delete a room with its whole log, roster, and run record.
    pub fn delete_room(&self, id: &str) -> Result<bool, RagError> {
        self.conn
            .execute("DELETE FROM room_messages WHERE room_id = ?1", [id])
            .map_err(db_err)?;
        self.conn
            .execute("DELETE FROM room_members WHERE room_id = ?1", [id])
            .map_err(db_err)?;
        self.conn
            .execute("DELETE FROM room_runs WHERE room_id = ?1", [id])
            .map_err(db_err)?;
        let removed = self
            .conn
            .execute("DELETE FROM rooms WHERE id = ?1", [id])
            .map_err(db_err)?;
        Ok(removed > 0)
    }

    pub fn touch_room(&self, id: &str) -> Result<(), RagError> {
        self.conn
            .execute("UPDATE rooms SET updated_at = ?1 WHERE id = ?2", params![now_ms(), id])
            .map_err(db_err)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_room_message(
        &self,
        room_id: &str,
        sender: &str,
        sender_agent_id: Option<&str>,
        recipient: Option<&str>,
        recipient_agent_id: Option<&str>,
        content: &str,
        tool_calls: &[RoomToolCall],
        model: &str,
        prompt_tokens: Option<i64>,
        completion_tokens: Option<i64>,
        total_tokens: Option<i64>,
        latency_ms: Option<i64>,
    ) -> Result<RoomMessage, RagError> {
        if sender != "user" && sender != "agent" && sender != "system" {
            return Err(RagError::Input(
                "Room message sender must be user, agent, or system.".to_string(),
            ));
        }
        if let Some(recipient) = recipient {
            if recipient != "agent" && recipient != "room" {
                return Err(RagError::Input(
                    "Room message recipient must be an agent or the room.".to_string(),
                ));
            }
        }
        let tool_json =
            serde_json::to_string(tool_calls).unwrap_or_else(|_| "[]".to_string());
        let msg = RoomMessage {
            id: uuid::Uuid::new_v4().to_string(),
            room_id: room_id.to_string(),
            sender: sender.to_string(),
            sender_agent_id: sender_agent_id
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            recipient: recipient.map(str::to_string),
            recipient_agent_id: recipient_agent_id
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            content: content.to_string(),
            tool_calls: tool_calls.to_vec(),
            model: model.to_string(),
            prompt_tokens,
            completion_tokens,
            total_tokens,
            latency_ms,
            created_at: now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO room_messages(id, room_id, sender, sender_agent_id, recipient,
                                           recipient_agent_id, content, tool_calls, model,
                                           prompt_tokens, completion_tokens, total_tokens,
                                           latency_ms, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    msg.id,
                    msg.room_id,
                    msg.sender,
                    msg.sender_agent_id,
                    msg.recipient,
                    msg.recipient_agent_id,
                    msg.content,
                    tool_json,
                    msg.model,
                    msg.prompt_tokens,
                    msg.completion_tokens,
                    msg.total_tokens,
                    msg.latency_ms,
                    msg.created_at,
                ],
            )
            .map_err(db_err)?;
        self.touch_room(room_id)?;
        Ok(msg)
    }

    /// Replace the tool calls stored on a message (pending → executed).
    pub fn set_room_message_tool_calls(
        &self,
        message_id: &str,
        calls: &[RoomToolCall],
    ) -> Result<(), RagError> {
        let tool_json = serde_json::to_string(calls).unwrap_or_else(|_| "[]".to_string());
        let changed = self
            .conn
            .execute(
                "UPDATE room_messages SET tool_calls = ?1 WHERE id = ?2",
                params![tool_json, message_id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That room message is gone.".to_string()));
        }
        Ok(())
    }

    pub fn list_room_messages(&self, room_id: &str) -> Result<Vec<RoomMessage>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, room_id, sender, sender_agent_id, recipient, recipient_agent_id,
                        content, tool_calls, model, prompt_tokens, completion_tokens,
                        total_tokens, latency_ms, created_at
                 FROM room_messages WHERE room_id = ?1 ORDER BY created_at, rowid",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([room_id], row_to_room_message)
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Record a room's run state (replaces the previous row).
    pub fn save_room_run(&self, state: &RoomRunState) -> Result<(), RagError> {
        self.conn
            .execute(
                "INSERT INTO room_runs(room_id, status, stop_reason, turn_count, tokens_used,
                                      started_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(room_id) DO UPDATE SET
                    status = excluded.status, stop_reason = excluded.stop_reason,
                    turn_count = excluded.turn_count, tokens_used = excluded.tokens_used,
                    updated_at = excluded.updated_at",
                params![
                    state.room_id,
                    state.status,
                    state.stop_reason,
                    state.turn_count,
                    state.tokens_used,
                    state.updated_at,
                    state.updated_at,
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn get_room_run(&self, room_id: &str) -> Result<Option<RoomRunState>, RagError> {
        self.conn
            .query_row(
                "SELECT room_id, status, stop_reason, turn_count, tokens_used, updated_at
                 FROM room_runs WHERE room_id = ?1",
                [room_id],
                row_to_room_run,
            )
            .optional()
            .map_err(db_err)
    }

    // -- phase 4: external tools, audit, remote ---------------------------------

    /// Replace an agent's MCP tool assignments (`"server:tool"` ids).
    /// Sanitized the same way as builder input: trimmed, deduped.
    pub fn set_agent_tools(&self, id: &str, tools: &[String]) -> Result<Vec<String>, RagError> {
        let mut seen = std::collections::HashSet::new();
        let mut clean = Vec::new();
        for tool in tools {
            let tool = tool.trim();
            if !tool.is_empty() && seen.insert(tool.to_string()) {
                clean.push(tool.to_string());
            }
        }
        let json = serde_json::to_string(&clean).unwrap_or_else(|_| "[]".to_string());
        let changed = self
            .conn
            .execute(
                "UPDATE agents SET tools = ?1, updated_at = ?2 WHERE id = ?3",
                params![json, now_ms(), id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That agent is gone.".to_string()));
        }
        Ok(clean)
    }

    /// Replace an agent's out-of-chat permissions (all off by default).
    pub fn set_agent_permissions(
        &self,
        id: &str,
        permissions: &AgentPermissions,
    ) -> Result<AgentPermissions, RagError> {
        let json = serde_json::to_string(permissions).unwrap_or_else(|_| "{}".to_string());
        let changed = self
            .conn
            .execute(
                "UPDATE agents SET permissions = ?1, updated_at = ?2 WHERE id = ?3",
                params![json, now_ms(), id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That agent is gone.".to_string()));
        }
        Ok(*permissions)
    }

    pub fn add_mcp_server(&self, input: &NewMcpServer) -> Result<McpServer, RagError> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err(RagError::Input("Give the MCP server a name.".to_string()));
        }
        let transport = input.transport.trim().to_lowercase();
        if transport != "stdio" && transport != "http" {
            return Err(RagError::Input(
                "MCP transport must be stdio or http.".to_string(),
            ));
        }
        let command = input.command.as_deref().filter(|s| !s.trim().is_empty());
        let url = input.url.as_deref().filter(|s| !s.trim().is_empty());
        if transport == "stdio" && command.is_none() {
            return Err(RagError::Input(
                "A stdio MCP server needs the command to run.".to_string(),
            ));
        }
        if transport == "http" {
            match url {
                Some(u) if u.starts_with("http://") || u.starts_with("https://") => {}
                _ => {
                    return Err(RagError::Input(
                        "An HTTP MCP server needs an http(s) URL.".to_string(),
                    ))
                }
            }
        }
        let now = now_ms();
        let server = McpServer {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            transport,
            command: command.map(str::to_string),
            args: input.args.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect(),
            url: url.map(str::to_string),
            enabled: true,
            created_at: now,
        };
        let args_json = serde_json::to_string(&server.args).unwrap_or_else(|_| "[]".to_string());
        self.conn
            .execute(
                "INSERT INTO mcp_servers(id, name, transport, command, args, url, enabled, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    server.id,
                    server.name,
                    server.transport,
                    server.command,
                    args_json,
                    server.url,
                    1,
                    server.created_at,
                ],
            )
            .map_err(db_err)?;
        Ok(server)
    }

    pub fn list_mcp_servers(&self) -> Result<Vec<McpServer>, RagError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, transport, command, args, url, enabled, created_at
                 FROM mcp_servers ORDER BY name",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([], row_to_mcp_server).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn get_mcp_server(&self, id: &str) -> Result<Option<McpServer>, RagError> {
        self.conn
            .query_row(
                "SELECT id, name, transport, command, args, url, enabled, created_at
                 FROM mcp_servers WHERE id = ?1",
                [id],
                row_to_mcp_server,
            )
            .optional()
            .map_err(db_err)
    }

    pub fn set_mcp_server_enabled(&self, id: &str, enabled: bool) -> Result<McpServer, RagError> {
        let changed = self
            .conn
            .execute(
                "UPDATE mcp_servers SET enabled = ?1 WHERE id = ?2",
                params![if enabled { 1 } else { 0 }, id],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Err(RagError::Input("That MCP server is gone.".to_string()));
        }
        self.get_mcp_server(id)?
            .ok_or_else(|| RagError::Input("That MCP server is gone.".to_string()))
    }

    pub fn delete_mcp_server(&self, id: &str) -> Result<bool, RagError> {
        let removed = self
            .conn
            .execute("DELETE FROM mcp_servers WHERE id = ?1", [id])
            .map_err(db_err)?;
        Ok(removed > 0)
    }

    /// Append one audit row. Called for every tool execution, allowed or not.
    pub fn log_audit(
        &self,
        room_id: Option<&str>,
        agent_id: Option<&str>,
        tool: &str,
        args_json: &str,
        decision: &str,
        result_summary: &str,
    ) -> Result<(), RagError> {
        self.conn
            .execute(
                "INSERT INTO tool_audit(created_at, room_id, agent_id, tool, args_json,
                                       decision, result_summary)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    now_ms(),
                    room_id,
                    agent_id,
                    tool,
                    args_json,
                    decision,
                    result_summary.chars().take(500).collect::<String>(),
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Newest first, optionally restricted to one room.
    pub fn list_audit(
        &self,
        room_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AuditRow>, RagError> {
        let limit = limit.clamp(1, 1000);
        let sql = if room_id.is_some() {
            "SELECT id, created_at, room_id, agent_id, tool, args_json, decision, result_summary
             FROM tool_audit WHERE room_id = ?1 ORDER BY id DESC LIMIT ?2"
        } else {
            "SELECT id, created_at, room_id, agent_id, tool, args_json, decision, result_summary
             FROM tool_audit ORDER BY id DESC LIMIT ?1"
        };
        let mut stmt = self.conn.prepare(sql).map_err(db_err)?;
        let rows = if let Some(room) = room_id {
            stmt.query_map(params![room, limit], row_to_audit)
                .map_err(db_err)?
        } else {
            stmt.query_map([limit], row_to_audit).map_err(db_err)?
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Mint a remote-access token. Returns the row plus the plaintext token,
    /// which is shown ONCE (at pairing) and only the hash is stored.
    pub fn create_remote_token(&self, label: &str) -> Result<(RemoteTokenInfo, String), RagError> {
        let label = label.trim();
        if label.is_empty() {
            return Err(RagError::Input("Name the device you are pairing.".to_string()));
        }
        let plaintext: String = uuid::Uuid::new_v4().to_string().replace('-', "")
            + &uuid::Uuid::new_v4().to_string().replace('-', "");
        let info = RemoteTokenInfo {
            id: uuid::Uuid::new_v4().to_string(),
            label: label.to_string(),
            created_at: now_ms(),
        };
        self.conn
            .execute(
                "INSERT INTO remote_tokens(id, label, token_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![info.id, info.label, hash_token(&plaintext), info.created_at],
            )
            .map_err(db_err)?;
        Ok((info, plaintext))
    }

    /// Check a presented token. Returns the device label when it matches.
    pub fn verify_remote_token(&self, token: &str) -> Result<Option<String>, RagError> {
        let hash = hash_token(token.trim());
        self.conn
            .query_row(
                "SELECT label FROM remote_tokens WHERE token_hash = ?1",
                [&hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)
    }

    pub fn list_remote_tokens(&self) -> Result<Vec<RemoteTokenInfo>, RagError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, label, created_at FROM remote_tokens ORDER BY created_at DESC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RemoteTokenInfo {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn revoke_remote_token(&self, id: &str) -> Result<bool, RagError> {
        let removed = self
            .conn
            .execute("DELETE FROM remote_tokens WHERE id = ?1", [id])
            .map_err(db_err)?;
        Ok(removed > 0)
    }

    /// Whether the agent was granted "always" for this tool via an approval
    /// dialog. Grant ids are stable (`mcp:<server_id>:<tool>`, `run_code`,
    /// `write_file`).
    pub fn has_tool_grant(&self, agent_id: &str, tool: &str) -> Result<bool, RagError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM agent_tool_grants WHERE agent_id = ?1 AND tool = ?2",
                params![agent_id, tool],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        Ok(count > 0)
    }

    pub fn add_tool_grant(&self, agent_id: &str, tool: &str) -> Result<(), RagError> {
        self.conn
            .execute(
                "INSERT INTO agent_tool_grants(agent_id, tool, granted_at)
                 VALUES (?1, ?2, ?3) ON CONFLICT(agent_id, tool) DO NOTHING",
                params![agent_id, tool, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }

    // -- runs ------------------------------------------------------------------

    /// Record the last model call of a conversation (replaces the previous).
    pub fn save_run(&self, run: &RunInfo) -> Result<(), RagError> {
        self.conn
            .execute(
                "INSERT INTO runs(conversation_id, model, provider_id, agent_id, request_json,
                                  prompt_tokens, completion_tokens, total_tokens,
                                  latency_ms, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(conversation_id) DO UPDATE SET
                    model = excluded.model, provider_id = excluded.provider_id,
                    agent_id = excluded.agent_id, request_json = excluded.request_json,
                    prompt_tokens = excluded.prompt_tokens,
                    completion_tokens = excluded.completion_tokens,
                    total_tokens = excluded.total_tokens,
                    latency_ms = excluded.latency_ms, created_at = excluded.created_at",
                params![
                    run.conversation_id,
                    run.model,
                    run.provider_id,
                    run.agent_id,
                    run.request_json,
                    run.prompt_tokens,
                    run.completion_tokens,
                    run.total_tokens,
                    run.latency_ms,
                    run.created_at,
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn get_run(&self, conversation_id: &str) -> Result<Option<RunInfo>, RagError> {
        self.conn
            .query_row(
                "SELECT conversation_id, model, provider_id, agent_id, request_json,
                        prompt_tokens, completion_tokens, total_tokens,
                        latency_ms, created_at
                 FROM runs WHERE conversation_id = ?1",
                [conversation_id],
                |row| {
                    Ok(RunInfo {
                        conversation_id: row.get(0)?,
                        model: row.get(1)?,
                        provider_id: row.get(2)?,
                        agent_id: row.get(3)?,
                        request_json: row.get(4)?,
                        prompt_tokens: row.get(5)?,
                        completion_tokens: row.get(6)?,
                        total_tokens: row.get(7)?,
                        latency_ms: row.get(8)?,
                        created_at: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)
    }

    // -- export / import -------------------------------------------------------

    pub fn export_data(&self) -> Result<ExportBundle, RagError> {
        let mut settings = Vec::new();
        for key in EXPORT_SETTINGS {
            if let Some(value) = self.get_setting(key)? {
                settings.push(((*key).to_string(), value));
            }
        }
        // Chat-relevant settings only; knowledge settings stay out of the bundle.
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM settings WHERE key LIKE 'default.%' OR key LIKE 'embeddings.%'")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(db_err)?;
        let mut extra: Vec<(String, String)> = rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?;
        settings.append(&mut extra);
        settings.sort_by(|a, b| a.0.cmp(&b.0));
        settings.dedup_by(|a, b| a.0 == b.0);

        let conversations = self.list_conversations()?;
        let mut messages = Vec::new();
        for conv in &conversations {
            messages.extend(self.list_messages(&conv.id)?);
        }
        Ok(ExportBundle {
            app: "roundtable".to_string(),
            version: 3,
            exported_at: now_ms(),
            providers: self.list_providers()?,
            conversations,
            messages,
            agents: self.list_agents()?,
            rooms: self.list_rooms()?,
            room_members: self.list_all_room_members()?,
            room_messages: {
                let mut all = Vec::new();
                for room in self.list_rooms()? {
                    all.extend(self.list_room_messages(&room.id)?);
                }
                all
            },
            settings,
        })
    }

    /// Replace all chat + agent + room data + whitelisted settings with the bundle.
    /// Returns (conversations, messages) counts.
    pub fn import_data(&self, bundle: &ExportBundle) -> Result<(usize, usize), RagError> {
        if bundle.app != "roundtable" {
            return Err(RagError::Input("That file is not a Roundtable export.".to_string()));
        }
        let tx = self.conn.unchecked_transaction().map_err(db_err)?;
        tx.execute("DELETE FROM messages", []).map_err(db_err)?;
        tx.execute("DELETE FROM conversations", []).map_err(db_err)?;
        tx.execute("DELETE FROM providers", []).map_err(db_err)?;
        tx.execute("DELETE FROM agents", []).map_err(db_err)?;
        tx.execute("DELETE FROM room_messages", []).map_err(db_err)?;
        tx.execute("DELETE FROM room_members", []).map_err(db_err)?;
        tx.execute("DELETE FROM room_runs", []).map_err(db_err)?;
        tx.execute("DELETE FROM rooms", []).map_err(db_err)?;
        for provider in &bundle.providers {
            tx.execute(
                "INSERT INTO providers(id, name, base_url, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![provider.id, provider.name, provider.base_url, provider.created_at],
            )
            .map_err(db_err)?;
        }
        for agent in &bundle.agents {
            let checked = validate_agent(&agent_to_new(agent))?;
            tx.execute(
                "INSERT INTO agents(id, name, avatar_color, avatar_icon, role, system_prompt,
                                    provider_id, model, temperature, max_tokens, enabled,
                                    tool_protocol, permissions, tools, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    agent.id,
                    checked.name,
                    checked.avatar_color,
                    checked.avatar_icon,
                    checked.role,
                    checked.system_prompt,
                    checked.provider_id,
                    checked.model,
                    checked.temperature,
                    checked.max_tokens,
                    if checked.enabled { 1 } else { 0 },
                    checked.tool_protocol,
                    serde_json::to_string(&checked.permissions).unwrap_or_else(|_| "{}".to_string()),
                    serde_json::to_string(&checked.tools).unwrap_or_else(|_| "[]".to_string()),
                    agent.created_at,
                    agent.updated_at,
                ],
            )
            .map_err(db_err)?;
        }
        for room in &bundle.rooms {
            if room.title.trim().is_empty() {
                return Err(RagError::Input("The export contains a room without a title.".to_string()));
            }
            if !["directed", "orchestrated", "round_robin"].contains(&room.mode.as_str()) {
                return Err(RagError::Input("The export contains a room with a bad mode.".to_string()));
            }
            tx.execute(
                "INSERT INTO rooms(id, title, mode, lead_agent_id, max_turns, token_budget,
                                   timeout_ms, loop_detection, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    room.id,
                    room.title,
                    room.mode,
                    room.lead_agent_id,
                    room.max_turns,
                    room.token_budget,
                    room.timeout_ms,
                    if room.loop_detection { 1 } else { 0 },
                    room.created_at,
                    room.updated_at,
                ],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO room_runs(room_id, status, stop_reason, turn_count, tokens_used,
                                      started_at, updated_at)
                 VALUES (?1, 'idle', NULL, 0, 0, NULL, ?2)",
                params![room.id, room.updated_at],
            )
            .map_err(db_err)?;
        }
        for member in &bundle.room_members {
            let talk = serde_json::to_string(&member.can_talk_to).unwrap_or_else(|_| "[]".to_string());
            tx.execute(
                "INSERT INTO room_members(room_id, agent_id, seat, can_talk_to)
                 VALUES (?1, ?2, ?3, ?4)",
                params![member.room_id, member.agent_id, member.seat, talk],
            )
            .map_err(db_err)?;
        }
        for msg in &bundle.room_messages {
            if msg.sender != "user" && msg.sender != "agent" && msg.sender != "system" {
                return Err(RagError::Input("The export contains an invalid room message.".to_string()));
            }
            let tool_json =
                serde_json::to_string(&msg.tool_calls).unwrap_or_else(|_| "[]".to_string());
            tx.execute(
                "INSERT INTO room_messages(id, room_id, sender, sender_agent_id, recipient,
                                           recipient_agent_id, content, tool_calls, model,
                                           prompt_tokens, completion_tokens, total_tokens,
                                           latency_ms, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    msg.id,
                    msg.room_id,
                    msg.sender,
                    msg.sender_agent_id,
                    msg.recipient,
                    msg.recipient_agent_id,
                    msg.content,
                    tool_json,
                    msg.model,
                    msg.prompt_tokens,
                    msg.completion_tokens,
                    msg.total_tokens,
                    msg.latency_ms,
                    msg.created_at,
                ],
            )
            .map_err(db_err)?;
        }
        for conv in &bundle.conversations {
            tx.execute(
                "INSERT INTO conversations(id, title, provider_id, model, agent_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    conv.id,
                    conv.title,
                    conv.provider_id,
                    conv.model,
                    conv.agent_id,
                    conv.created_at,
                    conv.updated_at
                ],
            )
            .map_err(db_err)?;
        }
        for msg in &bundle.messages {
            if msg.role != "user" && msg.role != "assistant" {
                return Err(RagError::Input("The export contains an invalid message role.".to_string()));
            }
            tx.execute(
                "INSERT INTO messages(id, conversation_id, role, content, agent_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    msg.id,
                    msg.conversation_id,
                    msg.role,
                    msg.content,
                    msg.agent_id,
                    msg.created_at
                ],
            )
            .map_err(db_err)?;
        }
        for (key, value) in &bundle.settings {
            if key.starts_with("default.") || key.starts_with("embeddings.") {
                tx.execute(
                    "INSERT INTO settings(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map_err(db_err)?;
            }
        }
        tx.commit().map_err(db_err)?;
        Ok((bundle.conversations.len(), bundle.messages.len()))
    }
}

fn row_to_agent(row: &rusqlite::Row<'_>) -> rusqlite::Result<Agent> {
    Ok(Agent {
        id: row.get(0)?,
        name: row.get(1)?,
        avatar_color: row.get(2)?,
        avatar_icon: row.get(3)?,
        role: row.get(4)?,
        system_prompt: row.get(5)?,
        provider_id: row.get(6)?,
        model: row.get(7)?,
        temperature: row.get(8)?,
        max_tokens: row.get::<_, i64>(9)? as u32,
        enabled: row.get::<_, i64>(10)? != 0,
        tool_protocol: row.get(11)?,
        permissions: parse_permissions(row.get::<_, String>(12)?.as_str()),
        tools: parse_string_list(row.get::<_, String>(13)?.as_str()),
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

/// A stored agent back into builder shape (for duplicate / export round-trip).
fn agent_to_new(agent: &Agent) -> NewAgent {
    NewAgent {
        name: agent.name.clone(),
        avatar_color: agent.avatar_color.clone(),
        avatar_icon: agent.avatar_icon.clone(),
        role: agent.role.clone(),
        system_prompt: agent.system_prompt.clone(),
        provider_id: agent.provider_id.clone(),
        model: agent.model.clone(),
        temperature: agent.temperature,
        max_tokens: agent.max_tokens,
        enabled: agent.enabled,
        tool_protocol: agent.tool_protocol.clone(),
        permissions: agent.permissions,
        tools: agent.tools.clone(),
    }
}

fn parse_permissions(json: &str) -> AgentPermissions {
    serde_json::from_str(json).unwrap_or_default()
}

fn parse_string_list(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

fn parse_tool_calls(json: &str) -> Vec<RoomToolCall> {
    serde_json::from_str(json).unwrap_or_default()
}

fn row_to_room(row: &rusqlite::Row<'_>) -> rusqlite::Result<Room> {
    Ok(Room {
        id: row.get(0)?,
        title: row.get(1)?,
        mode: row.get(2)?,
        lead_agent_id: row.get(3)?,
        max_turns: row.get(4)?,
        token_budget: row.get(5)?,
        timeout_ms: row.get(6)?,
        loop_detection: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn row_to_member(row: &rusqlite::Row<'_>) -> rusqlite::Result<RoomMember> {
    Ok(RoomMember {
        room_id: row.get(0)?,
        agent_id: row.get(1)?,
        seat: row.get(2)?,
        can_talk_to: parse_string_list(row.get::<_, String>(3)?.as_str()),
    })
}

fn row_to_room_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<RoomMessage> {
    Ok(RoomMessage {
        id: row.get(0)?,
        room_id: row.get(1)?,
        sender: row.get(2)?,
        sender_agent_id: row.get(3)?,
        recipient: row.get(4)?,
        recipient_agent_id: row.get(5)?,
        content: row.get(6)?,
        tool_calls: parse_tool_calls(row.get::<_, String>(7)?.as_str()),
        model: row.get(8)?,
        prompt_tokens: row.get(9)?,
        completion_tokens: row.get(10)?,
        total_tokens: row.get(11)?,
        latency_ms: row.get(12)?,
        created_at: row.get(13)?,
    })
}

fn row_to_room_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RoomRunState> {
    Ok(RoomRunState {
        room_id: row.get(0)?,
        status: row.get(1)?,
        stop_reason: row.get(2)?,
        turn_count: row.get(3)?,
        tokens_used: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn row_to_mcp_server(row: &rusqlite::Row<'_>) -> rusqlite::Result<McpServer> {
    Ok(McpServer {
        id: row.get(0)?,
        name: row.get(1)?,
        transport: row.get(2)?,
        command: row.get(3)?,
        args: parse_string_list(row.get::<_, String>(4)?.as_str()),
        url: row.get(5)?,
        enabled: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
    })
}

fn row_to_audit(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditRow> {
    Ok(AuditRow {
        id: row.get(0)?,
        created_at: row.get(1)?,
        room_id: row.get(2)?,
        agent_id: row.get(3)?,
        tool: row.get(4)?,
        args_json: row.get(5)?,
        decision: row.get(6)?,
        result_summary: row.get(7)?,
    })
}

fn row_to_provider(row: &rusqlite::Row<'_>) -> rusqlite::Result<Provider> {
    Ok(Provider {
        id: row.get(0)?,
        name: row.get(1)?,
        base_url: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn row_to_conversation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        title: row.get(1)?,
        provider_id: row.get(2)?,
        model: row.get(3)?,
        agent_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        agent_id: row.get(4)?,
        created_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (std::path::PathBuf, KbStore) {
        let path = std::env::temp_dir().join(format!("roundtable-chat-test-{name}-{}", uuid::Uuid::new_v4()));
        let store = KbStore::open(&path).unwrap();
        store.migrate_chat().unwrap();
        (path, store)
    }

    fn cleanup(path: &std::path::Path) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn provider_crud_validates() {
        let (path, store) = temp_store("provider");
        assert!(store
            .add_provider(&NewProvider {
                name: "".to_string(),
                base_url: "https://api.example.com/v1".to_string(),
            })
            .is_err());
        assert!(store
            .add_provider(&NewProvider {
                name: "X".to_string(),
                base_url: "api.example.com".to_string(),
            })
            .is_err());
        let p = store
            .add_provider(&NewProvider {
                name: "Example".to_string(),
                base_url: "https://api.example.com/v1/".to_string(),
            })
            .unwrap();
        assert_eq!(p.base_url, "https://api.example.com/v1");
        assert_eq!(store.list_providers().unwrap().len(), 1);
        let updated = store.update_provider(&p.id, "Renamed", "http://localhost:11434/v1").unwrap();
        assert_eq!(updated.name, "Renamed");
        assert!(store.delete_provider(&p.id).unwrap());
        assert!(store.list_providers().unwrap().is_empty());
        cleanup(&path);
    }

    #[test]
    fn conversation_message_lifecycle() {
        let (path, store) = temp_store("conv");
        let conv = store.create_conversation("Hello", None, "gpt-4o").unwrap();
        let user = store.add_message(&conv.id, "user", "hi", None).unwrap();
        store.add_message(&conv.id, "assistant", "hello!", None).unwrap();
        assert_eq!(store.list_messages(&conv.id).unwrap().len(), 2);

        // Edit-and-resend support: truncate after the user message.
        assert_eq!(store.truncate_after(&conv.id, &user.id).unwrap(), 1);
        assert_eq!(store.list_messages(&conv.id).unwrap().len(), 1);

        // Regenerate support: nothing trailing to drop now.
        assert!(!store.drop_trailing_assistant(&conv.id).unwrap());
        store.add_message(&conv.id, "assistant", "again", None).unwrap();
        assert!(store.drop_trailing_assistant(&conv.id).unwrap());

        // Search hits titles and bodies.
        assert_eq!(store.search_conversations("hello").unwrap().len(), 1);
        assert!(store.search_conversations("nomatch").unwrap().is_empty());

        assert!(store.delete_conversation(&conv.id).unwrap());
        assert!(store.list_messages(&conv.id).unwrap().is_empty());
        cleanup(&path);
    }

    #[test]
    fn export_import_roundtrip() {
        let (path, store) = temp_store("export");
        store
            .add_provider(&NewProvider {
                name: "P".to_string(),
                base_url: "https://api.example.com/v1".to_string(),
            })
            .unwrap();
        let conv = store.create_conversation("T", None, "m").unwrap();
        store.add_message(&conv.id, "user", "q", None).unwrap();
        store.set_setting("default.model", "m").unwrap();

        let bundle = store.export_data().unwrap();
        let json = serde_json::to_string(&bundle).unwrap();

        let (path2, store2) = temp_store("export2");
        let parsed: ExportBundle = serde_json::from_str(&json).unwrap();
        let (convs, msgs) = store2.import_data(&parsed).unwrap();
        assert_eq!((convs, msgs), (1, 1));
        assert_eq!(store2.list_providers().unwrap().len(), 1);
        assert_eq!(
            store2.get_setting("default.model").unwrap().as_deref(),
            Some("m")
        );

        let bad: Result<ExportBundle, _> = serde_json::from_str(r#"{"app":"other"}"#);
        assert!(bad.is_err());
        cleanup(&path);
        cleanup(&path2);
    }

    fn new_agent(name: &str) -> NewAgent {
        NewAgent {
            name: name.to_string(),
            avatar_color: "#2c5fd0".to_string(),
            avatar_icon: "T".to_string(),
            role: "tester".to_string(),
            system_prompt: "You test things.".to_string(),
            provider_id: None,
            model: "mock-model".to_string(),
            temperature: 0.5,
            max_tokens: 1000,
            enabled: true,
            tool_protocol: "auto".to_string(),
            permissions: crate::AgentPermissions::default(),
            tools: Vec::new(),
        }
    }

    #[test]
    fn agent_crud_search_duplicate() {
        let (path, store) = temp_store("agents");
        let agent = store.add_agent(&new_agent("Coder")).unwrap();
        assert_eq!(store.list_agents().unwrap().len(), 1);

        let updated = store
            .update_agent(&agent.id, &new_agent("Coder II"))
            .unwrap();
        assert_eq!(updated.name, "Coder II");

        assert_eq!(store.search_agents("coder", None).unwrap().len(), 1);
        assert!(store.search_agents("plumber", None).unwrap().is_empty());
        assert_eq!(store.search_agents("", Some(true)).unwrap().len(), 1);
        assert!(store.search_agents("", Some(false)).unwrap().is_empty());

        let copy = store.duplicate_agent(&agent.id).unwrap();
        assert_ne!(copy.id, agent.id);
        assert!(copy.name.contains("(copy)"));
        assert_eq!(store.list_agents().unwrap().len(), 2);

        // Binding + unbinding a conversation.
        let conv = store.create_conversation("T", None, "m").unwrap();
        assert_eq!(conv.agent_id, None);
        let bound = store
            .set_conversation_agent(&conv.id, Some(&agent.id))
            .unwrap();
        assert_eq!(bound.agent_id.as_deref(), Some(agent.id.as_str()));
        assert!(store
            .set_conversation_agent(&conv.id, Some("missing"))
            .is_err());
        let unbound = store.set_conversation_agent(&conv.id, None).unwrap();
        assert_eq!(unbound.agent_id, None);

        // Deleting the agent unbinds conversations instead of dangling.
        store
            .set_conversation_agent(&conv.id, Some(&agent.id))
            .unwrap();
        assert!(store.delete_agent(&agent.id).unwrap());
        let after = store.get_conversation(&conv.id).unwrap().unwrap();
        assert_eq!(after.agent_id, None);

        // Attributed messages survive the round trip.
        store.add_message(&conv.id, "user", "hi", None).unwrap();
        let saved = store
            .add_message(&conv.id, "assistant", "hello", Some(&copy.id))
            .unwrap();
        assert_eq!(saved.agent_id.as_deref(), Some(copy.id.as_str()));
        cleanup(&path);
    }

    #[test]
    fn run_record_upserts() {
        let (path, store) = temp_store("runs");
        let conv = store.create_conversation("T", None, "m").unwrap();
        assert!(store.get_run(&conv.id).unwrap().is_none());
        let run = RunInfo {
            conversation_id: conv.id.clone(),
            model: "mock".to_string(),
            provider_id: None,
            agent_id: None,
            request_json: r#"[{"role":"user","content":"hi"}]"#.to_string(),
            prompt_tokens: Some(5),
            completion_tokens: Some(7),
            total_tokens: Some(12),
            latency_ms: 321,
            created_at: 0,
        };
        store.save_run(&run).unwrap();
        let loaded = store.get_run(&conv.id).unwrap().unwrap();
        assert_eq!(loaded.total_tokens, Some(12));
        assert_eq!(loaded.latency_ms, 321);
        let run2 = RunInfo { model: "mock-2".to_string(), ..run };
        store.save_run(&run2).unwrap();
        assert_eq!(store.get_run(&conv.id).unwrap().unwrap().model, "mock-2");
        cleanup(&path);
    }

    #[test]
    fn room_crud_validation_and_roster() {
        let (path, store) = temp_store("rooms");
        let a = store.add_agent(&new_agent("Ann")).unwrap();
        let b = store.add_agent(&new_agent("Ben")).unwrap();

        // Validation first.
        assert!(store.create_room("", &[a.id.clone()], "directed", None).is_err());
        assert!(store.create_room("R", &[], "directed", None).is_err());
        assert!(store.create_room("R", &[a.id.clone()], "bogus", None).is_err());
        assert!(store.create_room("R", &["missing".to_string()], "directed", None).is_err());
        assert!(store
            .create_room("R", &[a.id.clone()], "orchestrated", None)
            .is_err());
        assert!(store
            .create_room("R", &[a.id.clone()], "orchestrated", Some(&b.id))
            .is_err());
        assert!(store
            .create_room("R", &[a.id.clone()], "directed", Some(&a.id))
            .is_err());

        let detail = store
            .create_room("Team", &[a.id.clone(), b.id.clone()], "orchestrated", Some(&a.id))
            .unwrap();
        assert_eq!(detail.members.len(), 2);
        assert_eq!(detail.room.lead_agent_id.as_deref(), Some(a.id.as_str()));
        assert_eq!(detail.room.max_turns, 12);
        // Default allowlist: everyone else plus the room.
        let ann = detail.members.iter().find(|m| m.agent_id == a.id).unwrap();
        assert!(ann.can_talk_to.contains(&b.id));
        assert!(ann.can_talk_to.contains(&"room".to_string()));
        // Fresh run row.
        let run = store.get_room_run(&detail.room.id).unwrap().unwrap();
        assert_eq!(run.status, "idle");

        // Patch: rename + mode switch clears the lead.
        let updated = store
            .update_room(
                &detail.room.id,
                &RoomPatch {
                    title: Some("Team II".to_string()),
                    mode: Some("round_robin".to_string()),
                    max_turns: Some(5),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.room.title, "Team II");
        assert_eq!(updated.room.mode, "round_robin");
        assert_eq!(updated.room.lead_agent_id, None);
        assert_eq!(updated.room.max_turns, 5);
        assert!(store
            .update_room(&detail.room.id, &RoomPatch { max_turns: Some(0), ..Default::default() })
            .is_err());

        // Roster replace with a restricted allowlist.
        let updated = store
            .set_room_members(
                &detail.room.id,
                &[RoomMemberInput {
                    agent_id: b.id.clone(),
                    seat: 0,
                    can_talk_to: vec!["room".to_string()],
                }],
            )
            .unwrap();
        assert_eq!(updated.members.len(), 1);
        assert_eq!(updated.members[0].can_talk_to, vec!["room".to_string()]);
        // Dangling allowlist entries are rejected.
        assert!(store
            .set_room_members(
                &detail.room.id,
                &[RoomMemberInput {
                    agent_id: b.id.clone(),
                    seat: 0,
                    can_talk_to: vec![a.id.clone()],
                }],
            )
            .is_err());

        // Messages round-trip with tool calls.
        let calls = vec![crate::rooms::RoomToolCall {
            id: "c1".to_string(),
            tool: "finish".to_string(),
            args: serde_json::json!({"result": "done"}),
            result: Some("done".to_string()),
        }];
        let saved = store
            .add_room_message(
                &detail.room.id,
                "agent",
                Some(&b.id),
                None,
                None,
                "working",
                &calls,
                "mock",
                Some(10),
                Some(20),
                Some(30),
                Some(99),
            )
            .unwrap();
        assert_eq!(saved.tool_calls.len(), 1);
        let messages = store.list_room_messages(&detail.room.id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].total_tokens, Some(30));
        assert_eq!(messages[0].tool_calls[0].tool, "finish");
        assert!(store
            .add_room_message(&detail.room.id, "robot", None, None, None, "x", &[], "", None, None, None, None)
            .is_err());

        // Protocol switch.
        let agent = store.set_agent_protocol(&b.id, "json").unwrap();
        assert_eq!(agent.tool_protocol, "json");
        assert!(store.set_agent_protocol(&b.id, "smoke").is_err());

        // Delete cascades the log, roster, and run.
        assert!(store.delete_room(&detail.room.id).unwrap());
        assert!(store.get_room_detail(&detail.room.id).unwrap().is_none());
        assert!(store.list_room_messages(&detail.room.id).unwrap().is_empty());
        assert!(!store.delete_room(&detail.room.id).unwrap());
        cleanup(&path);
    }

    #[test]
    fn phase4_tools_audit_and_remote_tokens() {
        let (path, store) = temp_store("phase4");
        let agent = store.add_agent(&new_agent("Ann")).unwrap();
        assert!(agent.tools.is_empty());
        assert!(!agent.permissions.shell);

        // Tool + permission assignment round-trips.
        let tools = store
            .set_agent_tools(&agent.id, &["mcp:search".to_string(), " mcp:search ".to_string(), "".to_string()])
            .unwrap();
        assert_eq!(tools, vec!["mcp:search".to_string()]);
        let perms = crate::AgentPermissions { workspace: true, network: false, shell: true };
        let saved = store.set_agent_permissions(&agent.id, &perms).unwrap();
        assert!(saved.workspace && saved.shell && !saved.network);
        let reloaded = store.get_agent(&agent.id).unwrap().unwrap();
        assert_eq!(reloaded.tools, vec!["mcp:search".to_string()]);
        assert!(reloaded.permissions.workspace);
        assert!(store.set_agent_tools("missing", &[]).is_err());
        // Builder saves leave assignments alone.
        let updated = store.update_agent(&agent.id, &new_agent("Ann II")).unwrap();
        assert_eq!(updated.tools, vec!["mcp:search".to_string()]);
        assert!(updated.permissions.shell);

        // MCP servers validate their transport.
        assert!(store
            .add_mcp_server(&crate::db::NewMcpServer {
                name: "".to_string(),
                transport: "stdio".to_string(),
                command: Some("npx".to_string()),
                args: vec![],
                url: None,
            })
            .is_err());
        assert!(store
            .add_mcp_server(&crate::db::NewMcpServer {
                name: "bad".to_string(),
                transport: "stdio".to_string(),
                command: None,
                args: vec![],
                url: None,
            })
            .is_err());
        assert!(store
            .add_mcp_server(&crate::db::NewMcpServer {
                name: "bad".to_string(),
                transport: "http".to_string(),
                command: None,
                args: vec![],
                url: Some("not-a-url".to_string()),
            })
            .is_err());
        let server = store
            .add_mcp_server(&crate::db::NewMcpServer {
                name: "Tools".to_string(),
                transport: "stdio".to_string(),
                command: Some("npx".to_string()),
                args: vec!["-y".to_string(), "pkg".to_string()],
                url: None,
            })
            .unwrap();
        assert_eq!(store.list_mcp_servers().unwrap().len(), 1);
        let disabled = store.set_mcp_server_enabled(&server.id, false).unwrap();
        assert!(!disabled.enabled);
        assert!(store.delete_mcp_server(&server.id).unwrap());
        assert!(store.list_mcp_servers().unwrap().is_empty());

        // Audit log records and lists newest-first.
        store
            .log_audit(Some("r1"), Some(&agent.id), "finish", "{\"result\":\"ok\"}", "allow", "ok")
            .unwrap();
        store
            .log_audit(None, None, "run_code", "{}", "deny", "no shell permission")
            .unwrap();
        let all = store.list_audit(None, 100).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].tool, "run_code");
        let room_only = store.list_audit(Some("r1"), 100).unwrap();
        assert_eq!(room_only.len(), 1);
        assert_eq!(room_only[0].decision, "allow");

        // Remote tokens: mint once, verify by hash, revoke.
        let (info, plaintext) = store.create_remote_token("Phone").unwrap();
        assert_eq!(plaintext.len(), 64);
        assert_eq!(
            store.verify_remote_token(&plaintext).unwrap().as_deref(),
            Some("Phone")
        );
        assert!(store.verify_remote_token("wrong").unwrap().is_none());
        assert_eq!(store.list_remote_tokens().unwrap().len(), 1);
        assert_eq!(info.label, "Phone");
        assert!(store.revoke_remote_token(&info.id).unwrap());
        assert!(store.verify_remote_token(&plaintext).unwrap().is_none());
        assert!(store.create_remote_token("  ").is_err());
        cleanup(&path);
    }

    #[test]
    fn sha256_matches_known_vector() {
        // "abc" — FIPS 180-4 sanity check for the std-only implementation.
        assert_eq!(
            super::sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn room_export_import_roundtrip() {
        let (path, store) = temp_store("room-export");
        let a = store.add_agent(&new_agent("Ann")).unwrap();
        let detail = store
            .create_room("R", &[a.id.clone()], "directed", None)
            .unwrap();
        store
            .add_room_message(&detail.room.id, "user", None, None, None, "hi", &[], "", None, None, None, None)
            .unwrap();
        let bundle = store.export_data().unwrap();
        assert_eq!(bundle.version, 3);
        assert_eq!(bundle.rooms.len(), 1);
        assert_eq!(bundle.room_members.len(), 1);
        assert_eq!(bundle.room_messages.len(), 1);

        let json = serde_json::to_string(&bundle).unwrap();
        let (path2, store2) = temp_store("room-import");
        let parsed: ExportBundle = serde_json::from_str(&json).unwrap();
        store2.import_data(&parsed).unwrap();
        let detail2 = store2.get_room_detail(&detail.room.id).unwrap().unwrap();
        assert_eq!(detail2.room.title, "R");
        assert_eq!(detail2.members.len(), 1);
        assert_eq!(store2.list_room_messages(&detail.room.id).unwrap().len(), 1);
        // Old bundles without room fields still import (rooms come back empty).
        let legacy: ExportBundle = serde_json::from_str(
            r#"{"app":"roundtable","version":2,"exported_at":0,"providers":[],
                "conversations":[],"messages":[],"agents":[],"settings":[]}"#,
        )
        .unwrap();
        assert!(legacy.rooms.is_empty());
        assert!(legacy.room_members.is_empty());
        assert!(legacy.room_messages.is_empty());
        store2.import_data(&legacy).unwrap();
        cleanup(&path);
        cleanup(&path2);
    }
}
