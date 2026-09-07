//! F7（devtool-upgrade 阶段 5）：Dashboard 结构化提问 broker。
//!
//! agent 的 `question` 工具（nemesis-agent loop_tools）发起结构化提问时
//! 阻塞在这里：广播 `AgentEvent::QuestionAsked`（web pump 转 SSE
//! `question-asked`，前端 QuestionCard 渲染选项卡），并实现
//! [`nemesis_types::agent::QuestionResponder`]（WSAPI → broker 方向：
//! `question.respond` / `question.pending` 把用户作答送回等待中的 `ask`）。
//!
//! 与 [`crate::web_approval::WebApprovalManager`] 同构（pending map +
//! mpsc oneshot + broadcast 事件）：等待侧是 sync trait 方法内阻塞——tokio
//! 上下文（工具经 spawn_blocking 进来的阻塞线程）直接 `recv_timeout`，
//! 不新建 runtime。超时不是错误：广播 `QuestionResolved{timeout}` 并返回
//! `QuestionOutcome::Timeout`，工具侧回灌「按最佳判断继续」。
//!
//! 载荷校验（respond 侧）：空选择 / 非候选选项 / 单选多项 → 诚实报错，
//! 请求留在 pending——用户修正后可重试（校验失败不摘卡不惊动等待方）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use nemesis_types::agent::{AgentEvent, QuestionOutcome, QuestionRequest};

/// 一条等待作答的提问：元数据（`question.pending` 列表用）+ 作答回传通道
/// （`question.respond` 取出后 send）。
struct PendingEntry {
    question: String,
    options: Vec<String>,
    multi: bool,
    timeout_secs: u64,
    chat_id: String,
    session_key: String,
    created_at: std::time::Instant,
    tx: mpsc::Sender<Vec<String>>,
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
            "timeout_secs": entry.timeout_secs,
            "chat_id": entry.chat_id,
            "session_key": entry.session_key,
            "age_secs": entry.created_at.elapsed().as_secs(),
        })
    }
}

impl nemesis_types::agent::QuestionAsker for WebQuestionBroker {
    fn ask(&self, request: QuestionRequest) -> Result<QuestionOutcome, String> {
        let (tx, rx) = mpsc::channel::<Vec<String>>();
        let entry = PendingEntry {
            question: request.question.clone(),
            options: request.options.clone(),
            multi: request.multi,
            timeout_secs: request.timeout_secs,
            chat_id: request.chat_id.clone(),
            session_key: request.session_key.clone(),
            created_at: std::time::Instant::now(),
            tx,
        };
        self.broadcast_asked(&request.question_id, &entry);
        self.pending
            .lock()
            .map_err(|e| format!("pending map poisoned: {}", e))?
            .insert(request.question_id.clone(), entry);

        // 阻塞等作答。工具侧已经 spawn_blocking（不占 worker），这里直接
        // recv；block_in_place 分支保留给直接在 async 上下文调用 ask 的
        // 形态（与审批同构）。
        let wait = || rx.recv_timeout(Duration::from_secs(request.timeout_secs));
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
            Ok(selected) => {
                tracing::info!(
                    "[WebQuestion] question {} answered ({} item(s))",
                    request.question_id,
                    selected.len()
                );
                // 已了结广播由 respond 侧负责（单一真相源，同审批先例——
                // 这里再广播一次会发出重复 resolved 事件）。
                Ok(QuestionOutcome::Answered(selected))
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
            .send(selected)
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
