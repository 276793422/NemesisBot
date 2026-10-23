//! F7（devtool-upgrade 阶段 5）：Dashboard 结构化提问 broker。
//!
//! agent 的 `question` 工具（nemesis-agent loop_tools）发起结构化提问时
//! 阻塞在这里：广播 `AgentEvent::QuestionAsked`（web pump 转 SSE
//! `question-asked`，前端 QuestionCard 渲染选项卡/表单卡），并实现
//! [`nemesis_types::agent::QuestionResponder`]（WSAPI → broker 方向：
//! `question.respond` / `question.pending` 把用户作答送回等待中的 `ask`）。
//!
//! 与 [`crate::web_approval::WebApprovalManager`] 同构（pending map +
//! mpsc oneshot + broadcast 事件）：等待侧是 sync trait 方法内阻塞——tokio
//! 上下文（工具经 spawn_blocking 进来的阻塞线程）直接 `recv_timeout`，
//! 不新建 runtime。超时不是错误：广播 `QuestionResolved{timeout}` 并返回
//! `QuestionOutcome::Timeout`，工具侧回灌「按最佳判断继续」。
//!
//! B8（提问两形态）：同一 pending 通道承载两种作答形态
//! （[`Answer`] 枚举）——选项形态（`respond`）/ 表单形态（`respond_form`）。
//! 两形态互斥：选项卡收表单作答（或反之）诚实报错（请求留在 pending）。
//!
//! 载荷校验（respond 侧）：空选择 / 非候选选项 / 单选多项（选项形态）；
//! 未知键 / 必填空值 / select 非候选（表单形态）→ 诚实报错，请求留在
//! pending——用户修正后可重试（校验失败不摘卡不惊动等待方）。

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use nemesis_types::agent::{AgentEvent, QuestionField, QuestionOutcome, QuestionRequest};

/// 用户作答载荷（B8 两形态统一通道；ask 侧 match 还原成 QuestionOutcome）。
enum Answer {
    /// 选项形态：选中项原文（单选恰一项）。
    Selection(Vec<String>),
    /// 表单形态：键=字段 key，值=文本或选中项原文（可选空值已剔除）。
    Form(BTreeMap<String, String>),
    /// 循环干预阶梯（2026-10-24）：[`Self::cancel_pending_all`]（estop
    /// watcher）的强制了结形态——不是超时也不是用户作答。
    Cancelled,
}

/// 一条等待作答的提问：元数据（`question.pending` 列表用）+ 作答回传通道
/// （`question.respond`/`respond_form` 取出后 send）。
struct PendingEntry {
    question: String,
    options: Vec<String>,
    multi: bool,
    /// B8：表单字段；空 = 选项形态，非空 = 表单形态（两形态互斥）。
    fields: Vec<QuestionField>,
    timeout_secs: u64,
    chat_id: String,
    session_key: String,
    created_at: std::time::Instant,
    tx: mpsc::Sender<Answer>,
}

pub struct WebQuestionBroker {
    agent_event_tx: Option<tokio::sync::broadcast::Sender<AgentEvent>>,
    pending: Mutex<HashMap<String, PendingEntry>>,
}

