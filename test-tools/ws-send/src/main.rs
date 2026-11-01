//! One-shot WebSocket sender for E2E testing.
//!
//! Connects to the **web server `/ws` endpoint** (three-level protocol:
//! `{type, module, cmd, data}` — NOT the standalone websocket-channel port,
//! whose flat `{type, content}` protocol is a different integration surface),
//! sends one chat.send message, waits for the **assistant reply** (user-echo
//! frames are skipped), prints it, and exits.
//!
//! Usage:
//!   ws-send --url ws://127.0.0.1:37300/ws --token 12345 --msg "hello"
//!   ws-send --url ws://127.0.0.1:37300/ws --token 12345 --file message.txt
//!   ws-send --timeout 30 --url ws://127.0.0.1:37300/ws --token 12345 --file eicar.txt

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[derive(Parser)]
#[command(
    name = "ws-send",
    about = "One-shot WebSocket message sender for NemesisBot E2E testing"
)]
struct Args {
    /// WebSocket URL (e.g. ws://127.0.0.1:37300/ws)
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
    #[arg(long, default_value = "30")]
    timeout: u64,

    /// Optional conversation session_id (multi-session: routes the backend to
    /// `agent:main:session:{sid}`). Used for Dashboard multi-session E2E.
    #[arg(long)]
    session_id: Option<String>,

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

    // Build protocol message: {type: "message", module: "chat", cmd: "send", data: {content, session_id?}}
    let mut data = serde_json::json!({ "content": content });
    if let Some(sid) = &args.session_id {
        data["session_id"] = serde_json::Value::String(sid.clone());
    }
    let msg = serde_json::json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": data
    });

    ws.send(Message::Text(msg.to_string().into()))
        .await
        .context("Failed to send message")?;

    if args.verbose {
        eprintln!(
            "Message sent. Waiting for response ({}s timeout)...",
            args.timeout
        );
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
                                // BUG-20261101-287：chat.send 的入站回显帧
                                // （role="user"）与 AI 回复帧（role="assistant"）
                                // 同为 type=="message"，此前不区分——回显恒先到
                                // （LLM 处理期间），ws-send 把用户自己的消息当
                                // bot 回复打印退出，AI 回复从未被等待（E2E 假绿：
                                // 只验证了入站链路，回复路径零覆盖）。按 role
                                // 过滤：user = 回显继续等；其余 = 真回复。
                                let role = resp.get("data")
                                    .and_then(|d| d.get("role"))
                                    .and_then(|r| r.as_str())
                                    .unwrap_or("");
                                if role == "user" {
                                    if args.verbose {
                                        eprintln!(
                                            "[echo] user message echoed, waiting for assistant reply..."
                                        );
                                    }
                                    continue;
                                }
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
                                // BUG-20261101-287 同族：web 端点错误帧形态是
                                // {type:"system", module:"error", cmd:"notify",
                                // data:{content}}——此前落 default 分支被静默
                                // 忽略（非 -v 时），客户端干等满 timeout 才退。
                                // 识别后报错退出。
                                let module =
                                    resp.get("module").and_then(|m| m.as_str()).unwrap_or("");
                                let cmd = resp.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
                                if module == "error" && cmd == "notify" {
                                    let content = resp.get("data")
                                        .and_then(|d| d.get("content"))
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("unknown error");
                                    eprintln!("Error: {}", content);
                                    break;
                                }
                                if args.verbose {
                                    eprintln!("[system] {}", resp.get("data").map(|d| d.to_string()).unwrap_or_default());
                                }
                                // Continue waiting for actual response
                            }
                            _ => {
                                if args.verbose {
                                    // UTF-8 安全截断：字节索引 200 可能落在多字节
                                    // 字符中间（中英文混排回复必撞），退到字符边界。
                                    fn floor_char_boundary(text: &str, mut n: usize) -> usize {
                                        while n > 0 && !text.is_char_boundary(n) {
                                            n -= 1;
                                        }
                                        n
                                    }
                                    let end = floor_char_boundary(text.as_str(), text.len().min(200));
                                    eprintln!("[{}] {}", r#type, &text[..end]);
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
