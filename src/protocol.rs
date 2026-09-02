//! WebAI bridge protocol — single source of truth for the wire format.
//!
//! All messages are JSON objects with a `"type"` field, sent over WebSocket.
//!
//! ┌──── CLI ────┐        ┌──── Rust daemon (serve) ────┐        ┌──── Extension ────┐
//! │  webai ask  │ ─────► │  routes by role + request id │ ─────► │  background.js    │
//! └─────────────┘        └──────────────────────────────┘        └───────────────────┘
//!
//! Wire types:
//!   hello            {type, role: "cli" | "extension"}
//!   ask              {type, id, payload: {message, conversation?, timeoutMs?}}
//!   ask_result       {type, id, ok, data?: {text, url, title}, error?: {code, message}}
//!   status           {type, status: "extension_connected" | "extension_disconnected" | "busy"}
//!   ping / pong      {type, ...}

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 8765;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Cli,
    Extension,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Hello {
    #[serde(rename = "type")]
    pub ty: String,
    pub role: Role,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AskPayload {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Site key: chatgpt | grok | kimi | glm (default chatgpt)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Ask {
    #[serde(rename = "type")]
    pub ty: String,
    pub id: String,
    pub payload: AskPayload,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ErrorObj {
    pub code: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AskResultData {
    pub text: String,
    pub url: String,
    pub title: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AskResult {
    #[serde(rename = "type")]
    pub ty: String,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<AskResultData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObj>,
}

impl AskResult {
    pub fn error(id: &str, code: &str, message: &str) -> Self {
        AskResult {
            ty: "ask_result".into(),
            id: id.into(),
            ok: false,
            data: None,
            error: Some(ErrorObj {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

pub const ERR_NO_EXTENSION: &str = "NO_EXTENSION";
pub const ERR_TIMEOUT: &str = "TIMEOUT";
pub const ERR_BUSY: &str = "BUSY";
pub const ERR_TAB: &str = "NO_TAB";
pub const ERR_LOGIN_REQUIRED: &str = "LOGIN_REQUIRED";
pub const ERR_SITE_DRIFT: &str = "SITE_DRIFT";
pub const ERR_INTERNAL: &str = "INTERNAL";
