//! Security handler — config.get/config.save/audit/stats/signature_verify_status.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// §2 免打扰运行时态槽（无人值守计划 Part 2，editor.rs 同款依赖方向：
/// `ApprovalAutoApproveState` 本体在 nemesis-security，gateway 装配处
/// `install_auto_approve` 注入，本 handler `approvals.status` 读 +
/// `config.save` 热翻——单一真相源 Arc；config.json 是持久真相源）。
static APPROVAL_AUTO: std::sync::RwLock<
    Option<Arc<nemesis_security::approval_auto::ApprovalAutoApproveState>>,
> = std::sync::RwLock::new(None);

/// gateway 装配处（security_setup.rs）调用；未注入 = headless/降级形态，
/// `approvals.status` 返回 `injected: false`（前端展示「未装配」不误判）。
pub fn install_auto_approve(state: Arc<nemesis_security::approval_auto::ApprovalAutoApproveState>) {
    *APPROVAL_AUTO
        .write()
        .expect("approval auto state lock poisoned") = Some(state);
}

/// 全局槽只读访问（question broker 等 nemesisbot 侧消费方复用同一 Arc；
/// editor.rs 依赖方向同款）。未注入 = `None`。
pub fn approval_auto_state()
-> Option<Arc<nemesis_security::approval_auto::ApprovalAutoApproveState>> {
    APPROVAL_AUTO
        .read()
        .expect("approval auto state lock poisoned")
        .clone()
}

/// 槽置换口（仅测试，editor.rs `set_editor_access_for_test` 同款）：
/// None = 恢复未装配形态。
#[cfg(test)]
pub(crate) fn set_approval_auto_for_test(
    state: Option<Arc<nemesis_security::approval_auto::ApprovalAutoApproveState>>,
) {
    *APPROVAL_AUTO
        .write()
        .expect("approval auto state lock poisoned") = state;
}

/// 槽触碰类测试的共享串行锁（editor.rs EDITOR_TEST_LOCK 同款纪律——
/// 进程级槽是共享态，并行测试互相置换会 flake）。
#[cfg(test)]
pub(crate) static APPROVAL_AUTO_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn approval_auto() -> Option<Arc<nemesis_security::approval_auto::ApprovalAutoApproveState>> {
    approval_auto_state()
}

pub struct SecurityHandler {
    _priv: (),
}

impl Default for SecurityHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl SecurityHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for SecurityHandler {
    fn module_name(&self) -> &str {
        "security"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "config.get",
            "config.save",
            "audit",
            "stats",
            "approvals.list",
            "approvals.clear",
            "approvals.status",
            "signature_verify_status",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "config.get" => self.config_get(workspace),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(workspace, &data)
            }
            "audit" => {
                let limit = data
                    .as_ref()
                    .and_then(|d| d.get("limit"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as usize;
                let offset = data
                    .as_ref()
                    .and_then(|d| d.get("offset"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                self.audit(workspace, limit, offset)
            }
            "stats" => self.stats(workspace),
            // F3: 审批记忆规则表管理（文件直读，与 config.get/audit 同形态；
            // 运行中的 gateway 经 HotReloader mtime 在下一次审批查询时自动
            // 感知 clear）。
            "approvals.list" => self.approvals_list(workspace),
            "approvals.clear" => self.approvals_clear(workspace),
            // §2: 免打扰开关状态（运行期投影透传；config.get 的 approvals
            // 段是持久真相源，enabled 可能已被 config.save 热翻）。
            "approvals.status" => Ok(Some(Self::approvals_status())),
            // 签名验证状态（只读；无 set 命令——开关只走 config 且只读于
            // 启动，不开运行时写入口，少一个可篡改面）。
            "signature_verify_status" => Ok(Some(self.signature_verify_status(ctx))),
            _ => Err(format!("unknown command: security.{}", cmd)),
        }
    }
}

fn security_config_path(workspace: &str) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_security_config_path_in_workspace(Path::new(workspace))
}

/// F3: 审批记忆规则表管理（list/clear）。路径唯一真相源 nemesis-path。
fn approval_rules_path(workspace: &str) -> PathBuf {
    nemesis_path::resolve_approval_rules_path_in_workspace(Path::new(workspace))
}

fn security_log_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("logs/security_logs")
}

