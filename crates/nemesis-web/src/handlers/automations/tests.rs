//! Tests for `AutomationsHandler`（B12）——匹配核纯函数 / 注册表文件
//! round-trip / WSAPI dispatch 链路。pump 全链路（broadcast → engine 观察
//! 执行效果）不铺：需要注册 workflow 才能观测，纯件已覆盖匹配与冷却；
//! spawn_event_pump 的 Closed 退出用冒烟测试钉住。
//!
//! 缓存隔离注记：`TRIGGER_CACHE` 是进程级静态槽，本文件内只有
//! `dispatch_set_list_remove_roundtrip` 写它（set/remove 自带刷新），断言
//! 也自成闭环（写→读→清空），与并行测试无交叉断言。

use super::*;

// ---------------------------------------------------------------------------
// pattern_matches / validate_pattern
// ---------------------------------------------------------------------------

#[test]
fn pattern_matches_exact_and_wildcard() {
    assert!(pattern_matches("exec", "exec"));
    assert!(!pattern_matches("exec", "exec2"));
    assert!(pattern_matches("mcp_*", "mcp_fetch"));
    assert!(pattern_matches("mcp_*", "mcp_"));
    assert!(!pattern_matches("mcp_*", "web_fetch"));
    // 裸 `*` = 前缀空串，命中一切（合法形态，防风暴靠冷却）。
    assert!(pattern_matches("*", "anything"));
    assert!(pattern_matches("*", ""));
}

#[test]
fn validate_pattern_rejects_bad_shapes() {
    assert!(validate_pattern("exec").is_ok());
    assert!(validate_pattern("mcp_*").is_ok());
    assert!(validate_pattern("*").is_ok());
    assert!(validate_pattern("").is_err());
    assert!(validate_pattern("   ").is_err());
    assert!(validate_pattern("a*b").is_err(), "中间星号拒绝");
    assert!(validate_pattern("**").is_err(), "多星号拒绝");
    assert!(validate_pattern("a*").is_ok(), "结尾星号合法");
}

// ---------------------------------------------------------------------------
// 冷却闸 + select_firing
// ---------------------------------------------------------------------------

fn trigger(id: &str, pattern: &str, cooldown_secs: u64, enabled: bool) -> AutomationTrigger {
    AutomationTrigger {
        id: id.to_string(),
        pattern: pattern.to_string(),
        workflow_id: format!("wf-{id}"),
        cooldown_secs,
        enabled,
        created_at: 0,
    }
}

#[test]
fn cooldown_suppresses_repeat_but_per_trigger() {
    let mut gate = CooldownGate::default();
    let t0 = Instant::now();
    let all = vec![
        trigger("a", "exec", 60, true),
        trigger("b", "exec", 60, true),
        trigger("c", "exec", 0, false),
        trigger("d", "exec", 0, true),
    ];

    // 首触发：disabled 不触发；cooldown=0 每次都触发。
    let fired = select_firing(&all, "exec", &mut gate, t0);
    assert_eq!(
        fired.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "d"]
    );

    // 同一时刻再触发：a/b 被各自冷却压住，d（0 冷却）继续。
    let fired = select_firing(&all, "exec", &mut gate, t0);
    assert_eq!(
        fired.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["d"]
    );

    // 冷却中点：a/b 仍被压住，d 继续。
    let mid = t0 + Duration::from_secs(30);
    let fired = select_firing(&all, "exec", &mut gate, mid);
    assert_eq!(
        fired.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["d"]
    );

    // 冷却过后 a/b 都恢复（per-trigger 记账互不干扰；记账时刻=首触发）。
    let later = t0 + Duration::from_secs(61);
    let fired = select_firing(&all, "exec", &mut gate, later);
    assert_eq!(
        fired.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "d"]
    );
}

#[test]
fn select_firing_no_match_is_free() {
    let mut gate = CooldownGate::default();
    let all = vec![trigger("a", "web_*", 0, true)];
    assert!(select_firing(&all, "exec", &mut gate, Instant::now()).is_empty());
    assert!(select_firing(&[], "exec", &mut gate, Instant::now()).is_empty());
}

// ---------------------------------------------------------------------------
// 注册表文件 round-trip
// ---------------------------------------------------------------------------

