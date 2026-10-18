//! M7（devtool-upgrade 阶段 5）：Dashboard 审批管理器。
//!
//! 实现 `nemesis_security::auditor::ApprovalManager`（auditor → manager 方向：
//! auditor 命中 `require_approval` 时同步调用 `request_approval_sync` 阻塞等
//! 用户裁决），内部把请求广播为 `AgentEvent::ApprovalRequested`（web pump 转
//! SSE `approval-requested`），并实现 `nemesis_types::agent::ApprovalResponder`
//! （WSAPI → manager 方向：`approval.respond` / `approval.pending` 把用户裁决
//! 送回等待中的调用）。
//!
//! 与已停用的 `ApprovalPopupAdapter`（desktop 原生弹窗子进程）同构替换：审批
//! 统一走 dashboard 审批卡——plugin-ui WebView 窗口承载的就是 dashboard 前端，
//! 桌面场景天然兼容。等待侧与 PopupAdapter 一样是 sync trait 方法内阻塞：
//! tokio 上下文用 `block_in_place` 让出 worker，同步上下文（CLI/测试）直接
//! recv——两者都不新建 tokio runtime。
//!
//! 恢复方法（如需回到原生弹窗形态）：gateway.rs 审批装配块里换回
//! `ApprovalPopupAdapter::new(process_manager)` 即可（结构体保留未删）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use nemesis_types::agent::{AgentEvent, ApprovalResponder};

/// 一条等待裁决的审批请求：元数据（`approval.pending` 列表用）+ 裁决回传
/// 通道（`approval.respond` 取出后 send；mpsc 在 async handler 侧 send 无需
/// runtime）。通道载荷是 `ApprovalVerdict`（F6：拒绝可携带用户备注）。
struct PendingEntry {
    operation: String,
    target: String,
    /// F3:「总是允许」对应的规则 pattern（exec 类=B5 归约前缀），随事件
    /// 与 pending 列表下发，前端按钮回显确认串。
    pattern: String,
    risk_level: String,
    reason: String,
    timeout_secs: u64,
    chat_id: String,
    session_key: String,
    /// 授权卡自解释三字段（感知层 §5；`approval_card_fields` 派生）。
    why: String,
    impact: String,
    reject_note: String,
    created_at: std::time::Instant,
    tx: mpsc::Sender<nemesis_security::auditor::ApprovalVerdict>,
}

/// 授权卡自解释三字段派生（单一真相源；channel_approval 的 IM 文本卡同源
/// 消费）。why 优先用触发原因原文（preexec 扫描/connector 强审批声明的
/// 理由已自解释），缺失回落风险级固定文案；impact 按 op_type 推导操作
/// 会碰什么；reject_note 固定口径（拒绝=零效果，agent 继续对话）。
pub fn approval_card_fields(
    operation: &str,
    target: &str,
    risk_level: &str,
    reason: &str,
) -> (String, String, String) {
    let why = if reason.trim().is_empty() {
        match risk_level.to_ascii_uppercase().as_str() {
            "CRITICAL" => "CRITICAL 级操作按安全策略必须人工确认".to_string(),
            "HIGH" => "HIGH 级操作按安全策略需要人工确认".to_string(),
            _ => "该操作按安全策略需要人工确认".to_string(),
        }
    } else {
        reason.to_string()
    };
    let impact = match operation {
        "process_exec" | "process_spawn" => format!("将在本机执行命令/进程：{target}"),
        "file_write" | "file_edit" | "file_append" | "dir_create" => {
            format!("将写入/修改文件系统：{target}")
        }
        "file_delete" | "dir_delete" => format!("将删除文件系统内容：{target}"),
        "network_request" | "network_download" | "network_upload" => {
            format!("将发起网络访问：{target}")
        }
        "registry_write" => format!("将修改注册表：{target}"),
        "system_install" | "system_shutdown" | "system_reboot" | "system_config" => {
            format!("将变更系统状态：{target}")
        }
        _ => {
            if target.is_empty() {
                "操作目标见操作类型".to_string()
            } else {
                format!("操作目标：{target}")
            }
        }
    };
    let reject_note =
        "拒绝后本动作不执行、不产生任何效果；agent 会收到拒绝通知并继续对话。".to_string();
    (why, impact, reject_note)
}