/// Flatten a nested AuditEvent JSON into the flat format expected by the frontend.
/// JSONL structure: {event_id, request: {op_type, danger_level, target, ...}, decision, reason, timestamp}
/// Frontend expects (AuditEntry in web/src/components/logs/mockData.ts):
///   {id, timestamp, operation, risk_level, target, result, decision, user?, reason?, policy?, raw}
/// where `result` is normalized to "allow" or "deny", `decision` keeps the original
/// string ("allowed"/"denied"/"approved"/...), and `raw` carries the full JSONL line
/// for the detail panel.
pub(crate) fn flatten_audit_entry(val: &serde_json::Value) -> serde_json::Value {
    let req = val.get("request");
    let raw_decision = val.get("decision").and_then(|v| v.as_str()).unwrap_or("");
    let result = if raw_decision.to_lowercase().starts_with("allow")
        || raw_decision.eq_ignore_ascii_case("approved")
    {
        "allow"
    } else {
        "deny"
    };
    serde_json::json!({
        "id": val.get("event_id").and_then(|v| v.as_str()).unwrap_or(""),
        "timestamp": val.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
        "operation": req.and_then(|r| r.get("op_type")).and_then(|v| v.as_str()).unwrap_or(""),
        "risk_level": req.and_then(|r| r.get("danger_level")).and_then(|v| v.as_str()).unwrap_or(""),
        "target": req.and_then(|r| r.get("target")).and_then(|v| v.as_str()).unwrap_or(""),
        "result": result,
        "decision": raw_decision,
        "user": req.and_then(|r| r.get("user")).and_then(|v| v.as_str()).unwrap_or(""),
        "reason": val.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
        "policy": val.get("policy_rule").and_then(|v| v.as_str()).unwrap_or(""),
        "raw": val,
    })
}

/// Extract risk_level from a raw JSONL audit entry (nested in request.danger_level).
pub(crate) fn extract_risk_level(val: &serde_json::Value) -> &str {
    val.get("request")
        .and_then(|r| r.get("danger_level"))
        .and_then(|v| v.as_str())
        .or_else(|| val.get("risk_level").and_then(|v| v.as_str()))
        .unwrap_or("unknown")
}

impl SecurityHandler {
    /// 签名验证状态（只读透传；未注入 = 开发/降级形态，返回
    /// `injected: false` 让前端展示「无数据」而非误判成 off）。
    fn signature_verify_status(&self, ctx: &RequestContext) -> serde_json::Value {
        match ctx.state.signature_verify.as_ref() {
            Some(s) => serde_json::json!({
                "injected": true,
                "mode": s.mode,
                "locked": s.locked,
                "anchor_fp": s.anchor_fp,
                "last_result": s.last_result,
                "key_fp": s.key_fp,
                "detail": s.detail,
            }),
            None => serde_json::json!({ "injected": false }),
        }
    }

    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = security_config_path(workspace);
        let config = nemesis_config::load_security_config(&path)
            .map_err(|e| format!("failed to load security config: {}", e))?;
        let json =
            serde_json::to_value(&config).map_err(|e| format!("failed to serialize: {}", e))?;
        Ok(Some(json))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let config: nemesis_config::SecurityConfig = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid security config: {}", e))?;
        let path = security_config_path(workspace);
        nemesis_config::save_security_config(&path, &config)
            .map_err(|e| format!("failed to save security config: {}", e))?;
        // §2: 免打扰热翻（运行期投影即时生效；config 是持久真相源，这里
        // 只翻投影——auditor 下一次 require_approval 即读新值）。
        if let Some(state) = approval_auto() {
            state.set_enabled(config.approvals.auto_approve);
        }
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    /// §2: 免打扰状态（未注入 = 开发/降级形态，返回 `injected: false` 让
    /// 前端展示「未装配」而非误判成关）。
    fn approvals_status() -> serde_json::Value {
        match approval_auto() {
            Some(state) => serde_json::json!({
                "injected": true,
                "enabled": state.enabled(),
                "auto_approved_count": state.count(),
            }),
            None => serde_json::json!({ "injected": false }),
        }
    }

