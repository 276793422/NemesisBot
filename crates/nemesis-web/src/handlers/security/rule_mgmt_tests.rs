//! 条目级规则管理（批次 3）：approvals.add/remove（nemesis-security
//! upsert/remove_manual_rule 同源闸）+ eval_rules CRUD（数据半边
//! nemesis-config::eval_rules 单一真相源，语义镜像 CLI eval rules *）。
//!
//! 用临时目录当 workspace（路径函数以显式 workspace 为根，无全局态），
//! 不触碰真实 home；ctx 构造同 f3_tests make_ctx 形态。

use super::*;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::Path;
use std::sync::Arc;

fn make_ctx(ws: &str) -> RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let state = Arc::new(AppState {
        cors_manager: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.to_string()),
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
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
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "rule-mgmt".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(ws.to_string()),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

// ---------- approvals.add / approvals.remove ----------

#[tokio::test]
async fn approvals_add_happy_and_matches() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let v = handler
        .handle_cmd(
            "approvals.add",
            Some(serde_json::json!({
                "op": "process_exec",
                "pattern": "  cargo test *  ",
                "action": "allow",
            })),
            &make_ctx(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["added"], true);
    assert_eq!(v["pattern"], "cargo test *", "pattern trim 后落盘");
    // list 回读同条目（trim 生效）
    let rules = handler
        .handle_cmd("approvals.list", None, &make_ctx(&ws))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rules["rules"].as_array().unwrap().len(), 1);
    assert_eq!(rules["rules"][0]["op"], "process_exec");
    // 匹配侧真放行（同源闸收口的实证：非死条目）
    let path = approval_rules_path(&ws);
    let stored = nemesis_security::approval_rules::load_rules(&path);
    assert!(
        nemesis_security::approval_rules::find_auto_allow_rule(
            &stored,
            "process_exec",
            "cargo test --release",
            "CRITICAL"
        )
        .is_some()
    );
}

#[tokio::test]
async fn approvals_add_gate_rejects_dead_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    // 安装类永不入规则
    let err = handler
        .handle_cmd(
            "approvals.add",
            Some(serde_json::json!({ "op": "plugins.install", "pattern": "/tmp/src" })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("never enters the rule table"), "got: {err}");
    // 静态 CRITICAL 非 exec（死条目）拒绝
    let err = handler
        .handle_cmd(
            "approvals.add",
            Some(serde_json::json!({ "op": "system_shutdown", "pattern": "now" })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("always stay manual"), "got: {err}");
    // v1 deny 拒绝
    let err = handler
        .handle_cmd(
            "approvals.add",
            Some(serde_json::json!({ "op": "file_write", "pattern": "/tmp/a", "action": "deny" })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("allow-only"), "got: {err}");
    // 拒绝臂零落盘
    assert!(!Path::new(&approval_rules_path(&ws)).exists());
}

#[tokio::test]
async fn approvals_remove_counts_honestly() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    for pat in ["cargo test *", "cargo build *"] {
        handler
            .handle_cmd(
                "approvals.add",
                Some(serde_json::json!({ "op": "process_exec", "pattern": pat })),
                &make_ctx(&ws),
            )
            .await
            .unwrap()
            .unwrap();
    }
    let v = handler
        .handle_cmd(
            "approvals.remove",
            Some(serde_json::json!({ "op": "process_exec", "pattern": "cargo test *" })),
            &make_ctx(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["removed"], 1);
    // 不存在条目 = removed 0（诚实上报不报错）
    let v = handler
        .handle_cmd(
            "approvals.remove",
            Some(serde_json::json!({ "op": "process_exec", "pattern": "cargo test *" })),
            &make_ctx(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["removed"], 0);
}

// ---------- eval_rules CRUD ----------

fn good_rule_json(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "description": "测试规则",
        "level": "high",
        "source": "tool_trace",
        "conditions": [
            { "field": "arguments.command", "op": "contains", "value": "reg add" }
        ],
        "min_count": 1,
    })
}

#[tokio::test]
async fn eval_rules_list_seeds_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let v = handler
        .handle_cmd("eval_rules.list", None, &make_ctx(&ws))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["version"], 1);
    let rules = v["rules"].as_array().expect("rules array");
    assert!(!rules.is_empty(), "首访种子内置默认集");
    // 种子真落盘（与 CLI list 同源语义）
    assert!(Path::new(&eval_rules_path(&ws)).exists());
}

