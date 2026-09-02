//! WSAPI request/response sender for E2E testing.
//!
//! Sends one `{type:"request", module, cmd, reqId, data}` message over the
//! gateway WebSocket, waits for the matching `{type:"response", reqId}`,
//! prints `data` (stdout) or `error` (stderr, exit 1), and exits.
//!
//! Usage:
//!   ws-api-send --url ws://127.0.0.1:49011/ws --token 276793422 \
//!     --module cluster --cmd nodes.list
//!   ws-api-send ... --cmd nodes.ping --data '{"node_id":"node-b"}'

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[derive(Parser)]
#[command(
    name = "ws-api-send",
    about = "One-shot WSAPI (request/response) sender for NemesisBot E2E testing"
)]
struct Args {
    /// WebSocket URL (e.g. ws://127.0.0.1:49011/ws)
    #[arg(short, long)]
    url: String,

    /// Auth token (channels.web.auth_token)
    #[arg(short, long)]
    token: String,

    /// WSAPI module (e.g. cluster, board, system)
    #[arg(short, long)]
    module: String,

    /// WSAPI command (e.g. nodes.list, nodes.ping)
    #[arg(short = 'c', long)]
    cmd: String,

    /// JSON data payload (default {})
    #[arg(short = 'd', long, default_value = "{}")]
    data: String,

    /// Response timeout in seconds (default 30)
    #[arg(long, default_value = "30")]
    timeout: u64,

    /// Print non-matching interim messages to stderr
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let data: serde_json::Value = serde_json::from_str(&args.data)
        .with_context(|| format!("--data is not valid JSON: {}", args.data))?;

    let req_id = format!(
        "e2e-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );

    let full_url = if args.url.contains('?') {
        format!("{}&token={}", args.url, args.token)
    } else {
        format!("{}?token={}", args.url, args.token)
    };

    let (mut ws, _) = tokio_tungstenite::connect_async(&full_url)
        .await
        .with_context(|| format!("WebSocket connect failed: {}", args.url))?;

    let msg = serde_json::json!({
        "type": "request",
        "module": args.module,
        "cmd": args.cmd,
        "reqId": req_id,
        "data": data,
        "timestamp": chrono_now_rfc3339(),
    });

    ws.send(Message::Text(msg.to_string().into()))
        .await
        .context("Failed to send request")?;

    let deadline = tokio::time::sleep(Duration::from_secs(args.timeout));
    tokio::pin!(deadline);

    let mut exit_code = 2; // 2 = timeout without matching response
    loop {
        tokio::select! {
            _ = &mut deadline => {
                eprintln!("Timeout after {}s waiting for reqId={}", args.timeout, req_id);
                break;
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let resp: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(_) => { eprintln!("[non-json] {}", &text[..text.len().min(200)]); continue; }
                        };
                        let r#type = resp.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match r#type {
                            "response" => {
                                let rid = resp.get("reqId").and_then(|v| v.as_str()).unwrap_or("");
                                if rid != req_id { continue; }
                                if let Some(err) = resp.get("error")
                                    && !err.is_null()
                                {
                                    eprintln!("ERROR {}", serde_json::to_string(err).unwrap_or_default());
                                    exit_code = 1;
                                    break;
                                }
                                println!("{}", serde_json::to_string_pretty(
                                    resp.get("data").unwrap_or(&serde_json::Value::Null)
                                ).unwrap_or_default());
                                exit_code = 0;
                                break;
                            }
                            "error" => {
                                let rid = resp.get("reqId").and_then(|v| v.as_str()).unwrap_or("");
                                if rid == req_id {
                                    eprintln!("ERROR {}", resp.get("data")
                                        .or_else(|| resp.get("message"))
                                        .map(|v| v.to_string()).unwrap_or_default());
                                    exit_code = 1;
                                    break;
                                }
                            }
                            _ => {
                                if args.verbose {
                                    eprintln!("[{}] {}", r#type, &text[..text.len().min(160)]);
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => { eprintln!("Connection closed by server"); break; }
                    Some(Err(e)) => { eprintln!("WebSocket error: {}", e); break; }
                    None => { eprintln!("Connection ended"); break; }
                    _ => {}
                }
            }
        }
    }

    let _ = ws.close(None).await;
    std::process::exit(exit_code);
}

/// RFC3339 timestamp without pulling chrono: seconds-precision is enough for
/// protocol decoration (server does not validate the field semantically).
fn chrono_now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // UTC ISO-8601; civil-from-days (Howard Hinnant's algorithm).
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}
