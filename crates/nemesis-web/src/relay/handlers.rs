//! 反向桥 HTTP/WS handlers（一期批次一）。
//!
//! 路由（WebServer build_router 在 `set_relay` 注入后挂载）：
//! - `/bridge`：桥设备接入 WS 端点（hello 帧 token 校验，token 不走 URL query）
//! - `/d/{node_id}/{*rest}`：设备面板隧道（先查授权会话 cookie——每设备
//!   path 限定独立授权；无会话 302 到 `__auth`；WS 升级请求转长泵，普通
//!   请求转短 conn）
//! - `/d/{node_id}/__auth`：设备访问令牌输入页（带外小页面，不走隧道；
//!   服务端自处理）
//! - `/relay` + `/relay/login`：极简状态页（ws token 门：未通过不可见
//!   任何设备信息）
//! - `/api/relay/status`：设备列表 JSON（状态页与批次三【中继通道】页同源）
//!
//! 帧语义见 `protocol.rs`；状态机见 `server.rs`；WS 帧转换见 `ws_codec.rs`。

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Request};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use super::protocol::{
    BridgeFrame, ChunkedDecoder, is_chunked, parse_http_response_head, serialize_http_request,
};
use super::server::{
    DEVICE_OUTBOUND_CAPACITY, RelayServer, decode_or_warn, encode_or_none, handle_control_frame,
};
use super::ws_codec::{encode_client_frame, parse_server_frame};

/// hello 握手等待超时（连上不发 hello 的连接是探测/扫描，尽快释放）。
const HELLO_TIMEOUT_SECS: u64 = 10;

/// 响应头等待超时（浏览器请求 → 设备响应的首包时限；含隧道链路往返）。
const RESPONSE_HEAD_TIMEOUT_SECS: u64 = 30;

/// HTTP 隧道请求 body 上限（dashboard 图片上传 25MB + 富余）。
const TUNNEL_BODY_LIMIT: usize = 256 * 1024 * 1024;

/// 授权会话 cookie 名（path 限定 `/d/<node_id>/` 由 Set-Cookie 控制——
/// 同名 cookie 按路径隔离，每设备独立授权）。
pub const AUTH_COOKIE: &str = "nemesis_bridge_auth";

/// 状态页管理 cookie 名（值为 ws token 的 SHA-256 hex；path=/ 覆盖
/// `/relay` 与 `/api/relay/*`）。
pub const ADMIN_COOKIE: &str = "nemesis_relay_admin";

// ---------------------------------------------------------------------------
// /bridge：桥设备接入
// ---------------------------------------------------------------------------

/// `/bridge` WS 升级入口。
pub async fn handle_bridge_ws(ws: WebSocketUpgrade, relay: Arc<RelayServer>) -> Response {
    // 惰性拉起维护循环（幂等）。
    relay.ensure_maintenance();
    ws.on_upgrade(move |socket| bridge_socket_loop(socket, relay))
}

