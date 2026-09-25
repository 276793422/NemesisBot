//! server.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 与 tests / agent_event_pump_tests / auth_tests / extra_tests / r4_tests
//! 互补，聚焦仍缺的确定性臂：
//! - `set_signature_verify` / `set_lsp_manager` + `lsp_manager()` 槽位
//! - `resolve_static_dir` 显式路径臂 + `DirectoryStaticFiles` 越界拒绝
//!   （symlink 指向 base 外 → canonical 比对拒绝；无权限环境跳过）
//! - `start()` 成功路径（`start_with_shutdown` 之外的另一条生命周期）
//! - `build_router` 的 relay 路由外壳：`/d/{node_id}`、`/d/{node_id}/`、
//!   `/api/relay/overview`、`/api/relay/enabled` + static_dir 缺失 warn
//! - SSE `last-event-id` 断线补拉双形态（缓冲内重放 / 滑出窗口 resync）
//! - `process_messages_with_router` 的 conv_router bind + 入站过滤链三裁决
//!   （Pass 直通 / Intercepted 就地应答 / Rejected 回执后丢弃）
//! - `pump_agent_events` 的 SessionCreated SSE 分支 + Lagged 存活
//! - `dispatch_outbound` 全路由矩阵（非 web 跳过 / 坏 chat_id 跳过 /
//!   普通回复 / history 帧 / 幽灵会话错误臂）
//!
//! 结构性豁免（见报告）：spawn 出的永不返回 future 的收括号行（562/661）、
//! serve 运行错误臂（1116-1119/1180）、resolve_static_dir 的 cwd 兜底
//! None 臂（1399-1401——测试进程 cwd 下 static/ 存在，置 None 需改全局
//! cwd，并发危险）、serde/encode 防御臂（1876-1881/2014-2016——AgentEvent
//! 与 Value 不可能序列化失败）、pump 非可丢广播臂（2004——现有事件变体
//! 凡带 chat_id 必带 session_key→必落环→恒走可丢通道）、dispatch 的
//! Closed 臂（2130-2131——任务自持 Arc<MessageBus>，通道永不关闭）。

use super::*;
use crate::api_handlers::AppState;
use crate::conv_router::ConvRouter;
use crate::events::EventHub;
use crate::handlers::signature_status::SignatureVerifyStatus;
use crate::relay::RelayServer;
use crate::session::SessionManager;
use crate::websocket_handler::{IncomingMessage, SendQueue};
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use nemesis_bus::{Filter, FilterChain, FilterDecision};
use nemesis_types::channel::OutboundMessage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;
use tokio::sync::mpsc;

fn agt_state() -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "agt-test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        cron: None,
        board: None,
    })
}

// ============================================================
// 槽位 setter / getter
// ============================================================

#[test]
fn agt_signature_verify_and_lsp_manager_slots() {
    let mut server = WebServer::new(WebServerConfig::default());
    assert!(!server.is_running());

    // set_signature_verify（此前无调用点）
    server.set_signature_verify(Arc::new(SignatureVerifyStatus {
        mode: "warn".to_string(),
        locked: false,
        anchor_fp: Some("abcdef".to_string()),
        last_result: Some("Valid".to_string()),
        key_fp: None,
        detail: String::new(),
    }));
    // set_lsp_manager + lsp_manager() getter（getter 此前无调用点）
    server.set_lsp_manager(Arc::new(nemesis_lsp::LspManager::new(None, None)));
    assert!(server.lsp_manager().is_some());
}

// ============================================================
// resolve_static_dir + DirectoryStaticFiles
// ============================================================

