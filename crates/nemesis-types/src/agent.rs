//! Agent-related types.

use serde::{Deserialize, Serialize};

/// Unique session key for agent conversations.
#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct SessionKey(pub String);

impl SessionKey {
    pub fn new(channel: &str, chat_id: &str) -> Self {
        Self(format!("{}:{}", channel, chat_id))
    }
}

/// Agent configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub max_iterations: u32,
    pub max_context_tokens: usize,
    pub system_prompt: Option<String>,
    pub temperature: f64,
    pub top_p: f64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 60,
            max_context_tokens: 128000,
            system_prompt: None,
            temperature: 0.7,
            top_p: 1.0,
        }
    }
}

/// Agent session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub session_key: SessionKey,
    pub channel: String,
    pub chat_id: String,
    pub messages: Vec<AgentMessage>,
    pub created_at: String,
    pub updated_at: String,
}

/// Agent message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: MessageRole,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
}

/// Message role in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Tool call from the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool result from tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Agent 内部事件（M1a，devtool-upgrade）：工具生命周期事件的跨 crate 传输形态。
///
/// 由 `nemesis-agent` 的 ToolEventHook 在工具调度前后发布（tokio broadcast），
/// `nemesis-web` 订阅后路由到 Dashboard WS push + EventHub（SSE 备用）。
/// 预览字段一律截断（args 200 字符 / result 1KB），防止大输出撑爆前端。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum AgentEvent {
    /// 工具开始执行（安全闸通过、around 链进入前）。
    ToolStarted {
        session_key: String,
        chat_id: String,
        call_id: String,
        tool: String,
        /// 工具 args 的 JSON 序列化预览（≤200 字符）。
        args_preview: String,
    },
    /// 工具执行结束（含失败）。
    ToolFinished {
        session_key: String,
        chat_id: String,
        call_id: String,
        tool: String,
        duration_ms: u64,
        /// ok 判定沿用 Forge 启发式：非 "Tool error:" 开头且不含 "SECURITY BLOCKED"。
        ok: bool,
        /// 工具结果预览（≤1KB）。
        result_preview: String,
    },
    /// Todo 更新事件（M1a 预留 → H1 落地：todowrite 写入成功后发布，
    /// 载荷是全量清单——前端收到即可渲染，无需二次拉取）。
    TodoUpdated {
        session_key: String,
        chat_id: String,
        /// 全量替换后的 todo 清单。
        todos: Vec<TodoItem>,
    },
    /// plan/build 模式切换事件（F1，devtool-upgrade 阶段 4）。`mode` 取
    /// `AgentMode::as_str`（`"plan"` / `"build"`）。切换入口（/plan /build
    /// slash、WSAPI `chat.set_mode`）在翻转后发布；前端据此刷新徽标。
    ModeChanged {
        session_key: String,
        chat_id: String,
        mode: String,
    },
    /// 审批请求事件（M7，devtool-upgrade 阶段 5）。安全 auditor 命中
    /// require_approval 时由 WebApprovalManager 发布；web pump 转 SSE
    /// `approval-requested` 全局广播，前端 ApprovalCard 渲染审批卡。
    /// `request_id` 是 auditor 生成的请求 ID（WSAPI `approval.respond`
    /// 用它路由回等待中的 `request_approval_sync` 调用）。
    ApprovalRequested {
        session_key: String,
        chat_id: String,
        request_id: String,
        /// 操作类型（`req.op_type`，如 `process_exec`）。
        operation: String,
        /// 操作目标（路径/命令等）。
        target: String,
        /// 风险级别字符串（`LOW`/`MEDIUM`/`HIGH`/`CRITICAL`）。
        risk_level: String,
        /// 触发审批的规则原因。
        reason: String,
        /// auditor 侧等待窗口（秒）——前端倒计时与之对齐，超时自动 deny。
        timeout_secs: u64,
        /// F3:「总是允许」将写入的规则 pattern（exec 类=B5 归约前缀，
        /// 其余=完整 target；前端按钮回显确认串）。空串=不适用（如空
        /// target / run_script 无 target 提取）。
        pattern: String,
    },
    /// 审批已裁决事件（F6，devtool-upgrade 阶段 5）。任一前端 respond 成功
    /// 或服务端超时自动 deny 后由 WebApprovalManager 发布；web pump 转 SSE
    /// `approval-resolved` 全局广播——所有窗口（桌面 WebView + 外部浏览器）
    /// 据此摘除本地审批卡，竞速败方不再挂到倒计时结束。
    ApprovalResolved {
        /// 与 [`AgentEvent::ApprovalRequested`] 的 request_id 对应。
        request_id: String,
        /// 裁决结果：`approved`（有人批准）/ `denied`（有人拒绝）/
        /// `timeout`（服务端超时自动拒绝）。
        decision: String,
    },
    /// 结构化提问事件（F7，devtool-upgrade 阶段 5）。agent 的 `question`
    /// 工具发起提问时由 WebQuestionBroker 发布；web pump 转 SSE
    /// `question-asked` 全局广播，前端 QuestionCard 渲染选项卡（单选
    /// radio / 多选 checkbox）。`question_id` 由工具侧生成（进程内自增），
    /// WSAPI `question.respond` 用它路由回等待中的 `ask` 调用。
    QuestionAsked {
        session_key: String,
        chat_id: String,
        question_id: String,
        /// 提问正文（工具 args 的 `question` 字段）。
        question: String,
        /// 候选选项（工具 args 的 `options`，≥2 项）。
        options: Vec<String>,
        /// `true` = 用户可多选（checkbox）；`false` = 单选（radio）。
        multi: bool,
        /// 等待窗口（秒）——前端倒计时与之对齐，超时工具侧收到
        /// Timeout 结局并按最佳判断继续。
        timeout_secs: u64,
    },
    /// 提问已了结事件（F7，devtool-upgrade 阶段 5）。任一前端 respond
    /// 成功或等待超时后由 WebQuestionBroker 发布；web pump 转 SSE
    /// `question-resolved` 全局广播——所有窗口据此摘除本地提问卡
    /// （与 approval-resolved 同语义）。
    QuestionResolved {
        /// 与 [`AgentEvent::QuestionAsked`] 的 question_id 对应。
        question_id: String,
        /// 了结方式：`answered`（有人作答）/ `timeout`（超时无人答）。
        decision: String,
    },
}