/// 桥连接主循环：hello 握手 →（welcome 回执）→ 双向泵（下行帧写出 +
/// 上行帧分发），直至任一方向关闭 / 被顶替 / 被踢。
async fn bridge_socket_loop(socket: WebSocket, relay: Arc<RelayServer>) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // ---- hello 握手（10s）----
    let hello = match tokio::time::timeout(Duration::from_secs(HELLO_TIMEOUT_SECS), async {
        loop {
            match ws_rx.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Some(frame) = decode_or_warn(&text)
                        && matches!(frame, BridgeFrame::BridgeHello { .. })
                    {
                        return Some(frame);
                    }
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => {
                    tracing::warn!("[Relay] /bridge 握手期间 ws 错误：{e}");
                    return None;
                }
                None => return None,
            }
        }
    })
    .await
    {
        Ok(Some(frame)) => frame,
        _ => {
            tracing::debug!("[Relay] /bridge 连接未在 {HELLO_TIMEOUT_SECS}s 内完成 hello，关闭");
            return;
        }
    };
    let BridgeFrame::BridgeHello {
        token,
        node_id,
        name,
        version,
        cluster_node_id,
        cluster_name,
        role,
        category,
        tags,
        capabilities,
        node_type,
        rpc_port,
        addresses,
    } = hello
    else {
        return; // unreachable：上面只放行 hello
    };

    // 二期：hello 集群身份字段 → 组装快照（集群 node_id 与显示名齐备才
    // 视为启用；老版本/纯隧道设备缺字段 → None，仅作隧道设备）。
    let cluster_identity = match (&cluster_node_id, &cluster_name) {
        (Some(cn), Some(cname)) if !cn.is_empty() => Some(super::identity::BridgeClusterIdentity {
            node_id: cn.clone(),
            name: cname.clone(),
            role: role.clone().unwrap_or_else(|| "worker".to_string()),
            category: category.clone().unwrap_or_else(|| "general".to_string()),
            tags: tags.clone().unwrap_or_default(),
            capabilities: capabilities.clone().unwrap_or_default(),
            node_type: node_type.clone().unwrap_or_else(|| "node".to_string()),
            rpc_port: rpc_port.unwrap_or(0),
            addresses: addresses.clone().unwrap_or_default(),
        }),
        _ => None,
    };

    // ---- 鉴权登记 ----
    let (out_tx, mut out_rx) = mpsc::channel::<BridgeFrame>(DEVICE_OUTBOUND_CAPACITY);
    let generation = match relay.authenticate_device(
        &token,
        &node_id,
        &name,
        &version,
        cluster_identity,
        out_tx,
    ) {
        Ok(g) => g,
        Err(reason) => {
            // 诚实拒绝：welcome{ok:false} + 关连接（客户端据此区分
            // 「配对失败」与「网络断」）。
            let welcome = BridgeFrame::BridgeWelcome {
                ok: false,
                reason: reason.clone(),
                hub_node_id: relay.hub_node_id(),
            };
            if let Some(text) = encode_or_none(&welcome) {
                let _ = ws_tx.send(Message::Text(text.into())).await;
            }
            tracing::warn!(node_id = %node_id, "[Relay] 桥接入被拒：{reason}");
            return;
        }
    };
    let welcome = BridgeFrame::BridgeWelcome {
        ok: true,
        reason: "welcome".to_string(),
        hub_node_id: relay.hub_node_id(),
    };
    if let Some(text) = encode_or_none(&welcome) {
        let _ = ws_tx.send(Message::Text(text.into())).await;
    }

    // ---- 双向泵 ----
    loop {
        if !relay.generation_valid(&node_id, generation) {
            tracing::debug!(node_id = %node_id, "[Relay] 本连接已被新连接顶替，退出");
            break;
        }
        tokio::select! {
            // 下行：服务端 → 设备（ ConnOpen/ConnData/AccessCheck/Pong/... ）
            maybe_frame = out_rx.recv() => {
                match maybe_frame {
                    Some(frame) => {
                        match encode_or_none(&frame) {
                            Some(text) => {
                                if ws_tx.send(Message::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    // 下行队列 sender 全部 drop：被顶替 / 被踢 / 开关关闭。
                    None => break,
                }
            }
            // 上行：设备 → 服务端
            incoming = ws_rx.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Some(frame) = decode_or_warn(&text) else { continue };
                        match frame {
                            BridgeFrame::BridgeHello { .. } => {
                                tracing::warn!(node_id = %node_id,
                                    "[Relay] 握手后收到重复 hello，忽略");
                            }
                            BridgeFrame::BridgeClose { reason } => {
                                tracing::info!(node_id = %node_id,
                                    "[Relay] 设备主动关闭桥连接：{reason}");
                                break;
                            }
                            BridgeFrame::ConnData { conn_id, seq: _, data_b64, fin } => {
                                use base64::Engine as _;
                                let data = match base64::engine::general_purpose::STANDARD
                                    .decode(&data_b64)
                                {
                                    Ok(d) => d,
                                    Err(e) => {
                                        tracing::warn!(node_id = %node_id, conn_id,
                                            "[Relay] ConnData base64 解码失败：{e}");
                                        relay.close_conn(conn_id, "bad base64");
                                        continue;
                                    }
                                };
                                let len = data.len() as u64;
                                if fin && data.is_empty() {
                                    // 响应 EOF 约定：空载荷 + fin = 本地连接
                                    // 已读完（EOF）→ 正常结束该 conn。
                                    relay.close_conn(conn_id, "eof");
                                } else {
                                    relay.touch_device(&node_id, len);
                                    if !relay.route_conn_data(conn_id, data) {
                                        // conn 已关闭/未登记——通知设备停发。
                                        let _ = relay.send_to_device(
                                            &node_id,
                                            BridgeFrame::ConnClose {
                                                conn_id,
                                                reason: "conn not found".to_string(),
                                            },
                                            0,
                                        );
                                    }
                                }
                            }
                            BridgeFrame::ConnOpen { conn_id, .. } => {
                                // ConnOpen 是服务端 → 设备方向；设备发它属于
                                // 协议错乱，防御性忽略。
                                tracing::warn!(node_id = %node_id, conn_id,
                                    "[Relay] 设备发来 ConnOpen（协议错乱），忽略");
                            }
                            BridgeFrame::ConnClose { conn_id, reason } => {
                                // 设备侧主动关闭（dial 失败 / 本地断开等）。
                                relay.close_conn(conn_id, &reason);
                            }
                            BridgeFrame::ClusterRpc { payload } => {
                                // 二期批次六：上行集群 RPC 帧 → 宿主出口
                                // （hub 形态：喂本地 RPC 链，响应封帧回上行）。
                                // 未装配 sink（`--relay` / 一期形态）→ 维持
                                // 一期「WARN 忽略」语义。
                                match relay.cluster_frame_sink() {
                                    Some(sink) => {
                                        if let Some(resp_payload) =
                                            sink.on_cluster_frame(&node_id, payload).await
                                        {
                                            let _ = relay.send_to_device(
                                                &node_id,
                                                BridgeFrame::ClusterRpc { payload: resp_payload },
                                                0,
                                            );
                                        }
                                    }
                                    None => {
                                        tracing::warn!(node_id = %node_id,
                                            "[Relay] 收到 cluster_rpc 帧但未装配集群帧出口（纯中继模式），忽略");
                                    }
                                }
                            }
                            other => handle_control_frame(other, &relay, &node_id),
                        }
                    }
                    Some(Ok(_)) => {} // Binary/Ping/Pong 由底层处理
                    Some(Err(e)) => {
                        tracing::warn!(node_id = %node_id, "[Relay] 桥连接 ws 错误：{e}");
                        break;
                    }
                    None => break, // ws 关闭
                }
            }
        }
    }
    relay.mark_device_gone(&node_id, generation);
}

// ---------------------------------------------------------------------------
// /d/{node_id}/：设备面板隧道
// ---------------------------------------------------------------------------

/// `/d/{node_id}/{*rest}` 入口（任意方法）。
pub async fn handle_device_request(
    relay: Arc<RelayServer>,
    node_id: String,
    req: Request,
) -> Response {
    // 门关 → 404（不暴露桥功能存在）。
    if !relay.is_gate_open() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    // 设备在线？
    if !device_online(&relay, &node_id) {
        return simple_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "设备离线",
            &format!("设备 {node_id} 当前不在线（未桥入或心跳超时）。请稍后重试。"),
        );
    }
    // 授权会话？（cookie path 限定本设备前缀；未授权先输设备访问令牌）
    let cookie = extract_cookie(req.headers(), AUTH_COOKIE);
    let authorized = cookie
        .as_deref()
        .map(|c| relay.session_valid(c, &node_id))
        .unwrap_or(false);
    if !authorized {
        return Redirect::to(&format!("/d/{node_id}/__auth")).into_response();
    }
    // WS 升级请求 → 长泵分支；否则短 conn 分支。
    if is_websocket_upgrade(req.headers()) {
        handle_ws_tunnel(relay, node_id, req).await
    } else {
        handle_http_tunnel(relay, node_id, req).await
    }
}

fn device_online(relay: &RelayServer, node_id: &str) -> bool {
    relay.device_online(node_id)
}

