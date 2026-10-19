//! B11（2026-10-18 snooze）：sessions.snooze / sessions.unsnooze 两命令 +
//! 到期唤醒 sweeper 的 handler 层测试。
//!
//! home 单例经 session_bindings::tests 同款 HomeGuard 重定向到 tempdir
//! （test_home::lock_home 可重入锁跨测试串行）；会话存活标记用
//! append_chat_log 走单例（与 chat_log_exists 同路径）；workspace 侧取
//! `<home>/workspace` 与单例派生规则一致。parse_snooze_until 是纯函数，
//! 不触碰 home，不取锁。

use super::sessions::{SessionsHandler, parse_snooze_until, wake_due_sessions};
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct HomeGuard {
    old: PathBuf,
    _lock: parking_lot::ReentrantMutexGuard<'static, ()>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        nemesis_path::default_path_manager().set_home_dir(self.old.clone());
    }
}

/// 重定向单例 home（workspace = `<home>/workspace`，与单例派生规则一致）。
fn isolate(home: &Path) -> HomeGuard {
    let lock = crate::test_home::lock_home();
    let old = nemesis_path::default_path_manager().home_dir();
    nemesis_path::default_path_manager().set_home_dir(home.to_path_buf());
    HomeGuard { old, _lock: lock }
}

fn make_ctx(home: &Path) -> RequestContext {
    let ws = home.join("workspace").to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
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
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "b11".to_string(),
        chat_id: "b11".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

async fn run(
    ctx: &RequestContext,
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    SessionsHandler.handle_cmd(cmd, data, ctx).await
}

/// 造一个存活会话（append 走单例 home——与 chat_log_exists 同路径）。
fn seed_session(home: &Path, sid: &str) -> String {
    let key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    );
    nemesis_agent::chat_log::append_chat_log(&key, "user", "hi");
    let _ = home; // 表意：会话落在 <home>/workspace/logs/session_logs
    key
}

