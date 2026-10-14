//! OS credential-store access for provider API keys.
//!
//! Rules, enforced by construction:
//! - Keys live ONLY in the OS store (Keychain / Secret Service / Keystore)
//!   under one account per provider.
//! - They are read into memory for a single call and never written to SQLite,
//!   files, logs, exports, or anything returned to the webview.
//! - An empty key means "no key": storing "" clears the entry, and an empty
//!   stored value reads back as absent.

const SERVICE: &str = "app.roundtable.desktop";

fn account(provider_id: &str) -> String {
    format!("provider:{provider_id}")
}

fn entry(provider_id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &account(provider_id))
        .map_err(|e| format!("The OS credential store is unavailable: {e}"))
}

/// Store the key, or clear it when `key` is empty.
pub fn set_key(provider_id: &str, key: &str) -> Result<(), String> {
    let entry = entry(provider_id)?;
    if key.is_empty() {
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Could not remove the stored key: {e}")),
        }
    } else {
        entry
            .set_password(key)
            .map_err(|e| format!("Could not store the key in the OS credential store: {e}"))
    }
}

/// Whether a non-empty key is currently stored. Never returns the key.
pub fn has_key(provider_id: &str) -> Result<bool, String> {
    Ok(get_key(provider_id)?.is_some())
}

/// Read the key into memory for a single call. `Ok(None)` means no key set.
pub fn get_key(provider_id: &str) -> Result<Option<String>, String> {
    let entry = entry(provider_id)?;
    match entry.get_password() {
        Ok(password) => Ok(if password.is_empty() { None } else { Some(password) }),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Could not read the OS credential store: {e}")),
    }
}

// -- Phase 4: external-tool keys + remote-client token -----------------------
// Same rules as provider keys: OS store only, memory-only reads, never in
// SQLite/logs/exports/the webview.

fn tool_entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account)
        .map_err(|e| format!("The OS credential store is unavailable: {e}"))
}

fn set_tool_secret(account: &str, secret: &str, what: &str) -> Result<(), String> {
    let entry = tool_entry(account)?;
    if secret.is_empty() {
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Could not remove the stored {what}: {e}")),
        }
    } else {
        entry
            .set_password(secret)
            .map_err(|e| format!("Could not store the {what} in the OS credential store: {e}"))
    }
}

fn get_tool_secret(account: &str, what: &str) -> Result<Option<String>, String> {
    let entry = tool_entry(account)?;
    match entry.get_password() {
        Ok(password) => Ok(if password.is_empty() { None } else { Some(password) }),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Could not read the stored {what}: {e}")),
    }
}

/// Web-search (Tavily-compatible) API key. Empty clears it.
pub fn set_web_search_key(key: &str) -> Result<(), String> {
    set_tool_secret("tool:tavily", key, "web-search key")
}

pub fn web_search_key() -> Result<Option<String>, String> {
    get_tool_secret("tool:tavily", "web-search key")
}

pub fn has_web_search_key() -> Result<bool, String> {
    Ok(web_search_key()?.is_some())
}

/// Paired remote-server token (Android client). Empty disconnects.
pub fn set_remote_token(token: &str) -> Result<(), String> {
    set_tool_secret("remote:token", token, "remote token")
}

pub fn remote_token() -> Result<Option<String>, String> {
    get_tool_secret("remote:token", "remote token")
}

pub fn has_remote_token() -> Result<bool, String> {
    Ok(remote_token()?.is_some())
}
