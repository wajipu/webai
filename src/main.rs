//! webai — ask free web AIs (chatgpt.com, …) from your terminal.
//!
//! Architecture:
//!   `webai serve`    →  Rust WebSocket daemon on ws://127.0.0.1:8765
//!   Chrome extension →  connects outbound to the daemon
//!   `webai ask "…"`  →  CLI client, routed through the daemon to the
//!                       extension, which drives the logged-in chatgpt.com tab
//!                       and streams the finished reply back to the terminal.

mod daemon;
mod mcp;
mod openai;
mod protocol;
mod state;

use std::time::Duration;

use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "webai", version, about = "Ask free web AIs from the terminal via a Chrome bridge")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the local WebSocket daemon that the Chrome extension connects to
    Serve {
        /// Port to listen on (default 8765)
        #[arg(long, short, default_value_t = protocol::DEFAULT_PORT)]
        port: u16,
    },
    /// Send a message to the AI and wait for the reply
    Ask {
        /// The message to send
        message: String,
        /// Port of the running daemon
        #[arg(long, short, default_value_t = protocol::DEFAULT_PORT)]
        port: u16,
        /// Seconds to wait for the AI to finish (default 300)
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Print the raw JSON result instead of plain text
        #[arg(long)]
        json: bool,
        /// Continue an existing conversation by its URL
        #[arg(long)]
        conversation: Option<String>,
        /// Site key: chatgpt (default) | grok | kimi | glm
        #[arg(long, default_value = "chatgpt")]
        site: String,
        /// Force re-inject constraints even if this conversation is already initialized
        #[arg(long)]
        reinit: bool,
        /// Constraint / system-prompt file injected once per conversation (repeatable)
        #[arg(long)]
        system: Vec<String>,
        /// Directory of skill markdown files injected once per conversation
        #[arg(long)]
        skills: Vec<String>,
    },
    /// Print the connection status of the daemon/extension
    Status {
        #[arg(long, short, default_value_t = protocol::DEFAULT_PORT)]
        port: u16,
    },
    /// Run as a stdio MCP server (register with Claude Code / Grok Code / Cursor)
    Mcp,
    /// OpenAI-compatible chat endpoint so opencode can use web AIs as regular models
    Openai {
        /// Port to listen on (default 19001)
        #[arg(long, short, default_value_t = openai::OPENAI_PORT)]
        port: u16,
    },
}

