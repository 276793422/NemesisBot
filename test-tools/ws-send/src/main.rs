//! One-shot WebSocket sender for E2E testing.
//!
//! Connects to NemesisBot WebSocket, sends one message in the correct protocol
//! format, waits for the bot response, prints it, and exits.
//!
//! Usage:
//!   ws-send --url ws://127.0.0.1:49000/ws --token 12345 --msg "hello"
//!   ws-send --url ws://127.0.0.1:49000/ws --token 12345 --file message.txt
//!   ws-send --timeout 30 --url ws://127.0.0.1:49000/ws --token 12345 --file eicar.txt

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[derive(Parser)]
#[command(name = "ws-send", about = "One-shot WebSocket message sender for NemesisBot E2E testing")]
struct Args {
    /// WebSocket URL (e.g. ws://127.0.0.1:49000/ws)
    #[arg(short, long)]
    url: String,

    /// Auth token
    #[arg(short, long)]
    token: String,

    /// Message content to send (mutually exclusive with --file)
    #[arg(short, long, group = "input")]
    msg: Option<String>,

    /// Read message content from file (avoids shell escaping issues)
    #[arg(short, long, group = "input")]
    file: Option<PathBuf>,

    /// Response timeout in seconds (default 30)
    #[arg(short, long, default_value = "30")]
    timeout: u64,

    /// Print verbose output
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let content = if let Some(ref path) = args.file {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file: {}", path.display()))?
    } else if let Some(ref msg) = args.msg {
        msg.clone()
    } else {
        anyhow::bail!("Either --msg or --file must be specified");
    };

    let full_url = if args.url.contains('?') {
        format!("{}&token={}", args.url, args.token)
    } else {
        format!("{}?token={}", args.url, args.token)
    };

    if args.verbose {
        eprintln!("Connecting to {}...", args.url);
    }

    let (mut ws, _) = tokio_tungstenite::connect_async(&full_url)
        .await
        .context("WebSocket connection failed")?;

    if args.verbose {
        eprintln!("Connected.");
    }

    // Build protocol message: {type: "message", module: "chat", cmd: "send", data: {content: ...}}
    let msg = serde_json::json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": {
            "content": content
        }
    });

    ws.send(Message::Text(msg.to_string().into()))
        .await
        .context("Failed to send message")?;

    if args.verbose {
        eprintln!("Message sent. Waiting for response ({}s timeout)...", args.timeout);
    }

    // Wait for bot response
    let deadline = tokio::time::sleep(Duration::from_secs(args.timeout));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => {
                eprintln!("Timeout after {}s waiting for response", args.timeout);
                break;
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let resp: serde_json::Value = serde_json::from_str(&text)
                            .unwrap_or_else(|_| serde_json::Value::String(text.to_string()));

                        let r#type = resp.get("type").and_then(|v| v.as_str()).unwrap_or("");

                        match r#type {
                            "message" => {
                                let content = resp.get("data")
                                    .and_then(|d| d.get("content"))
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("");
                                println!("{}", content);
                                break;
                            }
                            "error" => {
                                let content = resp.get("data")
                                    .and_then(|d| d.get("content"))
                                    .or_else(|| resp.get("message"))
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("unknown error");
                                eprintln!("Error: {}", content);
                                break;
                            }
                            "system" => {
                                if args.verbose {
                                    eprintln!("[system] {}", resp.get("data").map(|d| d.to_string()).unwrap_or_default());
                                }
                                // Continue waiting for actual response
                            }
                            _ => {
                                if args.verbose {
                                    eprintln!("[{}] {}", r#type, &text[..text.len().min(200)]);
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        eprintln!("Connection closed by server");
                        break;
                    }
                    Some(Err(e)) => {
                        eprintln!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        eprintln!("Connection ended");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = ws.close(None).await;
    Ok(())
}
