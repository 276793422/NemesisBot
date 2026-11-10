//! 十层页面化（2026-11-10）：`security.overview` 聚合投影 +
//! `security.command_blocklist` / `security.credential_rules` 只读快照。
//!
//! 核心契约：
//! - 层开关投影走 **raw 语义**（缺席 = 启用，与
//!   `apply_security_layer_switches` 逐键对齐）——typed serde default
//!   false 与 runtime「缺席=构造引擎」相反，`{}` 形态部署不得误显示关闭；
//! - 审计聚合**排除 audit_chain.jsonl**（链段不是审计事件），按
//!   `policy_rule` 聚合 total/denied/last_ts；
//! - 未注入槽（estop/agent_loop/editor/approvals/signature）与缺席
//!   home（global/executor）诚实降级，不猜默认值冒充读到。

use super::*;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;

fn make_ctx(ws: &str) -> RequestContext {
    // s10b_tests::make_ctx 同款（AppState 全字段字面量）；home: None =
    // global/executor 降级臂的唯一可达形态（真 home 需要整份 config.json）。
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
        session_id: "overview".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(ws.to_string()),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn audit_line(event_id: &str, decision: &str, policy: &str, ts: &str) -> String {
    serde_json::json!({
        "event_id": event_id,
        "request": { "op_type": "file_write", "danger_level": "HIGH", "target": "C:/x.txt" },
        "decision": decision,
        "reason": "policy",
        "policy_rule": policy,
        "timestamp": ts,
    })
    .to_string()
}

#[tokio::test]
async fn overview_empty_workspace_projects_all_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let ctx = make_ctx(&ws);

    let resp = handler
        .handle_cmd("overview", None, &ctx)
        .await
        .unwrap()
        .unwrap();

    // 层开关：配置文件缺席 = runtime 默认构造引擎 = 全部启用（raw 语义）。
    let layers = &resp["layers"];
    assert_eq!(layers["injection"]["enabled"], true);
    assert_eq!(layers["command_guard"]["enabled"], true);
    assert_eq!(layers["credential"]["enabled"], true);
    assert_eq!(layers["ssrf"]["enabled"], true);
    assert_eq!(layers["dlp"]["enabled"], true);
    // ① 阈值：未配置 = 默认 0.7 + configured:false。
    assert_eq!(layers["injection"]["threshold"], 0.7);
    assert_eq!(layers["injection"]["threshold_configured"], false);
    // ③ ABAC 无独立开关（跟总闸）。
    assert_eq!(layers["abac"]["standalone_switch"], false);
    // ⑧ 审计两开关缺省：log 文件 true / 链 false。
    assert_eq!(layers["audit"]["log_file_enabled"], true);
    assert_eq!(layers["audit"]["chain_enabled"], false);
    assert_eq!(layers["effective"], "restart");
    // 链：无文件 = 不在场。
    assert_eq!(resp["chain"]["present"], false);
    assert_eq!(resp["chain"]["segments"], 0);
    // 审计聚合：无日志 = 零计数。
    assert_eq!(resp["intercepts"]["total"], 0);
    assert_eq!(resp["intercepts"]["denied"], 0);
    // 未注入/缺席降级臂：诚实 available:false / injected:false，不猜默认。
    // （approvals/editor 两臂是进程级槽——并行测试互相装槽会 flake，
    // 断言挪进 overview_slot_snapshots_readthrough 的锁内专测。）
    assert_eq!(resp["estop"]["injected"], false);
    assert_eq!(resp["plan_mode"]["available"], false);
    assert_eq!(resp["signature"]["injected"], false);
    assert_eq!(resp["global"]["available"], false);
    assert_eq!(resp["executor"]["available"], false);
    // ⑦ scanner：无 config.scanner.json = 空投影。
    assert_eq!(resp["scanner"]["enabled"], false);
    assert_eq!(resp["scanner"]["total"], 0);
}

