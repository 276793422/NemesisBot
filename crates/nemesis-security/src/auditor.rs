//! ABAC Security Auditor - Attribute-Based Access Control
//!
//! Implements the core security evaluation engine that:
//! - Evaluates operation requests against configured rules
//! - Manages approval workflows (approve/deny pending requests)
//! - Validates paths for workspace isolation
//! - Checks commands for dangerous patterns
//! - Tracks statistics and exports audit logs

use crate::matcher;
use crate::types::*;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

// ---------------------------------------------------------------------------
// Default deny patterns
// ---------------------------------------------------------------------------

/// Default deny patterns for dangerous operations, keyed by operation type.
///
/// Equivalent to Go's `DefaultDenyPatterns`.
pub static DEFAULT_DENY_PATTERNS: std::sync::LazyLock<HashMap<OperationType, Vec<&'static str>>> =
    std::sync::LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert(
            OperationType::ProcessExec,
            vec![
                r"\brm\s+-[rf]{1,2}\b",
                r"\bdel\s+/[fq]\b",
                r"\b(format|mkfs|diskpart)\b",
                r"\bdd\s+if=",
                r"\b(shutdown|reboot|poweroff)\b",
                r"\bsudo\b",
                r"\bchmod\s+[0-7]{3,4}\b",
                r"\bchown\b",
                r"\bpkill\b",
                r"\bkillall\b",
                r"\bkill\s+-[9]\b",
                r"\bcurl\b.*\|\s*(sh|bash)",
                r"\bwget\b.*\|\s*(sh|bash)",
                r"\beval\b",
                r"\bsource\s+.*\.sh\b",
            ],
        );
        m.insert(
            OperationType::FileWrite,
            vec![
                r"\.\.[/\\]",
                r"^/etc/",
                r"^/sys/",
                r"^/proc/",
                r"^/dev/",
                r"C:\\Windows\\System32",
                r"C:\\Windows\\System32\\drivers\\etc\\hosts",
            ],
        );
        m.insert(OperationType::NetworkDownload, vec![r"file://", r"ftp://"]);
        m
    });

/// 解释器内层载荷的危险结构词表（CMD-06②，2026-09-16 横扫存量加固）。
///
/// 解释器包装（`python -c`/`node -e`…）的内层是代码片段，模板的命令行
/// glob 规则表达不了 `shutil.rmtree` 这类 API 形态；命中即送审批
/// （RequireApproval）——这是 F-U4-7（worker `python -c` 删自身 home 的
/// 真实事故）命令类的最后一道语义闸。只收高精度结构，防误伤正常脚本。
const INTERPRETER_DANGEROUS_STRUCTURES: &[&str] = &[
    "shutil.rmtree", // Python 递归删树
    "rmsync",        // node fs.rmSync / rmdirSync（小写归一后）
    "rmdirsync",
    "removedirectory", // .NET Directory.Delete/RemoveDirectory 族
    "deltree",         // 经典递归删除
];

// ---------------------------------------------------------------------------
// ApprovalRequiredError
// ---------------------------------------------------------------------------

/// Error returned when an operation requires approval but no interactive
/// approval manager is available.
///
/// Equivalent to Go's `ApprovalRequiredError`.
#[derive(Debug, Clone)]
pub struct ApprovalRequiredError {
    /// The request ID that needs approval.
    pub request_id: String,
    /// Human-readable reason why approval is needed.
    pub reason: String,
}

impl std::fmt::Display for ApprovalRequiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "approval required: {} (request ID: {})",
            self.reason, self.request_id
        )
    }
}

impl std::error::Error for ApprovalRequiredError {}

impl ApprovalRequiredError {
    /// Always returns `true` — this type only exists for approval-required cases.
    pub fn is_approval_required(&self) -> bool {
        true
    }
}

/// Trait for approval manager integration.
///
/// Mirrors Go's `approval.ApprovalManager` interface. The auditor calls into
/// this when a `require_approval` decision is reached.
/// 一次审批交互的裁决（F6，devtool-upgrade 阶段 5）。
///
/// v1 的 `bool` 升级为结构体：拒绝可携带用户备注（审批卡输入框），auditor
/// 把备注拼进拒绝消息回灌给模型（纠错回喂——模型知道
/// 为什么被拒，下一轮可改方案重试而不是盲试）。
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalVerdict {
    pub approved: bool,
    /// 拒绝备注（可选；批准时恒 `None`）。空串/纯空白视同 `None`（审计侧
    /// 拼消息前 trim）。
    pub note: Option<String>,
}

impl ApprovalVerdict {
    pub fn approved() -> Self {
        Self {
            approved: true,
            note: None,
        }
    }

    pub fn denied() -> Self {
        Self {
            approved: false,
            note: None,
        }
    }
}

/// K4 (devtool-upgrade 阶段 7): 审批请求的来源通道上下文。
///
/// 由 pipeline 从 `ToolInvocation.metadata` 提取（loop 在构造 invocation 时
/// 填入 `approval_chat_id` / `approval_sender_id`；channel 取
/// `invocation.source`）。审批管理器据此把审批卡路由回**发起操作的对话**
/// （IM 通道卡片 + 回执），而不是只会弹 Dashboard/桌面弹窗。字段为空 =
/// 无来源上下文（cron 直发无 chat 等），管理器自行决定回落行为。
#[derive(Debug, Clone, Default)]
pub struct ApprovalContext {
    /// 来源通道名（web / telegram / feishu / ...）。
    pub channel: String,
    /// 来源对话 ID（审批卡送达处 + 回执校验）。
    pub chat_id: String,
    /// 发起人 ID（记录用；v1 群聊语义下同 chat 任一成员可批复）。
    pub sender_id: String,
}

pub trait ApprovalManager: Send + Sync {
    /// Whether the approval manager is currently running and able to show dialogs.
    fn is_running(&self) -> bool;

    /// Request interactive approval. Returns the user's verdict (F6: a deny
    /// may carry the user's free-text note for model feedback).
    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<ApprovalVerdict, String>;

    /// K4 (b): 带来源上下文的审批请求。默认实现忽略 ctx 委托旧方法——
    /// 既有实现（桌面弹窗 / Web 卡片 / ACP）零改动保持现状；支持通道
    /// 卡片的实现 override 本方法做路由。
    fn request_approval_sync_ctx(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
        _ctx: &ApprovalContext,
    ) -> Result<ApprovalVerdict, String> {
        self.request_approval_sync(
            request_id,
            operation,
            target,
            risk_level,
            reason,
            timeout_secs,
        )
    }
}