pub struct WebApprovalManager {
    agent_event_tx: Option<tokio::sync::broadcast::Sender<AgentEvent>>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    /// F3:「总是允许」规则表路径（`<workspace>/config/approval_rules.json`）。
    /// None = 未配置（respond always 诚实报错，不静默吞）。
    rules_path: Option<std::path::PathBuf>,
    /// P1 审批台账（tasks.db approvals 表）。OnceLock 晚绑：manager 先于
    /// tasks.db 装配（与 LateWebSkillsGate 同款时序），runtime.rs 装配块
    /// `set_journal()` 注入；未注入 = 台账关闭（审批照常工作，仅丢重启
    /// 补裁能力——诚实降级，非静默：装配失败有 loud error）。
    journal:
        std::sync::OnceLock<std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal>>,
}

impl WebApprovalManager {
    pub fn new(
        agent_event_tx: Option<tokio::sync::broadcast::Sender<AgentEvent>>,
        rules_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            agent_event_tx,
            pending: Mutex::new(HashMap::new()),
            rules_path,
            journal: std::sync::OnceLock::new(),
        }
    }

    /// P1：注入审批台账（tasks.db）。幂等失败 = 重复注入，静默忽略（首个胜出）。
    pub fn set_journal(
        &self,
        journal: std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal>,
    ) {
        let _ = self.journal.set(journal);
    }

    fn journal(
        &self,
    ) -> Option<&std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal>> {
        self.journal.get()
    }

    /// F3:「总是允许」规则落盘。pattern 由 B5 归约生成（单一真相源
    /// `approval_rules::pattern_for`，与 auditor 匹配侧同源）；层级安全门
    /// 与写入语义单一收口在 `approval_rules::persist_auto_allow_rule`
    /// （§2 免打扰集群来源持久化同款）：`Ok(Some(pattern))` = 已落盘，
    /// `Ok(None)` = 层级门不过（CRITICAL 仅 process_exec 豁免 + 安装类
    /// 永不入规则），`Err` = pattern 推导失败或写盘失败（空 pattern =
    /// run_script 无 target 提取，这类操作保持每次人工）。
    fn save_always_rule(
        &self,
        operation: &str,
        target: &str,
        risk_level: &str,
    ) -> Result<Option<String>, String> {
        use nemesis_security::approval_rules as rules;
        let Some(path) = self.rules_path.as_ref() else {
            return Err("approval rules not configured (no workspace)".to_string());
        };
        rules::persist_auto_allow_rule(path, operation, target, risk_level)
    }

    /// 广播审批请求事件（无订阅者/通道关闭都是良性——审批等待不依赖广播）。
    fn broadcast_requested(&self, entry: &PendingEntry, request_id: &str) {
        if let Some(tx) = self.agent_event_tx.as_ref() {
            let event = AgentEvent::ApprovalRequested {
                session_key: entry.session_key.clone(),
                chat_id: entry.chat_id.clone(),
                request_id: request_id.to_string(),
                operation: entry.operation.clone(),
                target: entry.target.clone(),
                risk_level: entry.risk_level.clone(),
                reason: entry.reason.clone(),
                timeout_secs: entry.timeout_secs,
                pattern: entry.pattern.clone(),
                why: Some(entry.why.clone()),
                impact: Some(entry.impact.clone()),
                reject_note: Some(entry.reject_note.clone()),
            };
            let _ = tx.send(event);
        }
    }

    /// F6: 广播裁决事件（`approval-resolved`）——所有前端窗口（桌面
    /// WebView + 外部浏览器）据此摘除本地审批卡，竞速败方不再挂到倒计时
    /// 结束；`decision` ∈ approved/denied/timeout。
    fn broadcast_resolved(&self, request_id: &str, decision: &str) {
        if let Some(tx) = self.agent_event_tx.as_ref() {
            let _ = tx.send(AgentEvent::ApprovalResolved {
                request_id: request_id.to_string(),
                decision: decision.to_string(),
            });
        }
    }

    fn entry_json(request_id: &str, entry: &PendingEntry) -> serde_json::Value {
        serde_json::json!({
            "request_id": request_id,
            "operation": entry.operation,
            "target": entry.target,
            "risk_level": entry.risk_level,
            "reason": entry.reason,
            "timeout_secs": entry.timeout_secs,
            "pattern": entry.pattern,
            "chat_id": entry.chat_id,
            "session_key": entry.session_key,
            "why": entry.why,
            "impact": entry.impact,
            "reject_note": entry.reject_note,
            "age_secs": entry.created_at.elapsed().as_secs(),
        })
    }

    /// P1：构造台账条目（pending/decision 共用同一定位字段）。
    fn journal_entry(
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        pattern: &str,
        chat_id: &str,
        session_key: &str,
    ) -> nemesis_types::approval_journal::ApprovalJournalEntry {
        nemesis_types::approval_journal::ApprovalJournalEntry {
            id: request_id.to_string(),
            broker: "web".to_string(),
            short_id: None,
            operation: operation.to_string(),
            target: target.to_string(),
            risk_level: risk_level.to_string(),
            reason: reason.to_string(),
            payload: serde_json::json!({
                "pattern": pattern,
                "chat_id": chat_id,
                "session_key": session_key,
            }),
            status: nemesis_types::approval_journal::status::PENDING.to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
            decided_at: None,
            decision_note: None,
            decided_by: None,
        }
    }

    /// P1：pending 记账（尽力而为——台账失败 loud error 留痕，不阻断审批
    /// 主链路；审批可用性不绑 tasks.db 可用性）。
    fn journal_record_pending(
        &self,
        entry: &nemesis_types::approval_journal::ApprovalJournalEntry,
    ) {
        if let Some(j) = self.journal()
            && let Err(e) = j.record_pending(entry)
        {
            tracing::error!(
                "[WebApproval] approval journal record_pending failed for {}: {e}",
                entry.id
            );
        }
    }

    /// P1 重启补裁（respond 的 map-miss 兜底）：台账记一笔裁决，前端卡片
    /// 得以摘除。原操作进程已死、不恢复执行——纯记账性质，响应里由前端
    /// 注记提示。幂等：台账已裁决（超时/先到 respond）= Err already decided。
    fn respond_stale(&self, request_id: &str, approved: bool) -> Result<bool, String> {
        let Some(j) = self.journal() else {
            return Err(format!("unknown approval request: {request_id}"));
        };
        let decision = if approved {
            nemesis_types::approval_journal::ApprovalDecision::Approved
        } else {
            nemesis_types::approval_journal::ApprovalDecision::Denied
        };
        match j.record_decision(request_id, decision, None, "web-user-stale") {
            Ok(true) => {
                tracing::warn!(
                    "[WebApproval] stale respond for {request_id}: gateway restarted since this \
                     approval was raised; decision recorded for audit, original operation is gone"
                );
                self.broadcast_resolved(request_id, if approved { "approved" } else { "denied" });
                Ok(approved)
            }
            Ok(false) => Err(format!("approval already decided: {request_id}")),
            Err(e) => Err(format!("approval journal respond failed: {e}")),
        }
    }
}

