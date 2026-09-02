//! Minimal stdio MCP (Model Context Protocol) server — hand-rolled
//! JSON-RPC 2.0 over stdin/stdout, newline-delimited, no external deps.
//!
//! Register with Claude Code:
//!   claude mcp add webai -- /Users/wajipu/.webai/bin/webai mcp
//!
//! Exposes one tool: `ask` (web AI query via the browser bridge).

use std::io::{BufRead, Write};

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::protocol::DEFAULT_PORT;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub async fn run() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // read stdin on a blocking task; EOF closes the channel via task end
    let mut reader = tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let stdout = std::io::stdout();
    loop {
        // drain whatever is already buffered before waiting (avoids losing
        // requests when stdin closes right after the last message)
        while let Ok(line) = rx.try_recv() {
            let _ = handle_line(&line, &stdout).await;
        }
        if reader.is_finished() {
            break; // EOF: client went away
        }
        tokio::select! {
            _ = &mut reader => {}
            Some(line) = rx.recv() => {
                let _ = handle_line(&line, &stdout).await;
            }
        }
    }
    Ok(())
}

fn respond(stdout: &std::io::Stdout, v: &Value) -> anyhow::Result<()> {
    let mut out = stdout.lock();
    writeln!(out, "{v}")?;
    out.flush()?;
    Ok(())
}

async fn handle_line(line: &str, stdout: &std::io::Stdout) -> anyhow::Result<()> {
    let v: Value = serde_json::from_str(line)?;
    let id = v.get("id").cloned();
    let method = v
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    // notifications have no id and get no response
    let is_notification = id.is_none();
    if is_notification {
        return Ok(());
    }
    let id = id.unwrap();

    match method.as_str() {
        "initialize" => respond(
            stdout,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "webai", "version": env!("CARGO_PKG_VERSION") },
                }
            }),
        ),
        "ping" => respond(stdout, &json!({"jsonrpc":"2.0","id":id,"result":{}})),
        "tools/list" => respond(
            stdout,
            &json!({"jsonrpc":"2.0","id":id,"result":{"tools":[tool_ask()]}}),
        ),
        "tools/call" => {
            let name = v.pointer("/params/name").and_then(|x| x.as_str()).unwrap_or("");
            let args = v.pointer("/params/arguments").cloned().unwrap_or(Value::Null);
            if name != "ask" {
                respond(
                    stdout,
                    &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"unknown tool"}}),
                )?;
                return Ok(());
            }
            let result = call_ask(&args).await;
            respond(stdout, &json!({"jsonrpc":"2.0","id":id,"result":result}))
        }
        _ => respond(
            stdout,
            &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}}),
        ),
    }
}

fn tool_ask() -> Value {
    json!({
        "name": "ask",
        "description": "Ask a free web AI (chatgpt.com / grok.com / kimi.com / chatglm.cn) through the user's logged-in browser and return the reply. Reuses an existing conversation when `conversation` is given; constraints from local files can be injected once per conversation.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "The message to send to the AI" },
                "site": {
                    "type": "string",
                    "enum": ["chatgpt", "grok", "kimi", "glm"],
                    "description": "Which web AI to use (default chatgpt)"
                },
                "conversation": {
                    "type": "string",
                    "description": "Conversation URL to continue, e.g. https://grok.com/c/xxxx"
                },
                "system": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Local constraint/system-prompt file paths, injected once per conversation"
                },
                "skills": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Directories of skill markdown files, injected once per conversation"
                },
                "reinit": {
                    "type": "boolean",
                    "description": "Force re-inject constraints even if already initialized"
                },
                "timeout": {
                    "type": "number",
                    "description": "Seconds to wait for the AI (default 300)"
                }
            },
            "required": ["message"]
        }
    })
}

async fn call_ask(args: &Value) -> Value {
    let message = args
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let site = args.get("site").and_then(|s| s.as_str()).unwrap_or("chatgpt").to_string();
    let conversation = args
        .get("conversation")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    let reinit = args.get("reinit").and_then(|r| r.as_bool()).unwrap_or(false);
    let timeout = args.get("timeout").and_then(|t| t.as_f64()).unwrap_or(300.0) as u64;
    let system: Vec<String> = args
        .get("system")
        .and_then(|s| s.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let skills: Vec<String> = args
        .get("skills")
        .and_then(|s| s.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let log = |_s: &str| {}; // MCP: keep stderr clean, diagnostics go in the result text
    match crate::ask_flow(&message, DEFAULT_PORT, timeout, conversation, reinit, &site, &system, &skills, log).await {
        Ok(data) => json!({
            "content": [
                { "type": "text", "text": data.text },
                { "type": "text", "text": format!("conversation: {}", data.url) }
            ],
            "isError": false
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": format!("error: {e}") }],
            "isError": true
        }),
    }
}