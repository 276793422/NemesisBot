//! Security handler — config.get/config.save/audit/stats/signature_verify_status.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 免打扰运行时态槽（Part 2，editor.rs 同款依赖方向：
/// `ApprovalAutoApproveState` 本体在 nemesis-security，gateway 装配处
/// `install_auto_approve` 注入，本 handler `approvals.status` 读 +
/// `config.save` 热翻——统一定义 Arc；config.json 是持久真相源）。
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

/// 会话级全部放行运行时态槽（长任务，approval_auto
/// 同款双注入：gateway 装配处 `install_session_allow_all` 注入，本 handler
/// `session_allow`/`session_allow_status` 读写——统一定义 Arc；持久真相源
/// = 会话 sidecar `allow_all` 字段，set 时双写内存 + sidecar）。
static SESSION_ALLOW_ALL: std::sync::RwLock<
    Option<Arc<nemesis_security::session_allow_all::SessionAllowAllState>>,
> = std::sync::RwLock::new(None);

/// gateway 装配处（security_setup.rs）调用；未注入 = headless/降级形态，
/// `session_allow*` 返回 `injected: false`（前端展示「未装配」不误判）。
pub fn install_session_allow_all(
    state: Arc<nemesis_security::session_allow_all::SessionAllowAllState>,
) {
    *SESSION_ALLOW_ALL
        .write()
        .expect("session allow-all state lock poisoned") = Some(state);
}

/// 槽置换口（仅测试，set_approval_auto_for_test 同款）。
#[cfg(test)]
pub(crate) fn set_session_allow_all_for_test(
    state: Option<Arc<nemesis_security::session_allow_all::SessionAllowAllState>>,
) {
    *SESSION_ALLOW_ALL
        .write()
        .expect("session allow-all state lock poisoned") = state;
}

/// 槽触碰类测试的共享串行锁（APPROVAL_AUTO_TEST_LOCK 同款纪律）。
/// 用 tokio Mutex 而非 parking_lot：异步用例持锁跨 await（tokio::test 单线程
/// 串行执行，无竞争），sync guard 跨 await 会撞 clippy await_holding_lock。
#[cfg(test)]
pub(crate) static SESSION_ALLOW_ALL_TEST_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

/// 内存裁决与 sidecar 持久半边的串行锁：`state.set` 与
/// `write_session_allow_all` 必须相邻原子——快速开/关交错时，若无此锁，
/// 「后发的 disable 先写盘、先发的 enable 后写盘」会让内存终值=关而磁盘
/// 终值=开（重启恢复出放行）。parking_lot 同步锁：handler 全同步（fs 写），
/// 无 await 点。
static SESSION_ALLOW_PERSIST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn session_allow_all_state()
-> Option<Arc<nemesis_security::session_allow_all::SessionAllowAllState>> {
    SESSION_ALLOW_ALL
        .read()
        .expect("session allow-all state lock poisoned")
        .clone()
}

/// 当前全部启用中的会话键（sessions.list 投影用；未注入 = 空集）。
pub(crate) fn session_allow_enabled_keys() -> std::collections::HashSet<String> {
    session_allow_all_state()
        .map(|s| s.enabled_keys().into_iter().collect())
        .unwrap_or_default()
}

