//! （2026-11-03 snooze 排队 + 跨设备移动 + session PR 最小形态）：
//! sessions handler 层测试。
//!
//! - import/changed_files 新动词（守卫链 / VERBATIM roundtrip / 导出→删→
//!   导入搬运闭环 / file_changes 聚合投影）；
//! - snooze 重放 hook（unsnooze delivered 计数 / wake 到期重放消费 pending /
//!   无 hook 滞留保留契约）；
//! - drain_orphan_pending 滞留兜底四臂（已删会话作废 / 活会话补投 / 暂停中
//!   跳过 / hook 缺席保留）；
//! - list pendingCount 投影（>0 才带字段）。
//!
//! home 单例经 HomeGuard 重定向（sessions_snooze_tests 同款）；hook 槽是
//! 进程全局态——所有 hook 置换测试都在 lock_home 串行区内先显式设槽、
//! 结束复位（对前序 panic 残留鲁棒：每个需要特定 hook 形态的测试自设）。

use super::sessions::{
    SessionsHandler, drain_orphan_pending, replay_pending_for, set_snooze_replay_hook,
    wake_due_sessions,
};
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
        quic_relay: None,
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
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "l22".to_string(),
        chat_id: "l22".to_string(),
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

/// 造一个存活会话（与 sessions_snooze_tests::seed_session 同款）。
fn seed_session(home: &Path, sid: &str) -> String {
    let key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    );
    nemesis_agent::chat_log::append_chat_log(&key, "user", "hi");
    let _ = home;
    key
}

/// 造一条待排队消息（session_key 留空——append_session_pending 烧入裁决键）。
fn queued_msg(content: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

/// 假 hook：记录被调键 + 按真 hook 语义消费 pending（take 后返条数）。
fn install_consuming_hook() -> Arc<parking_lot::Mutex<Vec<String>>> {
    let seen = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let captured = seen.clone();
    set_snooze_replay_hook(Some(Arc::new(move |key: &str| {
        captured.lock().push(key.to_string());
        nemesis_agent::chat_log::take_session_pending(key).len()
    })));
    seen
}

fn reset_hook() {
    set_snooze_replay_hook(None);
}

// import：守卫链 + VERBATIM roundtrip + 冲突拒绝

#[tokio::test]
async fn l22_import_guards() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);

    let row = serde_json::json!({"role": "user", "content": "q"});

    // data 缺 / messages 缺 / 空 / 超行形态（单条守卫即可验证分支存在）。
    for (data, expect) in [
        (None, "missing data"),
        (Some(serde_json::json!({})), "missing messages"),
        (Some(serde_json::json!({"messages": []})), "messages 为空"),
    ] {
        let err = run(&ctx, "import", data).await.unwrap_err();
        assert_eq!(err, expect, "guard 臂报错不符");
    }

    // 行校验：非对象 / 缺 role+content。
    let err = run(
        &ctx,
        "import",
        Some(serde_json::json!({"messages": ["not-an-object"]})),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "messages[0] 不是对象");
    let err = run(
        &ctx,
        "import",
        Some(serde_json::json!({"messages": [{"role": "user"}]})),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "messages[0] 缺 role/content 字段");

    // 显式 session_id 指向已存在会话 = 诚实拒绝。
    let key = seed_session(&home, "exists");
    let err = run(
        &ctx,
        "import",
        Some(serde_json::json!({"session_id": "exists", "messages": [row]})),
    )
    .await
    .unwrap_err();
    assert!(err.contains("会话已存在"), "got: {err}");
    assert!(err.contains("导入不覆盖"), "拒绝文案须说明语义: {err}");

    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn l22_import_happy_explicit_sid_and_uuid_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);

    // 显式 sid + title：落盘 VERBATIM（含非白名单字段 timestamp/model）。
    let rows = serde_json::json!([
        {"role": "user", "content": "问题", "timestamp": "2026-01-01T00:00:00"},
        {"role": "assistant", "content": "回答", "model": "testai-1.1"}
    ]);
    let out = run(
        &ctx,
        "import",
        Some(serde_json::json!({
            "session_id": "  imp1  ",
            "title": "导入会话",
            "messages": rows
        })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["session_id"], "imp1", "sid 应 trim");
    assert_eq!(out["imported"], 2);

    let key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id("imp1")
    );
    let (msgs, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 100_000, None);
    assert_eq!(total, 2);
    assert_eq!(msgs[0]["content"], "问题");
    assert_eq!(msgs[0]["timestamp"], "2026-01-01T00:00:00", "VERBATIM 保真");
    assert_eq!(msgs[1]["model"], "testai-1.1", "非白名单字段不剥离");
    let meta = nemesis_agent::chat_log::read_session_meta_full(&key).unwrap();
    assert_eq!(
        meta.title.as_deref(),
        Some("导入会话"),
        "manual title 落 meta"
    );

    // 缺 session_id：uuid 兜底（合法 sid 形态，行数一致）。
    let out = run(
        &ctx,
        "import",
        Some(serde_json::json!({"messages": [
            {"role": "user", "content": "auto"}
        ]})),
    )
    .await
    .unwrap()
    .unwrap();
    let auto_sid = out["session_id"].as_str().expect("uuid sid 缺失");
    assert!(!auto_sid.is_empty() && auto_sid != "imp1");
    let auto_key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(auto_sid)
    );
    assert_eq!(out["imported"], 1);
    let (_, total, _, _) = nemesis_agent::chat_log::read_chat_log(&auto_key, 100_000, None);
    assert_eq!(total, 1);

    nemesis_agent::chat_log::delete_chat_log(&key);
    nemesis_agent::chat_log::delete_chat_log(&auto_key);
}