impl WebQuestionBroker {
    pub fn new(agent_event_tx: Option<tokio::sync::broadcast::Sender<AgentEvent>>) -> Self {
        Self {
            agent_event_tx,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// 循环干预阶梯（2026-10-24）：急停挂钩——把全部 pending 提问以
    /// [`Answer::Cancelled`] 强制了结（等待方醒来 → 消费方按「停止」语义
    /// 处理；`broadcast_resolved("cancelled")` 让前端摘卡）。无超时终级卡
    /// 停在等答态会跨 estop 存续——estop watcher（gateway 装配处 spawn）
    /// 触发时调本方法，停车的 loop 醒来 → 下一次 dispatch 被 estop 冻结
    /// 拒绝 → 本轮诚实收尾。返回了结条数（0 = 无 pending，良性）。
    pub fn cancel_pending_all(&self, reason: &str) -> usize {
        let entries: Vec<(String, PendingEntry)> = match self.pending.lock() {
            Ok(mut map) => map.drain().collect(),
            Err(_) => return 0,
        };
        let n = entries.len();
        for (id, entry) in entries {
            let _ = entry.tx.send(Answer::Cancelled);
            self.broadcast_resolved(&id, "cancelled");
        }
        if n > 0 {
            tracing::warn!(
                "[WebQuestion] cancelled {} pending question(s): {}",
                n,
                reason
            );
        }
        n
    }

    /// 广播提问事件（无订阅者/通道关闭都是良性——提问等待不依赖广播）。
    fn broadcast_asked(&self, question_id: &str, entry: &PendingEntry) {
        if let Some(tx) = self.agent_event_tx.as_ref() {
            let _ = tx.send(AgentEvent::QuestionAsked {
                session_key: entry.session_key.clone(),
                chat_id: entry.chat_id.clone(),
                question_id: question_id.to_string(),
                question: entry.question.clone(),
                options: entry.options.clone(),
                multi: entry.multi,
                fields: entry.fields.clone(),
                timeout_secs: entry.timeout_secs,
            });
        }
    }

    /// 广播了结事件——所有前端窗口据此摘除本地提问卡。
    fn broadcast_resolved(&self, question_id: &str, decision: &str) {
        if let Some(tx) = self.agent_event_tx.as_ref() {
            let _ = tx.send(AgentEvent::QuestionResolved {
                question_id: question_id.to_string(),
                decision: decision.to_string(),
            });
        }
    }

    fn entry_json(question_id: &str, entry: &PendingEntry) -> serde_json::Value {
        serde_json::json!({
            "question_id": question_id,
            "question": entry.question,
            "options": entry.options,
            "multi": entry.multi,
            "fields": entry.fields,
            "timeout_secs": entry.timeout_secs,
            "chat_id": entry.chat_id,
            "session_key": entry.session_key,
            "age_secs": entry.created_at.elapsed().as_secs(),
        })
    }

    /// 锁守卫借用下校验表单作答并规约为最终回灌 map：键必须命中字段
    /// 定义；必填 trim 后非空；select 值 ∈ 该字段候选；text trim；
    /// 可选空值字段剔除。返回规约后的 map（不摘卡——摘卡在取出段）。
    fn validate_form_values(
        fields: &[QuestionField],
        values: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut out = BTreeMap::new();
        for (k, v) in values {
            let field = fields
                .iter()
                .find(|f| &f.key == k)
                .ok_or_else(|| format!("unknown field: '{}'", k))?;
            let trimmed = v.trim();
            if trimmed.is_empty() {
                if field.required {
                    return Err(format!("field '{}' is required", field.key));
                }
                continue; // 可选空值字段剔除
            }
            if field.field_type == "select" && !field.options.iter().any(|o| o == trimmed) {
                return Err(format!(
                    "'{}' is not one of the offered options for field '{}'",
                    trimmed, field.key
                ));
            }
            out.insert(field.key.clone(), trimmed.to_string());
        }
        // 缺席的必填字段同样拒绝（前端只提交已填项也要带空串占位）。
        for f in fields {
            if f.required && !out.contains_key(&f.key) {
                return Err(format!("field '{}' is required", f.key));
            }
        }
        Ok(out)
    }
}

impl nemesis_types::agent::QuestionAsker for WebQuestionBroker {
    fn ask(&self, request: QuestionRequest) -> Result<QuestionOutcome, String> {
        // §2 免打扰（无人值守计划 Part 2）：开关开 = 提问卡不广播不进
        // pending，直接超时语义返回——工具侧回灌「按最佳判断继续」，无人
        // 值守下 agent 全自主推进。
        // **bypass_auto_approve 绕过**（循环干预阶梯，2026-10-24）：终级卡
        // （doom-loop escalation 停车问询）必须绕过本分支——任何自动化开关
        // 都压不掉它（烟雾报警器必须响，用户裁决第 5 点）。
        // security 关闭 = 审批体系整体缺席（approval_auto_state 在
        // handlers::security 模块内，随 feature 消失）→ 不可能有自动兜底，
        // 恒发卡。feature matrix 实录：漏 gate 编译红 E0433（2026-10-14）。
        #[cfg(feature = "security")]
        if !request.bypass_auto_approve
            && nemesis_web::handlers::security::approval_auto_state().is_some_and(|s| s.enabled())
        {
            tracing::info!(
                "[WebQuestion] auto-approve on: question {} skipped (timeout semantics)",
                request.question_id
            );
            return Ok(QuestionOutcome::Timeout);
        }
        let (tx, rx) = mpsc::channel::<Answer>();
        let entry = PendingEntry {
            question: request.question.clone(),
            options: request.options.clone(),
            multi: request.multi,
            fields: request.fields.clone(),
            timeout_secs: request.timeout_secs,
            chat_id: request.chat_id.clone(),
            session_key: request.session_key.clone(),
            created_at: std::time::Instant::now(),
            tx,
        };
        // **先入册、后广播**（channel_approval 同款教训：先广播后挂表时，
        // 消费方收到事件立即 respond 查表落空报 unknown——高负载并行下
        // 请求线程被调度延迟卡在广播与入册之间，web_approval 实录同根因
        // 假红）。broadcast send 是同步非阻塞操作，持锁发送无死锁面。
        {
            let mut map = self
                .pending
                .lock()
                .map_err(|e| format!("pending map poisoned: {}", e))?;
            map.insert(request.question_id.clone(), entry);
            if let Some(registered) = map.get(request.question_id.as_str()) {
                self.broadcast_asked(&request.question_id, registered);
            }
        }

        // 阻塞等作答。工具侧已经 spawn_blocking（不占 worker），这里直接
        // recv；block_in_place 分支保留给直接在 async 上下文调用 ask 的
        // 形态（与审批同构）。
        // **timeout_secs == 0 = 永不超时**（循环干预阶梯终级卡专用语义，
        // 2026-10-24）：`rx.recv()` 无限等（**不是** `recv_timeout(0)`——那
        // 是立即返回）。唯一的了结路径 = 用户作答 / respond_stale 类重启
        // 补裁（本题无——提问 pending 纯内存、无重启 sweep，不存在自动
        // 拒绝面，`timeout_secs==0` 条目天然「永不自动拒绝」）/ estop 经
        // `cancel_pending_all` 强制了结（Cancelled）。
        let wait = || {
            if request.timeout_secs == 0 {
                rx.recv()
                    .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
            } else {
                rx.recv_timeout(Duration::from_secs(request.timeout_secs))
            }
        };
        let result = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(wait)
        } else {
            wait()
        };

        // 无论结果如何都清 pending（超时/断开后迟到的 respond 诚实报 unknown）。
        if let Ok(mut map) = self.pending.lock() {
            map.remove(&request.question_id);
        }

        match result {
            Ok(Answer::Selection(selected)) => {
                tracing::info!(
                    "[WebQuestion] question {} answered ({} item(s))",
                    request.question_id,
                    selected.len()
                );
                // 已了结广播由 respond 侧负责（单一真相源，同审批先例——
                // 这里再广播一次会发出重复 resolved 事件）。
                Ok(QuestionOutcome::Answered(selected))
            }
            Ok(Answer::Form(values)) => {
                tracing::info!(
                    "[WebQuestion] question {} answered (form, {} field(s))",
                    request.question_id,
                    values.len()
                );
                Ok(QuestionOutcome::AnsweredForm(values))
            }
            Ok(Answer::Cancelled) => {
                // 已了结广播（"cancelled"）由 cancel_pending_all 负责。
                tracing::warn!(
                    "[WebQuestion] question {} cancelled (emergency stop)",
                    request.question_id
                );
                Ok(QuestionOutcome::Cancelled)
            }
            Err(_) => {
                tracing::warn!(
                    "[WebQuestion] question {} timed out after {}s — model proceeds on best judgment",
                    request.question_id,
                    request.timeout_secs
                );
                self.broadcast_resolved(&request.question_id, "timeout");
                Ok(QuestionOutcome::Timeout)
            }
        }
    }
}

impl nemesis_types::agent::QuestionResponder for WebQuestionBroker {
    fn respond(&self, question_id: &str, selected: Vec<String>) -> Result<bool, String> {
        // 先校验后取出：校验失败时请求留在 pending，用户修正后可重试。
        {
            let map = self
                .pending
                .lock()
                .map_err(|e| format!("pending map poisoned: {}", e))?;
            let entry = map
                .get(question_id)
                .ok_or_else(|| format!("unknown question: {}", question_id))?;
            if !entry.fields.is_empty() {
                return Err(
                    "this question expects form values (use question.respond with values)"
                        .to_string(),
                );
            }
            if selected.is_empty() {
                return Err("selection is empty".to_string());
            }
            if !entry.multi && selected.len() > 1 {
                return Err("this question allows a single choice only".to_string());
            }
            for s in &selected {
                if !entry.options.contains(s) {
                    return Err(format!("'{}' is not one of the offered options", s));
                }
            }
        }
        let entry = {
            let mut map = self
                .pending
                .lock()
                .map_err(|e| format!("pending map poisoned: {}", e))?;
            map.remove(question_id)
                .ok_or_else(|| format!("unknown question: {}", question_id))?
        };
        // 先到先得：remove 已把请求移出 pending，第二个 respond 天然 unknown。
        entry
            .tx
            .send(Answer::Selection(selected))
            .map_err(|_| "question waiter already gone".to_string())?;
        self.broadcast_resolved(question_id, "answered");
        Ok(true)
    }

