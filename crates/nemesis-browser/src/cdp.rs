//! CDP 客户端：HTTP/1.1 发现端点 + 常驻 WS 命令通道（id 多路复用 + 事件订阅）。
//!
//! 移植自 `crates/nemesis-guest/src/browser.rs` 手写件（662 行），宿主版改造：
//! - 常驻 WS 连接（guest 版单命令单连接）+ 请求 id 自增多路复用（并发附着
//!   多 tab / 同 tab 并发命令）+ 事件帧转发 broadcast（`Page.loadEventFired`
//!   等等待类动作需要事件流；guest 版无事件面）。
//!
//! **guest 版教训随移植带走**（forensic8 实证，勿删）：
//! - chromium 155 的 DevTools HTTP server **明拒 HTTP/1.0**（505）——请求行
//!   必须写 HTTP/1.1；
//! - keep-alive 形态（不发 Connection: close）在响应后被 chromium 挂住连接
//!   等 EOF——回环裸 TCP 读会永远阻塞；必须 `Connection: close`；
//! - chromium DevTools HTTP 端点**只认 Content-Length 框架**——响应没有
//!   Content-Length = 诚实报错（不做 chunked/EOF 猜测）；
//! - 响应头增量读上限 64KiB，防异常服务器无限头。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

/// DevToolsActivePort 文件名（profile 根目录；首行端口、次行浏览器级 WS path）。
pub const DEVTOOLS_PORT_FILE: &str = "DevToolsActivePort";

/// spawn 后等待 DevToolsActivePort 出现的墙钟上限（秒）。
pub const SPAWN_READY_SECS: u64 = 120;
/// 端口文件出现后等 HTTP 端点就绪的上限（秒）。
pub const ENDPOINT_READY_SECS: u64 = 30;
/// 单条 CDP 命令默认超时。
pub const CDP_CMD_TIMEOUT: Duration = Duration::from_secs(30);
/// 单次 CDP HTTP 请求超时。
pub const CDP_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// DevToolsActivePort 解析
// ---------------------------------------------------------------------------

/// 读 profile 目录下的 DevToolsActivePort，返回 (端口, 浏览器级 WS path)。
///
/// `--remote-debugging-port=0` 时浏览器自选随机端口并写本文件——bot 读文件
/// 拿端口（Playwright/Puppeteer 同款），避免固定 9222 撞车。文件缺席/残缺 =
/// None（调用方继续轮询或超时报错）。
pub fn read_devtools_active_port(profile_dir: &Path) -> Option<(u16, String)> {
    let path = profile_dir.join(DEVTOOLS_PORT_FILE);
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let ws_path = lines
        .next()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if port == 0 {
        return None;
    }
    Some((port, ws_path))
}

// ---------------------------------------------------------------------------
// HTTP/1.1 发现端点（回环裸 TCP，教训见模块注释）
// ---------------------------------------------------------------------------

/// 响应头增量读上限（防异常服务器无限头）。
const HEADER_CAP: usize = 64 * 1024;

/// 解析状态行，返回 (状态码, 原因短语)。
fn parse_status_line(head: &str) -> Result<(u16, String), String> {
    let first = head.lines().next().unwrap_or("");
    // "HTTP/1.1 200 OK"
    let mut parts = first.split_whitespace();
    let _ver = parts.next().ok_or("空响应")?;
    let code: u16 = parts
        .next()
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("状态行缺状态码: {first}"))?;
    let reason = parts.collect::<Vec<_>>().join(" ");
    Ok((code, reason))
}

/// 从响应头文本提取 Content-Length（缺席 = Err——chromium DevTools 端点恒带；
/// 缺席即异常，诚实报错不做 EOF 猜测）。
fn extract_content_length(head: &str) -> Result<usize, String> {
    for line in head.lines() {
        if let Some(rest) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            let v: usize = rest.trim().parse().map_err(|_| "Content-Length 非数字")?;
            return Ok(v);
        }
    }
    Err("响应缺 Content-Length（chromium DevTools 端点恒带；缺席即异常）".to_string())
}

/// 响应体预分配上限（F43，2026-11-16 八审）：`vec![0u8; content_len]` 按
/// 服务器谎报的 Content-Length 一次性分配——正常端点是自 spawn 的本机
/// 浏览器（回环，响应 KB 级），上限只防异常端点巨量分配（/json/list 在
/// 上百 tab 场景也就数百 KB，16MiB 是 50 倍余量）。
const HTTP_BODY_CAP: usize = 16 * 1024 * 1024;

