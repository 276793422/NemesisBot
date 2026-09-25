// auditor.rs 覆盖率补充测试（guardian 失败审批 / limit 审批 /
// guardian verdict 审批三通道的全臂 + 审批 ctx 路由 + JSONL/log file
// 写失败臂 + get_audit_log 早退臂 + 自杀形态退化根 + workspace 外路径）。
//
// 豁免：1216-1218 / 1247-1249（JSONL 与 log file append 打开成功后的
// writeln! 失败臂）+ 1225-1226 / 1257-1258（serde_json::to_string 失败
// 臂，AuditEvent 全 String 字段序列化不可失败）——均为单测不可确定性
// 构造的环境故障。

use super::*;
use std::sync::Mutex as StdMutex;

/// 记录型审批管理器：sync/ctx 结果可配，调用被记录。
struct MockMgr {
    running: bool,
    sync_result: Option<Result<ApprovalVerdict, String>>,
    ctx_result: Option<Result<ApprovalVerdict, String>>,
    calls: StdMutex<Vec<String>>,
}

impl MockMgr {
    fn running() -> Arc<Self> {
        Arc::new(Self {
            running: true,
            sync_result: None,
            ctx_result: None,
            calls: StdMutex::new(Vec::new()),
        })
    }
    fn not_running() -> Arc<Self> {
        Arc::new(Self {
            running: false,
            sync_result: None,
            ctx_result: None,
            calls: StdMutex::new(Vec::new()),
        })
    }
    fn with(
        sync: Result<ApprovalVerdict, String>,
        ctx: Result<ApprovalVerdict, String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            running: true,
            sync_result: Some(sync),
            ctx_result: Some(ctx),
            calls: StdMutex::new(Vec::new()),
        })
    }
    fn called(&self, needle: &str) -> bool {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(needle))
    }
}

impl ApprovalManager for MockMgr {
    fn is_running(&self) -> bool {
        self.running
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<ApprovalVerdict, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("sync:{operation}:{target}:{risk_level}"));
        match &self.sync_result {
            Some(r) => r.clone(),
            None => Ok(ApprovalVerdict::approved()),
        }
    }
    fn request_approval_sync_ctx(
        &self,
        _request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
        _ctx: &ApprovalContext,
    ) -> Result<ApprovalVerdict, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("ctx:{operation}:{target}:{risk_level}"));
        match &self.ctx_result {
            Some(r) => r.clone(),
            None => Ok(ApprovalVerdict::approved()),
        }
    }
}

fn denied_with(note: Option<&str>) -> ApprovalVerdict {
    ApprovalVerdict {
        approved: false,
        note: note.map(str::to_string),
    }
}

fn ctx_of(channel: &str) -> ApprovalContext {
    ApprovalContext {
        channel: channel.into(),
        chat_id: "chat-1".into(),
        sender_id: "user-1".into(),
    }
}

fn judge(recommendation: &str) -> crate::guardian::JudgeVerdict {
    crate::guardian::JudgeVerdict {
        intent: "cov intent".into(),
        matches_rules: false,
        risk_level: "high".into(),
        recommendation: recommendation.into(),
        rationale: "cov rationale".into(),
    }
}