/// H1（devtool-upgrade 阶段 2）：单条 todo（todowrite 全量提交语义）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
}

/// Todo 状态（serde 小写蛇形：`pending` / `in_progress` / `completed`，
/// 与 todowrite 工具 schema 的 enum 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl AgentEvent {
    /// 该事件关联的 chat_id（web pump 按 `web:` 前缀路由到 session）。
    pub fn chat_id(&self) -> &str {
        match self {
            AgentEvent::ToolStarted { chat_id, .. }
            | AgentEvent::ToolFinished { chat_id, .. }
            | AgentEvent::TodoUpdated { chat_id, .. }
            | AgentEvent::ModeChanged { chat_id, .. }
            | AgentEvent::ApprovalRequested { chat_id, .. }
            // F7: 提问有会话上下文（工具从 RequestContext 取），随事件透传。
            | AgentEvent::QuestionAsked { chat_id, .. } => chat_id,
            // 审批不属单一会话（auditor 无 session 上下文，同 ApprovalRequested）。
            // 提问了结事件同理只带 id（全局广播）。
            AgentEvent::ApprovalResolved { .. }
            | AgentEvent::QuestionResolved { .. } => "",
        }
    }

    /// 事件 kind 标签（日志/路由用）。
    pub fn kind(&self) -> &'static str {
        match self {
            AgentEvent::ToolStarted { .. } => "ToolStarted",
            AgentEvent::ToolFinished { .. } => "ToolFinished",
            AgentEvent::TodoUpdated { .. } => "TodoUpdated",
            AgentEvent::ModeChanged { .. } => "ModeChanged",
            AgentEvent::ApprovalRequested { .. } => "ApprovalRequested",
            AgentEvent::ApprovalResolved { .. } => "ApprovalResolved",
            AgentEvent::QuestionAsked { .. } => "QuestionAsked",
            AgentEvent::QuestionResolved { .. } => "QuestionResolved",
        }
    }
}

