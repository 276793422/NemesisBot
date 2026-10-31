//! 会话回收站（2026-10-30）WSAPI handler 层测试：sessions.delete 默认
//! 移入回收站（拍板③）+ `hard:true` 旧硬删除语义 / trash_list /
//! trash_restore（含拍板② 冲突改名恢复）/ trash_purge / trash_purge_all
//! 四命令守卫与全链。
//!
//! home 单例经 session_bindings::tests 同款 HomeGuard 重定向到 tempdir
//! （test_home::lock_home 可重入锁跨测试串行）——production() 寻址按调用
//! 动态走 default_path_manager()，重定向后全部落 tempdir，零真实目录触碰。
//! 回收站核心语义（move 三件套/清单/孤儿/恢复冲突结构化返回）在
//! nemesis-agent chat_trash 注入测试（trash_tests.rs）已覆盖，这里钉
//! handler 编排面：delete 回执形态 + 四命令的参数契约与串联。

use super::sessions::SessionsHandler;
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
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "trash".to_string(),
        chat_id: "trash".to_string(),
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

fn key_of(sid: &str) -> String {
    format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    )
}

/// 造一个存活会话（append 走单例 home——重定向后落 tempdir）。
fn seed(home: &Path, sid: &str, msg: &str) -> String {
    let _ = home; // 表意：会话落在 <home>/workspace/logs/session_logs
    let key = key_of(sid);
    nemesis_agent::chat_log::append_chat_log(&key, "user", msg);
    key
}

// ---------------------------------------------------------------------------
// delete：默认回收站（拍板③） vs hard=true 旧语义
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_defaults_to_trash_and_list_sees_entry() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    let sid = "deltrash1";
    let key = seed(&home, sid, "要被删的话");

    let out = run(
        &ctx,
        "delete",
        Some(serde_json::json!({ "session_id": sid })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["deleted"], serde_json::json!(sid));
    assert_eq!(out["trashed"], serde_json::Value::Bool(true));
    let entry = out["trash_entry"].as_str().unwrap().to_string();
    assert!(!entry.is_empty());

    // 会话消失（jsonl 已移走）。
    assert!(!nemesis_agent::chat_log::chat_log_exists(&key));
    // 回收站列表看得到条目（title 缺席 = meta 未物化的诚实投影）。
    let list = run(&ctx, "trash_list", None).await.unwrap().unwrap();
    let entries = list["entries"].as_array().unwrap();
    let found = entries
        .iter()
        .find(|e| e["entry"] == serde_json::json!(entry))
        .expect("delete 后条目须在列表");
    assert_eq!(found["source"], serde_json::json!("single"));
    assert_eq!(found["orphan"], serde_json::Value::Bool(false));

    // 恢复走 handler 全链：会话复活 + 条目清。
    let out = run(
        &ctx,
        "trash_restore",
        Some(serde_json::json!({ "entry": entry })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["restored"], serde_json::json!(key));
    assert!(nemesis_agent::chat_log::chat_log_exists(&key));

    // 清场（防 tempdir 泄漏残留语义误判，虽然 Drop 兜底）。
    nemesis_agent::chat_log::delete_chat_log(&key);
}

/// 批量删除来源（2026-10-30 浏览器 E2E T4 根修）：`source: "batch"` 写进
/// trash.json 并在列表投影为「批量删除」徽标；未知值诚实落 single。
#[tokio::test]
async fn delete_source_batch_marks_entry_and_unknown_falls_back_single() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);

    // batch：侧栏批量循环的显式来源。
    let sid_b = "delbatch1";
    let key_b = seed(&home, sid_b, "批量删我");
    let out = run(
        &ctx,
        "delete",
        Some(serde_json::json!({ "session_id": sid_b, "source": "batch" })),
    )
    .await
    .unwrap()
    .unwrap();
    let entry_b = out["trash_entry"].as_str().unwrap().to_string();

    // 未知值：展示性字段不硬闸，落 single。
    let sid_u = "delbatch2";
    let key_u = seed(&home, sid_u, "未知来源");
    let out = run(
        &ctx,
        "delete",
        Some(serde_json::json!({ "session_id": sid_u, "source": "hooligan" })),
    )
    .await
    .unwrap()
    .unwrap();
    let entry_u = out["trash_entry"].as_str().unwrap().to_string();

    let list = run(&ctx, "trash_list", None).await.unwrap().unwrap();
    let entries = list["entries"].as_array().unwrap();
    let found = |e: &str| {
        entries
            .iter()
            .find(|x| x["entry"] == serde_json::json!(e))
            .expect("条目须在列表")
            .clone()
    };
    assert_eq!(found(&entry_b)["source"], serde_json::json!("batch"));
    assert_eq!(found(&entry_u)["source"], serde_json::json!("single"));

    // 恢复一条供清场对称（另一条 tempdir Drop 兜底）。
    nemesis_agent::chat_log::delete_chat_log(&key_b);
    nemesis_agent::chat_log::delete_chat_log(&key_u);
}

#[tokio::test]
async fn delete_hard_true_keeps_legacy_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    let sid = "delhard1";
    let key = seed(&home, sid, "硬删对象");

    let out = run(
        &ctx,
        "delete",
        Some(serde_json::json!({ "session_id": sid, "hard": true })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["trashed"], serde_json::Value::Bool(false));
    assert!(out["trash_entry"].is_null());

    let list = run(&ctx, "trash_list", None).await.unwrap().unwrap();
    assert!(
        list["entries"].as_array().unwrap().is_empty(),
        "硬删除不进回收站: {}",
        list["entries"]
    );
    // 退化 ctx（无 agent_loop）的已知形态（S10b 记录在案，delete 臂注释
    // 引用）：store 臂跳过 → jsonl 幸存。生产 gateway 恒有 loop；这里钉
    // 的是「hard 永不进回收站」的 trash 面契约（上方空列表断言）。
    assert!(
        nemesis_agent::chat_log::chat_log_exists(&key),
        "无 loop 退化 ctx 下 store 臂跳过，jsonl 幸存（S10b 行为）"
    );
}

