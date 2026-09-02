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

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::protocol::DEFAULT_PORT;

pub const OPENAI_PORT: u16 = 19001;

#[derive(Clone, Default)]
struct OpenAiState {
    last_conversation: Arc<Mutex<Option<String>>>,
    // serializes ask handling so concurrent requests never hit the
    // extension's single-session BUSY state
    serial: Arc<tokio::sync::Mutex<()>>,
    // idempotent retry cache: (request fingerprint, model, result text)
    recent: Arc<Mutex<Option<(u64, String, String)>>>,
    // last-attempt guard: (fingerprint, unix seconds) — a replayed request
    // within 90s is rejected instead of being sent to the page again
    last_attempt: Arc<Mutex<Option<(u64, u64)>>>,
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
) -> axum::response::Response {
    let model_id = req.model.clone();
    let site = site_for_model(&req.model);
    let (user_msg, fp) = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for m in &req.messages {
            m.role.hash(&mut hasher);
            m.text().hash(&mut hasher);
        }
        let fp = hasher.finish();
        // IMPORTANT: forward ONLY the user's actual input, verbatim.
        // System prompts / constraints are NOT injected into the web page —
        // pasting a big machine-generated preamble would look like a bot
        // and risk the account being flagged.
        let user_msg = req
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .last()
            .map(|m| m.text())
            .unwrap_or_default();
        (user_msg, fp)
    };

    if req.stream {
        // The SSE stream starts immediately, so its keepalive keeps the
        // client connection alive while the web page is generating. All the
        // slow work happens inside the stream.
        let state = state.clone();
        let stream = async_stream::stream! {
            // idempotent retry: same request within the last 2 minutes
            let recent = state.recent.lock().unwrap().clone();
            if let Some((f, model, text)) = recent {
                if f == fp && model == model_id {
                    for e in sse_chunks(model_id.clone(), &text) { yield Ok::<Event, std::convert::Infallible>(e); }
                    return;
                }
            }
            // replayed request while the first attempt is still in flight:
            // reject rather than send the same message to the page twice
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            {
                let last = state.last_attempt.lock().unwrap().clone();
                if let Some((f, at)) = last {
                    if f == fp && now.saturating_sub(at) < 90 {
                        for e in sse_chunks(model_id.clone(), &format!("[webai] duplicate request — previous attempt is still running, do not retry yet")) {
                            yield Ok::<Event, std::convert::Infallible>(e);
                        }
                        return;
                    }
                }
                *state.last_attempt.lock().unwrap() = Some((fp, now));
            }
            let _guard = state.serial.lock().await;
            let log = |_s: &str| {};
            let conversation = state.last_conversation.lock().unwrap().clone();

            // streaming round trip: deltas arrive as the page generates
            let (mut rx, mut handle) = match crate::send_one_stream(
                DEFAULT_PORT,
                conversation,
                &user_msg,
                600,
                site,
            )
            .await
            {
                Ok(pair) => pair,
                Err(e) => {
                    for e in sse_error_chunks(model_id, &e.to_string()) {
                        yield Ok::<Event, std::convert::Infallible>(e);
                    }
                    return;
                }
            };

            let mut emitted = 0usize;
            loop {
                tokio::select! {
                    Some(delta) = rx.recv() => {
                        emitted += delta.len();
                        for e in sse_chunk_events(model_id.clone(), &delta) {
                            yield Ok::<Event, std::convert::Infallible>(e);
                        }
                    }
                    res = &mut handle => {
                        // drain any last queued deltas
                        while let Ok(d) = rx.try_recv() {
                            emitted += d.len();
                            for e in sse_chunk_events(model_id.clone(), &d) {
                                yield Ok::<Event, std::convert::Infallible>(e);
                            }
                        }
                        match res {
                            Ok(Ok(data)) => {
                                *state.last_conversation.lock().unwrap() = Some(data.url.clone());
                                *state.recent.lock().unwrap() = Some((fp, model_id.clone(), data.text.clone()));
                                // the final result is authoritative: top up any
                                // text the deltas did not cover (rewrite case)
                                if emitted < data.text.len() {
                                    let tail = &data.text[emitted..];
                                    for e in sse_chunk_events(model_id.clone(), tail) {
                                        yield Ok::<Event, std::convert::Infallible>(e);
                                    }
                                }
                                yield Ok::<Event, std::convert::Infallible>(
                                    Event::default().event("chat.completion.chunk").data(
                                        json!({"id":"chatcmpl-webai","object":"chat.completion.chunk","model":model_id,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}).to_string(),
                                    ),
                                );
                                yield Ok::<Event, std::convert::Infallible>(Event::default().event("done").data("[DONE]"));
                            }
                            Ok(Err(e)) => {
                                for e in sse_error_chunks(model_id, &e.to_string()) {
                                    yield Ok::<Event, std::convert::Infallible>(e);
                                }
                            }
                            Err(e) => {
                                for e in sse_error_chunks(model_id, &format!("internal: {e}")) {
                                    yield Ok::<Event, std::convert::Infallible>(e);
                                }
                            }
                        }
                        break;
                    }
                }
            }
        };
        return Sse::new(stream).keep_alive(KeepAlive::new()).into_response();
    }

    // non-streaming: straightforward request/response
    let _guard = state.serial.lock().await;
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
    match result {
        Ok(data) => {
            *state.last_conversation.lock().unwrap() = Some(data.url.clone());
            *state.recent.lock().unwrap() = Some((fp, model_id.clone(), data.text.clone()));
            Json(json!({
                "id": "chatcmpl-webai",
                "object": "chat.completion",
                "created": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
                "model": model_id,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": data.text},
                    "finish_reason": "stop"
                }]
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error":{"message":e.to_string(),"type":"webai_error"}})),
        )
            .into_response(),
    }
}

fn sse_error_chunks(model_id: String, msg: &str) -> Vec<Event> {
    sse_chunks(model_id, &format!("[webai error] {msg}"))
}

fn sse_chunk_events(model_id: String, delta: &str) -> Vec<Event> {
    let mut events = Vec::new();
    for part in delta.as_bytes().chunks(120) {
        let part = String::from_utf8_lossy(part).to_string();
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
    events
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