/// 回环裸 TCP 单次 HTTP 请求（HTTP/1.1 + Connection: close + Content-Length
/// 框架读——三条 guest 教训的落地形态）。`body` 非空时按 Content-Length 发送。
async fn cdp_http_raw(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<String, String> {
    let addr = format!("127.0.0.1:{port}");
    let fut = async {
        let mut stream = TcpStream::connect(&addr)
            .await
            .map_err(|e| format!("connect {addr}: {e}"))?;
        let body_bytes = body.unwrap_or("").as_bytes();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            body_bytes.len()
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, req.as_bytes())
            .await
            .map_err(|e| format!("write: {e}"))?;
        if !body_bytes.is_empty() {
            tokio::io::AsyncWriteExt::write_all(&mut stream, body_bytes)
                .await
                .map_err(|e| format!("write body: {e}"))?;
        }

        // 响应头增量读至 \r\n\r\n（64KiB 上限）。
        use tokio::io::{AsyncReadExt, BufReader};
        let mut reader = BufReader::new(stream);
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = reader
                .read(&mut byte)
                .await
                .map_err(|e| format!("read header: {e}"))?;
            if n == 0 {
                return Err("连接在响应头结束前关闭".to_string());
            }
            head.push(byte[0]);
            if head.len() > HEADER_CAP {
                return Err("响应头超 64KiB 上限".to_string());
            }
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let head_str = String::from_utf8_lossy(&head).to_string();
        let (code, reason) = parse_status_line(&head_str)?;
        let content_len = extract_content_length(&head_str)?;
        if content_len > HTTP_BODY_CAP {
            return Err(format!(
                "Content-Length {content_len} 超 {HTTP_BODY_CAP} 上限（端点异常，拒绝分配）"
            ));
        }
        let mut body_buf = vec![0u8; content_len];
        reader
            .read_exact(&mut body_buf)
            .await
            .map_err(|e| format!("read body ({content_len}B): {e}"))?;
        let body = String::from_utf8_lossy(&body_buf).to_string();
        if (200..300).contains(&code) {
            Ok(body)
        } else {
            // 保留原因短语 + 截断 body——诚实错误信息带现场。
            let body_head: String = body.chars().take(200).collect();
            Err(format!("HTTP {code} {reason}: {body_head}"))
        }
    };
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| format!("cdp http 超时（{method} {path}，{}s）", timeout.as_secs()))?
}

/// GET /json/version，返回 JSON。同时也是进程存活探测端点。
pub async fn http_version(port: u16) -> Result<Value, String> {
    let body = cdp_http_raw(port, "GET", "/json/version", None, CDP_HTTP_TIMEOUT).await?;
    serde_json::from_str(&body).map_err(|e| format!("/json/version 非法 JSON: {e}"))
}

/// GET /json/list，返回全部 target（page/iframe/...）。
pub async fn http_list(port: u16) -> Result<Value, String> {
    let body = cdp_http_raw(port, "GET", "/json/list", None, CDP_HTTP_TIMEOUT).await?;
    serde_json::from_str(&body).map_err(|e| format!("/json/list 非法 JSON: {e}"))
}

/// 轮询 /json/version 直到就绪或超时（ENDPOINT_READY_SECS）。
pub async fn wait_for_endpoint(port: u16) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(ENDPOINT_READY_SECS);
    loop {
        match http_version(port).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!(
                        "DevTools 端点未在 {}s 内就绪: {e}",
                        ENDPOINT_READY_SECS
                    ));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 常驻 WS 命令通道（多路复用）
// ---------------------------------------------------------------------------

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 目标（tab）信息快照。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TargetInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub target_type: String,
    pub title: String,
    pub url: String,
    pub attached: bool,
}

