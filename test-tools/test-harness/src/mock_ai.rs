//! Scripted OpenAI-compatible mock AI server (R9 coverage infrastructure).
//!
//! TestAIServer (Go, 8 hardcoded models) is good for canned happy paths, but
//! it cannot be told to issue a specific tool call (`cluster_rpc`), reply
//! with an exact string (`HEARTBEAT_OK`), or return an empty completion.
//! Those "model-content-gated" branches need a programmable responder: this
//! module serves a queue of scripted `/v1/chat/completions` replies on an
//! ephemeral in-process port (std TcpListener + thread — same idiom as the
//! inline mocks in `nemesisbot/src/commands/*/tests.rs`, no axum dep).
//!
//! Failure philosophy: a request that arrives after the script is exhausted
//! gets HTTP 500 "script exhausted" instead of a made-up reply, so a test
//! that under-provisions its script fails loudly rather than hanging the
//! agent loop or silently walking extra branches.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// One scripted assistant turn.
#[derive(Debug, Clone)]
pub enum MockAiReply {
    /// Plain text content (finish_reason=stop, no tool_calls).
    Text(String),
    /// One tool call (finish_reason=tool_calls); arguments must be valid JSON.
    ToolCall { name: String, arguments: String },
    /// Empty assistant content (finish_reason=stop) — e.g. heartbeat "空回复" arm.
    Empty,
    /// HTTP status + body — makes the provider path surface an Err
    /// (connection-level failure is better simulated by pointing api_base at
    /// a dead port; this covers HTTP-level error statuses).
    Error { status: u16, body: String },
}

/// A running scripted mock. Drop the value to shut the listener down.
pub struct MockAiServer {
    pub port: u16,
    script: Arc<Mutex<Vec<MockAiReply>>>,
    hits: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
}

impl MockAiServer {
    /// Bind an ephemeral port and start serving `script` in FIFO order.
    pub fn start(script: Vec<MockAiReply>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let script = Arc::new(Mutex::new(script));
        let hits = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        {
            let script = Arc::clone(&script);
            let hits = Arc::clone(&hits);
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else { break };
                    let script = Arc::clone(&script);
                    let hits = Arc::clone(&hits);
                    // One request per connection: we always answer with
                    // `Connection: close`, so reqwest opens a fresh
                    // connection per call — no keep-alive state to manage.
                    std::thread::spawn(move || {
                        hits.fetch_add(1, Ordering::SeqCst);
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                        let _ = handle_request(&mut stream, &script);
                        let _ = stream.flush();
                    });
                }
            });
        }
        Ok(Self {
            port,
            script,
            hits,
            shutdown,
        })
    }

    /// Base URL to use as the model `api_base` (no trailing slash).
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Number of HTTP requests received so far.
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Script entries not yet consumed.
    pub fn remaining(&self) -> usize {
        self.script.lock().map(|s| s.len()).unwrap_or(0)
    }
}