#[tokio::test]
async fn delete_missing_session_id_bails() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    let err = run(&ctx, "delete", Some(serde_json::json!({})))
        .await
        .unwrap_err();
    assert_eq!(err, "missing session_id");
}

// ---------------------------------------------------------------------------
// trash 四命令守卫
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trash_command_guards() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);

    // trash_restore / trash_purge 缺 entry。
    for cmd in ["trash_restore", "trash_purge"] {
        let err = run(&ctx, cmd, Some(serde_json::json!({})))
            .await
            .unwrap_err();
        assert_eq!(err, "missing entry", "{cmd} 缺参须报 missing entry");
    }
    // 注入形态名字（/../）在 core 校验闸被拒（handler 不吞错）。
    for cmd in ["trash_restore", "trash_purge"] {
        let err = run(&ctx, cmd, Some(serde_json::json!({"entry": "../escape"})))
            .await
            .unwrap_err();
        assert!(err.contains("非法"), "{cmd} 须诚实拒绝注入名: {err}");
    }
    // 空回收站列表 = 空数组（目录不存在诚实空）。
    let list = run(&ctx, "trash_list", None).await.unwrap().unwrap();
    assert!(list["entries"].as_array().unwrap().is_empty());
    // 清空空回收站 = 0（幂等成功）。
    let out = run(&ctx, "trash_purge_all", None).await.unwrap().unwrap();
    assert_eq!(out["purged"], serde_json::json!(0));
}

// ---------------------------------------------------------------------------
// 全链：双删并存 + purge / purge_all / 冲突改名（拍板②）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trash_purge_and_purge_all_through_handler() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    // pg1 / pg3 删（进回收站）；pg2 存活（不进回收站，不该被 purge_all 波及）。
    seed(&home, "pg1", "一");
    seed(&home, "pg2", "二");
    seed(&home, "pg3", "三");
    let e1 = run(
        &ctx,
        "delete",
        Some(serde_json::json!({"session_id": "pg1"})),
    )
    .await
    .unwrap()
    .unwrap()["trash_entry"]
        .as_str()
        .unwrap()
        .to_string();
    run(
        &ctx,
        "delete",
        Some(serde_json::json!({"session_id": "pg3"})),
    )
    .await
    .unwrap()
    .unwrap();

    // 单条 purge。
    let out = run(&ctx, "trash_purge", Some(serde_json::json!({"entry": e1})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["purged"], serde_json::json!(e1));

    // 清空剩余（pg3 的条目）；pg2 仍存活在 session_logs。
    let out = run(&ctx, "trash_purge_all", None).await.unwrap().unwrap();
    assert_eq!(out["purged"], serde_json::json!(1));
    let list = run(&ctx, "trash_list", None).await.unwrap().unwrap();
    assert!(list["entries"].as_array().unwrap().is_empty());
    assert!(
        nemesis_agent::chat_log::chat_log_exists(&key_of("pg2")),
        "存活会话不受清空波及"
    );
}

#[tokio::test]
async fn restore_conflict_rename_flow_through_handler() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("nb-home");
    let _g = isolate(&home);
    let ctx = make_ctx(&home);
    let sid = "conflict1";
    let key = seed(&home, sid, "旧内容");

    let entry = run(&ctx, "delete", Some(serde_json::json!({"session_id": sid})))
        .await
        .unwrap()
        .unwrap()["trash_entry"]
        .as_str()
        .unwrap()
        .to_string();

    // 同 id 重建（占位冲突方）。
    let key2 = seed(&home, sid, "占位新内容");
    assert_eq!(key2, key);

    // 无 new_sid → 结构化冲突（Ok 非 Err）+ 建议 sid；文件不动。
    let out = run(
        &ctx,
        "trash_restore",
        Some(serde_json::json!({ "entry": entry })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["conflict"], serde_json::Value::Bool(true));
    let suggested = out["suggested_sid"].as_str().unwrap().to_string();
    assert!(!suggested.is_empty());
    let list = run(&ctx, "trash_list", None).await.unwrap().unwrap();
    assert_eq!(
        list["entries"].as_array().unwrap().len(),
        1,
        "冲突不消耗条目"
    );

    // 拍板②：带建议 sid 改名恢复成功，内容是回收站里的旧内容。
    let out = run(
        &ctx,
        "trash_restore",
        Some(serde_json::json!({ "entry": entry, "new_sid": suggested })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["renamed"], serde_json::Value::Bool(true));
    let new_key = out["restored"].as_str().unwrap().to_string();
    assert_ne!(new_key, key, "改名恢复落新键");
    let (msgs, n, _, _) = nemesis_agent::chat_log::read_chat_log(&new_key, 100, None);
    assert_eq!(n, 1, "恢复的是回收站旧内容");
    assert_eq!(msgs[0]["content"], serde_json::json!("旧内容"));
    // 条目清空。
    let list = run(&ctx, "trash_list", None).await.unwrap().unwrap();
    assert!(list["entries"].as_array().unwrap().is_empty());

    // 清场。
    nemesis_agent::chat_log::delete_chat_log(&new_key);
    nemesis_agent::chat_log::delete_chat_log(&key);
}