/// Security auditor configuration.
#[derive(Debug, Clone)]
pub struct AuditorConfig {
    pub enabled: bool,
    // ── 死键恢复区（2026-09-16 用户指令：代码不得随便删，注释保留待讨论）
    // ─────────────────────────────────────────────────────────────
    // CFG-05 曾裁「确认无用直接删」（D3），现注释恢复原字段。**未接线**
    // 说明：log_denials_only（与 log_all_operations=false 同义，接线版
    // log_all_operations 现居 pipeline.rs SecurityPluginConfig）、
    // max_pending_requests（无满额语义实现）、audit_log_retention_days
    // （无保留清扫器，盲清扫会误伤审计链文件）。删除与否待用户裁决。
    // pub log_all_operations: bool,   // 已接线迁移至 pipeline.rs（非删除，是 relocation）
    // pub log_denials_only: bool,
    // pub max_pending_requests: usize,
    // pub audit_log_retention_days: u32,
    // ── 死键恢复区结束 ──────────────────────────────────────────
    pub approval_timeout_secs: u64,
    pub audit_log_file_enabled: bool,
    pub audit_log_dir: Option<String>,
    pub default_action: String,
}

impl Default for AuditorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // 死键恢复区默认值（未接线，见 struct 注释）：false / 100 / 90
            approval_timeout_secs: 300,
            audit_log_file_enabled: false,
            audit_log_dir: None,
            default_action: "deny".to_string(),
        }
    }
}

/// Operation request for security evaluation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperationRequest {
    pub id: String,
    pub op_type: OperationType,
    pub danger_level: DangerLevel,
    pub user: String,
    pub source: String,
    pub target: String,
    pub timestamp: Option<chrono::DateTime<chrono::Local>>,
    /// Who approved (if applicable).
    pub approver: Option<String>,
    /// When approved.
    pub approved_at: Option<chrono::DateTime<chrono::Local>>,
    /// Reason for denial (if denied).
    pub denied_reason: Option<String>,
}

impl Default for OperationRequest {
    fn default() -> Self {
        Self {
            id: String::new(),
            op_type: OperationType::FileRead,
            danger_level: DangerLevel::Low,
            user: String::new(),
            source: String::new(),
            target: String::new(),
            timestamp: None,
            approver: None,
            approved_at: None,
            denied_reason: None,
        }
    }
}

/// Security auditor - ABAC engine.
pub struct SecurityAuditor {
    rules: RwLock<HashMap<OperationType, Vec<SecurityRule>>>,
    default_action: RwLock<String>,
    active_requests: RwLock<HashMap<String, OperationRequest>>,
    config: AuditorConfig,
    enabled: RwLock<bool>,
    total_events: AtomicI64,
    allowed_count: AtomicI64,
    denied_count: AtomicI64,
    approved_count: AtomicI64,
    pending_count: AtomicI64,
    /// Optional approval manager for interactive approval dialogs.
    approval_manager: RwLock<Option<Arc<dyn ApprovalManager>>>,
    /// F3: approval pattern memory（「总是允许」规则表热载器）。命中且层级
    /// 安全门放行 → require_approval 自动放行（审计标注 auto_by_rule）。
    approval_rules:
        RwLock<Option<Arc<nemesis_config::HotReloader<Vec<crate::approval_rules::ApprovalRule>>>>>,
    /// Optional explicit log file path for audit events (date-based).
    /// When set, audit events are appended to this file in JSON format.
    log_file_path: RwLock<Option<PathBuf>>,
    /// D1（2026-09-16 用户裁决）：exec/spawn 未知命令（无规则命中）姿态。
    /// "allow"/"ask"/"deny"；空串 = 未配置（落 default_action，旧行为）。
    /// 由 security_setup 按裸 JSON 键 `exec_unknown_policy` 注入（键存在才
    /// 注入——老配置文件缺键不得悄悄改变其 default_action 语义）。
    exec_unknown_policy: RwLock<String>,
    /// 自杀形态硬拦保护路径（2026-09-16 用户裁决：rm -rf 自身 home/workspace
    /// 保持硬拦，**不进** exec_unknown_policy 开关）。由 security_setup 注入
    /// workspace root + home（含 `~` 形态）；空 = 未注入（跳过扫描）。
    protected_paths: RwLock<Vec<String>>,
}

impl SecurityAuditor {
    pub fn new(config: AuditorConfig) -> Self {
        Self {
            rules: RwLock::new(HashMap::new()),
            default_action: RwLock::new(config.default_action.clone()),
            active_requests: RwLock::new(HashMap::new()),
            enabled: RwLock::new(config.enabled),
            config,
            total_events: AtomicI64::new(0),
            allowed_count: AtomicI64::new(0),
            denied_count: AtomicI64::new(0),
            approved_count: AtomicI64::new(0),
            pending_count: AtomicI64::new(0),
            approval_manager: RwLock::new(None),
            approval_rules: RwLock::new(None),
            log_file_path: RwLock::new(None),
            exec_unknown_policy: RwLock::new(String::new()),
            protected_paths: RwLock::new(Vec::new()),
        }
    }