/// 通知设备收口某条 conn（服务端已完成/放弃该 conn）。设备侧 drop_conn
/// no-op 幂等：conn 不在表（已自行收口）时静默。设备不在线时尽力而为
/// （发送失败直接忽略——conn 即将随设备断开一并消亡）。
fn notify_device_conn_closed(relay: &RelayServer, node_id: &str, conn_id: u64, reason: &str) {
    let _ = relay.send_to_device(
        node_id,
        BridgeFrame::ConnClose {
            conn_id,
            reason: reason.to_string(),
        },
        0,
    );
}

/// HTTP 短 conn：请求字节整段下发（fin=true），响应头解析 + body 流式回传。
async fn handle_http_tunnel(relay: Arc<RelayServer>, node_id: String, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, TUNNEL_BODY_LIMIT).await {
        Ok(b) => b,
        Err(e) => {
            return simple_page(StatusCode::BAD_REQUEST, "请求体读取失败", &format!("{e}"));
        }
    };

    let conn_id = relay.alloc_conn_id();
    // conn 登记的 generation 恒 0（register_conn 第三个参数）：设备重连
    // 顶替后旧桥连接关闭 → 设备侧 conn_close/EOF 自然收口，无需在 conn
    // 表按代际过滤。

    let req_bytes = serialize_http_request(&parts.method, &parts.uri, &parts.headers, &body_bytes);
    use base64::Engine as _;
    let req_b64 = base64::engine::general_purpose::STANDARD.encode(&req_bytes);

    if !relay.send_to_device(
        &node_id,
        BridgeFrame::ConnOpen {
            conn_id,
            target: "local".to_string(),
        },
        0,
    ) || !relay.send_to_device(
        &node_id,
        BridgeFrame::ConnData {
            conn_id,
            seq: 0,
            data_b64: req_b64,
            fin: true,
        },
        req_bytes.len() as u64,
    ) {
        relay.close_conn_id(&conn_id);
        return simple_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "设备离线",
            "请求无法送达设备（连接已断开）。请稍后重试。",
        );
    }

    let mut resp_rx = relay.register_conn(conn_id, &node_id, 0);

    // ---- 等响应头 ----
    let mut head_buf: Vec<u8> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(RESPONSE_HEAD_TIMEOUT_SECS);
    let (status, resp_headers, head_len) = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, resp_rx.recv()).await {
            Ok(Some(chunk)) => {
                if chunk.is_empty() {
                    // EOF 且头部未到：设备侧连接失败/立即关闭。
                    relay.close_conn_id(&conn_id);
                    return simple_page(
                        StatusCode::BAD_GATEWAY,
                        "设备响应异常",
                        "设备侧连接已关闭（未返回任何响应）。请查看设备日志。",
                    );
                }
                head_buf.extend_from_slice(&chunk);
                if let Some(head) = parse_http_response_head(&head_buf) {
                    break head;
                }
            }
            Ok(None) => {
                relay.close_conn_id(&conn_id);
                return simple_page(
                    StatusCode::BAD_GATEWAY,
                    "设备响应异常",
                    "设备侧连接已断开。请稍后重试。",
                );
            }
            Err(_) => {
                // 超时放弃：设备侧本机连接还挂着，通知设备收口（防设备侧
                // conn 泵泄漏）。
                notify_device_conn_closed(&relay, &node_id, conn_id, "server head timeout");
                relay.close_conn_id(&conn_id);
                return simple_page(
                    StatusCode::GATEWAY_TIMEOUT,
                    "设备响应超时",
                    &format!("{RESPONSE_HEAD_TIMEOUT_SECS}s 内未收到设备响应头。"),
                );
            }
        }
    };

    // ---- 构造响应 ----
    let chunked = is_chunked(&resp_headers);
    // 非 chunked 响应的 body 收口长度：设备侧不再半关闭写半（2026-09-19
    // 真机实测 shutdown 使 hyper 断连），本机连接 keep-alive 存续 → EOF
    // 兜底不可依赖。有 content-length 按长度收口；无长度头（罕见）仍留
    // EOF 兜底（设备侧读到本机关连接才发空 fin）。
    let content_len: Option<u64> = resp_headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<u64>().ok());
    let mut builder = Response::builder().status(status);
    const SKIP_HEADERS: &[&str] = &[
        "connection",
        "keep-alive",
        "transfer-encoding",
        "content-length", // chunked 时长度未知；非 chunked 由流实际字节数决定
    ];
    for (name, value) in resp_headers {
        let lower = name.to_ascii_lowercase();
        if SKIP_HEADERS.contains(&lower.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }

    let mut body_buf = head_buf.split_off(head_len);
    let relay_for_stream = relay.clone();
    let node_for_stream = node_id.clone();
    // body 流：resp_rx 字节 →（chunked 解块 / 长度计数）→ 浏览器；空 Vec = EOF。
    let stream = async_stream::stream! {
        if chunked {
            let mut decoder = ChunkedDecoder::new();
            // 先吐头部之后的字节前缀。
            let out = decoder.feed(&body_buf);
            if !out.is_empty() { yield Ok::<_, std::io::Error>(axum::body::Bytes::from(out)); }
            body_buf.clear();
            if !decoder.is_finished() {
                loop {
                    match resp_rx.recv().await {
                        Some(chunk) if chunk.is_empty() => break, // EOF
                        Some(chunk) => {
                            let out = decoder.feed(&chunk);
                            if !out.is_empty() {
                                relay_for_stream.touch_device(&node_for_stream, out.len() as u64);
                                yield Ok(axum::body::Bytes::from(out));
                            }
                            if decoder.is_finished() { break; }
                        }
                        None => break,
                    }
                }
            }
        } else {
            // 非 chunked：头后前缀字节（可能与头同帧到达、甚至已含完整
            // body）**必须先吐**——此前版本在此处丢过 body（2026-09-19
            // 测试暴露：remaining=0 时跳过 yield，缓冲里的 body 从未下发）。
            let mut served: u64 = 0;
            if !body_buf.is_empty() {
                served = body_buf.len() as u64;
                relay_for_stream.touch_device(&node_for_stream, body_buf.len() as u64);
                yield Ok(axum::body::Bytes::from(std::mem::take(&mut body_buf)));
            }
            // 长度未满才等更多（usize::MAX 哨兵 = 无 content-length 头，
            // 靠 EOF 兜底收口——设备读到本机关连接才发空 fin）。
            let total = content_len.map_or(usize::MAX as u64, |t| t);
            while served < total {
                match resp_rx.recv().await {
                    Some(chunk) if chunk.is_empty() => break, // EOF 兜底
                    Some(chunk) => {
                        relay_for_stream.touch_device(&node_for_stream, chunk.len() as u64);
                        served += chunk.len() as u64;
                        if served > total {
                            // 超出 content-length 的字节截断丢弃（畸形设备
                            // 响应防御；正常路径 served == total 精确收口）。
                            let overflow = (served - total) as usize;
                            let keep = chunk.len() - overflow;
                            if keep > 0 {
                                yield Ok(axum::body::Bytes::from(chunk[..keep].to_vec()));
                            }
                        } else {
                            yield Ok(axum::body::Bytes::from(chunk));
                        }
                        if served >= total {
                            break; // 长度收口：body 读满即完成
                        }
                    }
                    None => break,
                }
            }
        }
        // 收口：通知设备关 conn（设备侧 drop_conn：abort 读泵 + 关写泵，
        // 本机 keep-alive 连接随之关闭）+ 清登记。
        notify_device_conn_closed(&relay_for_stream, &node_for_stream, conn_id, "response complete");
        relay_for_stream.close_conn_id(&conn_id);
    };

    // 204/304 与 HEAD 响应无 body——hyper 不会消费 body 流（stream 不被
    // poll，尾部收口代码永不执行）。此类响应在构造时同步收口。
    let bodyless = matches!(status, StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED)
        || parts.method == http::Method::HEAD;
    if bodyless {
        notify_device_conn_closed(&relay, &node_id, conn_id, "response complete");
        relay.close_conn_id(&conn_id);
        return match builder.body(Body::empty()) {
            Ok(resp) => resp,
            Err(e) => simple_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "响应构造失败",
                &format!("{e}"),
            ),
        };
    }

    match builder.body(Body::from_stream(stream)) {
        Ok(resp) => resp,
        Err(e) => simple_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "响应构造失败",
            &format!("{e}"),
        ),
    }
}