#[test]
fn registry_round_trip_and_corrupt_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = registry_path(dir.path());
    assert!(
        load_triggers_from_file(&path).unwrap().is_empty(),
        "缺文件 = 空表（fresh 常态不报错）"
    );

    let triggers = vec![AutomationTrigger {
        id: "at-x".to_string(),
        pattern: "exec".to_string(),
        workflow_id: "wf".to_string(),
        cooldown_secs: 30,
        enabled: true,
        created_at: 42,
    }];
    save_triggers(&path, &triggers).unwrap();
    let loaded = load_triggers_from_file(&path).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "at-x");
    assert_eq!(loaded[0].pattern, "exec");
    assert_eq!(loaded[0].cooldown_secs, 30);
    assert_eq!(loaded[0].created_at, 42);

    // 坏 JSON 诚实报错（静默当空表 = 用户以为规则还在生效）。
    std::fs::write(&path, "{ not json").unwrap();
    let err = load_triggers_from_file(&path).unwrap_err();
    assert!(err.contains("解析失败"), "got: {err}");
}

// ---------------------------------------------------------------------------
// Dispatch wiring（ctx.workspace 路径 + 缓存同步）
// ---------------------------------------------------------------------------

/// Same AppState scaffold as hooks tests（agent_loop None etc.）。
fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
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
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn dispatch_set_list_remove_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = AutomationsHandler;

    // fresh list：空表常态。
    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["count"], 0);

    // set（engine None → workflow 存在性校验诚实跳过）+ 缺省 id 服务端生成。
    let r = h
        .handle_cmd(
            "set",
            Some(serde_json::json!({"pattern": "exec", "workflow_id": "wf-a"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(r["saved"].as_bool().unwrap_or(false));
    let id = r["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("at-"), "缺省 id 形态 at-<hex8>: {id}");

    // 同 id 再 set = upsert 覆盖（不新增条目），created_at 保留。
    let r = h
        .handle_cmd(
            "set",
            Some(serde_json::json!({
                "id": id,
                "pattern": "web_*",
                "workflow_id": "wf-a",
                "cooldown_secs": 5,
                "enabled": false,
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(r["saved"].as_bool().unwrap_or(false));
    assert_eq!(r["count"], 1, "upsert 不新增条目");

    let r = h.handle_cmd("list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["count"], 1);
    assert_eq!(r["triggers"][0]["pattern"], "web_*");
    assert!(!r["triggers"][0]["enabled"].as_bool().unwrap_or(true));
    assert_eq!(r["triggers"][0]["cooldown_secs"], 5);

    // 进程缓存同步（泵读的是缓存，不是盘）。
    let cached = cached_triggers();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].pattern, "web_*");
    assert!(!cached[0].enabled);

    // 校验臂：空 pattern / 中间星号 / 空 workflow_id。
    let err = h
        .handle_cmd(
            "set",
            Some(serde_json::json!({"pattern": "", "workflow_id": "wf"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("pattern"), "got: {err}");
    let err = h
        .handle_cmd(
            "set",
            Some(serde_json::json!({"pattern": "a*b", "workflow_id": "wf"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("结尾"), "got: {err}");
    let err = h
        .handle_cmd(
            "set",
            Some(serde_json::json!({"pattern": "exec", "workflow_id": ""})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("workflow_id"), "got: {err}");

    // remove + 再删诚实报错 + 缓存同步清空。
    let r = h
        .handle_cmd("remove", Some(serde_json::json!({"id": id})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["removed"], id);
    let err = h
        .handle_cmd("remove", Some(serde_json::json!({"id": id})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "got: {err}");
    assert!(cached_triggers().is_empty(), "remove 后缓存同步清空");

    // 未知命令。
    assert!(h.handle_cmd("bogus", None, &ctx).await.is_err());
}

// ---------------------------------------------------------------------------
// Pump smoke：sender 全 drop → 任务退出（gateway 收尾语义）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pump_exits_when_sender_dropped() {
    let (tx, rx) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(16);
    let engine = nemesis_workflow::engine::WorkflowEngine::new_arc();
    let handle = spawn_event_pump(rx, engine);
    drop(tx);
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("pump must exit after senders drop")
        .expect("pump task must not panic");
}