impl nemesis_security::auditor::ApprovalManager for WebApprovalManager {
    fn is_running(&self) -> bool {
        true
    }

    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        let (tx, rx) = mpsc::channel::<nemesis_security::auditor::ApprovalVerdict>();
        let (why, impact, reject_note) =
            approval_card_fields(operation, target, risk_level, reason);
        let pattern = nemesis_security::approval_rules::pattern_for(operation, target);
        let entry = PendingEntry {
            operation: operation.to_string(),
            target: target.to_string(),
            pattern: pattern.clone(),
            risk_level: risk_level.to_string(),
            reason: reason.to_string(),
            timeout_secs,
            chat_id: String::new(),
            session_key: String::new(),
            why,
            impact,
            reject_note,
            created_at: std::time::Instant::now(),
            tx,
        };
        // R3（2026-10-18 三方复评）：**先 durable pending、后内存入册+广播**
        // （不变量升级：内存可见 ⇒ DB pending 已在）。旧序（先广播后落库）
        // 有幽灵 pending 竞态：respond 可在 INSERT 前完成裁决 →
        // record_decision 撞空库表 → 请求线程随后补插 pending 无裁决 →
        // 重启复活已处理卡片。DB 失败维持「loud error 不阻断」立场（审批
        // 可用性不绑 tasks.db；此失败窗口重启后可能复活旧卡，respond_stale
        // 兜底可裁决摘卡）。
        {
            let journal_entry = Self::journal_entry(
                request_id, operation, target, risk_level, reason, &pattern, "", "",
            );
            self.journal_record_pending(&journal_entry);
        }
        // **先入册、后广播**（不变量：广播 ApprovalRequested ⇒ pending 已
        // 可裁决）。反序时消费方收到事件立即 respond 会查表落空报
        // unknown——高负载并行下请求线程被调度延迟卡在广播与入册之间
        // （实录 ET Linux resolved_broadcast_fires_on_respond_and_timeout
        // 假红），前端秒点同病。channel_approval 同款教训（2026-09-07）。
        // broadcast send 是同步非阻塞操作，持锁发送无死锁面。
        {
            let mut map = self
                .pending
                .lock()
                .map_err(|e| format!("pending map poisoned: {}", e))?;
            map.insert(request_id.to_string(), entry);
            if let Some(registered) = map.get(request_id) {
                self.broadcast_requested(registered, request_id);
            }
        }