impl Drop for MockAiServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Unblock a thread parked in accept() by connecting once.
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// Read one HTTP request, pop the next scripted reply, write the response.
fn handle_request(
    stream: &mut std::net::TcpStream,
    script: &Mutex<Vec<MockAiReply>>,
) -> std::io::Result<()> {
    let (head, body) = match read_request(stream) {
        Ok(v) => v,
        Err(_) => return Ok(()), // client went away; nothing to answer
    };
    let request_line = head.lines().next().unwrap_or("").to_string();
    let wants_stream = body.windows(13).any(|w| w == b"\"stream\":true")
        || body.windows(14).any(|w| w == b"\"stream\": true");

    // Model list probes get a minimal answer so client-side listing works.
    if request_line.starts_with("GET ") {
        let payload = r#"{"object":"list","data":[{"id":"mock-model","object":"model"}]}"#;
        return write_json(stream, 200, payload);
    }

    let reply = {
        let mut guard = script.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_empty() {
            MockAiReply::Error {
                status: 500,
                body: r#"{"error":{"message":"mock script exhausted — under-provisioned test"}}"#
                    .to_string(),
            }
        } else {
            guard.remove(0)
        }
    };

    match reply {
        MockAiReply::Error { status, body } => write_json(stream, status, &body),
        MockAiReply::Text(content) => {
            if wants_stream {
                write_sse_text(stream, &content)
            } else {
                let payload = serde_json::json!({
                    "id": "mock-chatcmpl", "object": "chat.completion",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": content},
                        "finish_reason": "stop",
                    }],
                });
                write_json(stream, 200, &payload.to_string())
            }
        }
        MockAiReply::Empty => {
            if wants_stream {
                let mut sse = String::new();
                sse.push_str("data: {\"id\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n");
                sse.push_str("data: {\"id\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n");
                sse.push_str("data: [DONE]\n\n");
                return write_raw(stream, &sse);
            }
            let payload = serde_json::json!({
                "id": "mock-chatcmpl", "object": "chat.completion",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": ""},
                    "finish_reason": "stop",
                }],
            });
            write_json(stream, 200, &payload.to_string())
        }
        MockAiReply::ToolCall { name, arguments } => {
            // `arguments` 本身就是 JSON 文本（如 `{"command":"date"}`）。json! 宏会把它
            // 作为字符串值做**单层**转义后放进 `function.arguments` —— 这正是 OpenAI
            // 协议要的形态。切勿在这里先 `Value::String(..).to_string()` 预序列化：那会
            // 产生 double-encoded 字符串（`"\"{\\\"command\\\"...}\""），客户端解出的
            // arguments 是带引号外壳的 Value::String，args_validator 一律判 Invalid。
            if wants_stream {
                let payload = serde_json::json!({
                    "id": "mock", "choices": [{
                        "index": 0,
                        "delta": {"role": "assistant", "tool_calls": [{
                            "index": 0, "id": "call_mock_1", "type": "function",
                            "function": {"name": name, "arguments": arguments},
                        }]},
                        "finish_reason": null,
                    }],
                });
                let mut sse = String::new();
                sse.push_str(&format!("data: {}\n\n", payload));
                sse.push_str("data: {\"id\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n");
                sse.push_str("data: [DONE]\n\n");
                return write_raw(stream, &sse);
            }
            let payload = serde_json::json!({
                "id": "mock-chatcmpl", "object": "chat.completion",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant", "content": None::<String>,
                        "tool_calls": [{
                            "id": "call_mock_1", "type": "function",
                            "function": {"name": name, "arguments": arguments},
                        }],
                    },
                    "finish_reason": "tool_calls",
                }],
            });
            write_json(stream, 200, &payload.to_string())
        }
    }
}

/// Read until end of headers, then Content-Length bytes of body.
fn read_request(stream: &mut std::net::TcpStream) -> std::io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Phase 1: headers.
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let content_length = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if !k.trim().eq_ignore_ascii_case("content-length") {
                return None;
            }
            v.trim().parse::<usize>().ok()
        })
        .unwrap_or(0);
    // Phase 2: body.
    let mut body = buf[head_end.min(buf.len())..].to_vec();
    while body.len() < content_length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    Ok((head, body))
}

fn write_json(stream: &mut std::net::TcpStream, status: u16, payload: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        reason,
        payload.len(),
        payload
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn write_sse_text(stream: &mut std::net::TcpStream, content: &str) -> std::io::Result<()> {
    let escaped = serde_json::json!(content).to_string();
    let mut sse = String::new();
    sse.push_str(&format!(
        "data: {{\"id\":\"mock\",\"choices\":[{{\"index\":0,\"delta\":{{\"role\":\"assistant\",\"content\":{}}}}}]}}\n\n",
        escaped
    ));
    sse.push_str("data: {\"id\":\"mock\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n");
    sse.push_str("data: [DONE]\n\n");
    write_raw(stream, &sse)
}

fn write_raw(stream: &mut std::net::TcpStream, payload: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}