/// 跨设备移动闭环：导出 → 删除 → 导入 → 读回一致（会话历史完整可续聊）。
#[tokio::test]
async fn l22_export_delete_import_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    let key = seed_session(&home, "mv1");
    nemesis_agent::chat_log::append_chat_log(&key, "assistant", "答");

    // 导出。
    let exported = run(
        &ctx,
        "export",
        Some(serde_json::json!({"session_id": "mv1"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(exported["count"], 2);
    let messages = exported["messages"].clone();
    assert_eq!(messages.as_array().unwrap().len(), 2);

    // 删除（模拟换设备：源端清掉）。
    nemesis_agent::chat_log::delete_chat_log(&key);
    assert!(!nemesis_agent::chat_log::chat_log_exists(&key));

    // 导入同 sid：重建成功且逐行保真。
    let out = run(
        &ctx,
        "import",
        Some(serde_json::json!({"session_id": "mv1", "messages": messages})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["session_id"], "mv1");
    assert_eq!(out["imported"], 2);
    let (msgs, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 100_000, None);
    assert_eq!(total, 2);
    assert_eq!(msgs[0]["role"], "user");
    assert_eq!(msgs[0]["content"], "hi");
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["content"], "答");

    nemesis_agent::chat_log::delete_chat_log(&key);
}

// changed_files：session PR 最小形态（file_changes 聚合投影）

#[tokio::test]
async fn l22_changed_files_aggregates_deduped() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    let key = seed_session(&home, "pr1");

    let fc = |path: &str| nemesis_agent::r#loop::FileChange {
        path: path.to_string(),
        kind: nemesis_agent::r#loop::FileChangeKind::Modify,
    };
    nemesis_agent::chat_log::append_chat_log_meta(
        &key,
        "assistant",
        "r1",
        &nemesis_agent::chat_log::ChatLogMeta {
            file_changes: &[fc("src/a.rs"), fc("src/b.rs")],
            ..Default::default()
        },
    );
    nemesis_agent::chat_log::append_chat_log_meta(
        &key,
        "assistant",
        "r2",
        &nemesis_agent::chat_log::ChatLogMeta {
            file_changes: &[fc("src/a.rs")], // 跨 turn 重复 → 去重
            ..Default::default()
        },
    );

    let out = run(
        &ctx,
        "changed_files",
        Some(serde_json::json!({"session_id": "pr1"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["session_id"], "pr1");
    assert_eq!(
        out["files"],
        serde_json::json!(["src/a.rs", "src/b.rs"]),
        "去重保首次出现序"
    );

    // 无 file_changes 的会话 = 空清单（诚实缺省）。
    let key2 = seed_session(&home, "pr2");
    let out = run(
        &ctx,
        "changed_files",
        Some(serde_json::json!({"session_id": "pr2"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["files"], serde_json::json!([]));

    nemesis_agent::chat_log::delete_chat_log(&key);
    nemesis_agent::chat_log::delete_chat_log(&key2);
}

// snooze 重放 hook：unsnooze delivered / wake 到期重放 / 无 hook 滞留保留

/// 无 hook（单测/未装配）= 0 投递——pending 保留在盘的诚实契约（调用方
/// 据此不动文件，sweeper 重试）。
#[tokio::test]
async fn l22_replay_pending_without_hook_is_zero() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    reset_hook();
    assert_eq!(replay_pending_for("agent:main:session:nowhere"), 0);
}

#[tokio::test]
async fn l22_unsnooze_reports_delivered_count() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    let key = seed_session(&home, "uq1");

    // 预置暂停期排队两条 + 消费型假 hook。
    for c in ["排队的1", "排队的2"] {
        nemesis_agent::chat_log::append_session_pending(&key, &queued_msg(c)).unwrap();
    }
    let seen = install_consuming_hook();

    let out = run(
        &ctx,
        "unsnooze",
        Some(serde_json::json!({"session_id": "uq1"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["unsnoozed"], false, "本会话无标记（幂等臂）");
    assert_eq!(out["delivered"], 2, "滞留救援重放计数");
    assert_eq!(
        seen.lock().as_slice(),
        [key.as_str()],
        "hook 收到完整会话键"
    );
    assert_eq!(
        nemesis_agent::chat_log::session_pending_count(&key),
        0,
        "消费型 hook 取走 pending"
    );

    reset_hook();
    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn l22_wake_replays_pending_and_consumes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let key = seed_session(&home, "wr1");

    // 到期标记 + 暂停期排队消息。
    nemesis_agent::chat_log::write_session_snooze(&key, "2020-01-01T00:00:00+08:00", None);
    nemesis_agent::chat_log::append_session_pending(&key, &queued_msg("暂停期消息")).unwrap();

    let seen = install_consuming_hook();
    let woken = wake_due_sessions();
    assert_eq!(woken, 1);
    assert_eq!(seen.lock().as_slice(), [key.as_str()], "hook 收完整键");
    assert_eq!(
        nemesis_agent::chat_log::session_pending_count(&key),
        0,
        "到期唤醒必须重放消费暂停期消息"
    );
    // 唤醒序（五件套）：恢复 system 行照常落盘。
    let (msgs, _, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 50, None);
    assert!(
        msgs.iter()
            .any(|m| m["role"] == "system" && m["content"].as_str().unwrap_or("").contains("⏰")),
        "唤醒恢复行应在场"
    );

    reset_hook();
    // 二次 wake = 0（标记已摘 + pending 已清）。
    assert_eq!(wake_due_sessions(), 0);
    nemesis_agent::chat_log::delete_chat_log(&key);
}

/// hook 缺席（loop 停机窗口）：wake 照常摘标记+落恢复行，pending **保留**
/// 在盘（sweeper 下轮 orphan 臂重试——自愈不丢失的诚实契约）。
#[tokio::test]
async fn l22_wake_without_hook_retains_pending() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let key = seed_session(&home, "wr2");

    nemesis_agent::chat_log::write_session_snooze(&key, "2020-01-01T00:00:00+08:00", None);
    nemesis_agent::chat_log::append_session_pending(&key, &queued_msg("滞留消息")).unwrap();

    reset_hook(); // 显式无 hook（对前序测试 panic 残留鲁棒）
    assert_eq!(wake_due_sessions(), 1, "唤醒本身不依赖 hook");
    assert_eq!(
        nemesis_agent::chat_log::session_pending_count(&key),
        1,
        "无 hook = 0 投递，pending 必须保留待重试"
    );

    // orphan 兜底接管：hook 到位后补投。
    let seen = install_consuming_hook();
    drain_orphan_pending();
    assert_eq!(seen.lock().as_slice(), [key.as_str()]);
    assert_eq!(nemesis_agent::chat_log::session_pending_count(&key), 0);

    reset_hook();
    nemesis_agent::chat_log::delete_chat_log(&key);
}

// drain_orphan_pending：滞留兜底四臂

#[tokio::test]
async fn l22_drain_orphan_four_arms() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let seen = install_consuming_hook();

    // 臂① 已删会话（jsonl 没了、pending 竞态残留）→ 作废（不走 hook——
    // 防复活已删会话）。
    let dead_key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id("orphan-dead")
    );
    nemesis_agent::chat_log::append_session_pending(&dead_key, &queued_msg("遗言")).unwrap();
    assert!(!nemesis_agent::chat_log::chat_log_exists(&dead_key));

    // 臂② 暂停中会话 → 跳过（正常唤醒流程负责）。
    let snoozed_key = seed_session(&home, "orphan-snoozed");
    nemesis_agent::chat_log::write_session_snooze(&snoozed_key, "2099-01-01T00:00:00+08:00", None);
    nemesis_agent::chat_log::append_session_pending(&snoozed_key, &queued_msg("还没到点")).unwrap();

    // 臂③ 活会话 + pending → 补投。
    let live_key = seed_session(&home, "orphan-live");
    nemesis_agent::chat_log::append_session_pending(&live_key, &queued_msg("补投")).unwrap();

    drain_orphan_pending();

    assert_eq!(
        nemesis_agent::chat_log::session_pending_count(&dead_key),
        0,
        "已删会话 pending 必须作废"
    );
    assert_eq!(
        nemesis_agent::chat_log::session_pending_count(&snoozed_key),
        1,
        "暂停中的 pending 不得被 orphan 臂抢投"
    );
    assert_eq!(
        nemesis_agent::chat_log::session_pending_count(&live_key),
        0,
        "活会话滞留 pending 应补投"
    );
    assert_eq!(
        seen.lock().as_slice(),
        [live_key.as_str()],
        "hook 只对活会话调用"
    );

    reset_hook();
    nemesis_agent::chat_log::delete_chat_log(&snoozed_key);
    nemesis_agent::chat_log::delete_chat_log(&live_key);
}

/// hook 缺席时 orphan 臂对活会话零动作（文件保留，下轮重试）。
#[tokio::test]
async fn l22_drain_orphan_without_hook_retains() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    reset_hook();

    let live_key = seed_session(&home, "orphan-stall");
    nemesis_agent::chat_log::append_session_pending(&live_key, &queued_msg("等 hook")).unwrap();

    drain_orphan_pending();
    assert_eq!(
        nemesis_agent::chat_log::session_pending_count(&live_key),
        1,
        "无 hook = 补投不了，文件保留"
    );

    nemesis_agent::chat_log::delete_chat_log(&live_key);
}

// list：pendingCount 投影

#[tokio::test]
async fn l22_list_projects_pending_count() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);

    let clean_key = seed_session(&home, "lp-clean");
    let queued_key = seed_session(&home, "lp-queued");
    nemesis_agent::chat_log::append_session_pending(&queued_key, &queued_msg("徽标")).unwrap();
    nemesis_agent::chat_log::append_session_pending(&queued_key, &queued_msg("两条")).unwrap();

    let out = run(&ctx, "list", None).await.unwrap().unwrap();
    let sessions = out["sessions"].as_array().expect("sessions 数组");
    let find = |sid: &str| {
        sessions
            .iter()
            .find(|s| s["id"] == serde_json::Value::String(sid.to_string()))
            .unwrap_or_else(|| panic!("list 必须含 {sid}"))
            .clone()
    };
    let queued_row = find("lp-queued");
    assert_eq!(
        queued_row["pendingCount"],
        serde_json::json!(2),
        "排队条数投影: {queued_row}"
    );
    let clean_row = find("lp-clean");
    assert!(
        clean_row.get("pendingCount").is_none(),
        "零排队不得带字段: {clean_row}"
    );

    nemesis_agent::chat_log::delete_chat_log(&clean_key);
    nemesis_agent::chat_log::delete_chat_log(&queued_key);
}