fn enabled_auditor() -> SecurityAuditor {
    SecurityAuditor::new(AuditorConfig {
        enabled: true,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// request_guardian_failure_approval —— 全臂
// ---------------------------------------------------------------------------

#[test]
fn guardian_failure_no_manager_errs() {
    let auditor = enabled_auditor();
    let err = auditor
        .request_guardian_failure_approval("exec", "guardian down", None)
        .unwrap_err();
    assert!(err.contains("no approval manager"), "{err}");
}

#[test]
fn guardian_failure_manager_not_running_errs() {
    let auditor = enabled_auditor();
    auditor.set_approval_manager(MockMgr::not_running());
    let err = auditor
        .request_guardian_failure_approval("exec", "guardian down", None)
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
}

#[test]
fn guardian_failure_ctx_none_approved_goes_through_sync() {
    let mgr = MockMgr::running();
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr.clone());

    let v = auditor
        .request_guardian_failure_approval("exec", "guardian down", None)
        .unwrap();
    assert!(v.approved);
    assert!(mgr.called("sync:guardian_review:exec:CRITICAL"));
    assert!(!mgr.called("ctx:"));
}

#[test]
fn guardian_failure_ctx_route_denied_with_note() {
    let mgr = MockMgr::with(
        Ok(ApprovalVerdict::approved()),
        Ok(denied_with(Some("too risky"))),
    );
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr.clone());
    let c = ctx_of("telegram");

    let v = auditor
        .request_guardian_failure_approval("exec", "guardian down", Some(&c))
        .unwrap();
    assert!(!v.approved);
    assert_eq!(v.note.as_deref(), Some("too risky"));
    assert!(mgr.called("ctx:guardian_review:exec:CRITICAL"));
}

#[test]
fn guardian_failure_ctx_route_denied_blank_note_means_timeout() {
    // 空白 note 视同 None → "rejected or approval timeout" 分支。
    let mgr = MockMgr::with(Err("channel gone".into()), Ok(denied_with(Some("   "))));
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr);

    let v = auditor
        .request_guardian_failure_approval("exec", "guardian down", Some(&ctx_of("web")))
        .unwrap();
    assert!(!v.approved);
    assert!(v.note.is_none() || v.note.as_deref().unwrap().trim().is_empty());
}

#[test]
fn guardian_failure_manager_error_propagates_with_audit() {
    let mgr = MockMgr::with(Err("approval channel broken".into()), Err("x".into()));
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr);

    let err = auditor
        .request_guardian_failure_approval("exec", "guardian down", None)
        .unwrap_err();
    assert!(err.contains("approval channel broken"), "{err}");
}

// ---------------------------------------------------------------------------
// request_limit_approval —— 全臂
// ---------------------------------------------------------------------------

#[test]
fn limit_approval_no_manager_fail_closed() {
    let auditor = enabled_auditor();
    let err = auditor
        .request_limit_approval("exec", "token budget exceeded", None)
        .unwrap_err();
    assert!(err.contains("no approval manager"), "{err}");
}

#[test]
fn limit_approval_manager_not_running_fail_closed() {
    let auditor = enabled_auditor();
    auditor.set_approval_manager(MockMgr::not_running());
    let err = auditor
        .request_limit_approval("exec", "token budget exceeded", None)
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
}

#[test]
fn limit_approval_ctx_none_approved() {
    let mgr = MockMgr::running();
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr.clone());

    let v = auditor
        .request_limit_approval("exec", "token budget exceeded", None)
        .unwrap();
    assert!(v.approved);
    assert!(mgr.called("sync:limit_review:exec:HIGH"));
}

#[test]
fn limit_approval_ctx_route_denied_with_note() {
    let mgr = MockMgr::with(
        Ok(ApprovalVerdict::approved()),
        Ok(denied_with(Some("wait for reset"))),
    );
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr.clone());

    let v = auditor
        .request_limit_approval("exec", "token budget exceeded", Some(&ctx_of("feishu")))
        .unwrap();
    assert!(!v.approved);
    assert!(mgr.called("ctx:limit_review:exec:HIGH"));
}

#[test]
fn limit_approval_denied_without_note() {
    let mgr = MockMgr::with(Ok(denied_with(None)), Ok(denied_with(None)));
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr);

    let v = auditor
        .request_limit_approval("exec", "token budget exceeded", None)
        .unwrap();
    assert!(!v.approved);
    assert!(v.note.is_none());
}

#[test]
fn limit_approval_manager_error_propagates() {
    let mgr = MockMgr::with(Err("card delivery failed".into()), Err("x".into()));
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr);

    let err = auditor
        .request_limit_approval("exec", "token budget exceeded", None)
        .unwrap_err();
    assert!(err.contains("card delivery failed"), "{err}");
}

// ---------------------------------------------------------------------------
// request_guardian_verdict_approval —— 全臂
// ---------------------------------------------------------------------------

#[test]
fn guardian_verdict_no_manager_errs() {
    let auditor = enabled_auditor();
    let err = auditor
        .request_guardian_verdict_approval("exec", "high", "ask", &judge("deny"), None)
        .unwrap_err();
    assert!(err.contains("no approval manager"), "{err}");
}

