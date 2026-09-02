//! OpenAI-compatible chat endpoint so the bridge can be registered as a
//! regular model provider (e.g. "WebAI/grok-web" in opencode).
//!
//! POST http://127.0.0.1:19001/v1/chat/completions
//!
//!   {"model":"grok-web","messages":[{"role":"system","content":"..."},{"role":"user","content":"hi"}],"stream":true}
//!
//! Behaviour:
//!   * the last user message is sent to the web AI through the daemon/extension
//!   * consecutive requests automatically continue the most recent web
//!     conversation (the web page keeps the memory — nothing to re-send)
//!   * streaming returns the reply as OpenAI chunk deltas (with SSE keepalive
//!     while the web page is still generating)

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use futures_util::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::protocol::DEFAULT_PORT;

pub const OPENAI_PORT: u16 = 19001;

#[derive(Clone, Default)]
struct OpenAiState {
    last_conversation: Arc<Mutex<Option<String>>>,
}

pub async fn run(port: u16) -> anyhow::Result<()> {
    let state = OpenAiState::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("webai openai-compatible endpoint on http://{addr}/v1/chat/completions");
    println!("  register as provider in opencode.json (baseURL http://127.0.0.1:{port}/v1)");
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Deserialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMsg>,
    #[serde(default)]
    stream: bool,
}

#[derive(Deserialize)]
struct ChatMsg {
    role: String,
    content: Value,
}

impl ChatMsg {
    fn text(&self) -> String {
        match &self.content {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }
}

fn site_for_model(model: &str) -> &'static str {
    let m = model.to_lowercase();
    if m.contains("grok") {
        "grok"
    } else if m.contains("kimi") {
        "kimi"
    } else if m.contains("glm") || m.contains("zhipu") {
        "glm"
    } else {
        "chatgpt"
    }
}

async fn chat_completions(
    State(state): State<OpenAiState>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    // assemble the prompt: system messages become an injected constraint
    let system_parts: Vec<String> = req
        .messages
        .iter()
        .filter(|m| m.role == "system" && !m.text().trim().is_empty())
        .map(|m| m.text())
        .collect();
    let user_msg = req
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .last()
        .map(|m| m.text())
        .unwrap_or_default();

    let site = site_for_model(&req.model);

    let log = |_s: &str| {};
    let conversation = state.last_conversation.lock().unwrap().clone();
    let result = crate::ask_flow(
        &user_msg,
        DEFAULT_PORT,
        600,
        conversation,
        false,
        site,
        &[],
        &[],
        log,
    )
    .await;

    let model_id = req.model.clone();
    let (text, err) = match result {
        Ok(data) => {
            *state.last_conversation.lock().unwrap() = Some(data.url.clone());
            (data.text, None)
        }
        Err(e) => (String::new(), Some(e.to_string())),
    };

    if let Some(e) = err {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":{"message":e,"type":"webai_error"}})),
        )
            .into_response();
    }

    if req.stream {
        let stream = stream::iter(sse_chunks(model_id, &text))
            .map(Ok::<_, std::convert::Infallible>);
        Sse::new(stream)
            .keep_alive(KeepAlive::new())
            .into_response()
    } else {
        Json(json!({
            "id": "chatcmpl-webai",
            "object": "chat.completion",
            "created": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            "model": model_id,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop"
            }]
        }))
        .into_response()
    }
}

fn sse_chunks(model_id: String, text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    events.push(
        Event::default()
            .event("chat.completion.chunk")
            .data(
                json!({
                    "id": "chatcmpl-webai",
                    "object": "chat.completion.chunk",
                    "model": model_id,
                    "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]
                })
                .to_string(),
            ),
    );
    // chunk the text so clients can render progressively
    let chunk_size = 120;
    for chunk in text.as_bytes().chunks(chunk_size) {
        let part = String::from_utf8_lossy(chunk).to_string();
        events.push(
            Event::default()
                .event("chat.completion.chunk")
                .data(
                    json!({
                        "id": "chatcmpl-webai",
                        "object": "chat.completion.chunk",
                        "model": model_id,
                        "choices": [{"index": 0, "delta": {"content": part}, "finish_reason": null}]
                    })
                    .to_string(),
                ),
        );
    }
    events.push(
        Event::default()
            .event("chat.completion.chunk")
            .data(
                json!({
                    "id": "chatcmpl-webai",
                    "object": "chat.completion.chunk",
                    "model": model_id,
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
                })
                .to_string(),
            ),
    );
    events.push(Event::default().event("done").data("[DONE]"));
    events
}