        // 阻塞等裁决。tokio 上下文（agent loop 工具调度链）用 block_in_place
        // 让出当前 worker——否则多会话并发审批可能占满 worker 池，WSAPI 无法
        // 处理 respond 导致死锁；同步上下文（CLI / 单测）直接 recv。
        let wait = || rx.recv_timeout(Duration::from_secs(timeout_secs));
        let result = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(wait)
        } else {
            wait()
        };

        // 无论结果如何都清 pending（超时/断开后迟到的 respond 诚实报 unknown）。
        if let Ok(mut map) = self.pending.lock() {
            map.remove(request_id);
        }

        match result {
            Ok(verdict) => {
                tracing::info!(
                    "[WebApproval] request {} {}: {}",
                    request_id,
                    operation,
                    if verdict.approved {
                        "approved"
                    } else {
                        "denied"
                    }
                );
                Ok(verdict)
            }
            Err(_) => {
                tracing::warn!(
                    "[WebApproval] request {} ({}) timed out after {}s — denying",
                    request_id,
                    operation,
                    timeout_secs
                );
                // R3：超时也是一次裁决——**先记台账**（幂等条件更新），再按
                // 记账结果决定后续（Ok(false) = respond 抢先完成裁决，不能
                // 顺着超时路径广播「timeout」——那是假广播）。
                let expired_record = self.journal().map(|j| {
                    j.record_decision(
                        request_id,
                        nemesis_types::approval_journal::ApprovalDecision::Expired,
                        Some("审批超时自动拒绝"),
                        "timeout",
                    )
                });
                match expired_record {
                    Some(Ok(false)) => {
                        // respond 赢了裁决竞争（先到先得）。它此刻大概率已
                        // tx.send（记账后微秒级）——短暂宽限收一次在途裁决，
                        // 返回真实结果；respond 已广播 resolved 摘卡，这里
                        // **不广播 timeout**。宽限耗尽/通道关闭 = respond 侧
                        // 异常中断：诚实返回拒绝且不补假广播（下次 pending
                        // 轮询按台账态摘卡）。
                        tracing::info!(
                            "[WebApproval] request {request_id}: respond won the decision race; recovering in-flight verdict"
                        );
                        match rx.recv_timeout(Duration::from_secs(2)) {
                            Ok(verdict) => Ok(verdict),
                            Err(_) => {
                                tracing::warn!(
                                    "[WebApproval] request {request_id}: raced decision not recoverable from channel; returning denial without timeout broadcast"
                                );
                                Ok(nemesis_security::auditor::ApprovalVerdict {
                                    approved: false,
                                    note: Some("审批已被并发裁决".to_string()),
                                })
                            }
                        }
                    }
                    other => {
                        if let Some(Err(e)) = other {
                            tracing::error!(
                                "[WebApproval] approval journal timeout record failed for {request_id}: {e}"
                            );
                        }
                        // None = 无 journal（审批可用性不绑 tasks.db，同
                        // journal_record_pending 立场）；Ok(true) = 正常超时
                        // 裁决。两者都广播 timeout。
                        // F6: 超时自动拒绝也是一次裁决——广播出去让所有前端摘卡。
                        self.broadcast_resolved(request_id, "timeout");
                        Ok(nemesis_security::auditor::ApprovalVerdict {
                            approved: false,
                            // 4.1（2026-09-29 交付审查）：超时拒绝带注记，与用户
                            // 显式拒绝区分——上游消费 verdict.note 的面（技能/插件
                            // 安装卡的拒绝原因）不再把超时误报成「用户拒绝了」。
                            note: Some("审批超时自动拒绝".to_string()),
                        })
                    }
                }
            }
        }
    }
}