    fn respond_form(
        &self,
        question_id: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<bool, String> {
        // 先校验后取出（同 respond：校验失败请求留在 pending）；校验同时
        // 规约作答（trim / 可选空值剔除）——**下游拿规约后的 map**，不是
        // 原始载荷（工具回灌与审计所见一致）。
        let normalized = {
            let map = self
                .pending
                .lock()
                .map_err(|e| format!("pending map poisoned: {}", e))?;
            let entry = map
                .get(question_id)
                .ok_or_else(|| format!("unknown question: {}", question_id))?;
            if entry.fields.is_empty() {
                return Err(
                    "this question expects a selection (use question.respond with selected)"
                        .to_string(),
                );
            }
            Self::validate_form_values(&entry.fields, values)?
        };
        let entry = {
            let mut map = self
                .pending
                .lock()
                .map_err(|e| format!("pending map poisoned: {}", e))?;
            map.remove(question_id)
                .ok_or_else(|| format!("unknown question: {}", question_id))?
        };
        // 先到先得：remove 已把请求移出 pending，第二个 respond 天然 unknown。
        entry
            .tx
            .send(Answer::Form(normalized))
            .map_err(|_| "question waiter already gone".to_string())?;
        self.broadcast_resolved(question_id, "answered");
        Ok(true)
    }

    fn pending(&self) -> Vec<serde_json::Value> {
        match self.pending.lock() {
            Ok(map) => map.iter().map(|(id, e)| Self::entry_json(id, e)).collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