/// 从 /json/list JSON 数组投影 TargetInfo 列表（只留 type=page）。
pub fn page_targets(list: &Value) -> Vec<TargetInfo> {
    list.as_array()
        .map(|arr| {
            arr.iter()
                .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
                .map(|t| TargetInfo {
                    id: t
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    target_type: "page".to_string(),
                    title: t
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    url: t
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    attached: t.get("attached").and_then(|v| v.as_bool()).unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 常驻 WS 命令通道：id 多路复用 + 事件订阅。
///
/// - `call()`：发命令 → 按 id 匹配响应（并发安全，pending 表 O(1)）；
/// - `subscribe()`：事件帧（无 id 的 method 帧）广播给订阅者
///   `(method, params)`；
/// - Drop 时发 shutdown 信号，reader task 退出（连接关闭，不泄漏 task/socket）。
pub struct CdpConn {
    sink: tokio::sync::Mutex<
        futures::stream::SplitSink<WsStream, tokio_tungstenite::tungstenite::Message>,
    >,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>,
    events_tx: tokio::sync::broadcast::Sender<(String, Value)>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl CdpConn {
    /// 连接 `ws://127.0.0.1:{port}{path}`（浏览器级或页面级端点均可）。
    /// 握手 15s 有界——connect_async 无内建超时，TCP 半开（浏览器挂起/安全
    /// 软件吞包）会永久挂死调用链，这是整条 act 链路唯一不受界的 await。
    pub async fn connect(port: u16, ws_path: &str) -> Result<Arc<CdpConn>, String> {
        const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
        let url = format!("ws://127.0.0.1:{port}{ws_path}");
        let (stream, _resp) = tokio::time::timeout(WS_CONNECT_TIMEOUT, connect_async(&url))
            .await
            .map_err(|_| format!("ws 连接超时（{}s）：{url}", WS_CONNECT_TIMEOUT.as_secs()))?
            .map_err(|e| format!("ws connect {url}: {e}"))?;
        let (sink, mut stream_half) = stream.split();
        let (events_tx, _) = tokio::sync::broadcast::channel(256);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // reader task：响应帧按 id 路由到 pending oneshot；事件帧广播。
        let pending_reader = pending.clone();
        let events_reader = events_tx.clone();
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    m = stream_half.next() => m,
                    _ = shutdown_rx.changed() => break,
                };
                let Some(Ok(msg)) = msg else { break };
                use tokio_tungstenite::tungstenite::Message as WsMessage;
                let WsMessage::Text(txt) = msg else { continue };
                let Ok(v) = serde_json::from_str::<Value>(txt.as_str()) else {
                    continue;
                };
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    if let Some(tx) = pending_reader.lock().remove(&id) {
                        if let Some(err) = v.get("error") {
                            let message = err
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("cdp error")
                                .to_string();
                            let _ = tx.send(Err(message));
                        } else {
                            let _ = tx.send(Ok(v.get("result").cloned().unwrap_or(Value::Null)));
                        }
                    }
                } else if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
                    let params = v.get("params").cloned().unwrap_or(Value::Null);
                    let _ = events_reader.send((method.to_string(), params));
                }
            }
            // 连接关闭：所有 pending 诚实失败（调用方 recv 得 Err 即知断线）。
            let mut map = pending_reader.lock();
            for (_, tx) in map.drain() {
                let _ = tx.send(Err("cdp connection closed".to_string()));
            }
        });

        Ok(Arc::new(CdpConn {
            sink: tokio::sync::Mutex::new(sink),
            next_id: AtomicU64::new(0),
            pending,
            events_tx,
            shutdown_tx,
        }))
    }

    /// 发送一条 CDP 命令并等响应（id 多路复用，超时诚实报错）。
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.call_in_session(None, method, params, timeout).await
    }

    /// 带可选 flatten 子会话发送（DOM 只读动作 frame 路由，计划
    /// 2026-11-18 §3.7）：`session = Some(sid)` 时消息体注入 `sessionId`
    /// 字段——flatten 模式下子 target 会话与浏览器级会话走同一条 WS
    /// socket，响应仍按 id 匹配（子会话响应带同 id），多路复用天然兼容。
    pub async fn call_in_session(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let mut frame = serde_json::json!({"id": id, "method": method, "params": params});
        if let Some(sid) = session {
            frame["sessionId"] = Value::String(sid.to_string());
        }
        {
            let mut sink = self.sink.lock().await;
            if let Err(e) = sink
                .send(tokio_tungstenite::tungstenite::Message::text(
                    frame.to_string(),
                ))
                .await
            {
                self.pending.lock().remove(&id);
                return Err(format!("cdp send {method}: {e}"));
            }
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_recv_dropped)) => Err(format!("cdp {method}: connection dropped")),
            Err(_elapsed) => {
                self.pending.lock().remove(&id);
                Err(format!("cdp {method}: timeout ({}s)", timeout.as_secs()))
            }
        }
    }

    /// 订阅事件帧广播。
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<(String, Value)> {
        self.events_tx.subscribe()
    }
}

impl Drop for CdpConn {
    fn drop(&mut self) {
        // watch::send 是同步的——Drop 里安全；reader task 见信号即退出。
        let _ = self.shutdown_tx.send(true);
    }
}

/// JS 字符串字面量安全嵌入（注入安全：JSON 编码字符串体，再包引号）。
pub fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(test)]
mod tests;