impl ApprovalResponder for WebApprovalManager {
    fn respond(
        &self,
        request_id: &str,
        approved: bool,
        always: bool,
        note: Option<String>,
    ) -> Result<bool, String> {
        let entry = {
            let mut map = self
                .pending
                .lock()
                .map_err(|e| format!("pending map poisoned: {}", e))?;
            match map.remove(request_id) {
                Some(entry) => entry,
                // P1 重启补裁：pending 表里还有、内存 map 已丢（gateway
                // 重启窗口内的卡片）→ 台账裁决落账（幂等条件更新先到先得），
                // 原操作进程已死不继续执行——记账性质，卡片可以摘了。
                None => return self.respond_stale(request_id, approved),
            }
        };
        // 先到先得：remove 已把请求移出 pending，第二个 respond 天然 unknown。
        // F6: 拒绝备注只在 denied 时携带（空串/纯空白视同无备注）。
        let note = note.filter(|n| !n.trim().is_empty());
        let verdict = if approved {
            nemesis_security::auditor::ApprovalVerdict::approved()
        } else {
            nemesis_security::auditor::ApprovalVerdict {
                approved: false,
                note: note.clone(),
            }
        };
        // R3（2026-10-18 三方复评）：**裁决状态转移以台账条件更新为准——先
        // 记账、后通知**。R3 前置修复（先 durable pending 后入册广播）保证
        // 此刻 pending 行必已在，Ok(false) 只可能来自超时分支抢先。
        // - Ok(true)：本 respond 是先到者 → 送裁决；
        // - Ok(false)：已有人抢先 → 不送不广播（先到者已广播摘卡），诚实
        //   回 already decided；
        // - Err：台账写失败 → 裁决照常送达（审批可用性不绑 tasks.db，审计
        //   缺失 loud 留痕——语义与台账设计一致但顺序正确）。
        let decision = if approved {
            nemesis_types::approval_journal::ApprovalDecision::Approved
        } else {
            nemesis_types::approval_journal::ApprovalDecision::Denied
        };
        let decided = match self
            .journal()
            .map(|j| j.record_decision(request_id, decision, note.as_deref(), "web-user"))
        {
            None => true,
            Some(Ok(won)) => won,
            Some(Err(e)) => {
                tracing::error!(
                    "[WebApproval] approval journal decision record failed for {request_id}: {e} \
                     (verdict still delivered — availability not bound to tasks.db)"
                );
                true
            }
        };
        if !decided {
            tracing::warn!(
                "[WebApproval] respond for {request_id}: approval already decided (timeout \
                 raced); verdict not delivered"
            );
            return Err(format!("approval already decided: {request_id}"));
        }
        if entry.tx.send(verdict).is_err() {
            // 等待方已退出（requester 线程提前收场）：台账已持裁决，审计面
            // 可恢复——如实报错，不假装送达。
            tracing::warn!(
                "[WebApproval] respond for {request_id}: decision recorded but waiter channel \
                 is gone; audit holds the decision"
            );
            return Err("approval waiter already gone".to_string());
        }
        // F6: 裁决已送达 → 广播 resolved，竞速败方窗口摘卡。在 always 规则
        // 写盘之前发（写失败不影响「卡已处理」事实）。
        self.broadcast_resolved(request_id, if approved { "approved" } else { "denied" });

        // F3: 批准 + 总是允许 → 写规则。裁决已送达不回滚；层级安全门
        // （CRITICAL 仅 process_exec 豁免 + 安装类永不入规则）不过则忽略
        // always 并 warn；规则写失败诚实回 Err（前端 toast 提示，本次批准
        // 仍生效）。
        if approved && always {
            match self.save_always_rule(&entry.operation, &entry.target, &entry.risk_level) {
                Ok(Some(pattern)) => {
                    tracing::info!(
                        "[WebApproval] always-allow rule saved: {} {}",
                        entry.operation,
                        pattern
                    );
                }
                Ok(None) => {
                    tracing::warn!(
                        "[WebApproval] always-allow ignored for {} (risk {}): CRITICAL ops and code installs stay manual",
                        entry.operation,
                        entry.risk_level
                    );
                }
                Err(e) => return Err(e),
            }
        }
        Ok(approved)
    }