#[test]
fn guardian_verdict_manager_not_running_errs() {
    let auditor = enabled_auditor();
    auditor.set_approval_manager(MockMgr::not_running());
    let err = auditor
        .request_guardian_verdict_approval("exec", "high", "ask", &judge("deny"), None)
        .unwrap_err();
    assert!(err.contains("not running"), "{err}");
}

#[test]
fn guardian_verdict_blank_risk_normalizes_to_unknown() {
    let mgr = MockMgr::running();
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr.clone());

    let v = auditor
        .request_guardian_verdict_approval("exec", "   ", "ask", &judge("deny"), None)
        .unwrap();
    assert!(v.approved);
    assert!(mgr.called(":UNKNOWN"), "空白 risk 归一化为 UNKNOWN");
}

#[test]
fn guardian_verdict_ctx_route_denied_with_note() {
    let mgr = MockMgr::with(
        Ok(ApprovalVerdict::approved()),
        Ok(denied_with(Some("not on my watch"))),
    );
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr.clone());

    let v = auditor
        .request_guardian_verdict_approval(
            "exec",
            "critical",
            "ask",
            &judge("deny"),
            Some(&ctx_of("slack")),
        )
        .unwrap();
    assert!(!v.approved);
    assert!(mgr.called("ctx:guardian_review:exec:CRITICAL"));
}

#[test]
fn guardian_verdict_denied_without_note() {
    let mgr = MockMgr::with(Ok(denied_with(None)), Ok(denied_with(None)));
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr);

    let v = auditor
        .request_guardian_verdict_approval("exec", "high", "ask", &judge("deny"), None)
        .unwrap();
    assert!(!v.approved);
}

#[test]
fn guardian_verdict_manager_error_propagates() {
    let mgr = MockMgr::with(Err("no user at keyboard".into()), Err("x".into()));
    let auditor = enabled_auditor();
    auditor.set_approval_manager(mgr);

    let err = auditor
        .request_guardian_verdict_approval("exec", "high", "ask", &judge("deny"), None)
        .unwrap_err();
    assert!(err.contains("no user at keyboard"), "{err}");
}

// ---------------------------------------------------------------------------
// request_permission_with_ctx —— 审批 ctx 路由 + 拒绝 + 管理器失败回落
// ---------------------------------------------------------------------------

