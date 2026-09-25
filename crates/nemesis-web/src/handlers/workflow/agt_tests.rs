//! workflow.rs AGT 覆盖率批次（2026-09-25）。与 tests 互补，聚焦仍缺的
//! 确定性臂：
//! - WebhookRateLimiter 窗口过期弹出臂（向私有 hits 队列直接种 61s 前的
//!   旧时间戳——子模块可见私有字段，同 backdate 家族语义）
//! - clear_chat_password 的持久化失败诚实上抛臂（store 路径父目录是文件
//!   → create_dir_all 必败，Windows/Unix 双平台同构）
//! - draft_get / draft_apply / draft_discard 的缺参/错型 bail 臂
//!
//! 结构性豁免（见报告）：
//! - 778-785 / 1146 / 858-865 三个「workflow vanished after resolve/verify」
//!   臂：workflow_by_chat_index 与 get_workflow 背靠背读同一 DashMap（中间
//!   无 await），单线程内不可分歧；命中需并发删除恰好插在两调用之间——
//!   竞争窗口防御臂，不作确定性测试。
//! - 1371 / 1392 / 1407 的 serde_json::to_value 失败臂：序列化能力表/
//!   草稿详情/应用结果均为可派生 Serialize 的纯数据，实际不可失败——
//!   防御性 ERROR 分支。

use super::*;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};

type WfEngine = nemesis_workflow::engine::WorkflowEngine;
type SecretStore = nemesis_workflow::chat_secrets::ChatSecretStore;

fn agt_ctx(
    engine: Option<std::sync::Arc<WfEngine>>,
    store: Option<std::sync::Arc<SecretStore>>,
) -> RequestContext {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let ws = dir.path().to_string_lossy().to_string();
    let state = std::sync::Arc::new(AppState {
        auth_token: String::new(),
        session_count: std::sync::Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: std::sync::Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: std::sync::Arc::new(AtomicBool::new(false)),
        event_hub: std::sync::Arc::new(crate::events::EventHub::new()),
        running: std::sync::Arc::new(AtomicBool::new(true)),
        session_manager: std::sync::Arc::new(crate::session::SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: engine,
        chat_secret_store: store.unwrap_or_else(|| std::sync::Arc::new(SecretStore::in_memory())),
        webhook_rate_limiter: std::sync::Arc::new(WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

// ---------------------------------------------------------------------------
// WebhookRateLimiter：窗口过期弹出臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_rate_limiter_window_expiry_pops_stale_hits() {
    let limiter = WebhookRateLimiter::new();
    let ip: IpAddr = "203.0.113.9".parse().unwrap();
    // 种一旧一新两个时间戳（旧 = 窗口外 61s）。
    {
        let mut hits = limiter.hits.lock().await;
        let q = hits.entry(ip).or_insert_with(VecDeque::new);
        q.push_back(Instant::now() - Duration::from_secs(61));
        q.push_back(Instant::now());
    }
    // check：先弹窗口外的旧戳（pop_front 臂），遇新戳 break（recent 臂），
    // 队列长度 1 < MAX → 放行并 push 本次命中。
    limiter.check(ip).await.expect("窗口内仍有名额");
    let hits = limiter.hits.lock().await;
    let q = hits.get(&ip).expect("该 IP 队列应存在");
    // 旧戳被弹：种子剩 1（新戳）+ 本次命中 1 = 2。
    assert_eq!(q.len(), 2, "旧时间戳必须被弹出");
}

// ---------------------------------------------------------------------------
// clear_chat_password：持久化失败上抛臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_clear_chat_password_persist_error_surfaces() {
    let dir = tempfile::tempdir().unwrap();
    // 父目录位置放一个普通文件 → create_dir_all 必败。
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a dir").unwrap();
    let store = std::sync::Arc::new(SecretStore::open(blocker.join("secrets.json")));
    // 先落一个密码（磁盘写失败不影响内存表插入——失败被忽略）。
    let _ = store.set_password("agtdead01", "pw");

    let ctx = agt_ctx(Some(WfEngine::new_arc()), Some(store));

    let err = WorkflowHandler
        .handle_cmd(
            "clear_chat_password",
            Some(serde_json::json!({ "index": "agtdead01" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("clear_chat_password failed"),
        "持久化失败必须诚实上抛: {err}"
    );
}

// ---------------------------------------------------------------------------
// draft_* 缺参 / 错型 bail
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_draft_commands_missing_name_guards() {
    let ctx = agt_ctx(Some(WfEngine::new_arc()), None);

    // draft_get：data 缺 / name 缺 / name 非串。
    let err = WorkflowHandler
        .handle_cmd("draft_get", None, &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing data");
    for data in [serde_json::json!({}), serde_json::json!({ "name": 5 })] {
        let err = WorkflowHandler
            .handle_cmd("draft_get", Some(data), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, "missing field: name");
    }

    // draft_apply：name 错型。
    let err = WorkflowHandler
        .handle_cmd("draft_apply", Some(serde_json::json!({ "name": 5 })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing field: name");

    // draft_discard：name 缺 / 非串。
    for data in [serde_json::json!({}), serde_json::json!({ "name": true })] {
        let err = WorkflowHandler
            .handle_cmd("draft_discard", Some(data), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, "missing field: name");
    }
}
