//! pty.rs 测试 —— L8 PTY 内嵌终端。
//!
//! 三层：
//! 1. 纯逻辑（shell 选择 / control JSON 解析 / 管理器注册-满员-kill）；
//! 2. 真 PTY roundtrip（浏览器侧等价：tokio-tungstenite 客户端 → axum
//!    最小 router → ConPTY/Unix PTY → echo 标记往返）；
//! 3. 安全闸（token 失配 401 / estop 触发即 kill 会话）。
//!
//! 隔离：PTY_MANAGER OnceLock（模块级，只挂会话句柄、不碰数据读路径）
//! 首装不撤，本模块所有碰它的测试持同一把 TEST_LOCK 串行；tempdir 一律
//! Box::leak 保 'static（OnceLock 里的路径不能悬空）。terminal 配置走
//! pty::TEST_TERMINAL_CFG 模块级覆盖——**绝不 set_global 装进程级 config
//! store**：OnceLock first-wins 不可拆卸，装上后同 binary 里 models.rs
//! `load_config` 的全局优先分支会劫持依赖 home 隔离的既有测试（stress_*
//! 三连红的教训，2026-09-07）。

use super::*;
use futures::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// PTY manager 串行锁。tokio Mutex——async 测试
/// 持锁跨 await（roundtrip 全程串行化），std Mutex 会触发
/// await_holding_lock（跨 await 锁一律 tokio Mutex 的家规）。
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 安装 terminal.enabled=true 的**模块级测试覆盖**（幂等；重复 set 无害，
/// 首设生效）。不碰进程级 config store——见模块顶部隔离说明。
fn install_terminal_override() {
    let cfg = nemesis_config::TerminalConfig {
        enabled: true,
        max_sessions: 4,
        shell: None,
    };
    let _ = TEST_TERMINAL_CFG.set(Some(cfg));
}

/// 确保模块级 PTY 管理器就位（workspace 指向泄漏的 tempdir，审计文件
/// 落在那里随进程结束清理）。
fn ensure_test_manager() -> PathBuf {
    static WS_DIR: OnceLock<PathBuf> = OnceLock::new();
    let p = WS_DIR.get_or_init(|| {
        let d = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        d.path().to_path_buf()
    });
    crate::pty::ensure_manager(Some(p.to_string_lossy().into_owned()));
    p.clone()
}

/// 复用 websocket_handler/s10b_tests 的最小 AppState 形态。
fn make_state(
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
    })
}