    fn pending(&self) -> Vec<serde_json::Value> {
        let mut out = match self.pending.lock() {
            Ok(map) => map
                .iter()
                .map(|(id, e)| Self::entry_json(id, e))
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        // P1：台账里仍 pending、内存已丢（重启前遗留）→ 作为 stale 卡片
        // 一并下发（前端可裁决，respond 走重启补裁记账路径；stale: true
        // 供前端注记「原操作已失效」）。字段对齐 entry_json 形态。
        if let Some(j) = self.journal() {
            let live_ids: std::collections::HashSet<String> = match self.pending.lock() {
                Ok(map) => map.keys().cloned().collect(),
                Err(_) => std::collections::HashSet::new(),
            };
            for e in j.pending_entries() {
                if live_ids.contains(&e.id) {
                    continue;
                }
                let payload_pattern = e
                    .payload
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(serde_json::json!({
                    "request_id": e.id,
                    "operation": e.operation,
                    "target": e.target,
                    "risk_level": e.risk_level,
                    "reason": e.reason,
                    "timeout_secs": serde_json::Value::Null,
                    "pattern": payload_pattern,
                    "chat_id": serde_json::Value::Null,
                    "session_key": serde_json::Value::Null,
                    "why": e.reason,
                    "impact": e.target,
                    "reject_note": "重启前的审批请求：原操作已随进程终止，裁决仅记账留痕。",
                    "age_secs": serde_json::Value::Null,
                    "stale": true,
                }));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// WS4（P13）：技能装前审批门（gateway 中央接线）
// ---------------------------------------------------------------------------

/// 装配顺序适配器：`nemesis_skills::InstallGate` ← `WebApprovalManager`。
///
/// WebServer 的 AppState 在 init_services（B6）即构建，而审批真身
/// `WebApprovalManager` 在 run_runtime（B7 审批接线块）才存在——用
/// `OnceLock` 晚绑：gateway.rs 主流程把本类型注入 `web_server`（AppState
/// 快照捕获），run_runtime 审批块 `bind()` 真身。技能安装卡与 auditor
/// 审批共用**同一 manager 实例**，`approval.respond`（responder 槽）才能
/// 命中。未 bind 前收到安装请求 = fail-closed（启动窗口亚秒级，诚实拒绝）。
pub struct LateWebSkillsGate {
    inner: std::sync::OnceLock<std::sync::Arc<WebApprovalManager>>,
    timeout_secs: u64,
}

impl LateWebSkillsGate {
    /// `timeout_secs`：卡片等待用户裁决的时长（超时自动拒绝）。
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            inner: std::sync::OnceLock::new(),
            timeout_secs,
        }
    }

    /// run_runtime 审批接线块调用：绑定审批真身（幂等失败 = 重复 bind，
    /// 静默忽略——首个真身胜出）。
    pub fn bind(&self, manager: std::sync::Arc<WebApprovalManager>) {
        let _ = self.inner.set(manager);
    }
}

#[async_trait::async_trait]
impl nemesis_skills::install_gate::InstallGate for LateWebSkillsGate {
    async fn decide(
        &self,
        plan: &nemesis_skills::install_gate::InstallPlan,
    ) -> nemesis_skills::install_gate::InstallDecision {
        use nemesis_skills::install_gate::InstallDecision;

        let Some(manager) = self.inner.get() else {
            return InstallDecision::Deny {
                reason: "审批门未就绪（gateway 审批装配未完成），请稍后重试".to_string(),
            };
        };

        // 安装类操作不走「总是允许」规则（2026-09-30 复查 #12 横向同源：
        // 与 WASM 插件安装同判——source 目录记忆 ≠ 载荷记忆，同一路径明天
        // 可以装不同内容的技能包；每次安装保持人工裁决。历史遗留的
        // skills.install 规则成为死信；规则写入侧由 rule_permitted_for 的
        // is_install_op 子句拒绝新增）。

        // request_approval_sync 是阻塞等待（内部 block_in_place / 直接
        // recv）；decide 本身已在 async 上下文，再套一层 spawn_blocking
        // 避免占用当前 worker（block_in_place 非多线程 runtime 会 panic，
        // WSAPI 装配的 runtime 形态不可假设）。
        let manager = manager.clone();
        let timeout_secs = self.timeout_secs;
        let plan = plan.clone();
        let wait = tokio::task::spawn_blocking(move || {
            use nemesis_security::auditor::ApprovalManager as _;
            let request_id = format!("skills-install-{}", uuid::Uuid::new_v4());
            manager.request_approval_sync(
                &request_id,
                "skills.install",
                &plan.source,
                "HIGH",
                &plan.summary(),
                timeout_secs,
            )
        })
        .await;

        match wait {
            Ok(Ok(verdict)) if verdict.approved => InstallDecision::Approve,
            Ok(Ok(verdict)) => InstallDecision::Deny {
                reason: verdict
                    .note
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| "用户拒绝了安装".to_string()),
            },
            Ok(Err(e)) => InstallDecision::Deny {
                reason: format!("审批请求失败: {e}"),
            },
            Err(e) => InstallDecision::Deny {
                reason: format!("审批任务失败: {e}"),
            },
        }
    }

    fn name(&self) -> &str {
        "web-approval"
    }
}

#[cfg(test)]
mod tests;
