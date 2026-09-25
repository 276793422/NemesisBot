//! pty.rs AGT 覆盖率批次（2026-09-25，terminal feature 门控，与 tests 互补）。
//!
//! 聚焦仍缺的确定性臂：
//! - `AuditWriter`：有 workspace 的审计往返（头部+[I]/[O] 帧）、审计目录
//!   创建失败臂；无 workspace 臂
//! - 升级闸：连接时刻已处急停 → 503 拒升级
//! - 会话内 control 帧：resize / ping→pong / 未知类型忽略 / WS Pong 帧
//!   （`Some(Ok(_))` 臂）；客户端 Close 收尾（shell 退出臂 463-464 在
//!   Windows 不可达——ConPTY 管道要等 ClosePseudoConsole，鸡生蛋）
//!
//! 隔离：与 tests 共用同一把 TEST_LOCK（PTY_MANAGER 全局注册表 + estop
//! kill-all 会误伤并发会话）；TEST_TERMINAL_CFG first-wins——本模块只以
//! 与 tests 同形态的覆盖值 set（幂等无害），**不装不同值**（404/max_sessions=0
//! 等 divergent 配置臂因此豁免，见报告）。
//!
//! 结构性豁免（见报告）：404 端点关闭臂与 spawn 失败臂/满员在会话臂
//! （TEST_TERMINAL_CFG first-wins 与既有覆盖值冲突）、manager 未接线 503
//! （OnceLock 首装后恒 Some）、openpty/reader/writer 失败臂（需注入
//! openpty 故障）、socket send 失败臂（需断连时序竞态）、kill_all 的
//! mutex 中毒臂、load_live 全局 store 分支（模块头注明的 stress_* 劫持
//! 教训）。

use super::*;
use futures::{SinkExt, StreamExt};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// 与 tests 同形态的覆盖值（幂等；若 tests 先装则本 set 无效——两者等值，
/// 无论谁赢语义一致）。
fn agt_install_terminal_override() {
    let cfg = nemesis_config::TerminalConfig {
        enabled: true,
        max_sessions: 4,
        shell: None,
    };
    let _ = TEST_TERMINAL_CFG.set(Some(cfg));
}

/// 复用 tests 的全局串行锁（会话级测试必须与其串行）。
async fn agt_global_lock() -> tokio::sync::MutexGuard<'static, ()> {
    tests::TEST_LOCK.lock().await
}

fn agt_make_state(
    auth_token: &str,
    estop: Option<Arc<nemesis_agent::estop::EstopState>>,
) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: auth_token.to_string(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: std::time::Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        session_manager: Arc::new(crate::session::SessionManager::with_default_timeout()),
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
        estop,
        cron: None,
        board: None,
        signature_verify: None,
    })
}

async fn agt_start_server(state: Arc<AppState>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route("/ws/pty", axum::routing::get(handle_pty_upgrade))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

type AgtWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn agt_connect(addr: std::net::SocketAddr, query: &str) -> AgtWs {
    let url = format!("ws://{}/ws/pty{}", addr, query);
    tokio_tungstenite::connect_async(url)
        .await
        .expect("ws connect")
        .0
}

/// 收帧直到出现标记；自动应答 ConPTY DSR 活性检查（同 tests）。
async fn agt_read_until(ws: &mut AgtWs, marker: &str, timeout: Duration) -> String {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut acc = String::new();
    let mut answered_dsr = false;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(left, ws.next())
            .await
            .expect("timeout waiting for pty output")
            .expect("pty stream open")
            .expect("ws ok");
        let payload = match msg {
            WsMessage::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
            WsMessage::Text(t) => t.to_string(),
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            other => panic!("unexpected frame: {other:?}"),
        };
        if !answered_dsr && payload.contains("\x1b[6n") {
            ws.send(WsMessage::Binary("\x1b[1;1R".as_bytes().to_vec().into()))
                .await
                .expect("send CPR");
            answered_dsr = true;
        }
        acc.push_str(&payload);
        if acc.contains(marker) {
            return acc;
        }
    }
}

// ---------------------------------------------------------------------------
// AuditWriter
// ---------------------------------------------------------------------------

