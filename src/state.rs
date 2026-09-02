//! Local per-conversation initialization state.
//!
//! The web AI has no idea it was "initialized" with a set of constraints, so
//! we keep a tiny table on disk: conversation key -> fingerprint of the
//! constraint text that was injected. On the first ask (or when the
//! fingerprint changes) we send the constraints once; afterwards the same
//! conversation reuses them without re-sending.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct InitRecord {
    pub fingerprint: String,
    pub injected_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct State {
    #[serde(default)]
    pub conversations: HashMap<String, InitRecord>,
}

fn state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = PathBuf::from(home).join(".webai");
    dir.join("conversations.json")
}

fn read() -> State {
    let p = state_path();
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write(state: &State) -> anyhow::Result<()> {
    let p = state_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

/// True if this conversation has already been initialized with the given
/// fingerprint (and the fingerprint hasn't changed).
pub fn is_initialized(key: &str, fingerprint: &str) -> bool {
    read()
        .conversations
        .get(key)
        .map(|r| r.fingerprint == fingerprint)
        .unwrap_or(false)
}

/// Record that `key` was initialized with `fingerprint`.
pub fn mark_initialized(key: &str, fingerprint: &str) -> anyhow::Result<()> {
    let mut state = read();
    state.conversations.insert(
        key.to_string(),
        InitRecord {
            fingerprint: fingerprint.to_string(),
            injected_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|_| "?".into()),
        },
    );
    write(&state)
}

/// Derive a stable conversation key from a URL: `/c/<id>` is the canonical
/// shape; anything else falls back to the whole URL.
pub fn key_from_conversation(url: &str) -> String {
    if let Some(pos) = url.find("/c/") {
        let id = &url[pos + 3..];
        let id = id.trim_end_matches('/');
        if !id.is_empty() {
            return format!("/c/{id}");
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_from_url() {
        assert_eq!(key_from_conversation("https://chatgpt.com/c/abc-def"), "/c/abc-def");
        assert_eq!(key_from_conversation("https://chatgpt.com/c/abc-def/"), "/c/abc-def");
        assert_eq!(key_from_conversation("https://chatgpt.com/other"), "https://chatgpt.com/other");
    }
}