/// 会话键推导（chat.rs:61-63 `chat.sync` 同一公式——**字节一致是硬契约**：
/// 两处口径不一致 = 开关静默不命中）。入参是前端持有的 session_id
///（sessions.list 行 `id` 字段）。
fn derive_session_key(session_id: &str) -> String {
    format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(session_id)
    )
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
            "approvals.add",
            "approvals.remove",
            "approvals.status",
            "eval_rules.list",
            "eval_rules.add",
            "eval_rules.update",
            "eval_rules.remove",
            "eval_rules.set_enabled",
            "eval_rules.reset",
            "session_allow",
            "session_allow_status",
            "signature_verify_status",
            // 十层页面化（2026-11-10）：总览聚合 + ② 黑名单/④ 凭据规则只读快照。
            "overview",
            "command_blocklist",
            "credential_rules",
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
            // 审批记忆规则表管理（文件直读，与 config.get/audit 同形态；
            // 运行中的 gateway 经 HotReloader mtime 在下一次审批查询时自动
            // 感知 clear）。
            "approvals.list" => self.approvals_list(workspace),
            "approvals.clear" => self.approvals_clear(workspace),
            // 条目级管理（SecurityView 表单 + 行级删除）：写入/删除走
            // nemesis-security 同源闸（安装类拒 + 危级静态闸 + 读侧死形态
            // 拒）——人工写入与「总是允许」自动写入同一准入，无旁路。
            "approvals.add" => {
                let data = data.ok_or("missing data")?;
                let op = data
                    .get("op")
                    .and_then(|v| v.as_str())
                    .ok_or("missing op")?;
                let pattern = data
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or("missing pattern")?;
                let action = data
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("allow");
                let stored = nemesis_security::approval_rules::upsert_manual_rule(
                    &approval_rules_path(workspace),
                    op,
                    pattern,
                    action,
                )?;
                Ok(Some(
                    serde_json::json!({ "added": true, "pattern": stored }),
                ))
            }
            "approvals.remove" => {
                let data = data.ok_or("missing data")?;
                let op = data
                    .get("op")
                    .and_then(|v| v.as_str())
                    .ok_or("missing op")?;
                let pattern = data
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or("missing pattern")?;
                let removed = nemesis_security::approval_rules::remove_manual_rule(
                    &approval_rules_path(workspace),
                    op,
                    pattern,
                )?;
                Ok(Some(serde_json::json!({ "removed": removed })))
            }
            // eval 评估规则管理（SecurityView 评估规则 TAB）。数据半边单一
            // 真相源 nemesis-config::eval_rules（CLI 经 nemesisbot re-export
            // 消费同一模块）；路径真相源 nemesis-path（与 CLI
            // rules_file_path 同一拼接点）。写命令语义镜像 CLI（add 冲突拒、
            // update/remove/set_enabled 不存在报错、reset 回内置默认集）。
            "eval_rules.list" => self.eval_rules_list(workspace),
            "eval_rules.add" => {
                let rule = eval_rules_payload(data)?;
                self.eval_rules_add(workspace, rule)
            }
            "eval_rules.update" => {
                let rule = eval_rules_payload(data)?;
                self.eval_rules_update(workspace, rule)
            }
            "eval_rules.remove" => {
                let id = require_str_field(data.as_ref(), "id")?;
                self.eval_rules_remove(workspace, id)
            }
            "eval_rules.set_enabled" => {
                let id = require_str_field(data.as_ref(), "id")?;
                let enabled = data
                    .as_ref()
                    .and_then(|d| d.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .ok_or("missing enabled")?;
                self.eval_rules_set_enabled(workspace, id, enabled)
            }
            "eval_rules.reset" => self.eval_rules_reset(workspace),
            // 免打扰开关状态（运行期投影透传；config.get 的 approvals
            // 段是持久真相源，enabled 可能已被 config.save 热翻）。
            "approvals.status" => Ok(Some(Self::approvals_status())),
            // 会话级全部放行（长任务）：set 双写
            // （内存即时生效 + sidecar 重启恢复）；status 供前端刷新。
            "session_allow" => {
                let data = data.ok_or("missing data")?;
                let session_id = data
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing session_id")?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .ok_or("missing enabled")?;
                self.session_allow(session_id, enabled)
            }
            "session_allow_status" => {
                let data = data.unwrap_or(serde_json::Value::Null);
                let session_id = data
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing session_id")?;
                Ok(Some(Self::session_allow_status(session_id)))
            }
            // 签名验证状态（只读；无 set 命令——开关只走 config 且只读于
            // 启动，不开运行时写入口，少一个可篡改面）。
            "signature_verify_status" => Ok(Some(self.signature_verify_status(ctx))),
            // ── 十层页面化（2026-11-10）────────────────────────────
            // 总览页聚合快照（全部只读；层开关/审计计数/链/estop/Plan/
            // Full Access/免打扰/签名/⑦ scanner/总闸/executor 十一段）。
            "overview" => self.overview(ctx, workspace),
            // 第②层页：内置命令黑名单只读快照（§2.7 核实：编译期静态表，
            // 扩展 API 生产零接线——`extra_wired:false` 供层页诚实注记）。
            "command_blocklist" => Ok(Some(Self::command_blocklist())),
            // 第④层页：内置凭据规则名只读快照（§2.7 核实：32 条 OnceLock
            // 纯硬编码，无 config 扩展面）。
            "credential_rules" => Ok(Some(Self::credential_rules())),
            _ => Err(format!("unknown command: security.{}", cmd)),
        }
    }
}

fn security_config_path(workspace: &str) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_security_config_path_in_workspace(Path::new(workspace))
}

/// config.security.json RMW 事务唯一入口——写串行锁在函数内取得，罩
/// 「load → 闭包改 → 落盘」全程；闭包返回 Err 不落盘（abort 语义）。
/// 纪律：闭包内不得再调任何 update_* / [`nemesis_config::config_write_lock`]
/// （Mutex 非重入，嵌套即死锁）。（同类横扫七期补网，。）
fn update_security_config<T>(
    workspace: &str,
    f: impl FnOnce(&mut nemesis_config::SecurityConfig) -> Result<T, String>,
) -> Result<T, String> {
    let _serial = nemesis_config::config_write_lock();
    let path = security_config_path(workspace);
    let mut cfg = nemesis_config::load_security_config(&path)
        .map_err(|e| format!("failed to load security config: {}", e))?;
    let out = f(&mut cfg)?;
    nemesis_config::save_security_config(&path, &cfg)
        .map_err(|e| format!("failed to save security config: {}", e))?;
    Ok(out)
}

