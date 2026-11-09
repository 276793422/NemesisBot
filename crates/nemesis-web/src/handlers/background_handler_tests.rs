//! 后台进程 WSAPI handler 测试（2026-11-06 P1）。AppState 槽注入形态
//! （estop 同款）：make_ctx 按需注入真实注册表（每用例独立实例，表内容
//! 可全量断言）；None 态诚实报错单独一则。真实子进程用例 spawn 一条
//! echo/慢命令即 kill 清理（每用例独立注册表实例，无跨用例串扰，不加
//! 模块级锁；B4 侧 TEST_LOCK 串行纪律管的是 nemesis-agent 模块内并发
//! spawn/树杀放大延迟的场景，这里单 spawn 风险可控）。

use super::background::BackgroundHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};

fn make_ctx(registry: Option<Arc<nemesis_agent::BackgroundProcessRegistry>>) -> RequestContext {
    let state = Arc::new(AppState {
        cors_manager: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
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
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        background_registry: registry,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "t".to_string(),
        chat_id: "c-t".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 平台条件 echo 命令（registry 的 build_platform_shell 会包平台 shell）。
#[cfg(windows)]
fn echo_cmd() -> &'static str {
    "echo handler-echo-marker"
}
#[cfg(not(windows))]
fn echo_cmd() -> &'static str {
    "echo handler-echo-marker"
}

/// 平台条件慢命令（运行中态用；结束前必被 kill 清理）。
#[cfg(windows)]
fn long_cmd() -> &'static str {
    "ping -n 30 127.0.0.1"
}
#[cfg(not(windows))]
fn long_cmd() -> &'static str {
    "sleep 30"
}

fn temp_cwd() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

#[tokio::test]
async fn registry_not_injected_honest_error() {
    let ctx = make_ctx(None);
    let h = BackgroundHandler;
    // 四命令全诚实报错（数据形态齐全——错误只来自缺注入，不来自缺参数）。
    for cmd in ["list", "output", "kill", "steer"] {
        let err = h
            .handle_cmd(cmd, Some(serde_json::json!({ "id": 1, "text": "x" })), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("registry not injected"), "{cmd} err={err}");
    }
}

#[tokio::test]
async fn commands_surface_and_unknown_command_rejected() {
    let ctx = make_ctx(None);
    let h = BackgroundHandler;
    assert_eq!(h.module_name(), "background");
    assert_eq!(h.commands(), &["list", "output", "kill", "steer"]);
    // unknown command 先于 registry 检查——None 注入下拼错命令也报 unknown。
    let err = h.handle_cmd("nope", None, &ctx).await.unwrap_err();
    assert!(
        err.contains("unknown command: background.nope"),
        "err={err}"
    );
}