#[test]
fn agt_resolve_static_dir_explicit_and_directory_provider_traversal() {
    let dir = tempfile::tempdir().unwrap();

    // 显式路径存在且为目录 → 原样返回
    let explicit = dir.path().join("assets");
    std::fs::create_dir_all(&explicit).unwrap();
    assert_eq!(
        resolve_static_dir(Some(explicit.to_str().unwrap()), None).as_deref(),
        Some(explicit.to_str().unwrap())
    );

    // 显式路径缺失 → warn 后落 workspace/static 臂
    std::fs::create_dir_all(dir.path().join("ws").join("static")).unwrap();
    let ws = dir.path().join("ws").to_string_lossy().to_string();
    let missing = dir.path().join("no-such-dir");
    assert!(resolve_static_dir(Some(missing.to_str().unwrap()), Some(&ws)).is_some());

    // DirectoryStaticFiles：正常读 + 缺失 None + ".." 拒绝
    let base = dir.path().join("site");
    std::fs::create_dir_all(base.join("sub")).unwrap();
    std::fs::write(base.join("a.txt"), b"hello").unwrap();
    std::fs::write(base.join("sub").join("b.css"), b"body{}").unwrap();
    let provider = DirectoryStaticFiles::new(&base);
    assert_eq!(provider.get_file("a.txt").as_deref(), Some(&b"hello"[..]));
    assert!(provider.get_file("missing.txt").is_none());
    assert!(provider.get_file("../secret.txt").is_none());

    // 越界符号链接 → canonical 比对拒绝（需符号链接特权；无权限环境跳过）。
    // 指向 base 的**兄弟**目录（不能指祖先——list_files 会循环保 recursion）。
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("config.json"), b"{}").unwrap();
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(&outside, base.join("out")).is_ok() {
            assert!(
                provider.get_file("out/config.json").is_none(),
                "symlinked path escaping base must be rejected"
            );
        }
    }
    #[cfg(not(windows))]
    {
        if std::os::unix::fs::symlink(&outside, base.join("out")).is_ok() {
            assert!(provider.get_file("out/config.json").is_none());
        }
    }

    let mut files = provider.list_files();
    files.sort();
    assert!(files.contains(&"a.txt".to_string()), "files: {files:?}");
    assert!(files.contains(&"sub/b.css".to_string()), "files: {files:?}");
}

// ============================================================
// start() 成功路径（start_with_shutdown 之外）
// ============================================================