    /// 注入自杀形态硬拦保护路径（workspace root / home / `~`）。存小写 +
    /// 反斜杠转正斜杠形态，与命令归一化对齐。
    pub fn set_protected_paths(&self, paths: Vec<String>) {
        let normalized = paths
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| p.replace('\\', "/").to_lowercase())
            .collect();
        *self.protected_paths.write() = normalized;
    }

    /// D2（2026-09-16 用户裁决）：guardian（LLM judge）自身故障时的审批
    /// 直通车——同步阻塞等用户裁决，reason 自由文本。**fail-closed**：
    /// 无审批管理器 / 管理器未运行 / 调用失败 = Err（调用方按拒绝处理），
    /// 绝不回落 allow。审批 verdict 含用户备注（F6）。
    pub fn request_guardian_failure_approval(
        &self,
        tool: &str,
        reason: &str,
        ctx: Option<&ApprovalContext>,
    ) -> Result<ApprovalVerdict, String> {
        let mgr_opt = self.approval_manager.read().clone();
        let Some(mgr) = mgr_opt.as_ref() else {
            return Err("guardian failed and no approval manager is available".to_string());
        };
        if !mgr.is_running() {
            return Err("guardian failed and the approval manager is not running".to_string());
        }
        let request_id = format!("guardian-{}", uuid::Uuid::new_v4());
        let verdict = match ctx {
            Some(c) => mgr.request_approval_sync_ctx(
                &request_id,
                "guardian_review",
                tool,
                "CRITICAL",
                reason,
                self.config.approval_timeout_secs,
                c,
            ),
            None => mgr.request_approval_sync(
                &request_id,
                "guardian_review",
                tool,
                "CRITICAL",
                reason,
                self.config.approval_timeout_secs,
            ),
        };
        // A-F5（复核 2026-09-16）：guardian 故障审批的用户裁决必须落审计
        // （audit_log_event 同一 JSONL/审计链通道）——CRITICAL 工具的人为
        // 放行/拒绝是安全链上最重的决定，此前只回调用方（approved 分支仅
        // info log，审计无痕，事后无法回答「谁放行了这次 CRITICAL 操作」）。
        // op_type 无 CRITICAL-工具泛型变体，取族内最常见 ProcessExec 占位；
        // 真实工具名在 target、来源在 source，可追溯不依赖 op_type。
        let audit_verdict = |decision: &str, why: String| {
            self.log_audit_event(&AuditEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                request: OperationRequest {
                    id: request_id.clone(),
                    op_type: OperationType::ProcessExec,
                    danger_level: DangerLevel::Critical,
                    source: "guardian_failure_approval".to_string(),
                    target: tool.to_string(),
                    ..Default::default()
                },
                decision: decision.to_string(),
                reason: why,
                timestamp: chrono::Local::now().to_rfc3339(),
                policy_rule: "guardian_failure_policy=ask".to_string(),
            });
        };
        match verdict {
            Ok(v) => {
                let why = if v.approved {
                    format!("guardian judge failed ({reason}); user approved")
                } else {
                    let note = v.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
                    match note {
                        Some(n) => {
                            format!("guardian judge failed ({reason}); user rejected: {n}")
                        }
                        None => format!(
                            "guardian judge failed ({reason}); user rejected or approval timeout"
                        ),
                    }
                };
                audit_verdict(if v.approved { "approved" } else { "denied" }, why);
                Ok(v)
            }
            Err(e) => {
                audit_verdict(
                    "denied",
                    format!("guardian judge failed ({reason}); approval unavailable: {e}"),
                );
                Err(e)
            }
        }
    }

    /// Set the audit log file path for date-based log file output.
    ///
    /// When configured, `log_audit_event()` will append events as JSON lines
    /// to the specified file path. This mirrors Go's behavior of writing audit
    /// events directly to a date-based log file.
    pub fn set_log_file(&self, path: &str) {
        *self.log_file_path.write() = Some(PathBuf::from(path));
    }

    /// Get the current audit log file path, if configured.
    pub fn get_log_file_path(&self) -> Option<PathBuf> {
        self.log_file_path.read().clone()
    }

    /// Set the approval manager for interactive approval dialogs.
    ///
    /// Equivalent to Go's `SecurityAuditor.SetApprovalManager()`.
    pub fn set_approval_manager(&self, mgr: Arc<dyn ApprovalManager>) {
        *self.approval_manager.write() = Some(mgr);
    }

    /// Get a reference to the current approval manager, if any.
    ///
    /// Equivalent to Go's `SecurityAuditor.GetApprovalManager()`.
    pub fn get_approval_manager(&self) -> Option<Arc<dyn ApprovalManager>> {
        self.approval_manager.read().clone()
    }

    /// F3: 挂载审批记忆规则表热载器（gateway 装配；`approval_rules.json`
    /// 磁盘变化经 `HotReloader::check()` 在每次查询前自动重读）。
    pub fn set_approval_rules(
        &self,
        hot: Arc<nemesis_config::HotReloader<Vec<crate::approval_rules::ApprovalRule>>>,
    ) {
        *self.approval_rules.write() = Some(hot);
    }

    /// Cleanup old audit logs.
    ///
    /// Equivalent to Go's `SecurityAuditor.CleanupOldAuditLogs()`.
    /// Events are persisted to file; this method is a no-op but provided
    /// for API parity.
    pub fn cleanup_old_audit_logs(&self) -> Result<(), String> {
        // No-op: events are persisted to the audit log file.
        // File-based retention can be handled externally by rotating the log file.
        Ok(())
    }

    /// Set rules for an operation type.
    pub fn set_rules(&self, op_type: OperationType, rules: Vec<SecurityRule>) {
        let mut r = self.rules.write();
        r.insert(op_type, rules);
    }

    /// Set the default action for unmatched requests.
    pub fn set_default_action(&self, action: &str) {
        *self.default_action.write() = action.to_string();
    }

    /// D1：设置 exec/spawn 未知命令姿态（allow/ask/deny）。空串 = 未配置
    /// （落 default_action 旧行为）。只在配置键真实存在时调用。
    pub fn set_exec_unknown_policy(&self, policy: &str) {
        *self.exec_unknown_policy.write() = policy.to_lowercase();
    }

    /// Check if enabled.
    pub fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    /// Enable the auditor.
    pub fn enable(&self) {
        *self.enabled.write() = true;
        tracing::info!("[Security] Security auditor enabled");
    }

    /// Disable the auditor.
    pub fn disable(&self) {
        *self.enabled.write() = false;
        tracing::warn!("[Security] Security auditor DISABLED - all operations will be allowed!");
    }

    /// Request permission for an operation.
    /// Returns (allowed, error_message, request_id).
    pub fn request_permission(&self, req: &OperationRequest) -> (bool, Option<String>, String) {
        self.request_permission_with_ctx(req, None)
    }

    /// K4 (b): 带来源上下文的审批入口。`ctx` 来自 pipeline 提取的
    /// `ToolInvocation.metadata`（来源通道/对话）；None = 无上下文（行为
    /// 与旧路径逐字节一致）。上下文只影响 RequireApproval 臂传给审批
    /// 管理器的路由信息，策略评估/审计事件完全不变。
    pub fn request_permission_with_ctx(
        &self,
        req: &OperationRequest,
        ctx: Option<&ApprovalContext>,
    ) -> (bool, Option<String>, String) {
        if !self.is_enabled() {
            return (true, None, req.id.clone());
        }

        self.total_events.fetch_add(1, Ordering::SeqCst);

        let (decision, reason, policy) = self.evaluate_request(req);

        // F3: 审批 pattern 记忆——require_approval 先查「总是允许」规则表，
        // 命中且层级安全门放行（CRITICAL 仅 process_exec 豁免）则自动放行，
        // 审计事件标注 auto_by_rule。查前 check() 让 approval_rules.json 的
        // 磁盘变化（审批卡写入 / CLI 清理 / 手工编辑）即时生效。
        if decision == SecurityDecision::RequireApproval
            && let Some(hot) = self.approval_rules.read().clone()
        {
            hot.check();
            let rules = hot.get();
            if let Some(rule) = crate::approval_rules::find_auto_allow_rule(
                &rules,
                &req.op_type.to_string(),
                &req.target,
                &req.danger_level.to_string(),
            ) {
                self.allowed_count.fetch_add(1, Ordering::SeqCst);
                let event = AuditEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    request: req.clone(),
                    decision: "allowed".to_string(),
                    reason: format!(
                        "auto allowed by approval rule ({} {})",
                        rule.op, rule.pattern
                    ),
                    timestamp: chrono::Local::now().to_rfc3339(),
                    policy_rule: format!("auto_by_rule:{}", rule.pattern),
                };
                self.log_audit_event(&event);
                return (true, None, req.id.clone());
            }
        }

        let decision_str = match decision {
            SecurityDecision::Allowed => "allowed",
            SecurityDecision::Denied => "denied",
            SecurityDecision::RequireApproval => "pending",
        };

        // Log the audit event to persistent storage
        let event = AuditEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            request: req.clone(),
            decision: decision_str.to_string(),
            reason: reason.clone(),
            timestamp: chrono::Local::now().to_rfc3339(),
            policy_rule: policy.clone(),
        };
        self.log_audit_event(&event);

        match decision {
            SecurityDecision::Allowed => {
                self.allowed_count.fetch_add(1, Ordering::SeqCst);
                (true, None, req.id.clone())
            }
            SecurityDecision::Denied => {
                self.denied_count.fetch_add(1, Ordering::SeqCst);
                (
                    false,
                    Some(format!(
                        "Security policy denied {} on '{}' ({})",
                        req.op_type, req.target, reason
                    )),
                    req.id.clone(),
                )
            }
            SecurityDecision::RequireApproval => {
                self.pending_count.fetch_add(1, Ordering::SeqCst);

                // Try to use interactive approval manager if available (mirrors Go behavior)
                let mgr_opt = self.approval_manager.read().clone();
                if let Some(mgr) = mgr_opt
                    && mgr.is_running()
                {
                    // Call the approval manager synchronously — 带来源上下文时
                    // 走 ctx 方法（通道卡片路由），否则走旧方法（现状不变）。
                    let verdict_res = match ctx {
                        Some(c) => mgr.request_approval_sync_ctx(
                            &req.id,
                            &req.op_type.to_string(),
                            &req.target,
                            &req.danger_level.to_string(),
                            &reason,
                            self.config.approval_timeout_secs,
                            c,
                        ),
                        None => mgr.request_approval_sync(
                            &req.id,
                            &req.op_type.to_string(),
                            &req.target,
                            &req.danger_level.to_string(),
                            &reason,
                            self.config.approval_timeout_secs,
                        ),
                    };
                    match verdict_res {
                        Ok(v) if v.approved => {
                            // User approved the operation
                            self.pending_count.fetch_sub(1, Ordering::SeqCst);
                            self.approved_count.fetch_add(1, Ordering::SeqCst);
                            return (true, None, req.id.clone());
                        }
                        Ok(v) => {
                            // User explicitly denied or timed out. F6: 用户备注
                            // 拼进拒绝消息回灌（模型知道为什么被拒）。
                            self.pending_count.fetch_sub(1, Ordering::SeqCst);
                            self.denied_count.fetch_add(1, Ordering::SeqCst);
                            let base = format!(
                                "User rejected {} on '{}' ({})",
                                req.op_type, req.target, reason
                            );
                            let msg = match v.note.as_deref().map(str::trim) {
                                Some(n) if !n.is_empty() => format!("{}: {}", base, n),
                                _ => base,
                            };
                            return (false, Some(msg), req.id.clone());
                        }
                        Err(_) => {
                            // Dialog failed, fall through to pending request storage
                        }
                    }
                }

                // No approval manager available or dialog failed, store as pending request
                let mut active = self.active_requests.write();
                active.insert(req.id.clone(), req.clone());
                (
                    false,
                    Some(format!(
                        "approval required: {} (request ID: {})",
                        reason, req.id
                    )),
                    req.id.clone(),
                )
            }
        }
    }

    /// Approve a pending operation request.
    ///
    /// Removes the request from the pending list, increments the approved counter,
    /// and records the approver information.
    pub fn approve_request(&self, request_id: &str, approver: &str) -> Result<(), String> {
        let mut active = self.active_requests.write();
        match active.get_mut(request_id) {
            Some(req) => {
                req.approver = Some(approver.to_string());
                req.approved_at = Some(chrono::Local::now());
                let _reason = format!("Approved by {}", approver);
                tracing::info!(
                    request_id = request_id,
                    approver = approver,
                    operation = %req.op_type,
                    target = %req.target,
                    "[Security] Operation approved"
                );
                active.remove(request_id);
                self.pending_count.fetch_sub(1, Ordering::SeqCst);
                self.approved_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            None => Err(format!("request not found: {}", request_id)),
        }
    }

    /// Deny a pending operation request.
    ///
    /// Removes the request from the pending list, increments the denied counter,
    /// and records the reason.
    pub fn deny_request(
        &self,
        request_id: &str,
        approver: &str,
        reason: &str,
    ) -> Result<(), String> {
        let mut active = self.active_requests.write();
        match active.get_mut(request_id) {
            Some(req) => {
                req.denied_reason = Some(reason.to_string());
                let _deny_reason = format!("Denied by {}: {}", approver, reason);
                tracing::info!(
                    request_id = request_id,
                    approver = approver,
                    reason = reason,
                    operation = %req.op_type,
                    "[Security] Operation denied"
                );
                active.remove(request_id);
                self.pending_count.fetch_sub(1, Ordering::SeqCst);
                self.denied_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            None => Err(format!("request not found: {}", request_id)),
        }
    }

    /// Get pending request count.
    pub fn pending_count(&self) -> usize {
        self.active_requests.read().len()
    }

    /// Get all pending approval requests.
    pub fn get_pending_requests(&self) -> Vec<OperationRequest> {
        self.active_requests.read().values().cloned().collect()
    }

    /// Get statistics as a HashMap of string keys to i64 values.
    pub fn statistics(&self) -> HashMap<String, i64> {
        let mut stats = HashMap::new();
        stats.insert(
            "total_events".to_string(),
            self.total_events.load(Ordering::SeqCst),
        );
        stats.insert(
            "allowed".to_string(),
            self.allowed_count.load(Ordering::SeqCst),
        );
        stats.insert(
            "denied".to_string(),
            self.denied_count.load(Ordering::SeqCst),
        );
        stats.insert(
            "approved".to_string(),
            self.approved_count.load(Ordering::SeqCst),
        );
        stats.insert(
            "pending".to_string(),
            self.pending_count.load(Ordering::SeqCst),
        );
        stats
    }

    /// Get full statistics as a HashMap of string keys to various types.
    ///
    /// Equivalent to Go's `GetStatistics()`. Returns richer data including
    /// active request count, enabled status, and rule type count.
    pub fn get_statistics(&self) -> HashMap<String, serde_json::Value> {
        let mut stats = HashMap::new();
        stats.insert(
            "total_events".to_string(),
            serde_json::json!(self.total_events.load(Ordering::SeqCst)),
        );
        stats.insert(
            "allowed".to_string(),
            serde_json::json!(self.allowed_count.load(Ordering::SeqCst)),
        );
        stats.insert(
            "denied".to_string(),
            serde_json::json!(self.denied_count.load(Ordering::SeqCst)),
        );
        stats.insert(
            "approved".to_string(),
            serde_json::json!(self.approved_count.load(Ordering::SeqCst)),
        );
        stats.insert(
            "pending".to_string(),
            serde_json::json!(self.pending_count.load(Ordering::SeqCst)),
        );
        stats.insert(
            "active_requests".to_string(),
            serde_json::json!(self.active_requests.read().len()),
        );
        stats.insert("enabled".to_string(), serde_json::json!(self.is_enabled()));
        stats.insert(
            "rule_types".to_string(),
            serde_json::json!(self.rules.read().len()),
        );
        stats
    }

    /// Export the audit log to a file.
    ///
    /// In the Go implementation, this copies the persistent audit log file to
    /// the specified path. Here, we create a summary export with the statistics.
    pub fn export_audit_log(&self, file_path: &str) -> Result<(), String> {
        let path = Path::new(file_path);

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory: {}", e))?;
        }

        // Write statistics as JSON
        let stats = self.get_statistics();
        let content = serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string());

        std::fs::write(path, content)
            .map_err(|e| format!("failed to write audit log export: {}", e))?;

        Ok(())
    }

    /// Validate that a path is within the workspace and safe.
    ///
    /// Equivalent to Go's `ValidatePath()`. Checks:
    /// 1. Path resolves to an absolute path
    /// 2. Path is within workspace (if workspace is specified)
    /// 3. Path does not access dangerous system paths
    pub fn validate_path(
        path: &str,
        workspace: &str,
        _operation: OperationType,
    ) -> Result<String, String> {
        validate_path_internal(path, workspace)
    }

    /// Check if a command is safe to execute.
    ///
    /// Equivalent to Go's `IsSafeCommand()`. Checks against a set of
    /// dangerous command patterns.
    pub fn is_safe_command(command: &str) -> (bool, String) {
        is_safe_command_internal(command)
    }

    /// Close the auditor and release resources.
    pub fn close(&self) -> Result<(), String> {
        // Clear active requests
        self.active_requests.write().clear();
        Ok(())
    }

    /// Query the audit log with a filter.
    ///
    /// Equivalent to Go's `SecurityAuditor.GetAuditLog()`.
    /// Events are persisted to file rather than held in memory, so this
    /// delegates to the free function `get_audit_log`.
    pub fn get_audit_log(&self, filter: AuditFilter) -> Vec<AuditEvent> {
        get_audit_log(self, &filter)
    }

    /// Get a reference to the auditor configuration.
    pub fn config(&self) -> &AuditorConfig {
        &self.config
    }

    /// Append an audit event to the persistent JSONL log file.
    ///
    /// If `audit_log_file_enabled` is true and `audit_log_dir` is set,
    /// the event is serialized as JSON and appended as a new line to
    /// `{audit_log_dir}/audit.jsonl`.
    ///
    /// Additionally, if `log_file_path` is configured via `set_log_file()`,
    /// the event is appended to that file as well (mirrors Go's date-based
    /// log file behavior).
    pub fn log_audit_event(&self, event: &AuditEvent) {
        // Write to the configured JSONL audit log directory
        if self.config.audit_log_file_enabled
            && let Some(ref log_dir) = self.config.audit_log_dir
            && !log_dir.is_empty()
        {
            let log_path = Path::new(log_dir).join("audit.jsonl");

            // Create directory if it doesn't exist
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match serde_json::to_string(event) {
                Ok(line) => {
                    use std::io::Write;
                    // Open in append mode, create if not exists
                    match std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        Ok(mut file) => {
                            if let Err(e) = writeln!(file, "{}", line) {
                                tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to write audit event");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to open audit log for writing");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Security] Failed to serialize audit event");
                }
            }
        }

        // Write to the explicit log_file_path if configured (mirrors Go's date-based log file)
        let log_path_opt = self.log_file_path.read().clone();
        if let Some(ref log_path) = log_path_opt {
            // Create parent directory if needed
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match serde_json::to_string(event) {
                Ok(line) => {
                    use std::io::Write;
                    match std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(log_path)
                    {
                        Ok(mut file) => {
                            if let Err(e) = writeln!(file, "{}", line) {
                                tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to write audit event to log file");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to open log file for writing");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Security] Failed to serialize audit event for log file");
                }
            }
        }
    }

    fn evaluate_request(&self, req: &OperationRequest) -> (SecurityDecision, String, String) {
        let rules = self.rules.read();
        let op_rules = match rules.get(&req.op_type) {
            Some(r) if !r.is_empty() => Some(r),
            _ => None,
        };

        // CMD-01/CMD-06①（2026-09-16 横扫存量加固）：评估顺序改为
        // **deny-first 三遍扫**（deny → ask → allow）——旧的
        // first-match-wins 下模板的 allow 规则（`python *`/`git *`…）排在
        // deny 之前，命中 allow 即放行，deny 永远没机会看。
        //
        // CMD-04/05：命令类操作匹配前对 target 做归一化（剥引号/合并空白/
        // 统一小写）——`rm "-rf"`、`rm --Recursive` 曾借 shell 剥引号与
        // 大小写差异同时绕过 Guard 与 ABAC。pattern 也按小写比较（模板
        // pattern 书写大小写不再敏感，`Remove-Item*` 可命中
        // `remove-item ...`）。审计日志保留原文。
        let is_command_op = matches!(
            req.op_type,
            OperationType::ProcessExec
                | OperationType::ProcessSpawn
                | OperationType::ProcessKill
                | OperationType::ProcessSuspend
        );
        let match_target = if is_command_op {
            matcher::normalize_exec_command(&req.target)
        } else {
            req.target.clone()
        };

        // 自杀形态硬拦（2026-09-16 用户裁决，先于一切规则遍——不进
        // exec_unknown_policy 开关）：递归删除类命令瞄准保护路径
        // （workspace root / home / `~` / 根）即拒。
        if matches!(
            req.op_type,
            OperationType::ProcessExec | OperationType::ProcessSpawn
        ) {
            let protected = self.protected_paths.read();
            if !protected.is_empty()
                && let Some(reason) = detect_self_destruct(&match_target, &protected)
            {
                return (
                    SecurityDecision::Denied,
                    reason,
                    "self_destruct".to_string(),
                );
            }
        }
        let action_bucket = |action: &str| -> &'static str {
            match action {
                "deny" | "denied" => "deny",
                "ask" | "require_approval" | "approval" | "pending" => "ask",
                "allow" | "allowed" => "allow",
                // 未知 action 语义 = normalize_decision 的 Denied 兜底 →
                // 归入 deny 遍。
                _ => "deny",
            }
        };
        let rule_matches = |rule: &SecurityRule| -> bool {
            match req.op_type {
                OperationType::FileRead
                | OperationType::FileWrite
                | OperationType::FileDelete
                | OperationType::DirRead
                | OperationType::DirCreate
                | OperationType::DirDelete
                | OperationType::RegistryRead
                | OperationType::RegistryWrite
                | OperationType::RegistryDelete => {
                    matcher::match_pattern(&rule.pattern, &req.target)
                }
                OperationType::ProcessExec
                | OperationType::ProcessSpawn
                | OperationType::ProcessKill
                | OperationType::ProcessSuspend => {
                    matcher::match_command_pattern(&rule.pattern.to_lowercase(), &match_target)
                }
                OperationType::NetworkDownload
                | OperationType::NetworkUpload
                | OperationType::NetworkRequest => {
                    matcher::match_domain_pattern(&rule.pattern, &req.target)
                }
                _ => rule.pattern == "*" || matcher::match_pattern(&rule.pattern, &req.target),
            }
        };

        // 三遍扫的 deny/ask 两遍先行；allow 遍刻意延后到解释器拆段扫描
        // 之后——外层 `python *` allow 若先命中就直接放行，内层载荷扫描
        // 永远到不了（`python -c "os.system('rm -rf ...')"` 被包装放行）。
        for want in ["deny", "ask"] {
            if let Some(op_rules) = op_rules {
                for (i, rule) in op_rules.iter().enumerate() {
                    if action_bucket(&rule.action) != want {
                        continue;
                    }
                    if rule_matches(rule) {
                        let reason = format!("rule matched: pattern={}", rule.pattern);
                        return (
                            normalize_decision(&rule.action),
                            reason,
                            format!("rule[{}]", i),
                        );
                    }
                }
            }
        }

        // CMD-02/06②：解释器包装拆段——外层 allow（`python *`、
        // `powershell *`…）不得屏蔽内层载荷（`-c`/`-e` 后的代码）的视线：
        // 载荷独立过一遍 deny/ask 规则（glob_contains 子串语义——载荷是
        // 代码片段，整串锚定打不中 `os.system('rm -rf /data')` 里的
        // `rm -r`）+ 危险结构词表。内层良性载荷自然落到下面的 allow 遍，
        // 与「内层 allow」同义不损失语义。仅 ProcessExec（spawn 的 target
        // 是可执行体不是命令行）。
        if req.op_type == OperationType::ProcessExec {
            for payload in matcher::extract_interpreter_payloads(&match_target) {
                if let Some(op_rules) = op_rules {
                    let mut inner_hit: Option<(SecurityDecision, String, String)> = None;
                    for want in ["deny", "ask"] {
                        for (i, rule) in op_rules.iter().enumerate() {
                            if action_bucket(&rule.action) != want {
                                continue;
                            }
                            if matcher::glob_contains(&rule.pattern.to_lowercase(), &payload) {
                                inner_hit = Some((
                                    normalize_decision(&rule.action),
                                    format!(
                                        "interpreter payload matched rule: pattern={} (inner of wrapper)",
                                        rule.pattern
                                    ),
                                    format!("rule[{}]:interpreter_inner", i),
                                ));
                                break;
                            }
                        }
                        if inner_hit.is_some() {
                            break;
                        }
                    }
                    if let Some(hit) = inner_hit {
                        return hit;
                    }
                }
                for structure in INTERPRETER_DANGEROUS_STRUCTURES {
                    if payload.contains(structure) {
                        return (
                            SecurityDecision::RequireApproval,
                            format!(
                                "interpreter payload contains dangerous structure `{}` (需要人工确认)",
                                structure
                            ),
                            "interpreter_structure".to_string(),
                        );
                    }
                }
            }
        }

        // allow 遍（延后，见上）。
        if let Some(op_rules) = op_rules {
            for (i, rule) in op_rules.iter().enumerate() {
                if action_bucket(&rule.action) != "allow" {
                    continue;
                }
                if rule_matches(rule) {
                    return (
                        normalize_decision(&rule.action),
                        format!("rule matched: pattern={}", rule.pattern),
                        format!("rule[{}]", i),
                    );
                }
            }
        }

        // D1：exec/spawn 未知命令（无规则命中）姿态。空串 = 未配置 →
        // 落 default_action（旧行为；老配置文件缺键不得悄悄变语义）。
        if matches!(
            req.op_type,
            OperationType::ProcessExec | OperationType::ProcessSpawn
        ) {
            let policy = self.exec_unknown_policy.read().clone();
            match policy.as_str() {
                "ask" => {
                    return (
                        SecurityDecision::RequireApproval,
                        "unknown command (no rule matched), exec_unknown_policy=ask".to_string(),
                        "exec_unknown_policy".to_string(),
                    );
                }
                "deny" => {
                    return (
                        SecurityDecision::Denied,
                        "unknown command (no rule matched), exec_unknown_policy=deny".to_string(),
                        "exec_unknown_policy".to_string(),
                    );
                }
                "allow" => {
                    return (
                        SecurityDecision::Allowed,
                        "unknown command (no rule matched), exec_unknown_policy=allow".to_string(),
                        "exec_unknown_policy".to_string(),
                    );
                }
                _ => {}
            }
        }

        let action = self.default_action.read();
        (
            normalize_decision(&action),
            "no rules matched, using default action".to_string(),
            "default".to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Internal helper functions
// ---------------------------------------------------------------------------

/// 自杀形态检测（exec/spawn 专用，2026-09-16 用户裁决硬拦）：破坏性删除
/// 动词 + 递归旗标 + 删除目标命中保护路径（workspace root / home / `~` /
/// 根 / `.` / `..` / `*` / `.git`）。判定窗口从动词起到下一个命令边界
/// （`&&` `;` `||` `|` `&`）——`cd <workspace> && rm -rf build` 不误伤
/// （窗口内无保护路径），`cd <workspace> && rm -rf .` 拦截。
/// 诚实边界：单文件 rm（无递归旗标）不拦——那是 ABAC 模板/exec_unknown_policy
/// 的治理面；相对路径多级目标（`rm -rf src/.git`）不在保护判定内。
fn detect_self_destruct(normalized: &str, protected: &[String]) -> Option<String> {
    const DESTRUCTIVE_BINS: &[&str] = &[
        "rm",
        "del",
        "rd",
        "erase",
        "rmdir",
        "deltree",
        "rimraf",
        "remove-item",
    ];
    fn bin_key(t: &str) -> &str {
        t.strip_suffix(".exe").unwrap_or(t)
    }
    let tokens: Vec<&str> = normalized.split(' ').filter(|t| !t.is_empty()).collect();

    let is_hit = |t: &str| -> bool {
        if t == "/" || t == "\\" {
            return true; // 根
        }
        if t.starts_with('-') {
            return false; // 旗标不是目标
        }
        // Windows 短旗标（/s /q /f /y，≤2 字符）按旗标处理；
        // 更长的 / 开头 token 是 POSIX 绝对路径，照常判定。
        if t.starts_with('/') && t.chars().count() <= 2 {
            return false;
        }
        let tt = t.replace('\\', "/");
        let tt = tt.trim_end_matches('/');
        if tt.is_empty() {
            return true; // `//` 之类退化即根
        }
        tt == "."
            || tt == ".."
            || tt == "*"
            || tt == ".git"
            || t.starts_with('~')
            || protected.iter().any(|p| {
                let p = p.trim_end_matches('/');
                // 目标=保护路径本身；目标是保护路径的祖先（删父目录带掉
                // 保护路径）；目标在保护路径之内（删子树同样毁灭保护内容）。
                p == tt
                    || (p.len() > tt.len() && p.starts_with(tt) && p.as_bytes()[tt.len()] == b'/')
                    || (tt.len() > p.len() && tt.starts_with(p) && tt.as_bytes()[p.len()] == b'/')
            })
    };

    for (i, token) in tokens.iter().enumerate() {
        if !DESTRUCTIVE_BINS.contains(&bin_key(token)) {
            continue;
        }
        let window: Vec<&str> = tokens[i + 1..]
            .iter()
            .copied()
            .take_while(|t| !matches!(*t, "&&" | ";" | "||" | "|" | "&"))
            .collect();
        let recursive = window.iter().any(|t| {
            (t.starts_with('-') && t.contains('r'))
                || (t.starts_with('/') && t.chars().count() == 2 && t.ends_with('s'))
        });
        if recursive && let Some(target) = window.iter().find(|t| is_hit(t)) {
            return Some(format!(
                "self-destruct protection: recursive delete targeting `{}` (protected path / bot home / workspace root)",
                target
            ));
        }
    }
    None
}

fn normalize_decision(action: &str) -> SecurityDecision {
    match action {
        "allow" | "allowed" => SecurityDecision::Allowed,
        "deny" | "denied" => SecurityDecision::Denied,
        "ask" | "require_approval" => SecurityDecision::RequireApproval,
        _ => SecurityDecision::Denied,
    }
}

/// Validate path is within workspace and safe.
fn validate_path_internal(path: &str, workspace: &str) -> Result<String, String> {
    use nemesis_path::paths::canonicalize_for_compare;
    // 2026-09-01 8.3 短名统一修复：裸 canonicalize + 词法回退在「workspace
    // 已存在（canonicalize 成长名）而 path 尚不存在（create 前守卫常态，回退
    // 保留 RUNNER~1 短名）」时前缀比较恒 false → 根内写入全被误拒。
    // canonicalize_for_compare 借最长存在祖先对齐双方表示后再比。
    let abs_path = canonicalize_for_compare(Path::new(path));

    if !workspace.is_empty() {
        let abs_workspace = canonicalize_for_compare(Path::new(workspace));

        match abs_path.strip_prefix(&abs_workspace) {
            Ok(rel) => {
                if rel.starts_with("..") {
                    return Err("access denied: path outside workspace".to_string());
                }
            }
            Err(_) => {
                if !abs_path.starts_with(&abs_workspace) {
                    return Err("access denied: path outside workspace".to_string());
                }
            }
        }
    }

    // Check dangerous system paths —— 对**原始输入**与规范化结果都查：
    // 规范化会把 POSIX 风格输入经根祖先拼成 Windows 盘符路径（/etc/passwd →
    // C:\etc\passwd），只查规范化结果会漏掉字面前缀命中（2026-09-01）。
    let dangerous = [
        "/etc/passwd",
        "/etc/shadow",
        "/etc/sudoers",
        "C:\\Windows\\System32\\drivers\\etc\\hosts",
    ];
    for candidate in [path, abs_path.to_string_lossy().as_ref()] {
        for d in &dangerous {
            if candidate.starts_with(d) {
                return Err("access denied: protected system path".to_string());
            }
        }
    }

    Ok(abs_path.to_string_lossy().to_string())
}

/// Check if a command is safe to execute.
fn is_safe_command_internal(command: &str) -> (bool, String) {
    use std::sync::OnceLock;
    static DANGEROUS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let patterns = DANGEROUS.get_or_init(|| {
        let raw = [
            r"(?i)\brm\s+-[rf]{1,2}\b",
            r"(?i)\bdel\s+/[fq]\b",
            r"(?i)\b(format|mkfs)\b",
            r"(?i)\bdd\s+if=",
            r"(?i)\b(shutdown|reboot|poweroff)\b",
            r"(?i)\bsudo\b",
            r"(?i)\bchmod\s+[0-7]{3,4}\b",
            r"(?i)\bchown\b",
        ];
        raw.iter()
            .filter_map(|p| regex::Regex::new(p).ok())
            .collect()
    });

    for re in patterns {
        if re.is_match(command) {
            return (false, "command contains dangerous pattern".to_string());
        }
    }
    (true, String::new())
}

// ---------------------------------------------------------------------------
// Audit log types and querying
// ---------------------------------------------------------------------------

/// An audit log event recording a security decision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditEvent {
    /// Unique event ID.
    pub event_id: String,
    /// The operation request that triggered this event.
    pub request: OperationRequest,
    /// Decision made: "allowed", "denied", "approved", "pending".
    pub decision: String,
    /// Human-readable reason for the decision.
    pub reason: String,
    /// When the decision was made (RFC 3339).
    pub timestamp: String,
    /// Which policy rule matched.
    pub policy_rule: String,
}

/// Filter for querying audit log events.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Filter by operation type.
    pub operation_type: Option<OperationType>,
    /// Filter by user.
    pub user: Option<String>,
    /// Filter by source.
    pub source: Option<String>,
    /// Filter by decision ("allowed", "denied", "approved", "pending").
    pub decision: Option<String>,
    /// Filter events after this time (RFC 3339).
    pub start_time: Option<String>,
    /// Filter events before this time (RFC 3339).
    pub end_time: Option<String>,
}

