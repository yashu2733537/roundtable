//! Roundtable shared core.
//!
//! Everything platform-independent lives here so the desktop (Windows/Linux)
//! and Android shells both reuse it. Shell crates (`src-tauri`) only wire
//! Tauri commands, plugins, and target-specific plumbing.

pub mod agents;
pub mod chunk;
pub mod db;
pub mod embed;
pub mod kb;
pub mod mcp;
pub mod providers;
pub mod rooms;

pub use agents::{
    agent_templates, validate_agent, validate_protocol, Agent, AgentPermissions, AgentTemplate,
    NewAgent,
};
pub use chunk::{chunk_text, Chunk, ChunkOptions};
pub use db::{
    AuditRow, Conversation, ExportBundle, McpServer, Message, NewMcpServer, NewProvider, Provider,
    RemoteTokenInfo, RunInfo,
};
pub use embed::{
    blob_to_vec, cosine, embed_local, embed_texts, vec_to_blob, EmbedderConfig, LOCAL_DIMS,
};
pub use kb::{KbDocument, KbHit, KbStore};
pub use mcp::{call_tool as mcp_call_tool, list_tools as mcp_list_tools, tool_id as mcp_tool_id, McpToolInfo};
pub use providers::{
    chat_stream, list_models, ollama_status, ChatMsg, ChatOptions, ChatResult, ChatUsage,
    OllamaStatus, OLLAMA_API, OLLAMA_V1_BASE,
};

use serde::Serialize;

/// Errors that cross the command boundary as human-readable strings.
#[derive(Debug, thiserror::Error)]
pub enum RagError {
    /// SQLite / filesystem failures.
    #[error("{0}")]
    Db(String),
    /// Embedding / provider backend failures (network, auth, bad payload).
    #[error("{0}")]
    Embed(String),
    /// Bad input, missing configuration, or cancellation; the message says
    /// how to fix it.
    #[error("{0}")]
    Input(String),
}

impl From<rusqlite::Error> for RagError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e.to_string())
    }
}

/// Build/platform facts surfaced in the UI (Inspector → About).
#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
}

/// Human-readable platform for the current target, e.g. `linux (x86_64)`,
/// `windows (aarch64)`, `android (aarch64)`.
pub fn platform() -> String {
    format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)
}

/// A 6-digit remote-pairing code. Single-use, short-lived; the shell
/// enforces expiry and attempt limits.
pub fn pairing_code() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    format!("{n:06}")
}

/// Assemble [`AppInfo`] from the shell-provided identity (the shell knows
/// the bundled product name/version; this crate knows the platform).
pub fn app_info(name: impl Into<String>, version: impl Into<String>) -> AppInfo {
    AppInfo {
        name: name.into(),
        version: version.into(),
        platform: platform(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_os_and_arch() {
        let p = platform();
        assert!(p.contains(std::env::consts::OS));
        assert!(p.contains(std::env::consts::ARCH));
        assert!(p.contains('('));
    }

    #[test]
    fn app_info_carries_identity() {
        let info = app_info("Roundtable", "0.1.0");
        assert_eq!(info.name, "Roundtable");
        assert_eq!(info.version, "0.1.0");
        assert_eq!(info.platform, platform());
    }
}