#[tokio::test]
async fn argument_validation_and_honest_miss() {
    let reg = Arc::new(nemesis_agent::BackgroundProcessRegistry::new());
    let ctx = make_ctx(Some(reg));
    let h = BackgroundHandler;

    // 缺 id / 缺 text。
    let err = h.handle_cmd("output", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires {id}"), "err={err}");
    let err = h.handle_cmd("kill", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires {id}"), "err={err}");
    let err = h.handle_cmd("steer", None, &ctx).await.unwrap_err();
    assert!(err.contains("requires {id}"), "err={err}");
    let err = h
        .handle_cmd("steer", Some(serde_json::json!({ "id": 1 })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("requires {text}"), "err={err}");
    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": 1, "text": "" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("requires {text}"), "err={err}");

    // 不存在的 job：数字 id 与数字串 id 都走注册表诚实 miss（证明串形态
    // 解析通过——深链 query 回传字符串兜底）。
    for id in [serde_json::json!(999_999), serde_json::json!("999999")] {
        let err = h
            .handle_cmd("output", Some(serde_json::json!({ "id": id })), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("no such job_id"), "err={err}");
        let err = h
            .handle_cmd("kill", Some(serde_json::json!({ "id": id })), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("no such job_id"), "err={err}");
        let err = h
            .handle_cmd(
                "steer",
                Some(serde_json::json!({ "id": id, "text": "x" })),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("no such job_id"), "err={err}");
    }
}

#[tokio::test]
async fn echo_process_list_output_kill_flow() {
    let reg = Arc::new(nemesis_agent::BackgroundProcessRegistry::new());
    let ctx = make_ctx(Some(reg.clone()));
    let h = BackgroundHandler;

    let empty = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(empty["count"], serde_json::json!(0), "空注册表 list");

    let out = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": 0, "text": "warm" })),
            &ctx,
        )
        .await;
    assert!(out.is_err(), "id 0 不存在，steer 诚实拒绝");

    // —— 运行中行形态用慢命令（echo 秒退，running/exit_code 缺席断言两面
    // 竞态：满载下 echo 尚未起、快机上又已收尸——慢命令给 30s 确定窗口）。
    let slow = reg.start(long_cmd(), &temp_cwd(), false).await.unwrap();
    let slow_id = slow["job_id"].as_u64().unwrap();
    let out = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(out["count"], serde_json::json!(1));
    let row = &out["jobs"].as_array().unwrap()[0];
    assert_eq!(row["id"], serde_json::json!(slow_id));
    assert_eq!(row["command"], serde_json::json!(long_cmd()));
    assert_eq!(row["running"], serde_json::json!(true));
    assert!(
        row.get("exit_code").is_none(),
        "运行中/未收尸行不平铺 exit_code: {row}"
    );
    // 清理慢命令（kill 幂等，返回即终态；行仍在册）。
    h.handle_cmd("kill", Some(serde_json::json!({ "id": slow_id })), &ctx)
        .await
        .unwrap()
        .unwrap();

    // —— echo 全链。start 走 registry 直调（WSAPI 面没有 start——start 是
    // agent 工具通道）。
    let started = reg.start(echo_cmd(), &temp_cwd(), false).await.unwrap();
    let job_id = started["job_id"].as_u64().unwrap();

    // output：轮询到 chunk 出现 marker（echo 完成有窗口期，不假设即时）。
    let mut chunk = String::new();
    for _ in 0..50 {
        let out = h
            .handle_cmd("output", Some(serde_json::json!({ "id": job_id })), &ctx)
            .await
            .unwrap()
            .unwrap();
        chunk = out["chunk"].as_str().unwrap_or("").to_string();
        if chunk.contains("handler-echo-marker") {
            assert_eq!(out["offset"], serde_json::json!(0), "offset 缺省 0");
            // steerable 随 output 面透出（UI 详情态注入框渲染依据，与
            // start()/list() 同源）——本任务按 false 启动。
            assert_eq!(out["steerable"], serde_json::json!(false), "out={out}");
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        chunk.contains("handler-echo-marker"),
        "echo 输出未在 5s 内可见: {chunk:?}"
    );

    // marker 可见 = 输出已入环形缓冲 → list 行的 total_bytes 断言此时才
    // 确定（start 后立即断言满载下读到 0——实录 2026-11-06 全量跑挂）。
    // kill 可能在 marker 前已自然收尸，行形态取「按 id 找行」不受影响。
    let out = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    let row = out["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == serde_json::json!(job_id))
        .expect("echo 行在册");
    assert_eq!(row["command"], serde_json::json!(echo_cmd()));
    assert!(row["total_bytes"].as_u64().unwrap_or(0) > 0, "row={row}");

    // kill：echo 可能已自然退出也可能没有——kill 幂等，两种形态都返回
    // 终态载荷（exit_code 在场）。
    let out = h
        .handle_cmd("kill", Some(serde_json::json!({ "id": job_id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["job_id"], serde_json::json!(job_id));
    assert!(out["exit_code"].is_number(), "kill 返回终态: {out}");

    // kill 返回即终态已记录 → list/output 都平铺终态（jobs 含慢命令行，
    // 按 id 找行）。
    let out = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    let row = out["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == serde_json::json!(job_id))
        .expect("echo 行在册");
    assert_eq!(row["running"], serde_json::json!(false));
    assert_eq!(row["exit_code"], serde_json::json!(0), "row={row}");
    let out = h
        .handle_cmd("output", Some(serde_json::json!({ "id": job_id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["running"], serde_json::json!(false));
    assert_eq!(out["exit_code"], serde_json::json!(0));
}

#[tokio::test]
async fn steer_rejects_non_steerable_running_job_then_kill_cleans_up() {
    let reg = Arc::new(nemesis_agent::BackgroundProcessRegistry::new());
    let ctx = make_ctx(Some(reg.clone()));
    let h = BackgroundHandler;

    let started = reg.start(long_cmd(), &temp_cwd(), false).await.unwrap();
    let job_id = started["job_id"].as_u64().unwrap();

    // 非 steerable 启动（stdin 从未开）→ steer 诚实拒绝。
    let err = h
        .handle_cmd(
            "steer",
            Some(serde_json::json!({ "id": job_id, "text": "hi" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("steerable=true"), "err={err}");

    // 清理（慢命令必须 kill，不留 30s 孤儿）。
    let out = h
        .handle_cmd("kill", Some(serde_json::json!({ "id": job_id })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["job_id"], serde_json::json!(job_id));
    assert_eq!(out["killed"], serde_json::json!(true), "out={out}");
}