impl AuditFilter {
    /// Check if the filter has no constraints.
    pub fn is_empty(&self) -> bool {
        self.operation_type.is_none()
            && self.user.is_none()
            && self.source.is_none()
            && self.decision.is_none()
            && self.start_time.is_none()
            && self.end_time.is_none()
    }

    /// Check if an event matches this filter.
    pub fn matches(&self, event: &AuditEvent) -> bool {
        if let Some(ref op_type) = self.operation_type
            && event.request.op_type != *op_type
        {
            return false;
        }
        if let Some(ref user) = self.user
            && event.request.user != *user
        {
            return false;
        }
        if let Some(ref source) = self.source
            && (event.request.source.is_empty() || !event.request.source.contains(source))
        {
            return false;
        }
        if let Some(ref decision) = self.decision
            && event.decision != *decision
        {
            return false;
        }
        if let Some(ref start) = self.start_time
            && event.timestamp.as_str() < start.as_str()
        {
            return false;
        }
        if let Some(ref end) = self.end_time
            && event.timestamp.as_str() > end.as_str()
        {
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Global auditor singleton
// ---------------------------------------------------------------------------

/// Global auditor singleton.
///
/// Equivalent to Go's `globalAuditor` / `auditorOnce` pattern.
static GLOBAL_AUDITOR: std::sync::OnceLock<Arc<SecurityAuditor>> = std::sync::OnceLock::new();

/// Initialize the global security auditor.
///
/// Returns the global auditor. If already initialized, returns the existing
/// instance (ignoring the provided config).
///
/// Equivalent to Go's `InitGlobalAuditor()`.
pub fn init_global_auditor(config: AuditorConfig) -> Arc<SecurityAuditor> {
    GLOBAL_AUDITOR
        .get_or_init(|| Arc::new(SecurityAuditor::new(config)))
        .clone()
}

/// Get the global security auditor.
///
/// If the global auditor has not been initialized yet, initializes it with
/// default configuration.
///
/// Equivalent to Go's `GetGlobalAuditor()`.
pub fn get_global_auditor() -> Arc<SecurityAuditor> {
    GLOBAL_AUDITOR
        .get_or_init(|| Arc::new(SecurityAuditor::new(AuditorConfig::default())))
        .clone()
}

/// Reset the global auditor (for testing purposes only).
///
/// This is not exposed publicly; it is only available within the crate for
/// test cleanup.
#[cfg(test)]
fn _reset_global_auditor() {
    // parking_lot::OnceLock does not support reset. In tests we just
    // re-initialize a new auditor each time via init_global_auditor
    // (which returns the existing one if already set).
    // NOTE: std::sync::OnceLock also does not support reset.
    // Each test should create its own auditor instance for isolation.
}

// ---------------------------------------------------------------------------
// Security status monitoring
// ---------------------------------------------------------------------------

/// Monitor security status continuously, logging statistics at regular intervals.
///
/// This function runs until the `shutdown` future resolves. At each `interval`,
/// it logs the current security statistics and attempts to clean up old audit
/// log data.
///
/// Equivalent to Go's `MonitorSecurityStatus()`.
pub async fn monitor_security_status(
    auditor: Arc<SecurityAuditor>,
    interval_secs: u64,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let stats = auditor.get_statistics();
                tracing::info!(
                    total_events = %stats.get("total_events").unwrap_or(&serde_json::json!(0)),
                    allowed = %stats.get("allowed").unwrap_or(&serde_json::json!(0)),
                    denied = %stats.get("denied").unwrap_or(&serde_json::json!(0)),
                    pending = %stats.get("pending").unwrap_or(&serde_json::json!(0)),
                    enabled = %stats.get("enabled").unwrap_or(&serde_json::json!(false)),
                    "[Security] Security status monitor tick"
                );
            }
            _ = shutdown.changed() => {
                tracing::info!("[Security] Security status monitor shutting down");
                return;
            }
        }
    }
}