#[tokio::test]
async fn overview_layers_mirror_raw_config_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // 覆盖投影全部消费键：显式 false 层 / `{}` 形态层（typed 会误投影
    // false，raw 语义必须 true）/ 阈值越界（warn + 默认）/ DLP 四键 /
    // SSRF 白名单 / 审计两开关 / default_action。
    std::fs::write(
        cfg_dir.join("config.security.json"),
        serde_json::json!({
            "default_action": "allow",
            "audit_log_file_enabled": false,
            "audit_chain_enabled": true,
            "layers": {
                "injection": { "enabled": false, "extra": { "threshold": 1.5 } },
                "command_guard": { "enabled": true },
                "ssrf": { "enabled": false, "allowed_hosts": ["127.0.0.1", "localhost"] },
                "dlp": { "enabled": false, "action": "block", "rules": ["email", "phone"], "low_confidence_action": "log", "inbound_action": "redact" },
                "credential": {}
            }
        })
        .to_string(),
    )
    .unwrap();

    let handler = SecurityHandler::new();
    let ctx = make_ctx(&ws);
    let resp = handler
        .handle_cmd("overview", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let layers = &resp["layers"];

    // 显式 bool = 该值。
    assert_eq!(layers["injection"]["enabled"], false);
    assert_eq!(layers["command_guard"]["enabled"], true);
    assert_eq!(layers["ssrf"]["enabled"], false);
    assert_eq!(layers["dlp"]["enabled"], false);
    // `{}` 形态（enabled 键缺席）= raw 语义启用——typed serde default
    // false 与 runtime 相反，本断言钉住投影不漂移。
    assert_eq!(layers["credential"]["enabled"], true);
    // 阈值越界（1.5）→ warn + 默认 0.7 + configured:false。
    assert_eq!(layers["injection"]["threshold"], 0.7);
    assert_eq!(layers["injection"]["threshold_configured"], false);
    // DLP 细粒度键透传 + 规则计数。
    assert_eq!(layers["dlp"]["action"], "block");
    assert_eq!(layers["dlp"]["low_confidence_action"], "log");
    assert_eq!(layers["dlp"]["inbound_action"], "redact");
    assert_eq!(layers["dlp"]["rules"].as_array().unwrap().len(), 2);
    // SSRF 白名单透传（runtime 读层对象顶层 allowed_hosts——typed
    // SecurityLayerConfig.allowed_hosts 字段随本批修复 round-trip）。
    let hosts = layers["ssrf"]["allowed_hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 2);
    assert_eq!(hosts[0], "127.0.0.1");
    // 审计两开关显式值。
    assert_eq!(layers["audit"]["log_file_enabled"], false);
    assert_eq!(layers["audit"]["chain_enabled"], true);
    assert_eq!(layers["default_action"], "allow");
}

#[tokio::test]
async fn overview_aggregates_audit_by_policy_excluding_chain_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let log_dir = dir.path().join("logs/security_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        log_dir.join("audit.jsonl"),
        format!(
            "{}\n{}\n{}\nnot-json-garbage\n{}\n",
            audit_line("e1", "denied", "injection_detector", "2026-01-02T10:00:00Z"),
            audit_line(
                "e2",
                "allowed",
                "injection_detector",
                "2026-01-03T10:00:00Z"
            ),
            audit_line("e3", "blocked", "command_guard", "2026-01-01T10:00:00Z"),
            audit_line("e4", "denied", "credential_scanner", "2026-01-04T10:00:00Z"),
        ),
    )
    .unwrap();
    // 链文件必须被聚合排除（链段不是审计事件）：4 段在场但不进计数。
    std::fs::write(
        log_dir.join("audit_chain.jsonl"),
        "{\"seg\":1}\n{\"seg\":2}\n{\"seg\":3}\n{\"seg\":4}\n",
    )
    .unwrap();

    let handler = SecurityHandler::new();
    let ctx = make_ctx(&ws);
    let resp = handler
        .handle_cmd("overview", None, &ctx)
        .await
        .unwrap()
        .unwrap();

    // 链在场态来自链文件本身：4 段。
    assert_eq!(resp["chain"]["present"], true);
    assert_eq!(resp["chain"]["segments"], 4);
    // 聚合计数排除链段：4 审计事件（垃圾行跳过）。
    assert_eq!(resp["intercepts"]["total"], 4);
    assert_eq!(resp["intercepts"]["denied"], 3, "denied+blocked+denied");
    let by_policy = resp["intercepts"]["by_policy"].as_object().unwrap();
    assert_eq!(by_policy.len(), 3);
    let inj = &by_policy["injection_detector"];
    assert_eq!(inj["total"], 2);
    assert_eq!(inj["denied"], 1);
    assert_eq!(inj["last_ts"], "2026-01-03T10:00:00Z", "last_ts 取最新");
    let cmd = &by_policy["command_guard"];
    assert_eq!(cmd["total"], 1);
    assert_eq!(cmd["denied"], 1, "blocked → deny");
    assert_eq!(by_policy["credential_scanner"]["denied"], 1);
}