#[tokio::test]
async fn agt_start_success_serves_health() {
    // 先借一个空闲端口（bind 后释放——存在微小竞窗，轮询容忍）
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let server = WebServer::new(WebServerConfig {
        listen_addr: format!("127.0.0.1:{port}"),
        ..Default::default()
    });
    let task = tokio::spawn(async move { server.start().await });

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/health");
    let mut served = false;
    for _ in 0..50 {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().as_u16() == 200
        {
            let body = resp.text().await.unwrap_or_default();
            assert!(
                body.contains("\"ok\"") || body.contains("ok"),
                "body: {body}"
            );
            served = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(served, "start() must bind and serve /health");
    task.abort();
}

// ============================================================
// build_router relay 路由外壳 + static_dir 缺失
// ============================================================

#[tokio::test]
async fn agt_relay_device_routes_and_overview_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let missing_static = dir.path().join("no-static");

    let mut server = WebServer::new(WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        static_dir: Some(missing_static.to_string_lossy().to_string()),
        ..Default::default()
    });
    server.set_relay(Arc::new(RelayServer::new("agt-token".to_string(), true)));

    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
    let (bound_tx, bound_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let task = tokio::spawn(async move {
        let _ = server
            .start_with_shutdown(shutdown_rx, Some(bound_tx))
            .await;
    });
    let addr = tokio::time::timeout(std::time::Duration::from_secs(5), bound_rx)
        .await
        .expect("server bind timeout")
        .expect("bound_tx dropped");
    let base = format!("http://{addr}");

    let client = reqwest::Client::new();

    // 设备根路径双变体（此前仅 {*rest} 走过）：无凭据 → relay 设备语义
    // 响应（未知设备 503），只要拿到正常 HTTP 响应即证明外壳把请求递进了
    // relay handler。
    for path in ["/d/agt-node", "/d/agt-node/"] {
        let resp = client.get(format!("{base}{path}")).send().await.unwrap();
        let st = resp.status().as_u16();
        assert!(st >= 300, "{path} → unexpected {st}");
    }

    // /api/relay/overview（relay 注入态）→ 200 JSON
    let resp = client
        .get(format!("{base}/api/relay/overview"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.get("server").is_some() || body.get("enabled").is_some(),
        "{body}"
    );

    // /api/relay/enabled POST（body 布尔切换）→ 非 5xx 即证明闭包执行
    let resp = client
        .post(format!("{base}/api/relay/enabled"))
        .header("content-type", "application/json")
        .body(r#"{"enabled":false}"#)
        .send()
        .await
        .unwrap();
    assert!(resp.status().as_u16() < 500, "enabled → {}", resp.status());

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
}

// ============================================================
// SSE 断线补拉（last-event-id 双形态）
// ============================================================

/// 直调 `handle_events_stream` 并从响应体逐帧解析 SSE 事件，收满 `want`
/// 条（event, id, data-json）为止。响应体是无限 live 流，解析完即弃。
async fn agt_sse_frames(
    state: Arc<AppState>,
    last_id: &str,
    want: usize,
) -> Vec<(String, String, serde_json::Value)> {
    use http_body_util::BodyExt;
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("last-event-id"),
        HeaderValue::from_str(last_id).unwrap(),
    );
    let sse = handle_events_stream(AxumState(state), headers).await;
    let mut body = sse.into_response().into_body();
    let mut buf = String::new();
    let mut out = Vec::new();
    while out.len() < want {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
            .await
            .expect("sse frame timeout")
            .expect("body error")
            .expect("body poll error");
        let bytes = frame.into_data().expect("sse frames are data frames");
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buf.find("\n\n") {
            let chunk: String = buf.drain(..pos + 2).collect();
            let mut event = String::new();
            let mut id = String::new();
            let mut data = String::new();
            for line in chunk.lines() {
                if let Some(v) = line.strip_prefix("event: ") {
                    event = v.to_string();
                } else if let Some(v) = line.strip_prefix("id: ") {
                    id = v.to_string();
                } else if let Some(v) = line.strip_prefix("data: ") {
                    data = v.to_string();
                }
            }
            let json = serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
            out.push((event, id, json));
        }
    }
    out
}

#[tokio::test]
async fn agt_sse_replay_resync_and_in_window_replay() {
    let state = agt_state();
    state.event_hub.publish("log", serde_json::json!({"n": 1}));
    state.event_hub.publish("log", serde_json::json!({"n": 2}));

    // ① last-event-id 超前于缓冲 → resync 提示（前端全量刷新兜底）
    let frames = agt_sse_frames(state.clone(), "99", 2).await;
    assert_eq!(frames[0].0, "heartbeat");
    assert_eq!(frames[1].0, "resync", "gap must emit resync hint");
    assert_eq!(frames[1].2["reason"], "events_outside_replay_window");
    assert_eq!(frames[1].2["latest_seq"], 2);

    // ② last-event-id=1 → 缓冲内重放 seq 2（无 resync）
    let frames = agt_sse_frames(state, "1", 2).await;
    assert_eq!(frames[0].0, "heartbeat");
    assert_eq!(frames[1].0, "log", "in-window replay must emit the event");
    assert_eq!(frames[1].1, "2");
    assert_eq!(frames[1].2["n"], 2);
}

// ============================================================
// process_messages_with_router：bind + 过滤链三裁决
// ============================================================

/// 按消息内容切换裁决的探针过滤器：reject-*/intercept-* 前缀命中对应
/// 裁决，其余 Pass。
struct ContentSwitch;

#[async_trait::async_trait]
impl Filter<InboundMessage> for ContentSwitch {
    fn name(&self) -> &'static str {
        "content-switch"
    }
    fn priority(&self) -> i32 {
        0
    }
    async fn inspect(&self, msg: &InboundMessage) -> FilterDecision {
        if msg.content.starts_with("reject-") {
            FilterDecision::Rejected("策略拒绝".to_string())
        } else if msg.content.starts_with("intercept-") {
            FilterDecision::Intercepted
        } else {
            FilterDecision::Pass
        }
    }
}

fn agt_incoming(chat_id: &str, content: &str, session_meta: Option<&str>) -> IncomingMessage {
    let mut metadata = HashMap::new();
    if let Some(sid) = session_meta {
        metadata.insert("session_id".to_string(), sid.to_string());
    }
    IncomingMessage {
        session_id: "conn-1".to_string(),
        sender_id: "agt-user".to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        metadata,
        voice_playback: None,
        media: Vec::new(),
    }
}

#[tokio::test]
async fn agt_process_messages_binds_router_and_runs_filter_ladder() {
    let bus = Arc::new(MessageBus::new());
    let mut inbound_rx = bus.subscribe_inbound();
    let mut outbound_rx = bus.subscribe_outbound();

    let router: crate::conv_router::SharedConvRouter = Arc::new(ConvRouter::new());
    let session_manager = Arc::new(SessionManager::with_default_timeout());

    let chain: FilterChain<InboundMessage> = FilterChain::new();
    chain.attach(Arc::new(ContentSwitch));

    let (tx, rx) = mpsc::unbounded_channel::<IncomingMessage>();
    let task = tokio::spawn(process_messages_with_router(
        rx,
        bus.clone(),
        Some(router.clone()),
        Some(session_manager),
        Some(Arc::new(chain)),
    ));

    // Pass：绑定 conv_router + 扇出，session_key 用显式 session_id
    tx.send(agt_incoming("web:connA", "pass-me", Some("agt-conv")))
        .unwrap();
    // Intercepted：过滤器就地应答，不扇出
    tx.send(agt_incoming("web:connB", "intercept-me", Some("agt-conv")))
        .unwrap();
    // Rejected：回执拒绝原因后丢弃
    tx.send(agt_incoming("web:connC", "reject-me", Some("agt-conv")))
        .unwrap();
    // 无 session_id 元数据 → legacy 兜底键
    tx.send(agt_incoming("web:connA", "pass-legacy", None))
        .unwrap();

    let first = tokio::time::timeout(std::time::Duration::from_secs(5), inbound_rx.recv())
        .await
        .expect("inbound timeout")
        .expect("bus closed");
    assert_eq!(first.session_key, "agent:main:session:agt-conv");
    assert_eq!(first.chat_id, "web:connA");

    let second = tokio::time::timeout(std::time::Duration::from_secs(5), inbound_rx.recv())
        .await
        .expect("inbound timeout")
        .expect("bus closed");
    assert_eq!(second.session_key, "agent:main:session:legacy");

    // 只有 2 条通过（intercept/reject 均不扇出）
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(inbound_rx.try_recv().is_err(), "no third inbound expected");

    // Rejected 的回执出站帧
    let reject = tokio::time::timeout(std::time::Duration::from_secs(5), outbound_rx.recv())
        .await
        .expect("outbound timeout")
        .expect("bus closed");
    assert_eq!(reject.channel, "web");
    assert_eq!(reject.chat_id, "web:connC");
    assert_eq!(reject.content, "策略拒绝");

    // conv_router 绑定生效（cron 实时推送据此选 chat_id）。注意绑定发生在
    // 过滤链**之前**——intercept/reject 的消息同样刷新 liveness，latest-wins
    // 下 agt-conv 的最终绑定是最后一条该会话消息（connC）。
    assert_eq!(
        router.target("agent:main:session:agt-conv").as_deref(),
        Some("web:connC")
    );

    drop(tx);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
}

// ============================================================
// pump_agent_events：SessionCreated 分支 + Lagged 存活
// ============================================================

#[tokio::test]
async fn agt_pump_session_created_sse_branch() {
    let event_hub = Arc::new(EventHub::new());
    let mut hub_rx = event_hub.subscribe();

    let (tx, rx) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(16);
    let pump = tokio::spawn(pump_agent_events(
        rx,
        Arc::new(SessionManager::with_default_timeout()),
        event_hub,
    ));

    tx.send(nemesis_types::agent::AgentEvent::SessionCreated {
        session_id: "agt-sid".to_string(),
        session_key: "agent:main:session:agt-sid".to_string(),
    })
    .unwrap();

    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), hub_rx.recv())
        .await
        .expect("hub timeout")
        .expect("hub closed");
    assert_eq!(ev.event_type, "session.created");
    assert_eq!(ev.data["session_id"], "agt-sid", "payload: {}", ev.data);

    pump.abort();
}