#[tokio::test]
async fn b11_snooze_guards() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);

    // data 缺 / session_id 缺 / until 缺。
    for (data, expect) in [
        (None, "missing data"),
        (Some(serde_json::json!({})), "missing session_id"),
        (
            Some(serde_json::json!({"session_id": "s1"})),
            "missing until",
        ),
    ] {
        let err = run(&ctx, "snooze", data).await.unwrap_err();
        assert_eq!(err, expect, "guard 臂报错不符");
    }

    // 会话不存在（chat_log_exists 落空）。
    let err = run(
        &ctx,
        "snooze",
        Some(serde_json::json!({"session_id": "ghost", "until": "3600"})),
    )
    .await
    .unwrap_err();
    assert!(err.contains("会话不存在"), "got: {err}");

    // 存活会话 + 非法 until（负数 / 零 / 垃圾串 / 过去时刻）。
    let key = seed_session(&home, "g1");
    for until in ["-5", "0", "not-a-date", "2020-01-01T00:00:00+08:00"] {
        let err = run(
            &ctx,
            "snooze",
            Some(serde_json::json!({"session_id": "g1", "until": until})),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("相对秒数必须为正整数")
                || err.contains("until 需为相对秒数")
                || err.contains("暂停时刻已过去"),
            "until={until} 应诚实报错, got: {err}"
        );
        // 失败请求不落标记。
        assert_eq!(
            nemesis_agent::chat_log::clear_session_snooze(&key),
            None,
            "until={until} 不应写标记"
        );
    }
    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn b11_snooze_unsnooze_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    let key = seed_session(&home, "r1");

    // 相对秒 snooze：返回归一 RFC3339 + meta 落盘。
    let out = run(
        &ctx,
        "snooze",
        Some(serde_json::json!({"session_id": "r1", "until": "3600", "note": " 等窗口 "})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["session_id"], "r1");
    let until = out["snoozed_until"].as_str().expect("snoozed_until 缺失");
    chrono::DateTime::parse_from_rfc3339(until).expect("归一结果应为 RFC3339");
    let meta = nemesis_agent::chat_log::read_session_meta_full(&key).unwrap();
    assert_eq!(meta.snoozed_until.as_deref(), Some(until));
    assert_eq!(
        meta.snooze_note.as_deref(),
        Some("等窗口"),
        "note trim 生效"
    );

    // RFC3339 绝对时刻 snooze（幂等改期）。
    let out = run(
        &ctx,
        "snooze",
        Some(serde_json::json!({
            "session_id": "r1",
            "until": "2099-01-01T09:00:00+08:00"
        })),
    )
    .await
    .unwrap()
    .unwrap();
    // 归一只换表示不换时刻：输出按本地时区 RFC3339（UTC runner 上是 +00:00 形态），
    // 断言同一时刻而非同一字符串——字符串相等会把断言钉死在开发机时区上（CI 实录）。
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(
            out["snoozed_until"].as_str().expect("snoozed_until 缺失")
        )
        .unwrap(),
        chrono::DateTime::parse_from_rfc3339("2099-01-01T09:00:00+08:00").unwrap(),
        "幂等改期应保持同一时刻"
    );

    // unsnooze：摘标记回 true；meta 清空；二次 unsnooze 幂等 false。
    let out = run(
        &ctx,
        "unsnooze",
        Some(serde_json::json!({"session_id": "r1"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["unsnoozed"], true);
    let meta = nemesis_agent::chat_log::read_session_meta_full(&key).unwrap();
    assert_eq!(meta.snoozed_until, None);
    let out = run(
        &ctx,
        "unsnooze",
        Some(serde_json::json!({"session_id": "r1"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["unsnoozed"], false);

    // unsnooze 守卫：session_id 缺。
    let err = run(&ctx, "unsnooze", Some(serde_json::json!({})))
        .await
        .unwrap_err();
    assert_eq!(err, "missing session_id");

    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn b11_wake_due_sessions_injects_recovery_line() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let key = seed_session(&home, "w1");

    // 直接写一个已到期的标记（parse 层拒过去时刻，sweep 消费它）。
    nemesis_agent::chat_log::write_session_snooze(
        &key,
        "2020-01-01T00:00:00+08:00",
        Some("旧暂停"),
    );

    let woken = wake_due_sessions();
    assert_eq!(woken, 1, "到期项应唤醒一次");
    // 标记摘除 + 恢复 system 行入 jsonl（真相源）。
    let meta = nemesis_agent::chat_log::read_session_meta_full(&key).unwrap();
    assert_eq!(meta.snoozed_until, None);
    let (msgs, _, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 50, None);
    let last = msgs.last().expect("恢复行应落盘");
    assert_eq!(last["role"], "system");
    let content = last["content"].as_str().unwrap_or("");
    assert!(content.contains("⏰"), "恢复行应带 ⏰ 前缀: {content}");
    assert!(content.contains("旧暂停"), "note 应追加: {content}");

    // 二次 wake = 0（无残留）。
    assert_eq!(wake_due_sessions(), 0);

    // 未来时刻的标记不被唤醒。
    nemesis_agent::chat_log::write_session_snooze(&key, "2099-01-01T00:00:00+08:00", None);
    assert_eq!(wake_due_sessions(), 0, "未来项不应唤醒");
    let meta = nemesis_agent::chat_log::read_session_meta_full(&key).unwrap();
    assert_eq!(
        meta.snoozed_until.as_deref(),
        Some("2099-01-01T00:00:00+08:00")
    );

    // 说明：会话删除（delete_chat_log）连带 meta——扫描期就看不到标记，
    // 「删除后仍唤醒」不可达；wake 内的 chat_log_exists 守卫覆盖的是
    // 扫描→唤醒窗口内的并发删除竞态（clear 返回 None 分支）。
    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[test]
fn b11_parse_snooze_until_units() {
    // 相对秒。
    let out = parse_snooze_until("3600").unwrap();
    chrono::DateTime::parse_from_rfc3339(&out).unwrap();
    // 带空白。
    let out = parse_snooze_until(" 60 ").unwrap();
    chrono::DateTime::parse_from_rfc3339(&out).unwrap();
    // 绝对时刻（跨时区归一本地）。
    let out = parse_snooze_until("2099-06-01T12:00:00Z").unwrap();
    let dt = chrono::DateTime::parse_from_rfc3339(&out).unwrap();
    // 断言同一时刻而非本地日期串——`%Y-%m-%d` 依赖 runner 时区（UTC+14 及更东会跨日）。
    assert_eq!(
        dt,
        chrono::DateTime::parse_from_rfc3339("2099-06-01T12:00:00Z").unwrap(),
        "归一只换表示不换时刻"
    );
    // 非法臂。
    for bad in ["0", "-3", "abc", "2020-01-01T00:00:00Z"] {
        assert!(parse_snooze_until(bad).is_err(), "{bad} 应报错");
    }
}