/// 审批记忆规则表管理（list/clear）。路径唯一依据 nemesis-path。
fn approval_rules_path(workspace: &str) -> PathBuf {
    nemesis_path::resolve_approval_rules_path_in_workspace(Path::new(workspace))
}

/// eval 评估规则表路径（CLI rules_file_path 经 nemesis-path 委托同一拼接点
/// ——两端字节一致是硬契约）。
fn eval_rules_path(workspace: &str) -> PathBuf {
    nemesis_path::resolve_eval_rules_path_in_workspace(Path::new(workspace))
}

fn security_log_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("logs/security_logs")
}

/// Flatten a nested AuditEvent JSON into the flat format expected by the frontend.
/// JSONL structure: {event_id, request: {op_type, danger_level, target,...}, decision, reason, timestamp}
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
        // 锁在 update_security_config 内（整档回存与 config.json 编辑臂同基准）。
        // 热翻所需布尔先捕获（闭包按值拿走 config）。
        let auto_approve = config.approvals.auto_approve;
        update_security_config(workspace, |cfg| {
            *cfg = config;
            Ok(())
        })?;
        // 免打扰热翻（运行期投影即时生效；config 是持久真相源，这里
        // 只翻投影——auditor 下一次 require_approval 即读新值）。
        if let Some(state) = approval_auto() {
            state.set_enabled(auto_approve);
        }
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    /// 免打扰状态（未注入 = 开发/降级形态，返回 `injected: false` 让
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

    /// 会话级全部放行 set：双写（内存态即时生效 + sidecar 持久位重启恢复）。
    /// 返回 set 后快照 `(enabled, since, count)` 供前端即时渲染；`persisted`
    /// 标注持久半边是否真的落盘（持久化失败不再谎报普通成功
    /// 响应带 `persisted:false` + `warning`，内存态按用户当次意志保留）。
    fn session_allow(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Result<Option<serde_json::Value>, String> {
        let Some(state) = session_allow_all_state() else {
            return Ok(Some(serde_json::json!({ "injected": false })));
        };
        let session_key = derive_session_key(session_id);
        // 内存裁决与持久半边串行（快速开/关交错不得让内存与磁盘终值
        // 相反）。parking_lot 同步锁——本 handler 全同步（fs 写），无 await。
        let _serial = SESSION_ALLOW_PERSIST_LOCK.lock();
        let (since, count) = state.set(&session_key, enabled);
        // sidecar 持久半边（nemesis-web 直调 chat_log 有先例——handlers/
        // sessions.rs clear_undelivered_replies）。since 传内存态返回值——
        // 单一真相，两半边不各打各的时间戳。写失败不回滚内存态：运行期裁决
        // 已生效（关=安全方向保持关；开=用户当次意志），但重启恢复面不可信
        // 响应显式 `persisted:false` + warning，由前端向用户告警。
        let mut body = serde_json::json!({
            "injected": true,
            "session_id": session_id,
            "session_key": session_key,
            "enabled": enabled,
            "since": since,
            "count": count,
            "persisted": true,
        });
        if let Err(e) =
            nemesis_agent::chat_log::write_session_allow_all(&session_key, enabled, &since)
        {
            body["persisted"] = serde_json::Value::Bool(false);
            let direction = if enabled {
                "已启用（本进程内生效）；持久化失败，重启后不保持"
            } else {
                "已关闭（本进程内生效）；持久化失败，重启后将恢复放行——请立即重试或人工核对 sidecar"
            };
            body["warning"] = serde_json::Value::String(format!("{direction}：{e}"));
            tracing::warn!(
                "[security] session_allow 持久化失败 session_key={} enabled={}：{}",
                session_key,
                enabled,
                e
            );
        }
        Ok(Some(body))
    }

    /// 会话级全部放行状态（前端刷新用；未注入 = `injected: false`）。
    fn session_allow_status(session_id: &str) -> serde_json::Value {
        match session_allow_all_state() {
            Some(state) => {
                let session_key = derive_session_key(session_id);
                match state.status(&session_key) {
                    Some((enabled, since, count)) => serde_json::json!({
                        "injected": true,
                        "session_id": session_id,
                        "session_key": session_key,
                        "enabled": enabled,
                        "since": since,
                        "count": count,
                    }),
                    None => serde_json::json!({
                        "injected": true,
                        "session_id": session_id,
                        "session_key": session_key,
                        "enabled": false,
                        "since": "",
                        "count": 0,
                    }),
                }
            }
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

    /// 列出「总是允许」规则表（纯 JSON 读——nemesis-web 不依赖
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

    /// 清空「总是允许」规则表（写回空表；文件保留 = 运行中 gateway 的
    /// HotReloader 照常经 mtime 感知）。
    fn approvals_clear(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        // 写串行锁：approval_rules.json 与 WebApprovalManager::save_always_rule
        // 是同一文件的两个写者——「总是允许」RMW 已持 config_write_lock（叠加
        // 自身 rules_write），本清空臂同锁互斥，防并发清空被复活/规则被丢
        //（同类横扫四期）。
        let _cfg_serial = nemesis_config::config_write_lock();
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
        // 统一原子写入（approval 规则表清空）。
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), b"[]\n", 0o600)
            .map_err(|e| format!("failed to write approval rules: {}", e))?;
        Ok(Some(
            serde_json::json!({ "cleared": true, "removed": removed }),
        ))
    }

    // ── eval 评估规则管理（SecurityView 评估规则 TAB）──────────────────
    //
    // 数据半边单一真相源 nemesis-config::eval_rules；路径真相源
    // nemesis-path。全部写命令 RMW 全程持 config 家族写串行锁（与
    // update_security_config 同纪律：方法内不得再取锁）。运行中的 eval
    // 每次评估现读盘，写入即对下一次评估生效（无热载问题）。

    fn eval_rules_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        // 与 CLI list 同源：load_rules 缺文件时种子默认集（首访即见内置规则）。
        let file = nemesis_config::eval_rules::load_rules(&eval_rules_path(workspace))
            .map_err(|e| format!("failed to load eval rules: {e}"))?;
        let json = serde_json::to_value(&file).map_err(|e| format!("failed to serialize: {e}"))?;
        Ok(Some(json))
    }

    fn eval_rules_add(
        &self,
        workspace: &str,
        rule: nemesis_config::eval_rules::Rule,
    ) -> Result<Option<serde_json::Value>, String> {
        nemesis_config::eval_rules::validate_rule(&rule)
            .map_err(|e| format!("invalid rule: {e}"))?;
        let path = eval_rules_path(workspace);
        let _serial = nemesis_config::config_write_lock();
        let mut file = nemesis_config::eval_rules::load_rules(&path)
            .map_err(|e| format!("failed to load eval rules: {e}"))?;
        // add 冲突拒绝（镜像 CLI cmd_add：改用 update 替换）。
        if file.rules.iter().any(|r| r.id == rule.id) {
            return Err(format!(
                "rule '{}' already exists (use eval_rules.update to replace it)",
                rule.id
            ));
        }
        file.rules.push(rule.clone());
        nemesis_config::eval_rules::save_rules(&path, &file)
            .map_err(|e| format!("failed to save eval rules: {e}"))?;
        Ok(Some(serde_json::json!({ "added": true, "id": rule.id })))
    }

    fn eval_rules_update(
        &self,
        workspace: &str,
        rule: nemesis_config::eval_rules::Rule,
    ) -> Result<Option<serde_json::Value>, String> {
        nemesis_config::eval_rules::validate_rule(&rule)
            .map_err(|e| format!("invalid rule: {e}"))?;
        let path = eval_rules_path(workspace);
        let _serial = nemesis_config::config_write_lock();
        let mut file = nemesis_config::eval_rules::load_rules(&path)
            .map_err(|e| format!("failed to load eval rules: {e}"))?;
        let idx = file
            .rules
            .iter()
            .position(|r| r.id == rule.id)
            .ok_or_else(|| format!("rule '{}' not found", rule.id))?;
        file.rules[idx] = rule.clone();
        nemesis_config::eval_rules::save_rules(&path, &file)
            .map_err(|e| format!("failed to save eval rules: {e}"))?;
        Ok(Some(serde_json::json!({ "updated": true, "id": rule.id })))
    }

    fn eval_rules_remove(
        &self,
        workspace: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = eval_rules_path(workspace);
        let _serial = nemesis_config::config_write_lock();
        let mut file = nemesis_config::eval_rules::load_rules(&path)
            .map_err(|e| format!("failed to load eval rules: {e}"))?;
        let before = file.rules.len();
        file.rules.retain(|r| r.id != id);
        if file.rules.len() == before {
            return Err(format!("rule '{id}' not found"));
        }
        nemesis_config::eval_rules::save_rules(&path, &file)
            .map_err(|e| format!("failed to save eval rules: {e}"))?;
        Ok(Some(serde_json::json!({ "removed": true, "id": id })))
    }

    fn eval_rules_set_enabled(
        &self,
        workspace: &str,
        id: &str,
        enabled: bool,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = eval_rules_path(workspace);
        let _serial = nemesis_config::config_write_lock();
        let mut file = nemesis_config::eval_rules::load_rules(&path)
            .map_err(|e| format!("failed to load eval rules: {e}"))?;
        let rule = file
            .rules
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("rule '{id}' not found"))?;
        rule.enabled = enabled;
        nemesis_config::eval_rules::save_rules(&path, &file)
            .map_err(|e| format!("failed to save eval rules: {e}"))?;
        Ok(Some(serde_json::json!({ "id": id, "enabled": enabled })))
    }

    fn eval_rules_reset(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = eval_rules_path(workspace);
        let _serial = nemesis_config::config_write_lock();
        // 先 load（缺文件时种子——与 CLI cmd_reset 同语义）拿到将被替换的
        // 条数；再以内置默认集覆写。UI 侧弹确认框（CLI 的交互确认在前端）。
        let existing = nemesis_config::eval_rules::load_rules(&path)
            .map_err(|e| format!("failed to load eval rules: {e}"))?;
        let replaced = existing.rules.len();
        let defaults =
            nemesis_config::eval_rules::parse_rules(nemesis_config::eval_rules::DEFAULT_RULES_JSON)
                .map_err(|e| format!("built-in default rules damaged: {e}"))?;
        let count = defaults.rules.len();
        nemesis_config::eval_rules::save_rules(&path, &defaults)
            .map_err(|e| format!("failed to save eval rules: {e}"))?;
        Ok(Some(
            serde_json::json!({ "reset": true, "replaced": replaced, "count": count }),
        ))
    }

    // ── 十层页面化（2026-11-10）：总览聚合 + 只读快照 ─────────────────
    //
    // 边界（计划 §5.1）：security 域能知晓的（层开关/审计计数/审计链/
    // estop/Plan/Full Access/免打扰/签名）+ ⑦ scanner（复用
    // `api_handlers::load_scanner_status` 单一真相源）+ 主 config.json 的
    // `security.enabled` 总闸与 `executor` 段（`read_raw_config`）——零跨
    // feature 耦合，scanner/sandbox/vm 模块门控裁掉时本命令仍可用（对应
    // 段落投影降级形态）。全部只读，无写入口。

    /// `security.overview`：总览页聚合快照（十层启停态 + 按 policy 聚合的
    /// 拦截计数 + 链在场态 + estop/Plan/Full Access/免打扰/签名快照）。
    fn overview(
        &self,
        ctx: &RequestContext,
        workspace: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(serde_json::json!({
            "layers": Self::layers_projection(workspace)?,
            "intercepts": Self::audit_policy_aggregate(workspace)?,
            "chain": Self::chain_snapshot(workspace),
            "estop": Self::estop_snapshot(ctx),
            "plan_mode": Self::plan_mode_snapshot(ctx),
            "editor": Self::editor_snapshot(),
            "approvals": Self::approvals_status(),
            "guardian": Self::guardian_snapshot(workspace),
            "vault": Self::vault_snapshot(workspace),
            "signature": self.signature_verify_status(ctx),
            "scanner": Self::scanner_snapshot(workspace),
            "global": Self::global_switch_snapshot(ctx),
            "executor": Self::executor_snapshot(ctx),
        })))
    }

    /// 层开关投影——**raw JSON 直读，镜像 runtime 消费语义**。
    ///
    /// `apply_security_layer_switches`（security_setup.rs）只认
    /// `layers.<name>.enabled` 显式 bool，缺席 = `SecurityPluginConfig::default()`
    /// 构造引擎 = 启用；而 typed `SecurityLayerConfig.enabled` 的 serde
    /// default 是 false——`{}` 形态部署经 typed 投影会误显示为关闭。
    /// 本函数与 runtime 逐键对齐：缺席 = true、显式 bool = 该值。
    /// 全部 layers.* 开关改动需重启网关生效（reload_config 假姿态，计划
    /// §2.5），`effective: "restart"` 提示字段供前端标注生效时机；
    /// `global.enabled`（主 config.json security.enabled 总闸）是例外，
    /// 真热生效，另段投影。
    fn layers_projection(workspace: &str) -> Result<serde_json::Value, String> {
        let path = security_config_path(workspace);
        let raw = match nemesis_utils::fsutil::read_config_string(&path) {
            Ok(s) => serde_json::from_str::<serde_json::Value>(&s)
                .map_err(|e| format!("invalid security config: {e}"))?,
            // 缺文件 = 全默认（runtime ensure_security_config 自愈/构造期
            // 默认同语义），不是错误。
            Err(_) => serde_json::json!({}),
        };
        let layers = raw.get("layers");
        let layer = |name: &str| layers.and_then(|l| l.get(name));
        // 层开关缺席 = runtime 默认构造引擎 = 启用（raw 语义，非 typed）。
        let flag = |name: &str| {
            layer(name)
                .and_then(|d| d.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        };
        let str_opt = |v: Option<&serde_json::Value>| -> Option<String> {
            v.and_then(|x| x.as_str()).map(String::from)
        };
        // ① 注入阈值（CFG-06 同款语义）：配置在 [0,1] 才生效，否则默认
        // 0.7（runtime 越界 warn + 保持 Default）。
        let threshold_raw = layer("injection")
            .and_then(|d| d.get("extra"))
            .and_then(|x| x.get("threshold"))
            .and_then(|v| v.as_f64());
        let (threshold, threshold_configured) = match threshold_raw {
            Some(t) if (0.0..=1.0).contains(&t) => (t, true),
            _ => (0.7, false),
        };
        let dlp = layer("dlp");
        let dlp_arr = |key: &str| -> Option<Vec<String>> {
            dlp.and_then(|d| d.get(key))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
        };
        Ok(serde_json::json!({
            "injection": {
                "enabled": flag("injection"),
                "threshold": threshold,
                "threshold_configured": threshold_configured,
            },
            "command_guard": { "enabled": flag("command_guard") },
            // ③ ABAC：无独立开关（跟总闸），auditor 构造期无条件装配。
            "abac": { "standalone_switch": false },
            "credential": { "enabled": flag("credential") },
            "dlp": {
                "enabled": dlp.and_then(|d| d.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(true),
                "rules": dlp_arr("rules").unwrap_or_default(),
                "action": str_opt(dlp.and_then(|d| d.get("action"))),
                "low_confidence_action": str_opt(dlp.and_then(|d| d.get("low_confidence_action"))),
                "inbound_action": str_opt(dlp.and_then(|d| d.get("inbound_action"))),
            },
            "ssrf": {
                "enabled": flag("ssrf"),
                "allowed_hosts": layer("ssrf")
                    .and_then(|d| d.get("allowed_hosts"))
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|h| h.as_str().map(String::from)).collect::<Vec<_>>())
                    .unwrap_or_default(),
            },
            // ⑧ 审计两开关：顶层键（runtime 构造期读取）；缺省 log=true /
            // chain=false。
            "audit": {
                "log_file_enabled": raw.get("audit_log_file_enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                "chain_enabled": raw.get("audit_chain_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            },
            "default_action": raw.get("default_action").and_then(|v| v.as_str()).unwrap_or("deny"),
            "effective": "restart",
        }))
    }

    /// 审计事件按 policy 聚合（读全部 `*.jsonl` **排除 audit_chain.jsonl**
    /// ——链段是 Merkle 记账不是审计事件；现有 `audit()`/`stats()` 不过滤，
    /// 本聚合是层页过滤语义的聚合面）。policy ← JSONL 行 `policy_rule`
    /// 字段（flatten_audit_entry 同源）；空 = "unknown" 桶。
    fn audit_policy_aggregate(workspace: &str) -> Result<serde_json::Value, String> {
        let log_dir = security_log_dir(workspace);
        let mut total = 0u64;
        let mut denied = 0u64;
        // policy → (total, denied, last_ts)；BTreeMap 键序确定（重复审计
        // 请求输出字节稳定，同 typed config 段纪律）。
        let mut by_policy: std::collections::BTreeMap<String, (u64, u64, String)> =
            std::collections::BTreeMap::new();
        if log_dir.exists() {
            let read_dir = std::fs::read_dir(&log_dir)
                .map_err(|e| format!("failed to read security log dir: {}", e))?;
            for entry in read_dir {
                let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
                let path = entry.path();
                if path.file_name().and_then(|n| n.to_str()) == Some("audit_chain.jsonl") {
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                    && let Ok(content) = std::fs::read_to_string(&path)
                {
                    for line in content.lines() {
                        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
                            continue;
                        };
                        let flat = flatten_audit_entry(&val);
                        let policy = flat
                            .get("policy")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .unwrap_or("unknown");
                        let is_deny = flat.get("result").and_then(|v| v.as_str()) == Some("deny");
                        let ts = flat.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
                        total += 1;
                        if is_deny {
                            denied += 1;
                        }
                        let slot = by_policy.entry(policy.to_string()).or_default();
                        slot.0 += 1;
                        if is_deny {
                            slot.1 += 1;
                        }
                        if slot.2.as_str() < ts {
                            slot.2 = ts.to_string();
                        }
                    }
                }
            }
        }
        let by_policy: serde_json::Map<String, serde_json::Value> = by_policy
            .into_iter()
            .map(|(k, (t, d, ts))| {
                (
                    k,
                    serde_json::json!({ "total": t, "denied": d, "last_ts": ts }),
                )
            })
            .collect();
        Ok(serde_json::json!({
            "total": total,
            "denied": denied,
            "by_policy": by_policy,
        }))
    }

    /// 审计链在场态（轻量：文件行数 = 链段数；真 Merkle 校验走
    /// `logs.chain_verify`，层页「校验」按钮消费——本快照只回答「链在不在」）。
    fn chain_snapshot(workspace: &str) -> serde_json::Value {
        let path = security_log_dir(workspace).join("audit_chain.jsonl");
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let segments = content.lines().filter(|l| !l.trim().is_empty()).count();
                serde_json::json!({ "present": segments > 0, "segments": segments })
            }
            Err(_) => serde_json::json!({ "present": false, "segments": 0 }),
        }
    }

    fn estop_snapshot(ctx: &RequestContext) -> serde_json::Value {
        match ctx.state.estop.as_ref() {
            Some(e) => serde_json::json!({ "injected": true, "engaged": e.is_engaged() }),
            None => serde_json::json!({ "injected": false }),
        }
    }

    fn plan_mode_snapshot(ctx: &RequestContext) -> serde_json::Value {
        let guard = ctx.state.agent_loop.read();
        match guard.as_ref() {
            Some(loop_) => serde_json::json!({ "available": true, "mode": loop_.mode().as_str() }),
            None => serde_json::json!({ "available": false, "mode": serde_json::Value::Null }),
        }
    }

    /// Full Access / 外部写删只读态（editor.rs 槽透传；未注入 = headless/
    /// 降级形态）。操作面在 SettingsView【编辑器】TAB 与 ChatPanel，总览
    /// 只读呈现（用户拍板 2）。
    fn editor_snapshot() -> serde_json::Value {
        match crate::handlers::editor::editor_access_state() {
            Some(state) => {
                let (full_access, external_write) = state.snapshot();
                serde_json::json!({
                    "injected": true,
                    "full_access": full_access,
                    "external_write": external_write,
                })
            }
            None => serde_json::json!({ "injected": false }),
        }
    }

    /// ⑪ Guardian 二审快照（总览页行状态）：guardian_mode /
    /// guardian_failure_policy 是构造期键（装配读定、保存需重启生效），
    /// 配置文件即真相源——与 [`Self::layers_projection`] 同款 raw 直读
    /// （缺席 = runtime 默认：mode=off / policy=ask）。
    fn guardian_snapshot(workspace: &str) -> serde_json::Value {
        let raw = match nemesis_utils::fsutil::read_config_string(security_config_path(workspace)) {
            Ok(s) => serde_json::from_str::<serde_json::Value>(&s).unwrap_or(serde_json::json!({})),
            Err(_) => serde_json::json!({}),
        };
        let get = |k: &str| {
            raw.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        serde_json::json!({
            "mode": get("guardian_mode"),
            "failure_policy": get("guardian_failure_policy"),
        })
    }

    /// ⑬ 凭据 Vault 快照（总览页行状态）：加密存储文件在场态
    /// （`<workspace>/config/vault.enc`）。vault 无「装配」半边（子系统
    /// 恒在），初始化与否由存储文件存在性表达。
    fn vault_snapshot(workspace: &str) -> serde_json::Value {
        let path = nemesis_path::resolve_vault_path_in_workspace(Path::new(workspace));
        serde_json::json!({
            "exists": path.exists(),
        })
    }

    /// ⑦ 病毒扫描层快照（`load_scanner_status` 单一真相源投影 + 计数；
    /// scanner feature 裁掉时该模块不存在——本函数不依赖 scanner 模块，
    /// 只读 config.scanner.json，裁剪形态照常投影 enabled:false）。
    fn scanner_snapshot(workspace: &str) -> serde_json::Value {
        let status = crate::api_handlers::load_scanner_status(workspace);
        let engines = status.get("engines").and_then(|v| v.as_array());
        let is_enabled =
            |e: &serde_json::Value| e.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let is_ready =
            |e: &serde_json::Value| e.get("state").and_then(|v| v.as_str()) == Some("ready");
        serde_json::json!({
            "enabled": status.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            "total": engines.map(|a| a.len()).unwrap_or(0),
            "enabled_count": engines.map(|a| a.iter().filter(|e| is_enabled(e)).count()).unwrap_or(0),
            "ready_count": engines.map(|a| a.iter().filter(|e| is_ready(e)).count()).unwrap_or(0),
        })
    }

    /// security.enabled 总闸（**主 config.json**，typed Config.security；
    /// 缺省 true）。与层开关不同：总闸 runtime reload 真热生效——前端标
    /// 「即时生效」。home 缺席/主 config 读不了 = `available:false`（诚实
    /// 降级，不猜默认值冒充读到了）。
    fn global_switch_snapshot(ctx: &RequestContext) -> serde_json::Value {
        let Some(home) = ctx.state.home.as_deref() else {
            return serde_json::json!({ "available": false });
        };
        match crate::handlers::config::read_raw_config(home) {
            Ok(raw) => serde_json::json!({
                "available": true,
                "enabled": raw
                    .get("security")
                    .and_then(|s| s.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
            }),
            Err(_) => serde_json::json!({ "available": false }),
        }
    }

    /// 执行体隔离（⑨ 沙盒 / ⑩ 虚拟化的 config 面）：主 config.json
    /// `executor` 段（enabled/sandbox）+ `executor.vm.enabled`。全部缺省
    /// false（ExecutorSeparationConfig `#[serde(default)]`）。
    fn executor_snapshot(ctx: &RequestContext) -> serde_json::Value {
        let Some(home) = ctx.state.home.as_deref() else {
            return serde_json::json!({ "available": false });
        };
        let Ok(raw) = crate::handlers::config::read_raw_config(home) else {
            return serde_json::json!({ "available": false });
        };
        let executor = raw.get("executor");
        let flag = |key: &str| -> bool {
            executor
                .and_then(|e| e.get(key))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        };
        serde_json::json!({
            "available": true,
            "enabled": flag("enabled"),
            "sandbox": flag("sandbox"),
            "vm": executor
                .and_then(|e| e.get("vm"))
                .and_then(|v| v.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }

    /// 第②层页数据：内置命令黑名单只读快照。§2.7 核实结论（2026-11-10）：
    /// 内置表编译期静态（~60 条 OnceLock）；扩展 API（extra_patterns /
    /// add_entry）存在但生产零接线（生产构造 `CommandGuard::new(true)`）——
    /// `extra_wired:false` 让层页诚实注记「本期不开放编辑」，不谎报可编辑。
    fn command_blocklist() -> serde_json::Value {
        let entries: Vec<serde_json::Value> = nemesis_security::command::builtin_blocklist()
            .iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "category": e.category,
                    "severity": e.severity,
                    "platform": e.platform,
                    "reason": e.reason,
                })
            })
            .collect();
        serde_json::json!({
            "entries": entries,
            "extra_wired": false,
            "note": "内置黑名单为编译期静态表；扩展规则 API 存在但生产未接线，本期不开放编辑",
        })
    }

    /// 第④层页数据：内置凭据规则名只读快照。§2.7 核实结论：32 条 OnceLock
    /// 纯硬编码，无任何 config 扩展面；类别分组由前端按规则名映射
    /// （Rust 侧无类别元数据）。
    fn credential_rules() -> serde_json::Value {
        let rules: Vec<serde_json::Value> =
            nemesis_security::credential::builtin_credential_rules()
                .into_iter()
                .map(|name| serde_json::json!({ "name": name }))
                .collect();
        serde_json::json!({
            "rules": rules,
            "note": "内置规则表为编译期静态表（无 config 扩展面），只读呈现；类别分组由前端按规则名映射",
        })
    }
}