/// WS 长泵：升级请求原样下发 → 101 后浏览器 socket ↔ 设备 conn 双向搬运。
async fn handle_ws_tunnel(relay: Arc<RelayServer>, node_id: String, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, TUNNEL_BODY_LIMIT).await {
        Ok(b) => b,
        Err(e) => {
            return simple_page(StatusCode::BAD_REQUEST, "请求体读取失败", &format!("{e}"));
        }
    };

    let conn_id = relay.alloc_conn_id();
    let req_bytes = serialize_http_request(&parts.method, &parts.uri, &parts.headers, &body_bytes);
    use base64::Engine as _;
    let req_b64 = base64::engine::general_purpose::STANDARD.encode(&req_bytes);

    if !relay.send_to_device(
        &node_id,
        BridgeFrame::ConnOpen {
            conn_id,
            target: "local".to_string(),
        },
        0,
    ) || !relay.send_to_device(
        &node_id,
        BridgeFrame::ConnData {
            conn_id,
            seq: 0,
            data_b64: req_b64,
            fin: true,
        },
        req_bytes.len() as u64,
    ) {
        relay.close_conn_id(&conn_id);
        return simple_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "设备离线",
            "请求无法送达设备（连接已断开）。请稍后重试。",
        );
    }

    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upg) => upg,
        Err(_) => {
            relay.close_conn_id(&conn_id);
            return simple_page(StatusCode::BAD_REQUEST, "无效的 WS 升级请求", "");
        }
    };

    upgrade.on_upgrade(move |socket| async move {
        let (mut ws_tx, mut ws_rx) = socket.split();
        let mut resp_rx = relay.register_conn(conn_id, &node_id, 0);

        // ---- 等设备侧升级响应头（30s）----
        let mut head_buf: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(RESPONSE_HEAD_TIMEOUT_SECS);
        // (升级是否成功, 头部字节数)——头部之后若还有字节，属于首个 ws 帧
        // 的前缀，进泵前先喂。
        let mut pending_after_head: Vec<u8> = Vec::new();
        let upgrade_ok = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, resp_rx.recv()).await {
                Ok(Some(chunk)) => {
                    if chunk.is_empty() {
                        break false; // 设备 EOF
                    }
                    head_buf.extend_from_slice(&chunk);
                    if let Some((status, _, head_len)) = parse_http_response_head(&head_buf) {
                        // 101 = 升级成功；其余（404/401/...）= 设备侧拒绝。
                        // 浏览器侧 101 已回（axum upgrade 已发生），只能以
                        // close 帧诚实告知失败。
                        pending_after_head = head_buf.split_off(head_len);
                        break status == StatusCode::SWITCHING_PROTOCOLS;
                    }
                }
                Ok(None) => break false,
                Err(_) => break false,
            }
        };
        if !upgrade_ok {
            let _ = ws_tx.send(Message::Close(None)).await;
            // 设备侧本机连接可能还挂着（超时/表项丢失路径），通知收口
            // （EOF 路径设备已自知，drop_conn 幂等，重复通知无害）。
            notify_device_conn_closed(&relay, &node_id, conn_id, "upgrade failed");
            relay.close_conn_id(&conn_id);
            return;
        }

        // ---- 双向长泵 ----
        let mut down_seq: u64 = 0; // 浏览器 → 设备帧序号
        // 先把 101 头之后的残余字节（首个 ws 帧前缀）写给浏览器。
        let mut up_buf = std::mem::take(&mut pending_after_head);
        loop {
            // 上行缓冲先解帧发送（每轮前置处理；ws 帧可能跨 chunk 边界，
            // 解不掉的残余留在缓冲等下一块拼接）。
            if !up_buf.is_empty() {
                let mut consumed_total = 0usize;
                let mut send_failed = false;
                {
                    let mut buf: &[u8] = &up_buf;
                    while !buf.is_empty() {
                        match parse_server_frame(buf) {
                            Some((msg, consumed)) => {
                                buf = &buf[consumed..];
                                consumed_total += consumed;
                                if ws_tx.send(msg).await.is_err() {
                                    send_failed = true;
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
                up_buf.drain(..consumed_total);
                if send_failed {
                    break;
                }
            }
            tokio::select! {
                incoming = ws_rx.next() => {
                    match incoming {
                        Some(Ok(msg)) => {
                            let frame_bytes = encode_client_frame(&msg);
                            use base64::Engine as _;
                            let b64 = base64::engine::general_purpose::STANDARD
                                .encode(&frame_bytes);
                            down_seq += 1;
                            if !relay.send_to_device(
                                &node_id,
                                BridgeFrame::ConnData {
                                    conn_id,
                                    seq: down_seq,
                                    data_b64: b64,
                                    fin: false,
                                },
                                frame_bytes.len() as u64,
                            ) {
                                break;
                            }
                            if matches!(msg, Message::Close(_)) {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                resp = resp_rx.recv() => {
                    match resp {
                        Some(chunk) if chunk.is_empty() => break, // 设备 EOF
                        Some(chunk) => {
                            // 追加进缓冲，下一轮循环体开头统一解帧
                            // （ws 帧可能跨 chunk 边界）。
                            up_buf.extend_from_slice(&chunk);
                        }
                        None => break,
                    }
                }
            }
        }
        // 收口：通知设备关 conn + 清登记。
        let _ = relay.send_to_device(
            &node_id,
            BridgeFrame::ConnClose {
                conn_id,
                reason: "tunnel closed".to_string(),
            },
            0,
        );
        relay.close_conn_id(&conn_id);
    })
}

// ---------------------------------------------------------------------------
// /d/{node_id}/__auth：设备访问令牌输入页（服务端自处理，不走隧道）
// ---------------------------------------------------------------------------

/// GET：输入页。
pub async fn handle_auth_page(relay: Arc<RelayServer>, node_id: String, req: Request) -> Response {
    if !relay.is_gate_open() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    // 已有有效会话 → 直接进面板。
    let cookie = extract_cookie(req.headers(), AUTH_COOKIE);
    if cookie
        .as_deref()
        .map(|c| relay.session_valid(c, &node_id))
        .unwrap_or(false)
    {
        return Redirect::to(&format!("/d/{node_id}/")).into_response();
    }
    let device_name = relay
        .list_devices()
        .iter()
        .find(|d| d.node_id == node_id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| node_id.clone());
    let offline = !device_online(&relay, &node_id);
    let html = auth_page_html(&device_name, offline);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

/// POST：校验（浏览器端已 SHA-256；服务端转发哈希，零存储原文）。
pub async fn handle_auth_submit(
    relay: Arc<RelayServer>,
    node_id: String,
    req: Request,
) -> Response {
    if !relay.is_gate_open() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    #[derive(serde::Deserialize)]
    struct AuthBody {
        hash: String,
    }
    let (_, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, 4 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return auth_json_response(false, "请求体过大", None, None);
        }
    };
    let parsed: AuthBody = match serde_json::from_slice(&body_bytes) {
        Ok(p) => p,
        Err(_) => {
            return auth_json_response(false, "请求格式错误", None, None);
        }
    };
    // 哈希形态防御（64 位十六进制）。
    if parsed.hash.len() != 64 || !parsed.hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return auth_json_response(false, "令牌哈希格式错误", None, None);
    }
    match relay.access_check(&node_id, &parsed.hash).await {
        Err(reason) => auth_json_response(false, &reason, None, None),
        Ok(true) => {
            let cookie_value = relay.create_auth_session(&node_id);
            auth_json_response(
                true,
                "",
                Some(&cookie_value),
                Some(&format!("/d/{node_id}/")),
            )
        }
        Ok(false) => auth_json_response(false, "令牌错误", None, None),
    }
}

/// auth POST 的 JSON 回复（ok=true 时带 Set-Cookie）。
fn auth_json_response(
    ok: bool,
    reason: &str,
    cookie: Option<&str>,
    redirect: Option<&str>,
) -> Response {
    let mut resp = axum::Json(serde_json::json!({
        "ok": ok,
        "reason": reason,
        "redirect": redirect,
    }))
    .into_response();
    if let Some(value) = cookie {
        // path 限定本设备前缀：同名 cookie 按路径隔离，每设备独立授权。
        let path = redirect.map(|r| {
            // /d/<node_id>/ → /d/<node_id>
            r.trim_end_matches('/').to_string()
        });
        let set_cookie = format!(
            "{}={}; Path={}; HttpOnly; SameSite=Lax; Max-Age={}",
            AUTH_COOKIE,
            value,
            path.unwrap_or_else(|| "/".to_string()),
            7 * 24 * 3600
        );
        if let Ok(hv) = header::HeaderValue::from_str(&set_cookie) {
            resp.headers_mut().append(header::SET_COOKIE, hv);
        }
    }
    resp
}

/// 浏览器端 SHA-256 兜底脚本（`/d/<id>/__auth` 授权页 + `/relay` 登录页
/// 共用；随页面 `<script>` 注入）。
///
/// `crypto.subtle` 是安全上下文（HTTPS / localhost）专属——经
/// `http://<公网IP>` 访问时为 undefined（2026-09-20 用户真机暴露：输入
/// 令牌报 "Cannot read properties of undefined (reading 'digest')"）。
/// `sha256Hex` 优先走 WebCrypto，非安全上下文自动回落纯 JS 实现
/// （FIPS 180-4，标准 K/H 常量），两者输出一致；`TextEncoder` 非安全
/// 上下文可用，负责 UTF-8 编码。
fn sha256_fallback_js() -> &'static str {
    r#"async function sha256Hex(text) {
  if (window.crypto && crypto.subtle && typeof crypto.subtle.digest === 'function') {
    const d = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
    return Array.from(new Uint8Array(d)).map(b => b.toString(16).padStart(2, '0')).join('');
  }
  return jsSha256Hex(new TextEncoder().encode(text));
}
function jsSha256Hex(bytes) {
  function rr(v, a) { return (v >>> a) | (v << (32 - a)); }
  const K = [0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
             0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
             0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
             0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
             0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
             0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
             0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
             0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2];
  const H0 = [0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19];
  const l = bytes.length;
  const total = Math.ceil((l + 9) / 64) * 64;
  const buf = new Uint8Array(total);
  buf.set(bytes);
  buf[l] = 0x80;
  const dv = new DataView(buf.buffer);
  dv.setUint32(total - 8, Math.floor((l * 8) / 4294967296));
  dv.setUint32(total - 4, (l * 8) >>> 0);
  const w = new Array(64);
  const H = H0.slice();
  for (let off = 0; off < total; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(off + i * 4);
    for (let i = 16; i < 64; i++) {
      const s0 = rr(w[i-15],7) ^ rr(w[i-15],18) ^ (w[i-15] >>> 3);
      const s1 = rr(w[i-2],17) ^ rr(w[i-2],19) ^ (w[i-2] >>> 10);
      w[i] = (w[i-16] + s0 + w[i-7] + s1) | 0;
    }
    let a=H[0],b=H[1],c=H[2],d=H[3],e=H[4],f=H[5],g=H[6],h=H[7];
    for (let i = 0; i < 64; i++) {
      const S1 = rr(e,6) ^ rr(e,11) ^ rr(e,25);
      const ch = (e & f) ^ (~e & g);
      const t1 = (h + S1 + ch + K[i] + w[i]) | 0;
      const S0 = rr(a,2) ^ rr(a,13) ^ rr(a,22);
      const mj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + mj) | 0;
      h=g; g=f; f=e; e=(d+t1)|0; d=c; c=b; b=a; a=(t1+t2)|0;
    }
    H[0]=(H[0]+a)|0; H[1]=(H[1]+b)|0; H[2]=(H[2]+c)|0; H[3]=(H[3]+d)|0;
    H[4]=(H[4]+e)|0; H[5]=(H[5]+f)|0; H[6]=(H[6]+g)|0; H[7]=(H[7]+h)|0;
  }
  return H.map(x => (x >>> 0).toString(16).padStart(8, '0')).join('');
}"#
}

fn auth_page_html(device_name: &str, offline: bool) -> String {
    let offline_note = if offline {
        "<p style=\"color:#c0392b\">⚠ 设备当前不在线，校验将无法完成。</p>"
    } else {
        ""
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>设备访问验证 · {device_name}</title>
<style>
  body {{ font-family: system-ui, sans-serif; background: #f5f6f8; display: flex;
         justify-content: center; align-items: center; min-height: 100vh; margin: 0; }}
  .card {{ background: #fff; border-radius: 12px; box-shadow: 0 2px 12px rgba(0,0,0,.08);
          padding: 32px; width: 340px; }}
  h1 {{ font-size: 18px; margin: 0 0 8px; }}
  p {{ color: #666; font-size: 13px; margin: 0 0 16px; }}
  input {{ width: 100%; box-sizing: border-box; padding: 10px 12px; border: 1px solid #ddd;
          border-radius: 8px; font-size: 14px; margin-bottom: 12px; }}
  button {{ width: 100%; padding: 10px; background: #2563eb; color: #fff; border: 0;
           border-radius: 8px; font-size: 14px; cursor: pointer; }}
  button:disabled {{ background: #9db8f0; }}
  .err {{ color: #c0392b; font-size: 13px; min-height: 18px; margin: 8px 0 0; }}
</style>
</head>
<body>
<div class="card">
  <h1>🔒 访问设备「{device_name}」</h1>
  <p>该设备设置了面板访问令牌（只存设备本机，服务端不保存）。输入令牌继续。</p>
  {offline_note}
  <input id="token" type="password" placeholder="设备访问令牌" autocomplete="current-password">
  <button id="go" onclick="submit()">确 认</button>
  <div class="err" id="err"></div>
</div>
<script>
{sha256_js}
async function submit() {{
  const btn = document.getElementById('go');
  const err = document.getElementById('err');
  const token = document.getElementById('token').value;
  if (!token) {{ err.textContent = '请输入令牌'; return; }}
  btn.disabled = true; err.textContent = '校验中…';
  try {{
    // 浏览器端先 SHA-256：令牌原文不出本机，服务端只见哈希。
    // （sha256Hex 在 http://IP 等非安全上下文自动回落纯 JS 实现。）
    const hex = await sha256Hex(token);
    const resp = await fetch(location.pathname, {{
      method: 'POST',
      headers: {{ 'Content-Type': 'application/json' }},
      body: JSON.stringify({{ hash: hex }})
    }});
    const data = await resp.json();
    if (data.ok && data.redirect) {{
      location.replace(data.redirect);
      return;
    }}
    err.textContent = data.reason || '校验失败';
  }} catch (e) {{
    err.textContent = '网络错误：' + e;
  }}
  btn.disabled = false;
}}
document.getElementById('token').addEventListener('keydown', e => {{
  if (e.key === 'Enter') submit();
}});
</script>
</body>
</html>"#,
        device_name = html_escape(device_name),
        offline_note = offline_note,
        // format! 参数值不再被解释——JS 里的 `{}` 无需转义。
        sha256_js = sha256_fallback_js(),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 无样式统一提示页（隧道错误路径用；不走隧道）。
fn simple_page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        "<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
<title>{}</title></head><body style=\"font-family:system-ui;display:flex;\
justify-content:center;align-items:center;min-height:100vh;color:#444\">\
<div style=\"text-align:center\"><h2>{}</h2><p>{}</p>\
<p><a href=\"/relay\">← 返回中继状态页</a></p></div></body></html>",
        html_escape(title),
        html_escape(title),
        html_escape(body)
    );
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// 从 Cookie 头提取指定 cookie 值。
pub fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((n, v)) = pair.split_once('=')
            && n.trim() == name
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false)
        && headers.contains_key(header::UPGRADE)
}

// ---------------------------------------------------------------------------
// /relay：极简状态页（ws token 门：未通过不可见任何设备信息）
// ---------------------------------------------------------------------------

/// 状态页管理门校验（cookie 值 == ws token SHA-256 hex）。
fn admin_gate_pass(relay: &RelayServer, req_headers: &HeaderMap) -> bool {
    extract_cookie(req_headers, ADMIN_COOKIE)
        .map(|c| c == relay.admin_hash_hex())
        .unwrap_or(false)
}

/// GET /relay：通过门 → 状态页；未通过 → 令牌输入页。
pub async fn handle_relay_status_page(relay: Arc<RelayServer>, req: Request) -> Response {
    if !relay.is_gate_open() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    let (parts, _) = req.into_parts();
    if !admin_gate_pass(&relay, &parts.headers) {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            relay_login_html(),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        relay_status_html(relay.is_full_mode()),
    )
        .into_response()
}

/// POST /relay/login：{hash} 比对 ws token SHA-256 → 发管理 cookie。
pub async fn handle_relay_login(relay: Arc<RelayServer>, req: Request) -> Response {
    if !relay.is_gate_open() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    #[derive(serde::Deserialize)]
    struct LoginBody {
        hash: String,
    }
    let (_, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, 4 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"ok": false, "reason": "请求体过大"})),
            )
                .into_response();
        }
    };
    let ok = serde_json::from_slice::<LoginBody>(&body_bytes)
        .map(|b| b.hash == relay.admin_hash_hex())
        .unwrap_or(false);
    if !ok {
        tracing::warn!("[Relay] 状态页令牌校验失败");
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"ok": false, "reason": "令牌错误"})),
        )
            .into_response();
    }
    let mut resp = axum::Json(serde_json::json!({"ok": true})).into_response();
    let set_cookie = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        ADMIN_COOKIE,
        relay.admin_hash_hex(),
        7 * 24 * 3600
    );
    if let Ok(hv) = header::HeaderValue::from_str(&set_cookie) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    resp
}