#[tokio::test]
async fn agt_pump_survives_broadcast_lag() {
    let event_hub = Arc::new(EventHub::new());
    let mut hub_rx = event_hub.subscribe();

    // 容量 2，先灌 6 条再启泵 → 首个 recv 必为 Lagged（前 4 条被覆盖）
    let (tx, rx) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(2);
    for i in 0..6 {
        tx.send(nemesis_types::agent::AgentEvent::ToolStarted {
            session_key: format!("agent:main:session:s{i}"),
            chat_id: format!("web:c{i}"),
            call_id: format!("c{i}"),
            tool: "exec".to_string(),
            args_preview: "{}".to_string(),
        })
        .unwrap();
    }
    drop(tx); // 泵消费完存量后经 Closed 退出

    let pump = tokio::spawn(pump_agent_events(
        rx,
        Arc::new(SessionManager::with_default_timeout()),
        event_hub,
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), pump)
        .await
        .expect("pump must survive Lagged and exit on close")
        .expect("pump panicked");

    // 被覆盖的 4 条丢失（Lagged 分支直接跳过），仅最后 2 条进 hub
    let mut tool_events = 0usize;
    while let Ok(ev) = hub_rx.try_recv() {
        if ev.event_type == "tool_event" {
            tool_events += 1;
        }
    }
    assert_eq!(tool_events, 2, "post-lag events only");
}

