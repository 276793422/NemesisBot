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
    /// 中间轮正文（R1，2026-09-21）：LLM 循环里带工具调用的中间轮，其正文
    /// （模型的过程叙述，如「先查看配置文件再修改」）此前只进 history——
    /// web 前端只见工具卡与最终回复，过程叙述完全不可见。发布后经 web pump
    /// 默认路径走 tool_event 通道（注入 session_id + record_tool 入环），
    /// 前端执行中展开显示、最终回复落地时折叠挂载；入环后切页/重连的
    /// sync 回放自动恢复。
    RoundText {
        session_key: String,
        chat_id: String,
        /// 该轮正文（完整不截断——与最终回复同地位的用户可读文本）。
        content: String,
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
        /// 行动代理 P0（2026-10-14）授权卡自解释三字段（感知层 §5）：
        /// why=为什么要人工确认（force/connector 声明原文）；impact=影响
        /// 面（操作会碰什么）；reject_note=拒绝后果。展示层程序化派生
        /// （web_approval::approval_card_fields 单一真相源），serde default
        /// 兼容旧事件流。
        #[serde(default)]
        why: Option<String>,
        #[serde(default)]
        impact: Option<String>,
        #[serde(default)]
        reject_note: Option<String>,
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
    /// 会话物化事件（SB，2026-09-17）。某会话的 jsonl **首行落盘**后发布
    /// （正常轮 user 行 / 集群续行 / workflow_chat 三条物化路径）；web pump
    /// 转 SSE `session.created` + WS push，前端据此 force 刷新会话列表
    /// （修「隐式会话不进侧栏，须手动刷新」）。data.session_id 为裸 sid。
    SessionCreated {
        /// 会话键推导出的裸 session id（`agent:main:session:{sid}` 剥前缀；
        /// 其他命名空间取 sanitize 后形态）。
        session_id: String,
        /// 完整会话键（web 层判别命名空间用）。
        session_key: String,
    },
    /// P30（WS14）Canvas widget 打开事件。agent 终答里检出**全部合法**的
    /// ```canvas fenced 块后逐块发布（同一回复多块时 `index` 递增）；web
    /// pump 转 SSE `canvas.open`（内层 data 展平 + session_id 注入），
    /// 前端 CanvasPanel 以 iframe `sandbox="allow-scripts"` + 严格 CSP 的
    /// srcdoc 渲染（v1 完全无网络）。`html` 载荷完整不截断；语法预检不过
    /// 的块不发布（错误回灌模型自纠，见 nemesis-agent canvas 模块）。
    CanvasOpen {
        session_key: String,
        chat_id: String,
        /// canvas 块内容原文（HTML 文档或片段，前端经 iframe srcdoc 注入）。
        html: String,
        /// 同一回复内的块序号（0 起）——前端按 (session_id, index) 幂等 upsert。
        index: usize,
    },
    /// N5（2026-10-13 机会项）：交付物登记事件。`present_files` 工具登记的
    /// 交付物随 assistant 行落盘（chat_log 权威源）后发布；web pump 转 SSE
    /// `deliverables` + WS push `deliverables`（不入 chat_event_log 环——
    /// 历史映射是权威恢复源，环内重放只会重复），前端缓冲挂到即将落地的
    /// assistant 回复卡。agent_event_tx 未装配（CLI / B 端 worker）不发布。
    DeliverablesRegistered {
        session_key: String,
        chat_id: String,
        /// 本 turn 的交付物清单（已按 path 去重；title/note 取最后登记）。
        files: Vec<PresentedFile>,
    },
    /// L10（2026-10-13 内部专项 T3）provider 流式输出事件。主 turn 的 LLM
    /// 调用经流式 lane（provider 支持 `chat_stream` 且 `agents.provider_
    /// stream_events` 开启）时按增量发布；web pump 转 SSE `provider_stream_
    /// event` + WS push——**易逝事件**（EventHub `publish_ephemeral`：不盖
    /// seq、不入补拉环——断线重放半截流无意义且会冲刷 1000 帧环）。载荷
    /// **零内容体**原则上不成立（本事件即内容流），但出站前照常过出站凭据
    /// 复扫；`phase`：`start`（首增量前）/ `delta`（增量合并窗）/ `end`
    /// （调用收尾）/ `error`（调用失败，`text`=诊断）。
    ProviderStream {
        session_key: String,
        chat_id: String,
        /// 事件序号（单次 LLM 调用流内 0 起单调；重试产生新流时从新流
        /// 起点重开——前端以 `start` 帧重置本地缓冲）。
        seq: u64,
        phase: String,
        /// 增量文本（合并窗聚合）或 end/error 阶段的诊断文本。
        text: String,
        /// 发起调用的模型别名。
        model: String,
        /// 自流开始的墙钟（毫秒）——end/error 阶段携带。
        elapsed_ms: Option<u64>,
    },
    /// 凭据索要卡事件（B4，2026-10-18 凭据 vault §7）：工具槽位 / 模型
    /// API key 缺凭据时由 WebCredentialBroker 发布；web pump 转 SSE
    /// `credential-requested` 全局广播，前端凭据卡渲染（key_name + reason
    /// 与倒计时）。**全局卡**：无会话字段（凭据是进程级资产，同
    /// ApprovalResolved 的全局语义）。`request_id` 由 broker 生成，WSAPI
    /// `credential.respond` 用它路由回等待中的 `request_credential` 调用。
    /// **凭据值绝不进事件**——载荷只有元数据，值只走 用户→WSAPI→broker
    /// 落 vault 的单向链，broker 内存零驻留。
    CredentialRequested {
        request_id: String,
        /// 凭据所属服务描述（如 `connector/<id>` 或 `model:<alias>`）。
        service: String,
        /// vault 别名（bare，无 `vault:` 前缀）——卡片说明的写入目标。
        key_name: String,
        /// 为什么需要（401 原文摘要 / 槽位缺失原因）。
        reason: String,
        /// 等待窗口（秒）——前端倒计时与之对齐，超时自动放弃（调用方
        /// 走原错误路径，不是错误）。
        timeout_secs: u64,
    },
    /// 凭据已了结事件（B4）。respond 落 vault 成功、用户拒绝或超时后由
    /// broker 发布；web pump 转 SSE `credential-resolved`——所有窗口据此
    /// 摘除本地凭据卡（与 approval-resolved 同语义）。
    CredentialResolved {
        /// 与 [`AgentEvent::CredentialRequested`] 的 request_id 对应。
        request_id: String,
        /// 了结方式：`stored`（已落 vault，调用方原地重解析续行）/
        /// `denied`（用户拒绝）/ `timeout`（超时无人提交）。
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

/// N5（2026-10-13 机会项）：`present_files` 工具登记的**交付物**——agent
/// 认为本 turn 的用户应该去看的产物文件（报告/图/导出物）。单一真相源在
/// nemesis-types（`AgentEvent::DeliverablesRegistered` 载荷 + nemesis-agent
/// chat_log 的 `deliverables` 字段共用，两处 serde 形态天然一致）；路径
/// 引用不落字节（与 images 同语义）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentedFile {
    /// 交付物路径（工具侧相对 workspace 根登记，存在性已验）。
    pub path: String,
    /// 展示标题（缺省 = 文件名）。
    #[serde(default)]
    pub title: String,
    /// 一句话说明（缺省 = 空串，前端隐藏）。
    #[serde(default)]
    pub note: String,
}

impl AgentEvent {
    /// 该事件关联的 chat_id（web pump 按 `web:` 前缀路由到 session）。
    pub fn chat_id(&self) -> &str {
        match self {
            AgentEvent::ToolStarted { chat_id, .. }
            | AgentEvent::ToolFinished { chat_id, .. }
            | AgentEvent::TodoUpdated { chat_id, .. }
            | AgentEvent::ModeChanged { chat_id, .. }
            | AgentEvent::RoundText { chat_id, .. }
            | AgentEvent::ApprovalRequested { chat_id, .. }
            // F7: 提问有会话上下文（工具从 RequestContext 取），随事件透传。
            | AgentEvent::QuestionAsked { chat_id, .. } => chat_id,
            // P30：canvas 事件带发起会话的 chat_id。
            // L10：provider 流事件同样绑定发起会话。
            AgentEvent::CanvasOpen { chat_id, .. }
            | AgentEvent::ProviderStream { chat_id, .. } => chat_id,
            // N5：交付物登记绑定发起会话。
            AgentEvent::DeliverablesRegistered { chat_id, .. } => chat_id,
            // 审批不属单一会话（auditor 无 session 上下文，同 ApprovalRequested）。
            // 提问了结事件同理只带 id（全局广播）。
            AgentEvent::ApprovalResolved { .. }
            | AgentEvent::QuestionResolved { .. }
            // 凭据卡是进程级全局卡（同审批了结的全局广播语义）。
            | AgentEvent::CredentialRequested { .. }
            | AgentEvent::CredentialResolved { .. }
            // 会话物化事件带裸 id 但无 chat 目标（全局广播语义）。
            | AgentEvent::SessionCreated { .. } => "",
        }
    }

    /// 该事件关联的 agent session_key（如 `agent:main:session:{sid}`）。
    /// 末段即发起任务的 web 会话 id（`handle_chat_send` 把前端 session_id
    /// 放 metadata，loop 派生 session_key）——web pump 据此把会话 id 注入
    /// WS push 帧，前端按当前会话精确过滤（2026-09-20 BUG-A）。
    /// 审批/提问了结等全局事件无会话上下文，返回 `None`。
    pub fn session_key(&self) -> Option<&str> {
        match self {
            AgentEvent::ToolStarted { session_key, .. }
            | AgentEvent::ToolFinished { session_key, .. }
            | AgentEvent::TodoUpdated { session_key, .. }
            | AgentEvent::ModeChanged { session_key, .. }
            | AgentEvent::RoundText { session_key, .. }
            | AgentEvent::ApprovalRequested { session_key, .. }
            | AgentEvent::QuestionAsked { session_key, .. }
            | AgentEvent::SessionCreated { session_key, .. }
            | AgentEvent::CanvasOpen { session_key, .. }
            | AgentEvent::ProviderStream { session_key, .. }
            | AgentEvent::DeliverablesRegistered { session_key, .. } => Some(session_key),
            AgentEvent::ApprovalResolved { .. } | AgentEvent::QuestionResolved { .. } => None,
            // 凭据卡是进程级全局卡，无会话上下文。
            AgentEvent::CredentialRequested { .. } | AgentEvent::CredentialResolved { .. } => None,
        }
    }

    /// 事件 kind 标签（日志/路由用）。
    pub fn kind(&self) -> &'static str {
        match self {
            AgentEvent::ToolStarted { .. } => "ToolStarted",
            AgentEvent::ToolFinished { .. } => "ToolFinished",
            AgentEvent::TodoUpdated { .. } => "TodoUpdated",
            AgentEvent::ModeChanged { .. } => "ModeChanged",
            AgentEvent::RoundText { .. } => "RoundText",
            AgentEvent::ApprovalRequested { .. } => "ApprovalRequested",
            AgentEvent::ApprovalResolved { .. } => "ApprovalResolved",
            AgentEvent::QuestionAsked { .. } => "QuestionAsked",
            AgentEvent::QuestionResolved { .. } => "QuestionResolved",
            AgentEvent::SessionCreated { .. } => "SessionCreated",
            AgentEvent::CanvasOpen { .. } => "CanvasOpen",
            AgentEvent::ProviderStream { .. } => "ProviderStream",
            AgentEvent::DeliverablesRegistered { .. } => "DeliverablesRegistered",
            AgentEvent::CredentialRequested { .. } => "CredentialRequested",
            AgentEvent::CredentialResolved { .. } => "CredentialResolved",
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

/// B4（2026-10-18 凭据 vault §7）：单次凭据索要请求（触发点 → broker 方向
/// 的载荷）。触发点 = recovery Auth 臂（模型 API key 401/403）与 connector
/// 工具必需凭据槽位缺失；`key_name` 是 vault 别名（bare，无 `vault:`
/// 前缀）。
#[derive(Debug, Clone)]
pub struct CredentialRequest {
    /// broker 生成的请求标识（`cred-<hex8>`）。
    pub request_id: String,
    /// 凭据所属服务描述（`model:<alias>` / `connector/<id>`，卡片展示）。
    pub service: String,
    /// vault 别名（bare）——respond 侧按它落库。
    pub key_name: String,
    /// 为什么需要（错误摘要，卡片展示）。
    pub reason: String,
    /// 等待窗口（秒）；超时返回 [`CredentialOutcome::Denied`]（语义等价：
    /// 调用方走原错误路径）。
    pub timeout_secs: u64,
}

/// B4：凭据索要结局。**通道只传裁决不传值**——`Stored` 表示用户已提交且
/// broker 已把值落 vault（调用方原地重解析即续行）；值从不经本通道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialOutcome {
    /// 已落 vault（调用方重解析）。
    Stored,
    /// 用户拒绝或超时（调用方走原错误路径）。
    Denied,
}

/// 凭据索要发起端 trait（B4，触发点 → broker 方向）。
///
/// 与 [`CredentialResponder`]（WSAPI → broker 方向）互补，实现方
/// （nemesisbot 的 `WebCredentialBroker`）同时实现两者。同步阻塞语义
/// （等用户提交）：调用方经 `spawn_blocking` 调用（tokio 上下文内 broker
/// 自身用 `block_in_place` 让出 worker，同 QuestionAsker 契约）。放在
/// nemesis-types 让 nemesis-agent 无需依赖 nemesisbot/nemesis-web。
pub trait CredentialBroker: Send + Sync {
    /// 发起索要并阻塞等结局。`Ok` = 窗口内有结局（落库/拒绝/超时）；
    /// `Err` = broker 内部故障（pending 锁中毒等）。
    fn request_credential(&self, request: &CredentialRequest) -> Result<CredentialOutcome, String>;
}

/// 凭据索要响应端 trait（B4，WSAPI → broker 方向）。
///
/// respond 侧职责：**先落 vault 再送裁决**——值经本 trait 进入 broker 后
/// 立即写 vault（force 语义=卡片提交即显式轮换意图），成功才回 `Stored`；
/// 落库失败回 `Err`（请求留在 pending，用户可修正重试）。凭据值不落日志/
/// 错误/审计（vault 纪律，只记 alias）。
pub trait CredentialResponder: Send + Sync {
    /// 提交凭据值。`Ok(json)` = 已落 vault 且裁决送达（json 带
    /// `stored`/`key_name` 回执）；`Err` = 请求不存在（已超时清理/已裁决/
    /// broker 未装配）或落库失败（请求留在 pending，可修正重试）。
    fn respond(&self, request_id: &str, value: &str) -> Result<serde_json::Value, String>;

    /// 当前等待提交的凭据请求元数据列表（前端刷新/恢复用；**无值字段**）。
    fn pending(&self) -> Vec<serde_json::Value>;
}

#[cfg(test)]
mod tests;