#[test]
fn command_blocklist_snapshot_is_readonly_contract() {
    let handler = SecurityHandler::new();
    // 纯静态快照（不触 ctx）——handle_cmd 也要能走通，这里直接调 impl
    // 方法验证形状；handle_cmd 分派臂由 l1 注册表测试覆盖命令清单。
    let resp = SecurityHandler::command_blocklist();
    let entries = resp["entries"].as_array().unwrap();
    assert!(!entries.is_empty(), "内置黑名单非空");
    let first = &entries[0];
    for key in ["name", "category", "severity", "platform", "reason"] {
        assert!(
            first.get(key).is_some_and(|v| !v.is_null()),
            "entry field {key} must serialize"
        );
    }
    // §2.7 核实结论：扩展 API 生产零接线——层页诚实注记的依据字段。
    assert_eq!(resp["extra_wired"], false);
    assert!(resp["note"].as_str().is_some_and(|s| !s.is_empty()));
    let _ = handler; // handler 构造臂（Default 转发已被 agt_tests 覆盖）
}

#[test]
fn credential_rules_snapshot_lists_builtin_names() {
    let resp = SecurityHandler::credential_rules();
    let rules = resp["rules"].as_array().unwrap();
    assert!(!rules.is_empty(), "内置规则表非空");
    // §2.7 核实：OnceLock 静态表 32 条（drift 锁——增删规则同步改此数）。
    assert_eq!(rules.len(), 32);
    let names: Vec<&str> = rules
        .iter()
        .map(|r| r["name"].as_str().unwrap_or(""))
        .collect();
    assert!(names.contains(&"aws_access_key"));
    assert!(names.contains(&"secret_assignment"));
    assert!(resp["note"].as_str().is_some_and(|s| !s.is_empty()));
}

#[test]
fn overview_slot_snapshots_readthrough() {
    // approvals / editor 是进程级槽（并行测试互装会 flake）——锁内专测，
    // 双槽各「未装配基线 → 注入透传 → 恢复」三相位。overview() 是同步
    // 方法（无 await），锁纪律与 approval_auto_tests::with_slot 同款。
    let _g1 = APPROVAL_AUTO_TEST_LOCK.lock();
    let _g2 = crate::handlers::editor::EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let ctx = make_ctx(&ws);

    // 相位 1：未装配基线（自清理先行）。
    set_approval_auto_for_test(None);
    crate::handlers::editor::set_editor_access_for_test(None);
    let resp = handler.overview(&ctx, &ws).unwrap().unwrap();
    assert_eq!(resp["approvals"]["injected"], false);
    assert_eq!(resp["editor"]["injected"], false);

    // 相位 2：注入 → 透传。
    let auto = Arc::new(nemesis_security::approval_auto::ApprovalAutoApproveState::new(true));
    auto.bump();
    install_auto_approve(auto);
    let editor_state = nemesis_security::editor_access::EditorAccessState::new();
    editor_state.set_flags(false, true);
    crate::handlers::editor::install_editor_access(editor_state);
    let resp = handler.overview(&ctx, &ws).unwrap().unwrap();
    assert_eq!(resp["approvals"]["injected"], true);
    assert_eq!(resp["approvals"]["enabled"], true);
    assert_eq!(resp["approvals"]["auto_approved_count"], 1);
    assert_eq!(resp["editor"]["injected"], true);
    assert_eq!(
        resp["editor"]["full_access"], true,
        "ext=true ⇒ full 联动收口"
    );
    assert_eq!(resp["editor"]["external_write"], true);

    // 相位 3：恢复未装配（进程级槽不留残态）。
    set_approval_auto_for_test(None);
    crate::handlers::editor::set_editor_access_for_test(None);
    let resp = handler.overview(&ctx, &ws).unwrap().unwrap();
    assert_eq!(resp["approvals"]["injected"], false);
    assert_eq!(resp["editor"]["injected"], false);
}