    fn audit(
        &self,
        workspace: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Option<serde_json::Value>, String> {
        let log_dir = security_log_dir(workspace);
        if !log_dir.exists() {
            return Ok(Some(serde_json::json!({ "entries": [], "total": 0 })));
        }

        let mut entries = Vec::new();
        let read_dir = std::fs::read_dir(&log_dir)
            .map_err(|e| format!("failed to read security log dir: {}", e))?;

        for entry in read_dir {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                for line in content.lines().rev() {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                        entries.push(flatten_audit_entry(&val));
                    }
                }
            }
        }

        // Sort by timestamp descending (if available)
        entries.sort_by(|a, b| {
            let ts_a = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            let ts_b = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            ts_b.cmp(ts_a)
        });

        let total = entries.len();
        let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
        Ok(Some(serde_json::json!({
            "entries": page,
            "total": total,
            "limit": limit,
            "offset": offset,
        })))
    }

    fn stats(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let log_dir = security_log_dir(workspace);
        if !log_dir.exists() {
            return Ok(Some(serde_json::json!({
                "total_events": 0,
                "by_level": {},
            })));
        }

        let mut total = 0usize;
        let mut by_level: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        let read_dir =
            std::fs::read_dir(&log_dir).map_err(|e| format!("failed to read dir: {}", e))?;
        for entry in read_dir {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                for line in content.lines() {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                        total += 1;
                        let level = extract_risk_level(&val).to_string();
                        *by_level.entry(level).or_insert(0) += 1;
                    }
                }
            }
        }

        Ok(Some(serde_json::json!({
            "total_events": total,
            "by_level": by_level,
        })))
    }

    /// F3: 列出「总是允许」规则表（纯 JSON 读——nemesis-web 不依赖
    /// nemesis-security，规则条目原样透传给前端展示）。
    fn approvals_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = approval_rules_path(workspace);
        let rules: Vec<serde_json::Value> = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read approval rules: {}", e))?;
            serde_json::from_str(&raw)
                .map_err(|e| format!("failed to parse approval rules: {}", e))?
        } else {
            Vec::new()
        };
        Ok(Some(serde_json::json!({ "rules": rules })))
    }

    /// F3: 清空「总是允许」规则表（写回空表；文件保留 = 运行中 gateway 的
    /// HotReloader 照常经 mtime 感知）。
    fn approvals_clear(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = approval_rules_path(workspace);
        let removed = if path.exists() {
            let raw = std::fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str::<Vec<serde_json::Value>>(&raw)
                .map(|v| v.len())
                .unwrap_or(0)
        } else {
            0
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create config dir: {}", e))?;
        }
        // REL-002：统一原子写入（approval 规则表清空）。
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), b"[]\n", 0o600)
            .map_err(|e| format!("failed to write approval rules: {}", e))?;
        Ok(Some(
            serde_json::json!({ "cleared": true, "removed": removed }),
        ))
    }
}

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): audit paging +
// decision normalization + flatten/extract fallback arms + config error arms.
#[cfg(test)]
mod s10b_tests;

// F3 (2026-09-06, devtool-upgrade 阶段 5): approvals.list / approvals.clear
// 规则表管理命令。
#[cfg(test)]
mod f3_tests;

// AGT 覆盖率批次（2026-09-25）：Default 转发、signature_verify_status 双槽
// （未注入降级 / 注入六字段透传）、audit·stats 非 jsonl 项跳过臂、
// approvals.clear 无文件 removed=0 + config 目录创建失败上抛。
#[cfg(test)]
mod agt_tests;

// §2 免打扰（2026-10-11 无人值守计划 Part 2）：approvals.status 双臂 +
// config.save 热翻投影。
#[cfg(test)]
mod approval_auto_tests;