#[derive(Clone, Debug, serde::Serialize)]
struct AskResultData {
    text: String,
    url: String,
    title: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Serve { port } => daemon::run(port).await.err().map(|e| e.to_string()),
        Command::Ask {
            message,
            port,
            timeout,
            json,
            conversation,
            reinit,
            site,
            system,
            skills,
        } => cmd_ask(&message, port, timeout, json, conversation, reinit, &site, &system, &skills)
            .await
            .err()
            .map(|e| e.to_string()),
        Command::Status { port } => cmd_status(port).await.err().map(|e| e.to_string()),
        Command::Mcp => mcp::run().await.err().map(|e| e.to_string()),
        Command::Openai { port } => openai::run(port).await.err().map(|e| e.to_string()),
    };
    if let Some(e) = code {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

// ------------------------------------------------------------------ ask

/// Full ask pipeline shared by the CLI and the MCP server:
/// constraint injection (per-conversation, fingerprint-aware) + one round trip.
pub async fn ask_flow(
    message: &str,
    port: u16,
    timeout_secs: u64,
    conversation: Option<String>,
    reinit: bool,
    site: &str,
    system: &[String],
    skills: &[String],
    on_log: impl Fn(&str),
) -> anyhow::Result<AskResultData> {
    if message.trim().is_empty() {
        anyhow::bail!("message is empty");
    }

    let init = build_init(system, skills)?; // (text, fingerprint)

    let conv_key = conversation.as_deref().map(state::key_from_conversation);

    // Inject constraints once per conversation (or on fingerprint change,
    // or when --reinit forces it).
    if let Some((text, fp)) = &init {
        match &conv_key {
            Some(key) => {
                let already = state::is_initialized(key, fp);
                if already && !reinit {
                    on_log(&format!("constraints already loaded for {key}, skipping injection"));
                } else {
                    if reinit && already {
                        on_log(&format!("--reinit: forcing constraint re-injection into {key}"));
                    } else {
                        on_log(&format!("injecting constraints into {key} (fingerprint {fp})…"));
                    }
                    send_one(port, conversation.clone(), text, timeout_secs, site, true).await?;
                    state::mark_initialized(key, fp)?;
                    on_log(&format!("constraints injected, marked {key} initialized"));
                }
            }
            None => {
                // brand-new conversation each time: inject before every ask
                on_log("injecting constraints (new conversation)…");
                send_one(port, None, text, timeout_secs, site, true).await?;
            }
        }
    }

    send_one(port, conversation, message, timeout_secs, site, false).await
}

async fn cmd_ask(
    message: &str,
    port: u16,
    timeout_secs: u64,
    json: bool,
    conversation: Option<String>,
    reinit: bool,
    site: &str,
    system: &[String],
    skills: &[String],
) -> anyhow::Result<()> {
    let log = |s: &str| eprintln!("→ {s}");
    let d = ask_flow(message, port, timeout_secs, conversation, reinit, site, system, skills, log)
        .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&d)?);
    } else {
        print!("{}", d.text);
        if !d.text.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

/// One round trip through the daemon. `quiet` suppresses the reply echo and
/// the "waiting…" line (used for the injected constraint message).
async fn send_one(
    port: u16,
    conversation: Option<String>,
    message: &str,
    timeout_secs: u64,
    site: &str,
    quiet: bool,
) -> anyhow::Result<AskResultData> {
    let url = format!("ws://127.0.0.1:{port}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.map_err(|e| {
        anyhow::anyhow!("cannot connect to daemon at {url}: {e}\n  run `webai serve` first")
    })?;

    send_json(&mut ws, &serde_json::json!({"type":"hello","role":"cli"})).await?;
    let id = Uuid::new_v4().to_string();
    let ask = serde_json::json!({
        "type": "ask",
        "id": id,
        "payload": {
            "message": message,
            "conversation": conversation,
            "timeout_ms": timeout_secs * 1000,
            "site": site,
        }
    });
    send_json(&mut ws, &ask).await?;
    if !quiet {
        eprintln!("→ waiting for ChatGPT… (timeout {timeout_secs}s)");
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs + 15);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("timed out after {timeout_secs}s waiting for a reply");
        }
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for reply"))?
            .ok_or_else(|| anyhow::anyhow!("daemon closed the connection"))?
            .map_err(|e| anyhow::anyhow!("ws error: {e}"))?;
        match msg {
            Message::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t)?;
                if v.get("type").and_then(|x| x.as_str()) == Some("ask_result")
                    && v.get("id").and_then(|x| x.as_str()) == Some(&id)
                {
                    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                        let data = AskResultData {
                            text: v
                                .pointer("/data/text")
                                .and_then(|x| x.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            url: v
                                .pointer("/data/url")
                                .and_then(|x| x.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            title: v
                                .pointer("/data/title")
                                .and_then(|x| x.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        };
                        return Ok(data);
                    } else {
                        let code = v.pointer("/error/code").and_then(|x| x.as_str()).unwrap_or("?");
                        let m = v.pointer("/error/message").and_then(|x| x.as_str()).unwrap_or("?");
                        anyhow::bail!("[{code}] {m}");
                    }
                }
            }
            Message::Close(_) => anyhow::bail!("daemon closed the connection"),
            _ => {}
        }
    }
}

// ------------------------------------------------------ init constraints