#[tokio::test]
async fn eval_rules_add_update_remove_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    // add
    let v = handler
        .handle_cmd(
            "eval_rules.add",
            Some(serde_json::json!({ "rule": good_rule_json("custom-reg") })),
            &make_ctx(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["added"], true);
    // add 同 id 冲突拒绝（镜像 CLI）
    let err = handler
        .handle_cmd(
            "eval_rules.add",
            Some(serde_json::json!({ "rule": good_rule_json("custom-reg") })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("already exists"), "got: {err}");
    // update 现有条目（level 改 critical）
    let mut edited = good_rule_json("custom-reg");
    edited["level"] = serde_json::json!("critical");
    let v = handler
        .handle_cmd(
            "eval_rules.update",
            Some(serde_json::json!({ "rule": edited })),
            &make_ctx(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["updated"], true);
    // update 不存在 id 报错
    let err = handler
        .handle_cmd(
            "eval_rules.update",
            Some(serde_json::json!({ "rule": good_rule_json("nope") })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
    // list 里看到替换后的 level
    let v = handler
        .handle_cmd("eval_rules.list", None, &make_ctx(&ws))
        .await
        .unwrap()
        .unwrap();
    let rules = v["rules"].as_array().unwrap();
    let found = rules.iter().find(|r| r["id"] == "custom-reg").unwrap();
    assert_eq!(found["level"], "critical");
    // remove
    let v = handler
        .handle_cmd(
            "eval_rules.remove",
            Some(serde_json::json!({ "id": "custom-reg" })),
            &make_ctx(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["removed"], true);
    // remove 不存在 id 报错
    let err = handler
        .handle_cmd(
            "eval_rules.remove",
            Some(serde_json::json!({ "id": "custom-reg" })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
}

#[tokio::test]
async fn eval_rules_set_enabled_and_reset() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    // 取一个内置规则 id（list 种子后首条）
    let v = handler
        .handle_cmd("eval_rules.list", None, &make_ctx(&ws))
        .await
        .unwrap()
        .unwrap();
    let first_id = v["rules"][0]["id"].as_str().unwrap().to_string();
    let defaults_len = v["rules"].as_array().map(|a| a.len()).unwrap_or(0);
    // 关
    let v = handler
        .handle_cmd(
            "eval_rules.set_enabled",
            Some(serde_json::json!({ "id": first_id, "enabled": false })),
            &make_ctx(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["enabled"], false);
    // 不存在 id 报错
    let err = handler
        .handle_cmd(
            "eval_rules.set_enabled",
            Some(serde_json::json!({ "id": "ghost", "enabled": true })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
    // 加一条自定义 → reset → replaced 含自定义，count = 默认集条数，自定义消失
    handler
        .handle_cmd(
            "eval_rules.add",
            Some(serde_json::json!({ "rule": good_rule_json("custom-x") })),
            &make_ctx(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    let v = handler
        .handle_cmd("eval_rules.reset", None, &make_ctx(&ws))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["reset"], true);
    assert_eq!(v["replaced"], defaults_len + 1);
    assert_eq!(v["count"], defaults_len);
    let v = handler
        .handle_cmd("eval_rules.list", None, &make_ctx(&ws))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !v["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "custom-x"),
        "reset 后自定义规则消失"
    );
    // 首条规则 enabled 回到默认 true（默认集原样）
    let first = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == first_id.as_str())
        .expect("default rule back after reset");
    assert_eq!(first["enabled"], true);
}

#[tokio::test]
async fn eval_rules_add_rejects_invalid_structure() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    // 未知 source → 结构校验拒绝（单一真相源校验函数）
    let mut bad = good_rule_json("bad-src");
    bad["source"] = serde_json::json!("nosuch");
    let err = handler
        .handle_cmd(
            "eval_rules.add",
            Some(serde_json::json!({ "rule": bad })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid rule"), "got: {err}");
    // 不可编译 regex 拒绝
    let mut bad = good_rule_json("bad-regex");
    bad["conditions"][0]["op"] = serde_json::json!("regex");
    bad["conditions"][0]["value"] = serde_json::json!("([bad");
    let err = handler
        .handle_cmd(
            "eval_rules.add",
            Some(serde_json::json!({ "rule": bad })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid regex"), "got: {err}");
    // 缺 rule 字段
    let err = handler
        .handle_cmd(
            "eval_rules.add",
            Some(serde_json::json!({ "norule": true })),
            &make_ctx(&ws),
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing rule"), "got: {err}");
}