/// 审批响应端 trait（M7，devtool-upgrade 阶段 5）。
///
/// 与 `nemesis_security::auditor::ApprovalManager`（auditor → manager 方向，
/// 同步阻塞等结果）互补：本 trait 是 WSAPI → manager 方向——dashboard 审批卡
/// 经 `approval.respond` / `approval.pending` 把用户裁决送回等待中的
/// `request_approval_sync` 调用。放在 nemesis-types 让 nemesis-web 无需依赖
/// nemesis-security 即可触达（AppState 走 agent_loop 槽）。
pub trait ApprovalResponder: Send + Sync {
    /// 对一次审批请求送出裁决。返回 `Ok(true/false)`=该裁决生效（先到先得）；
    /// `Err`=请求不存在（已超时清理、已裁决过、或 manager 未装配）。
    /// `always`=批准并记住（F3：写入「总是允许」规则表，同 pattern 之后
    /// 自动放行；仅 approved=true 且层级安全门放行时生效，否则忽略）。
    /// `note`=拒绝备注（F6：approved=false 时随裁决送达，auditor 拼进拒绝
    /// 消息回灌给模型——「User rejected ...: {note}」；approved=true 时忽略）。
    fn respond(
        &self,
        request_id: &str,
        approved: bool,
        always: bool,
        note: Option<String>,
    ) -> Result<bool, String>;

    /// 当前等待裁决的审批请求元数据列表（前端刷新/恢复用）。
    fn pending(&self) -> Vec<serde_json::Value>;
}

/// F7（devtool-upgrade 阶段 5）：单次结构化提问请求（question 工具 →
/// broker 方向的载荷）。`question_id` 由工具侧生成（进程内自增，如 `q-1`）。
#[derive(Debug, Clone)]
pub struct QuestionRequest {
    pub question_id: String,
    /// 提问正文（须自含——模型不再补话）。
    pub question: String,
    /// 候选选项（≥2 项）。
    pub options: Vec<String>,
    /// `true` = 用户可多选；`false` = 单选。
    pub multi: bool,
    /// 提问所属会话（事件透传给前端）。
    pub chat_id: String,
    pub session_key: String,
    /// 等待窗口（秒）；超时返回 [`QuestionOutcome::Timeout`]。
    pub timeout_secs: u64,
}

/// F7：提问结局。`Answered` 携带用户选中项（单选恰一项，均为工具给过的
/// 选项原文）；`Timeout` = 窗口内无人作答（工具侧回灌「按最佳判断继续」，
/// 不是错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionOutcome {
    Answered(Vec<String>),
    Timeout,
}

/// 结构化提问发起端 trait（F7，question 工具 → broker 方向）。
///
/// 与 [`ApprovalResponder`]（WSAPI → manager 方向）互补，实现方
/// （nemesisbot 的 `WebQuestionBroker`）同时实现两者。同步阻塞语义
/// （等用户作答）：调用方（工具）经 `spawn_blocking` 调用，tokio 上下文内
/// broker 自身用 `block_in_place` 让出 worker。放在 nemesis-types 让
/// nemesis-agent 无需依赖 nemesisbot/nemesis-web。
pub trait QuestionAsker: Send + Sync {
    /// 发起提问并阻塞等答复。`Ok` = 窗口内有结局（作答或超时）；
    /// `Err` = broker 内部故障（pending 锁中毒等）。
    fn ask(&self, request: QuestionRequest) -> Result<QuestionOutcome, String>;
}

/// 结构化提问响应端 trait（F7，WSAPI → broker 方向）。放在 nemesis-types
/// 让 nemesis-web 经 agent_loop 槽触达（同 [`ApprovalResponder`] 先例）。
pub trait QuestionResponder: Send + Sync {
    /// 送出用户作答。`Ok(true)` = 送达（先到先得）；`Err` = 请求不存在
    /// （已超时清理/已作答/broker 未装配）或载荷非法（空选择/非候选选项/
    /// 单选多项——校验失败时请求留在 pending，用户可修正重试）。
    fn respond(&self, question_id: &str, selected: Vec<String>) -> Result<bool, String>;

    /// 当前等待作答的提问元数据列表（前端刷新/恢复用）。
    fn pending(&self) -> Vec<serde_json::Value>;
}

#[cfg(test)]
mod tests;