/// 起 /ws/pty 单路由 server（动态端口，避开 Hyper-V 排除段）。
async fn start_server(state: Arc<AppState>) -> std::net::SocketAddr {
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

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_pty(addr: std::net::SocketAddr, query: &str) -> WsStream {
    let url = format!("ws://{}/ws/pty{}", addr, query);
    tokio_tungstenite::connect_async(url)
        .await
        .expect("ws connect")
        .0
}

/// 收二进制帧直到出现标记（拼接全部输出）。
///
/// 关键：ConPTY 启动时会发 `\x1b[6n`（DSR 光标位置查询）做**终端活性
/// 检查**，未收到 CPR 应答前不渲染任何输出（证据：审计日志里 spawn 后
/// 只有一帧 `[6n`）。xterm.js 会自动应答——生产路径无感；裸 WS 测试
/// 客户端必须自己模拟这个 VT 语义，否则 PTY 永远静默。
async fn read_until_marker(ws: &mut WsStream, marker: &str, timeout: Duration) -> String {
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
            other => panic!("unexpected frame while waiting for marker: {other:?}"),
        };
        if !answered_dsr && payload.contains("\x1b[6n") {
            // 模拟 xterm.js 应答 CPR：光标在 (1,1)。
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
// 纯逻辑
// ---------------------------------------------------------------------------

/// shell 覆盖优先；平台默认非空（Windows=powershell.exe / Unix=$SHELL|sh）。
#[test]
fn resolve_shell_prefers_override() {
    assert_eq!(resolve_shell(Some("pwsh.exe")), "pwsh.exe");
    let default = resolve_shell(None);
    assert!(!default.is_empty());
    #[cfg(windows)]
    assert_eq!(default, "powershell.exe");
}

/// resize 钳制 + ping 识别 + 未知/坏 JSON → Ignore（不炸会话）。
#[test]
fn parse_control_clamps_and_ignores() {
    match parse_control(r#"{"type":"resize","cols":9999,"rows":0}"#) {
        Control::Resize { cols, rows } => {
            assert_eq!(cols, 500);
            assert_eq!(rows, 2);
        }
        _ => panic!("expected resize"),
    }
    match parse_control(r#"{"type":"resize","cols":80,"rows":24}"#) {
        Control::Resize { cols, rows } => {
            assert_eq!((cols, rows), (80, 24));
        }
        _ => panic!("expected resize"),
    }
    assert!(matches!(parse_control(r#"{"type":"ping"}"#), Control::Ping));
    assert!(matches!(parse_control("not json"), Control::Ignore));
    assert!(matches!(
        parse_control(r#"{"type":"something-else"}"#),
        Control::Ignore
    ));
}

/// 满员闸 + kill_all 清场后再注册（estop kill-all 路径的注册表语义）。
#[test]
fn manager_register_cap_and_kill_all() {
    #[derive(Debug)]
    struct FakeKiller(AtomicUsize);
    impl portable_pty::ChildKiller for FakeKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            self.0.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FakeKiller(AtomicUsize::new(
                self.0.load(AtomicOrdering::SeqCst),
            )))
        }
    }
    let mk = || {
        Box::new(FakeKiller(AtomicUsize::new(0)))
            as Box<dyn portable_pty::ChildKiller + Send + Sync>
    };

    let mgr = PtySessionManager::new(Some("C:\\nonexistent-ws".into()));
    assert_eq!(mgr.session_count(), 0);
    // 满 2 拒第 3。
    let a = mgr.register(mk(), 2).expect("first");
    let b = mgr.register(mk(), 2).expect("second");
    assert!(mgr.register(mk(), 2).is_none(), "cap must reject");
    assert_eq!(mgr.session_count(), 2);
    // 摘牌一个后可再注册。
    mgr.unregister(a);
    assert_eq!(mgr.session_count(), 1);
    let c = mgr.register(mk(), 2).expect("after unregister");
    assert_ne!(b, c, "ids monotonic");
    // kill_all 清场。
    mgr.kill_all();
    assert_eq!(mgr.session_count(), 0);
    assert!(
        mgr.register(mk(), 2).is_some(),
        "registry empty after kill_all"
    );
}

// ---------------------------------------------------------------------------
// 真 PTY roundtrip + 安全闸
// ---------------------------------------------------------------------------

/// 端到端：WS 客户端连 /ws/pty → 真 shell 起来 → echo 标记往返。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn roundtrip_echo_via_real_pty() {
    let _g = TEST_LOCK.lock().await;
    install_terminal_override();
    ensure_test_manager();

    let addr = start_server(make_state("", None)).await;
    let mut ws = connect_pty(addr, "?token=").await;

    // 发 echo 命令（powershell / sh 通用 echo 内建）。
    let marker = format!(
        "nbt-pty-ok-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    tokio::time::sleep(Duration::from_millis(800)).await; // 等 shell 起来
    ws.send(WsMessage::Binary(
        format!("echo {marker}\r\n").into_bytes().into(),
    ))
    .await
    .expect("send command");
    read_until_marker(&mut ws, &marker, Duration::from_secs(20)).await;
    let _ = ws.close(None).await;
}

/// token 失配 → 升级被 401 拒（安全闸：镜像主 WS 语义）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_mismatch_rejected_401() {
    let _g = TEST_LOCK.lock().await;
    install_terminal_override();
    ensure_test_manager();

    let addr = start_server(make_state("sekret", None)).await;
    let url = format!("ws://{}/ws/pty", addr);
    let err = tokio_tungstenite::connect_async(url)
        .await
        .expect_err("must reject");
    match &err {
        WsError::Http(resp) => {
            assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
        }
        other => panic!("expected HTTP 401, got {other:?}"),
    }
    // 正确 token 放行（升级成功即断言，随后立即关闭）。
    let mut ws = connect_pty(addr, "?token=sekret").await;
    let _ = ws.close(None).await;
}

/// estop 触发 → 会话被 kill、socket 被服务端关闭（安全红线测试）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn estop_engage_kills_session() {
    let _g = TEST_LOCK.lock().await;
    install_terminal_override();
    ensure_test_manager();

    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    let addr = start_server(make_state("", Some(estop.clone()))).await;
    let mut ws = connect_pty(addr, "?token=").await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    estop.trigger();

    // 服务端 kill 会话后关 socket：期待 Close 帧或流结束（10s 内）。
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match ws.next().await {
                None => return,
                Some(Ok(WsMessage::Close(_))) => return,
                Some(Ok(_)) => continue, // shell 输出帧（提示语等）忽略
                Some(Err(_)) => return,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "session must be killed after estop");
}