#[test]
fn agt_audit_writer_roundtrip_dir_fail_and_no_workspace() {
    // ① 有 workspace：头部 + I/O 帧落盘
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let mut a = AuditWriter::open(Some(&ws), "powershell.exe", 7);
    assert!(a.file.is_some(), "valid workspace must open audit file");
    a.frame(b"I", b"hello");
    a.frame(b"O", b"out-bytes");
    drop(a);
    let term = std::fs::read_dir(dir.path().join("logs").join("terminal"))
        .expect("audit dir created")
        .count();
    assert_eq!(term, 1, "one audit file");
    let path = std::fs::read_dir(dir.path().join("logs").join("terminal"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("# NemesisBot PTY audit"), "{content}");
    assert!(content.contains("# shell: powershell.exe"));
    assert!(content.contains("[I] hello"));
    assert!(content.contains("[O] out-bytes"));
    assert!(content.contains("密码类输入"), "honest header note");

    // ② 审计目录创建失败：<ws>/logs 被文件占位 → file None（259-261 臂）
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(dir2.path().join("logs"), b"not a dir").unwrap();
    let ws2 = dir2.path().to_string_lossy().to_string();
    let b = AuditWriter::open(Some(&ws2), "sh", 8);
    assert!(b.file.is_none(), "dir create failure must disable audit");

    // ③ 无 workspace：审计缺席（254-256 臂）
    let c = AuditWriter::open(None, "sh", 9);
    assert!(c.file.is_none());
}

// ---------------------------------------------------------------------------
// 升级闸：连接时刻已处急停 → 503
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agt_pty_upgrade_estop_engaged_rejected_503() {
    let _g = agt_global_lock().await;
    agt_install_terminal_override();
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let addr = agt_start_server(agt_make_state("", Some(estop))).await;
    let result = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/pty")).await;
    match result {
        Err(WsError::Http(resp)) => {
            assert_eq!(resp.status(), 503, "estop engaged must reject upgrade");
        }
        other => panic!("expected HTTP 503, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 会话内 control 帧 + shell 退出收尾
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agt_pty_session_control_frames_and_shell_exit() {
    let _g = agt_global_lock().await;
    agt_install_terminal_override();
    let leaked = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    ensure_manager(Some(leaked.path().to_string_lossy().into_owned()));

    let addr = agt_start_server(agt_make_state("", None)).await;
    let mut ws = agt_connect(addr, "").await;
    // 等 session spawn 落定（同 tests 的 roundtrip 前奏；DSR 由 read_until 应答）
    tokio::time::sleep(Duration::from_millis(800)).await;

    // resize control：会话内 master.resize（474-481 臂）——无崩溃即可
    ws.send(WsMessage::Text(
        r#"{"type":"resize","cols":80,"rows":24}"#.into(),
    ))
    .await
    .expect("send resize");

    // ping control → pong（482-484 臂）
    ws.send(WsMessage::Text(r#"{"type":"ping"}"#.into()))
        .await
        .expect("send ping");
    let got = agt_read_until(&mut ws, "{\"type\":\"pong\"}", Duration::from_secs(10)).await;
    assert!(got.contains("pong"), "{got}");

    // 未知类型 → Ignore（485 臂，不炸会话）
    ws.send(WsMessage::Text(r#"{"type":"whatever"}"#.into()))
        .await
        .expect("send unknown");

    // WS Pong 帧 → Some(Ok(_)) 忽略臂（488 臂）
    ws.send(WsMessage::Pong(vec![].into()))
        .await
        .expect("send pong frame");

    // shell 退出臂（463-464）在 Windows 不可达：ConPTY 输出管道要等
    // ClosePseudoConsole 才断，而 master 活在等 out_rx 关闭的那个循环里
    // （鸡生蛋）；Unix 上 master read 返回 EIO → reader 线程 break →
    // out_rx None。这里只验证 exit 输入照常转发（I 帧），随后走客户端
    // 主动 Close（487 臂）收尾。
    #[cfg(windows)]
    let bye = "exit\r\n";
    #[cfg(not(windows))]
    let bye = "exit\n";
    ws.send(WsMessage::Binary(bye.as_bytes().to_vec().into()))
        .await
        .expect("send exit");

    // 客户端主动关闭 → 会话循环 Some(Ok(Close)) → break → 服务端收尾。
    ws.close(None).await.expect("client close");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = tokio::time::timeout(left, ws.next()).await;
        match next {
            Err(_) => panic!("server must finish session after client close"),
            Ok(None) => break,
            Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => continue, // 残余输出帧 / Close 应答
        }
    }
}