/// eval_rules.add/update 的 `data.rule` 载荷提取（单点：字段缺失/形态错
/// 统一口径）。
fn eval_rules_payload(
    data: Option<serde_json::Value>,
) -> Result<nemesis_config::eval_rules::Rule, String> {
    let raw = data
        .as_ref()
        .and_then(|d| d.get("rule"))
        .cloned()
        .ok_or("missing rule")?;
    serde_json::from_value(raw).map_err(|e| format!("invalid rule: {e}"))
}

/// 顶层字符串字段提取（id 等）。
fn require_str_field<'a>(
    data: Option<&'a serde_json::Value>,
    field: &str,
) -> Result<&'a str, String> {
    data.and_then(|d| d.get(field))
        .and_then(|v| v.as_str())
        .ok_or(format!("missing {field}"))
}

// (, quality-hardening goal 冲刺 web): audit paging +
// decision normalization + flatten/extract fallback arms + config error arms.
#[cfg(test)]
mod s10b_tests;

// (, devtool-upgrade): approvals.list / approvals.clear
// 规则表管理命令。
#[cfg(test)]
mod f3_tests;

// AGT 覆盖率批次：Default 转发、signature_verify_status 双槽
// （未注入降级 / 注入六字段透传）、audit·stats 非 jsonl 项跳过臂、
// approvals.clear 无文件 removed=0 + config 目录创建失败上抛。
#[cfg(test)]
mod agt_tests;

// 免打扰（Part 2）：approvals.status 双臂 +
// config.save 热翻投影。
#[cfg(test)]
mod approval_auto_tests;

// 会话级全部放行（长任务）：session_allow 双写 /
// status 双臂 / 会话键推导与 chat.sync 字节一致。
#[cfg(test)]
mod session_allow_tests;

// 条目级规则管理（批次 3）：approvals.add/remove（nemesis-security
// upsert/remove_manual_rule 同源闸）+ eval_rules CRUD（数据半边
// nemesis-config::eval_rules 同源，语义镜像 CLI eval rules *）。
#[cfg(test)]
mod rule_mgmt_tests;

// 十层页面化（2026-11-10）：security.overview 聚合投影（层开关 raw
// 语义/审计聚合排除链文件/chain/estop/plan/editor/scanner/global/executor
// 降级臂）+ command_blocklist / credential_rules 只读快照契约。
#[cfg(test)]
mod overview_tests;
