//! Daemon: a single WebSocket endpoint on 127.0.0.1 that holds one
//! extension connection and any number of CLI connections, routing
//! `ask` requests by request id.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::protocol::{AskResult, Role, DEFAULT_PORT};

type ConnId = u64;
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Conn {
    tx: mpsc::UnboundedSender<Message>,
}

#[derive(Default)]
struct Shared {
    conns: HashMap<ConnId, Conn>,
    clis: HashMap<ConnId, ()>,
    ext: Option<ConnId>,
    pending: HashMap<String, ConnId>, // ask id -> cli conn id
}

type SharedState = Arc<Mutex<Shared>>;

pub async fn run(port: u16) -> Result<()> {
    let port = if port == 0 { DEFAULT_PORT } else { port };
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).await.map_err(|e| {
        format!("cannot bind {addr}: {e} — is another `webai serve` already running?")
    })?;
    println!("webai daemon listening on ws://{addr}");
    println!("  - load the Chrome extension and it will connect here (editable in popup)");
    let shared: SharedState = Arc::new(Mutex::new(Shared::default()));
    let mut next_id: ConnId = 0;

    loop {
        let (stream, _) = listener.accept().await?;
        let ws = tokio_tungstenite::accept_async(stream).await?;
        next_id += 1;
        let id = next_id;
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(id, ws, shared).await {
                eprintln!("conn #{id} error: {e}");
            }
        });
    }
}

async fn handle_conn(
    id: ConnId,
    ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    shared: SharedState,
) -> Result<()> {
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let mut role: Option<Role> = None;

    {
        let mut s = shared.lock().await;
        s.conns.insert(id, Conn { tx: tx.clone() });
    }

    let writer_task = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if ws_tx.send(m).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                let text = text.as_str().to_string();
                if let Err(e) = on_text(id, &mut role, text, &shared).await {
                    eprintln!("conn #{id} msg error: {e}");
                }
            }
            Message::Close(_) => break,
            Message::Ping(p) => {
                if tx.send(Message::Pong(p)).is_err() {
                    break;
                }
            }
            _ => {}
        }
    }

    let mut s = shared.lock().await;
    s.conns.remove(&id);
    match role {
        Some(Role::Extension) if s.ext == Some(id) => {
            s.ext = None;
            println!("extension disconnected");
            // fail all in-flight asks
            let pendings: Vec<(String, ConnId)> = s.pending.drain().collect();
            for (ask_id, cli_id) in pendings {
                if let Some(c) = s.conns.get(&cli_id) {
                    let r = AskResult::error(
                        &ask_id,
                        crate::protocol::ERR_NO_EXTENSION,
                        "extension disconnected while waiting",
                    );
                    let _ = c.tx.send(Message::text(json(&r)));
                }
            }
        }
        Some(Role::Cli) => {
            s.clis.remove(&id);
        }
        _ => {}
    }
    writer_task.abort();
    Ok(())
}

async fn on_text(
    id: ConnId,
    role: &mut Option<Role>,
    text: String,
    shared: &SharedState,
) -> Result<()> {
    let v: Value = serde_json::from_str(&text)?;
    let ty = v
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let mut s = shared.lock().await;

    if ty == "hello" {
        let h: crate::protocol::Hello = serde_json::from_value(v)?;
        if role.is_none() {
            match h.role {
                Role::Extension => {
                    if let Some(old) = s.ext {
                        // kick the stale connection (e.g. SW restarted and reconnected)
                        if let Some(c) = s.conns.get(&old) {
                            let _ = c.tx.send(Message::Close(None));
                        }
                    }
                    s.ext = Some(id);
                    println!("extension connected");
                }
                Role::Cli => {
                    s.clis.insert(id, ());
                    println!("cli connected (#{id})");
                }
            }
            *role = Some(h.role);
        }
        return Ok(());
    }

    match role.as_ref() {
        Some(Role::Cli) if ty == "ask" => {
            let ask: crate::protocol::Ask = serde_json::from_value(v)?;
            match s.ext {
                None => {
                    let r = AskResult::error(
                        &ask.id,
                        crate::protocol::ERR_NO_EXTENSION,
                        "extension not connected — open Chrome and check the WebAI Bridge icon is ON",
                    );
                    if let Some(c) = s.conns.get(&id) {
                        let _ = c.tx.send(Message::text(json(&r)));
                    }
                }
                Some(ext_id) => {
                    s.pending.insert(ask.id.clone(), id);
                    let fwd = serde_json::to_string(&ask)?;
                    if let Some(c) = s.conns.get(&ext_id) {
                        let _ = c.tx.send(Message::text(fwd));
                    } else {
                        s.pending.remove(&ask.id);
                        let r = AskResult::error(
                            &ask.id,
                            crate::protocol::ERR_NO_EXTENSION,
                            "extension went away while routing",
                        );
                        if let Some(c) = s.conns.get(&id) {
                            let _ = c.tx.send(Message::text(json(&r)));
                        }
                    }
                }
            }
        }
        Some(Role::Extension) if ty == "ask_result" => {
            let res: AskResult = serde_json::from_value(v)?;
            if let Some(cli_id) = s.pending.remove(&res.id) {
                let msg = Message::text(json(&res));
                if let Some(c) = s.conns.get(&cli_id) {
                    let _ = c.tx.send(msg);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn json<T: serde::Serialize>(t: &T) -> String {
    serde_json::to_string(t).unwrap()
}