/// Build the combined constraint text from `--system` files and `--skills`
/// directories, plus a fingerprint that changes when any of them change.
fn build_init(system: &[String], skills: &[String]) -> anyhow::Result<Option<(String, String)>> {
    if system.is_empty() && skills.is_empty() {
        return Ok(None);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut hash_input = String::new();

    for f in system {
        let content = std::fs::read_to_string(f)
            .map_err(|e| anyhow::anyhow!("cannot read --system file {f}: {e}"))?;
        parts.push(format!("## Constraint ({f})\n{content}"));
        hash_input.push_str(&content);
    }

    for dir in skills {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| anyhow::anyhow!("cannot read --skills dir {dir}: {e}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .map(|x| x == "md" || x == "mdx")
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        if files.is_empty() {
            anyhow::bail!("no .md files found in --skills dir {dir}");
        }
        for p in files {
            let content = std::fs::read_to_string(&p)?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.to_string_lossy().to_string());
            parts.push(format!("## Skill: {name}\n{content}"));
            hash_input.push_str(&content);
        }
    }

    let mut header = String::new();
    if !parts.is_empty() {
        header = "\n\n[SYSTEM CONSTRAINTS — loaded once at the start of this conversation. \
Please acknowledge these rules and follow them for the rest of this conversation. \
Do not repeat them back, do not ask about them again; they apply to every follow-up message.]\n"
            .to_string();
    }

    let text = format!(
        "{}\n{}\n\n[Acknowledge by replying OK.]",
        parts.join("\n\n"),
        header
    );

    let fp = fingerprint(&hash_input);
    Ok(Some((text, fp)))
}

fn fingerprint(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

// ------------------------------------------------------------------ misc

// ------------------------------------------------------------ streaming

/// Streaming variant of send_one: returns a channel that receives incremental
/// `delta` texts as the page generates, plus a handle that resolves to the
/// final AskResultData. Use it for SSE passthrough.
pub async fn send_one_stream(
    port: u16,
    conversation: Option<String>,
    message: &str,
    timeout_secs: u64,
    site: &str,
) -> anyhow::Result<(
    tokio::sync::mpsc::UnboundedReceiver<String>,
    tokio::task::JoinHandle<anyhow::Result<AskResultData>>,
)> {
    let url = format!("ws://127.0.0.1:{port}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.map_err(|e| {
        anyhow::anyhow!("cannot connect to daemon at {url}: {e}\n  run `webai serve` first")
    })?;

    send_json(&mut ws, &serde_json::json!({"type":"hello","role":"cli"})).await?;
    let id = Uuid::new_v4().to_string();
    let ask = serde_json::json!({
        "type": "ask",
        "id": id,
        "payload": {
            "message": message,
            "conversation": conversation,
            "timeout_ms": timeout_secs * 1000,
            "site": site,
            "stream": true,
        }
    });
    send_json(&mut ws, &ask).await?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs + 15);
        let result = loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break Err(anyhow::anyhow!("timed out after {timeout_secs}s waiting for a reply"));
            }
            let msg = tokio::time::timeout(remaining, ws.next())
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for reply"))?
                .ok_or_else(|| anyhow::anyhow!("daemon closed the connection"))?
                .map_err(|e| anyhow::anyhow!("ws error: {e}"))?;
            match msg {
                Message::Text(t) => {
                    let v: serde_json::Value = serde_json::from_str(&t)?;
                    let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    if ty == "ask_chunk" && v.get("id").and_then(|x| x.as_str()) == Some(&id) {
                        if let Some(d) = v.get("delta").and_then(|x| x.as_str()) {
                            let _ = tx.send(d.to_string());
                        }
                    } else if ty == "ask_result" && v.get("id").and_then(|x| x.as_str()) == Some(&id)
                    {
                        if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                            break Ok(AskResultData {
                                text: v.pointer("/data/text").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                                url: v.pointer("/data/url").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                                title: v.pointer("/data/title").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                            });
                        } else {
                            let code = v.pointer("/error/code").and_then(|x| x.as_str()).unwrap_or("?");
                            let m = v.pointer("/error/message").and_then(|x| x.as_str()).unwrap_or("?");
                            break Err(anyhow::anyhow!("[{code}] {m}"));
                        }
                    }
                }
                Message::Close(_) => break Err(anyhow::anyhow!("daemon closed the connection")),
                _ => {}
            }
        };
        // all deltas are queued before the result; drop tx so the receiver
        // sees None once it has drained them
        drop(tx);
        result
    });

    Ok((rx, task))
}

async fn cmd_status(port: u16) -> anyhow::Result<()> {
    let url = format!("ws://127.0.0.1:{port}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.map_err(|e| {
        anyhow::anyhow!("cannot connect to daemon at {url}: {e}\n  is `webai serve` running?")
    })?;
    send_json(&mut ws, &serde_json::json!({"type":"hello","role":"cli"})).await?;
    println!("daemon: connected (ws://127.0.0.1:{port})");
    println!("extension: (pending status support)");
    let _ = ws.close(None).await;
    Ok(())
}

async fn send_json<S>(ws: &mut S, v: &serde_json::Value) -> anyhow::Result<()>
where
    S: futures_util::SinkExt<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    ws.send(Message::Text(serde_json::to_string(v)?.into()))
        .await
        .map_err(|e| anyhow::anyhow!("send failed: {e}"))?;
    Ok(())
}