/// GET /api/relay/status：设备列表 JSON（状态页轮询 + 批次三通道页同源消费）。
pub async fn handle_relay_api_status(relay: Arc<RelayServer>, req: Request) -> Response {
    if !relay.is_gate_open() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    let (parts, _) = req.into_parts();
    if !admin_gate_pass(&relay, &parts.headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    axum::Json(serde_json::json!({
        "enabled": relay.is_enabled(),
        "full_mode": relay.is_full_mode(),
        "devices": relay.list_devices(),
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// 批次三：通道页【中继通道】HTTP 端点（dashboard 语义——与 /api/status 同
// 信任边界：本机/内网 dashboard 用户；不走状态页的 admin cookie 门，那是
// 「知道 ws token」的带外通道，两者受众不同）。
// ---------------------------------------------------------------------------

/// GET /api/relay/overview：服务端态 + 客户端态一次取全（通道页轮询源）。
///
/// 「数据同源」承诺：服务端 `devices` 与状态页 `/api/relay/status` 一样
/// 直读 [`RelayServer::list_devices`]——同一状态机，两个出口。relay 未
/// 配置时 `server` 诚实回 null（客户端态独立于服务端，总是返回）。
pub async fn handle_relay_api_overview(relay: Option<Arc<RelayServer>>) -> Response {
    let server = relay.map(|r| {
        serde_json::json!({
            "enabled": r.is_enabled(),
            "full_mode": r.is_full_mode(),
            "devices": r.list_devices(),
        })
    });
    let client = super::client_status().map(|s| {
        serde_json::json!({
            "enabled": s.enabled,
            "state": s.state.as_str(),
            "relay_url": s.relay_url,
            "node_id": s.node_id,
            "last_error": s.last_error,
            "updated_at": s.updated_at,
        })
    });
    axum::Json(serde_json::json!({ "server": server, "client": client })).into_response()
}

/// POST /api/relay/enabled：中继服务端运行时开关。body `{"on": bool}`。
///
/// **运行时态不落盘**（goal 裁决）：只改 RelayServer 内存开关，config.json
/// 的 bridge.server 不动——正常启动默认开，重启自然恢复默认。
pub async fn handle_relay_api_enabled(relay: Option<Arc<RelayServer>>, req: Request) -> Response {
    let Some(relay) = relay else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"ok": false, "reason": "中继服务端未配置"})),
        )
            .into_response();
    };
    #[derive(serde::Deserialize)]
    struct EnabledBody {
        on: bool,
    }
    let (_, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, 4 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"ok": false, "reason": "请求体过大"})),
            )
                .into_response();
        }
    };
    let on = match serde_json::from_slice::<EnabledBody>(&body_bytes) {
        Ok(b) => b.on,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"ok": false, "reason": "缺少 on 字段"})),
            )
                .into_response();
        }
    };
    relay.set_enabled(on);
    tracing::info!(
        "[Relay] 通道页开关：中继服务端已{}",
        if on { "开启" } else { "关闭" }
    );
    axum::Json(serde_json::json!({"ok": true, "enabled": relay.is_enabled()})).into_response()
}