fn exec_request(id: &str, target: &str) -> OperationRequest {
    OperationRequest {
        id: id.into(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "cov".into(),
        source: "cli".into(),
        target: target.into(),
        ..Default::default()
    }
}

fn auditor_asking() -> SecurityAuditor {
    
    SecurityAuditor::new(AuditorConfig {
        enabled: true,
        default_action: "ask".into(),
        ..Default::default()
    })
}

#[test]
fn permission_with_ctx_routes_to_ctx_method_and_approves() {
    let mgr = MockMgr::running();
    let auditor = auditor_asking();
    auditor.set_approval_manager(mgr.clone());
    let c = ctx_of("discord");

    let (allowed, msg, _) =
        auditor.request_permission_with_ctx(&exec_request("r-ctx-ok", "cargo build"), Some(&c));
    assert!(allowed);
    assert!(msg.is_none());
    assert!(mgr.called("ctx:"), "ctx 命中 ctx 方法");
    assert!(!mgr.called("sync:"), "ctx 存在时不得回落 sync 旧方法");
}

#[test]
fn permission_with_ctx_denied_msg_carries_note() {
    let mgr_denied = MockMgr::with(
        Ok(ApprovalVerdict::approved()),
        Ok(denied_with(Some("not today"))),
    );
    let auditor = auditor_asking();
    auditor.set_approval_manager(mgr_denied);
    let c = ctx_of("web");

    let (allowed, msg, _) =
        auditor.request_permission_with_ctx(&exec_request("r-ctx-deny", "cargo build"), Some(&c));
    assert!(!allowed);
    let msg = msg.unwrap_or_default();
    assert!(
        msg.contains("User rejected") && msg.contains("not today"),
        "{msg}"
    );
}

#[test]
fn permission_with_ctx_manager_failure_stores_pending() {
    let mgr = MockMgr::with(Err("dialog dead".into()), Err("dialog dead".into()));
    let auditor = auditor_asking();
    auditor.set_approval_manager(mgr);

    let (allowed, msg, req_id) = auditor.request_permission_with_ctx(
        &exec_request("r-ctx-fail", "cargo build"),
        Some(&ctx_of("web")),
    );
    assert!(!allowed);
    let msg = msg.unwrap_or_default();
    assert!(msg.contains("approval required"), "{msg}");
    assert!(msg.contains("r-ctx-fail"), "{msg}");
    assert_eq!(auditor.pending_count(), 1);
    // 清理：拒绝掉这个 pending。
    auditor
        .deny_request(&req_id, "cov", "test cleanup")
        .unwrap();
}

/// 只实现 sync 方法的 mock：不 override ctx 方法 → trait 默认实现
/// （委托 sync，182-200）被真实走到。
struct DelegatingMgr;

impl ApprovalManager for DelegatingMgr {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<ApprovalVerdict, String> {
        Ok(ApprovalVerdict::approved())
    }
}

#[test]
fn permission_with_ctx_falls_back_to_trait_default_delegation() {
    let auditor = auditor_asking();
    auditor.set_approval_manager(Arc::new(DelegatingMgr));

    let (allowed, msg, _) = auditor.request_permission_with_ctx(
        &exec_request("r-ctx-default", "cargo build"),
        Some(&ctx_of("rpc")),
    );
    assert!(allowed);
    assert!(msg.is_none());
}

// ---------------------------------------------------------------------------
// 审计写盘臂：JSONL 空目录早退 / JSONL 打开失败 / log file 打开失败 /
// get_audit_log 早退臂 / monitor loop 一拍 + 关闭
// ---------------------------------------------------------------------------

#[test]
fn audit_jsonl_empty_dir_skips_file_write() {
    let auditor = enabled_auditor();
    auditor.set_audit_jsonl_log(true, String::new());
    auditor.log_audit_event(&AuditEvent {
        event_id: "e-empty".into(),
        request: OperationRequest::default(),
        decision: "denied".into(),
        reason: "cov".into(),
        timestamp: chrono::Local::now().to_rfc3339(),
        policy_rule: "cov".into(),
    });
}

#[test]
fn audit_jsonl_unopenable_target_warns_not_panics() {
    let tmp = std::env::temp_dir().join(format!("nmb-sec-audit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // audit.jsonl 是目录 → OpenOptions append 打开必败。
    std::fs::create_dir_all(tmp.join("audit.jsonl")).unwrap();

    let auditor = enabled_auditor();
    auditor.set_audit_jsonl_log(true, tmp.to_string_lossy().to_string());
    auditor.log_audit_event(&AuditEvent {
        event_id: "e-blocked".into(),
        request: OperationRequest::default(),
        decision: "denied".into(),
        reason: "cov".into(),
        timestamp: chrono::Local::now().to_rfc3339(),
        policy_rule: "cov".into(),
    });
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn audit_log_file_unopenable_warns_not_panics() {
    let tmp = std::env::temp_dir().join(format!("nmb-sec-logfile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let auditor = enabled_auditor();
    // log file 路径本身是目录 → open 必败 → warn 臂。
    auditor.set_log_file(tmp.to_string_lossy().as_ref());
    auditor.log_audit_event(&AuditEvent {
        event_id: "e-logfile".into(),
        request: OperationRequest::default(),
        decision: "denied".into(),
        reason: "cov".into(),
        timestamp: chrono::Local::now().to_rfc3339(),
        policy_rule: "cov".into(),
    });
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn export_audit_log_uncreatable_parent_errs() {
    let tmp = std::env::temp_dir().join(format!("nmb-sec-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // 父位是文件 → create_dir_all 失败 → Err 上抛。
    std::fs::write(tmp.join("blocker"), "not a dir").unwrap();

    let auditor = enabled_auditor();
    let target = tmp.join("blocker").join("stats.json");
    let err = auditor
        .export_audit_log(target.to_str().unwrap())
        .unwrap_err();
    assert!(err.contains("directory") || err.contains("create"), "{err}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn get_audit_log_free_fn_early_return_arms() {
    let auditor = enabled_auditor();
    let filter = AuditFilter::default();

    // 运行时接线为 true + 空 dir → 早退空。
    auditor.set_audit_jsonl_log(true, String::new());
    assert!(get_audit_log(&auditor, &filter).is_empty());

    // 运行时接线为 false → 早退空。
    auditor.set_audit_jsonl_log(false, "x".into());
    assert!(get_audit_log(&auditor, &filter).is_empty());
}

#[tokio::test]
async fn monitor_security_status_ticks_then_shuts_down() {
    let auditor = enabled_auditor();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(monitor_security_status(std::sync::Arc::new(auditor), 1, rx));
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), handle)
        .await
        .expect("monitor 必须在 shutdown 后退出")
        .unwrap();
}

// ---------------------------------------------------------------------------
// 自杀形态：退化根（`//`）+ workspace 外路径 validate
// ---------------------------------------------------------------------------

#[test]
fn self_destruct_degenerate_root_token_is_blocked() {
    let auditor = enabled_auditor();
    auditor.set_protected_paths(vec!["/ws".into()]);

    // "///"（3 个斜杠）越过「≤2 字符 / 开头按 Windows 短旗标处理」的
    // 前置臂，落到 tt.trim_end_matches('/') == "" 的退化根臂（1543-1544）。
    let (allowed, msg, _) = auditor.request_permission(&exec_request("r-root", "rm -rf ///"));
    assert!(!allowed, "退化根必须硬拦");
    let msg = msg.unwrap_or_default();
    assert!(msg.contains("self-destruct"), "{msg}");
}

#[test]
fn self_destruct_protected_path_itself_and_descendant_both_blocked() {
    let auditor = enabled_auditor();
    auditor.set_protected_paths(vec!["/ws".into()]);

    // 本体臂：目标 == 保护路径。
    let (allowed, msg, _) = auditor.request_permission(&exec_request("r-own", "rm -rf /ws"));
    assert!(!allowed);
    assert!(msg.unwrap_or_default().contains("self-destruct"));

    // 后代臂：目标在保护路径之内（无豁免时照拦）。
    let (allowed, msg, _) =
        auditor.request_permission(&exec_request("r-desc", "rm -rf /ws/node_modules"));
    assert!(!allowed);
    assert!(msg.unwrap_or_default().contains("self-destruct"));
}

#[test]
fn validate_path_outside_workspace_rejected() {
    // 词法上带 ".." 的前缀可剥离但 rel 以 .. 开头（1617-1619 臂）。
    let result = SecurityAuditor::validate_path(
        "C:\\some\\workspace\\..\\evil.txt",
        "C:\\some\\workspace",
        OperationType::FileRead,
    );
    assert!(result.is_err(), "workspace 内 .. 逃逸必须被拒");
    assert!(result.unwrap_err().contains("outside workspace"));

    // 前缀完全剥不上（另一块盘）→ strip_prefix Err 臂。
    let result = SecurityAuditor::validate_path(
        "Z:\\elsewhere\\secret.txt",
        "C:\\some\\workspace",
        OperationType::FileRead,
    );
    assert!(result.is_err(), "工作区外路径必须被拒");
    assert!(result.unwrap_err().contains("outside workspace"));
}

// ---------------------------------------------------------------------------
// action_bucket "allow" 桶 + log_guardian_verdict allow 事件
// ---------------------------------------------------------------------------

#[test]
fn rule_with_allow_action_buckets_as_allow() {
    let auditor = enabled_auditor();
    auditor.set_rules(
        OperationType::FileRead,
        vec![SecurityRule {
            pattern: "*.txt".into(),
            action: "allow".into(),
            comment: "cov".into(),
        }],
    );

    let (allowed, _, _) = auditor.request_permission(&OperationRequest {
        id: "r-allow".into(),
        op_type: OperationType::FileRead,
        danger_level: DangerLevel::Low,
        user: "cov".into(),
        source: "cli".into(),
        target: "notes.txt".into(),
        ..Default::default()
    });
    assert!(allowed);

    // guardian allow verdict 落审计后再取统计。
    auditor.log_guardian_verdict("exec", "audit", &judge("allow"));
    let stats = auditor.get_statistics();
    assert!(stats.contains_key("total_events"));
}