/// Query the audit log with optional filtering.
///
/// Since events are persisted to a file (if `audit_log_file_enabled` and
/// `audit_log_dir` are configured), this function reads the JSONL audit log
/// file and applies the provided filter. If no audit log file is configured,
/// returns an empty vector.
///
/// Equivalent to Go's `GetAuditLog()`.
pub fn get_audit_log(auditor: &SecurityAuditor, filter: &AuditFilter) -> Vec<AuditEvent> {
    let config = &auditor.config;
    if !config.audit_log_file_enabled {
        return Vec::new();
    }

    let log_dir = match &config.audit_log_dir {
        Some(d) if !d.is_empty() => d,
        _ => return Vec::new(),
    };

    let log_path = Path::new(log_dir).join("audit.jsonl");
    if !log_path.exists() {
        return Vec::new();
    }

    let content = match std::fs::read_to_string(&log_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %log_path.display(), error = %e, "[Security] Failed to read audit log");
            return Vec::new();
        }
    };

    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match serde_json::from_str::<AuditEvent>(line) {
            Ok(event) => {
                if filter.is_empty() || filter.matches(&event) {
                    events.push(event);
                }
            }
            Err(e) => {
                tracing::trace!(line = line, error = %e, "[Security] Skipping malformed audit log line");
            }
        }
    }

    events
}

#[cfg(test)]
mod tests;