/// POST /api/relay/client/reconnect：手动重连桥客户端（踢退避等待 /
/// 断开当前会话立即重连）。客户端未运行时调用无害（permit 留给下次
/// spawn，语义仍是「尽快连接」）。
pub async fn handle_relay_api_client_reconnect() -> Response {
    super::kick_reconnect();
    tracing::info!("[Relay] 通道页手动重连：已通知桥客户端");
    axum::Json(serde_json::json!({"ok": true})).into_response()
}

/// 状态页令牌输入页（JS SHA-256 后 POST /relay/login——与服务端持有值
/// 同哈希比对；token 本身不出现在任何往返中）。
fn relay_login_html() -> String {
    r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>中继状态页 · 令牌验证</title>
<style>
  body { font-family: system-ui, sans-serif; background: #f5f6f8; display: flex;
         justify-content: center; align-items: center; min-height: 100vh; margin: 0; }
  .card { background: #fff; border-radius: 12px; box-shadow: 0 2px 12px rgba(0,0,0,.08);
          padding: 32px; width: 340px; }
  h1 { font-size: 18px; margin: 0 0 8px; }
  p { color: #666; font-size: 13px; margin: 0 0 16px; }
  input { width: 100%; box-sizing: border-box; padding: 10px 12px; border: 1px solid #ddd;
          border-radius: 8px; font-size: 14px; margin-bottom: 12px; }
  button { width: 100%; padding: 10px; background: #2563eb; color: #fff; border: 0;
           border-radius: 8px; font-size: 14px; cursor: pointer; }
  button:disabled { background: #9db8f0; }
  .err { color: #c0392b; font-size: 13px; min-height: 18px; margin: 8px 0 0; }
</style>
</head>
<body>
<div class="card">
  <h1>🔒 中继状态页</h1>
  <p>输入接入门令牌（bridge.server.token）查看桥入设备。</p>
  <input id="token" type="password" placeholder="接入门令牌" autocomplete="current-password">
  <button id="go" onclick="submit()">确 认</button>
  <div class="err" id="err"></div>
</div>
<script>
__SHA256_JS__
async function submit() {
  const btn = document.getElementById('go');
  const err = document.getElementById('err');
  const token = document.getElementById('token').value;
  if (!token) { err.textContent = '请输入令牌'; return; }
  btn.disabled = true; err.textContent = '校验中…';
  try {
    const hex = await sha256Hex(token);
    const resp = await fetch('/relay/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ hash: hex })
    });
    const data = await resp.json();
    if (data.ok) { location.replace('/relay'); return; }
    err.textContent = data.reason || '令牌错误';
  } catch (e) {
    err.textContent = '网络错误：' + e;
  }
  btn.disabled = false;
}
document.getElementById('token').addEventListener('keydown', e => {
  if (e.key === 'Enter') submit();
});
</script>
</body>
</html>"#
        // __SHA256_JS__ 占位（本模板无 format!，用 replace 注入共用脚本）。
        .replace("__SHA256_JS__", sha256_fallback_js())
}

/// 状态页主体（JS 每 5s 轮询 /api/relay/status 渲染设备表）。
fn relay_status_html(full_mode: bool) -> String {
    let dashboard_entry = if full_mode {
        r#"<a class="btn" href="/">打开本机 Dashboard</a>"#
    } else {
        "<span class=\"muted\">纯中继模式（--relay）：无本机 Dashboard</span>"
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>中继状态页</title>
<style>
  body {{ font-family: system-ui, sans-serif; background: #f5f6f8; margin: 0; padding: 24px; }}
  h1 {{ font-size: 20px; }}
  .toolbar {{ margin: 12px 0 20px; display: flex; gap: 12px; align-items: center; }}
  .btn {{ display: inline-block; padding: 8px 14px; background: #2563eb; color: #fff;
         border-radius: 8px; text-decoration: none; font-size: 13px; }}
  table {{ width: 100%; border-collapse: collapse; background: #fff; border-radius: 10px;
          overflow: hidden; box-shadow: 0 1px 6px rgba(0,0,0,.06); }}
  th, td {{ text-align: left; padding: 10px 14px; font-size: 13px;
           border-bottom: 1px solid #eef0f2; }}
  th {{ background: #fafbfc; color: #555; }}
  .dot {{ display: inline-block; width: 8px; height: 8px; border-radius: 50%; }}
  .on {{ background: #22c55e; }} .off {{ background: #d1d5db; }}
  .muted {{ color: #999; font-size: 13px; }}
  .empty {{ text-align: center; color: #999; padding: 24px; }}
</style>
</head>
<body>
<h1>🛰 中继状态页</h1>
<div class="toolbar">{dashboard_entry}<span class="muted" id="updated"></span></div>
<table>
<thead><tr><th>状态</th><th>node_id</th><th>显示名</th><th>版本</th>
<th>接入时间</th><th>↑ 请求</th><th>↓ 响应</th><th></th></tr></thead>
<tbody id="rows"><tr><td colspan="8" class="empty">加载中…</td></tr></tbody>
</table>
<script>
function fmtBytes(n) {{
  if (n < 1024) return n + ' B';
  if (n < 1048576) return (n / 1024).toFixed(1) + ' KB';
  if (n < 1073741824) return (n / 1048576).toFixed(1) + ' MB';
  return (n / 1073741824).toFixed(2) + ' GB';
}}
async function refresh() {{
  try {{
    const resp = await fetch('/api/relay/status');
    if (resp.status === 401) {{ location.replace('/relay'); return; }}
    const data = await resp.json();
    const rows = document.getElementById('rows');
    if (!data.devices.length) {{
      rows.innerHTML = '<tr><td colspan="8" class="empty">暂无桥入设备</td></tr>';
    }} else {{
      rows.innerHTML = data.devices.map(d => {{
        const t = new Date(d.connected_at * 1000);
        return '<tr>' +
          '<td><span class="dot ' + (d.online ? 'on' : 'off') + '"></span></td>' +
          '<td>' + esc(d.node_id) + '</td>' +
          '<td>' + esc(d.name) + '</td>' +
          '<td>' + esc(d.version) + '</td>' +
          '<td>' + t.toLocaleString() + '</td>' +
          '<td>' + fmtBytes(d.bytes_up) + '</td>' +
          '<td>' + fmtBytes(d.bytes_down) + '</td>' +
          '<td><a class="btn" href="/d/' + encodeURIComponent(d.node_id) + '/">打开面板</a></td>' +
          '</tr>';
      }}).join('');
    }}
    document.getElementById('updated').textContent =
      '更新于 ' + new Date().toLocaleTimeString();
  }} catch (e) {{ /* 网络抖动，下一轮再试 */ }}
}}
function esc(s) {{
  return String(s).replace(/[&<>"']/g, c => ({{'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}}[c]));
}}
refresh();
setInterval(refresh, 5000);
</script>
</body>
</html>"#,
        dashboard_entry = dashboard_entry,
    )
}

// AGT 覆盖率批次（2026-09-24）：直调错误梯子（门关/过大/坏 JSON/坏哈希/
// 设备不在表）+ in-process 设备的隧道 EOF 臂 + /bridge 上行帧梯子与重连
// 顶替（真 socket）。超时臂/竞态臂/防御 encode 失败臂豁免（见报告）。
#[cfg(test)]
mod agt_tests;