// ============================================================
// dispatch_outbound 全路由矩阵
// ============================================================

#[tokio::test]
async fn agt_dispatch_outbound_routes_web_and_skips_others() {
    let bus = Arc::new(MessageBus::with_capacity(8));

    // 先灌一批非 web 消息再启泵 → 首个 recv 为 Lagged（存活继续）
    for i in 0..40 {
        bus.publish_outbound(OutboundMessage::new("telegram", &format!("t{i}"), "skip"));
    }

    let mgr = Arc::new(SessionManager::with_default_timeout());
    let session = mgr.create_session();
    let (queue, hi_rx, _lo_rx, _done_tx) = SendQueue::test_channels(16);
    mgr.set_send_queue(&session.id, Arc::new(queue));

    let task = tokio::spawn(dispatch_outbound(bus.clone(), mgr.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let sid = session.id.clone();
    // 非 web → 跳过；坏 chat_id 前缀 → warn 跳过；幽灵会话 → 错误臂吞掉
    bus.publish_outbound(OutboundMessage::new("discord", "d1", "skip"));
    bus.publish_outbound(OutboundMessage::new("web", "no-prefix", "skip"));
    bus.publish_outbound(OutboundMessage::new("web", "web:ghost", "lost"));
    // 正常回复 + history 帧
    bus.publish_outbound(OutboundMessage::new(
        "web",
        &format!("web:{sid}"),
        "hello-agt",
    ));
    bus.publish_outbound(OutboundMessage::with_type(
        "web",
        &format!("web:{sid}"),
        r#"[{"role":"user","content":"q"}]"#,
        "history",
    ));

    let mut hi_rx = hi_rx;
    let mut got_reply = false;
    let mut got_history = false;
    for _ in 0..2 {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), hi_rx.recv())
            .await
            .expect("hi lane timeout")
            .expect("queue closed");
        let text = String::from_utf8(frame).unwrap();
        if text.contains("hello-agt") {
            assert!(text.contains("\"cmd\":\"receive\""), "{text}");
            got_reply = true;
        } else if text.contains("history-body-or-json") || text.contains("history") {
            assert!(text.contains("\"cmd\":\"history\""), "{text}");
            got_history = true;
        }
    }
    assert!(got_reply, "assistant reply frame must reach the session");
    assert!(got_history, "history frame must reach the session");

    task.abort();
}

#[tokio::test]
async fn agt_agent_event_pump_none_and_some_arms() {
    let mut server = WebServer::new(WebServerConfig::default());

    // None 臂：未注入 receiver → 直接返回，不 spawn。
    server.start_agent_event_pump();

    // Some 臂：注入 receiver → spawn 泵任务（立即返回；泵在后台等事件）。
    let (_tx, rx) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(16);
    server.set_agent_event_rx(rx);
    server.start_agent_event_pump();
}
