//! Tool-level hook points (K1a — U14 seventh batch / Phase 1 of
//! `docs/PLAN/2026-07-04_agent-hook-discipline-gate.md`), plus LLM-call-level
//! hooks (K1b — Phase 2,「LLM 调用级才拦思考」层).
//!
//! # 布点（handle_tool_call，loop.rs）
//!
//! ```text
//! estop 闸 → ①security（固定内联首闸，见下）→ ②user pre hooks（本模块，
//! 首个 Block 即拦）→ set_context → checkpoint preview → tool.execute →
//! ③user post hooks（管线，可改写 result）→ ④Forge 记录（看到的是改写后
//! 的最终 result）
//! ```
//!
//! # LLM 调用级布点（K1b，run_llm_loop）
//!
//! ```text
//! messages 组装完（含 nudges）→ LlmRequest observer 事件之前：
//!   ⑤llm pre hooks（Append 注入提醒 / Block 拦下本轮）→ observer 事件
//!   （request_log 看到的就是注入后的 messages）→ LLM 调用（select 急停/
//!   取消守卫）→ 错误恢复（context 压缩重试 / transient 重试，既有）→
//!   ⑥llm post hooks（Allow / Replace 改写 / Retry 有限重呼 / Block 终止
//!   本轮）→ turns_used += 1 → LlmResponse observer 事件（记录的是改写后
//!   的最终 response）→ 下游
//! ```
//!
//! Retry 语义：post hook 可要求重呼（带着 hook 反馈消息重调 LLM）；每轮
//! 预算 [`MAX_LLM_HOOK_RETRIES`] 封顶，耗尽后**放行上一个 response** 并响亮
//! warn（fail-open——坏 hook 不能把会话锁死）。重呼带同样的取消/急停 select
//! 守卫。**账本边界（诚实标注）**：重呼会发 LlmRequest observer 事件（
//! request_log 可见），但**不**追加 T8 projection ledger 记录——与既有
//! transient 重试同款（每轮一次 build 记录；ledger 驱动匹配，多出的
//! request_log 条目不会破坏 verify）。pre 注入的消息则**会**记入
//! ledger（INJECTION_LLM_HOOK），主请求保持逐字节可重放。
//!
//! # 与 2026-07-04 计划的两处实现取舍
//!
//! 1. **security 不转 trait 对象**：计划原文写「security 变 pre[0]、Forge 变
//!    post[0]」。实现取**语义等价**而非字面转换——security 保持内联固定首闸
//!    （它在 pre hooks 之前无条件运行，就是事实上的 pre[0]），Forge 在 post
//!    hooks 之后记录最终 result（就是事实上的 post 末位）。字面转换要把
//!    security.execute + guardian 子路径搬进 async trait 边界，纯机械风险、
//!    零行为收益，且直接威胁验收③（security/Forge 行为不变）——按代码修改
//!    守则不赌。K2 的 CC 方言层只需要 user hooks + 事件，不需要 uniformity。
//! 2. **pre/post 钩子异步**（`#[async_trait]`）：K2 的 CC hook 是**子进程脚本**
//!    （stdin JSON / env / 退出码），同步 trait 装不下。用户钩子运行时注册、
//!    数量少（个位数），异步开销可忽略。
//!
//! # 覆盖范围与旁路
//!
//! 所有工具分发收口在 `AgentLoop::handle_tool_call`（loop.rs），包括 U5 只读
//! 并行批（`precompute_readonly_batch` 内部逐个调 handle_tool_call）——单一
//! 插入点即全覆盖。LLM 调用级钩子布在生产主路径 `run_llm_loop`（loop.rs 的
//! `'turn` 循环）；`loop_executor.rs` 是 legacy 旁路（生产零构造），不铺死
//! 代码，只留指针注释。
//!
//! # 语义（对齐 CC PreToolUse/PostToolUse）
//!
//! - pre：**每次分发尝试都跑**（含未知工具名——钩子可拦「模型想调什么」）；
//!   有序执行，首个 `Block` 生效并短路（后续钩子不再跑）。
//! - post：**仅工具真实执行后跑**（未知工具路径不触发——没有 Pre/Post 配对
//!   语义）；管线式——每个钩子看到「当前」result，`Replace` 改写后传给下一
//!   个；全部钩子都跑（观察者不被前面的 Replace 短路）。
//! - 拦截返回文案与 security 同风格（`⛔ HOOK BLOCKED:`），模型可辨识「被
//!   策略拒绝」而非「工具坏了」。

use async_trait::async_trait;
use std::sync::Arc;

use crate::r#loop::{LlmMessage, LlmResponse};

/// Snapshot of a tool dispatch, handed to hooks. Cheap to clone; carries
/// everything a hook needs to decide without touching the live loop state.
#[derive(Debug, Clone)]
pub struct HookToolCall {
    /// Tool name as requested by the LLM (may be unregistered).
    pub name: String,
    /// Raw JSON-encoded arguments string (unparsed — hooks that need fields
    /// parse it themselves; most deny-rules match on name alone).
    pub arguments: String,
    /// Channel the current conversation arrived on (e.g. "web", "telegram").
    pub channel: String,
    /// Chat/conversation ID on that channel.
    pub chat_id: String,
    /// Session key of the conversation driving this dispatch (K2: CC dialect
    /// scripts key per-session state off payload `session_id`).
    pub session_key: String,
}

/// Pre-tool decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecision {
    /// Proceed (later hooks / execution still apply).
    Allow,
    /// Block the dispatch. `reason` surfaces to the LLM in the
    /// `⛔ HOOK BLOCKED:` result, so it should say *what policy* denied.
    Block { reason: String },
}

/// Post-tool action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostHookAction {
    /// Leave the result as-is.
    Continue,
    /// Replace the result the model (and downstream consumers like Forge)
    /// will see. The original result is gone once replaced — hooks that want
    /// to observe should use `Continue`.
    Replace(String),
}

/// A tool-level hook. Both methods have permissive defaults so an observer
/// hook only implements what it needs.
#[async_trait]
pub trait ToolHook: Send + Sync {
    /// Diagnostic name (logs / future hooks.json wiring in K2).
    fn name(&self) -> String {
        "unnamed-hook".to_string()
    }

    /// Runs before the tool executes (after the fixed security gate). Ordered;
    /// the first `Block` short-circuits the rest.
    async fn pre_tool_use(&self, _call: &HookToolCall) -> HookDecision {
        HookDecision::Allow
    }

    /// Runs after the tool executed, as a pipeline — each hook sees the
    /// current (possibly already-replaced) result.
    async fn post_tool_use(&self, _call: &HookToolCall, _result: &str) -> PostHookAction {
        PostHookAction::Continue
    }
}

/// Ordered hook registry. Stored on `AgentLoop` behind a `parking_lot::RwLock`;
/// `snapshot()` clones the Arc list so callers never hold the lock across an
/// await (an async hook could otherwise deadlock a concurrent registration).
/// (No `Debug` derive: `dyn ToolHook` isn't `Debug`.)
#[derive(Clone, Default)]
pub struct ToolHookManager {
    hooks: Vec<Arc<dyn ToolHook>>,
}

impl ToolHookManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a hook (runs after previously-registered ones).
    pub fn add(&mut self, hook: Arc<dyn ToolHook>) {
        self.hooks.push(hook);
    }

    /// Number of registered hooks.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Cheap Arc-clone of the hook chain — the safe thing to hold across an
    /// await point.
    pub fn snapshot(&self) -> Vec<Arc<dyn ToolHook>> {
        self.hooks.clone()
    }
}

/// Run pre hooks in order. Returns `Some(reason)` when a hook blocked (the
/// FIRST block wins — later hooks don't run, matching CC's deny semantics),
/// `None` when the dispatch may proceed.
pub async fn run_pre_hooks(hooks: &[Arc<dyn ToolHook>], call: &HookToolCall) -> Option<String> {
    for (i, hook) in hooks.iter().enumerate() {
        match hook.pre_tool_use(call).await {
            HookDecision::Allow => continue,
            HookDecision::Block { reason } => {
                tracing::warn!(
                    "[hooks] pre '{}' blocked tool '{}' (hook {}/{})",
                    hook.name(),
                    call.name,
                    i + 1,
                    hooks.len()
                );
                return Some(reason);
            }
        }
    }
    None
}

/// Run post hooks as a pipeline. Every hook runs (observers are not
/// short-circuited by an earlier `Replace`); each sees the current result.
/// Returns the final (possibly rewritten) result.
pub async fn run_post_hooks(hooks: &[Arc<dyn ToolHook>], call: &HookToolCall, result: String) -> String {
    let mut current = result;
    for hook in hooks {
        match hook.post_tool_use(call, &current).await {
            PostHookAction::Continue => {}
            PostHookAction::Replace(new) => {
                tracing::info!(
                    "[hooks] post '{}' replaced result of '{}' ({} -> {} bytes)",
                    hook.name(),
                    call.name,
                    current.len(),
                    new.len()
                );
                current = new;
            }
        }
    }
    current
}

// ---------------------------------------------------------------------------
// LLM-call-level hooks (K1b —「拦思考」层)
// ---------------------------------------------------------------------------

/// Per-round budget for post-LLM `Retry` demands. Exhausted → the previous
/// response is allowed through with a loud warn (fail-open: a buggy hook
/// must not lock the session into an LLM-cost loop).
pub const MAX_LLM_HOOK_RETRIES: u32 = 2;

/// Snapshot of the LLM call a hook is consulted about.
#[derive(Debug, Clone)]
pub struct HookLlmCall {
    /// Active model id.
    pub model: String,
    /// Session key of the conversation driving this call.
    pub session_key: String,
    /// 1-based round within the turn (turns_used + 1).
    pub round: usize,
}

/// Pre-LLM decision. Append-only contract: hooks may APPEND messages
/// (reminders, discipline prompts) but not rewrite history — the request log
/// and the T8 replay ledger both depend on the built prefix being stable.
#[derive(Debug, Clone)]
pub enum LlmRequestDecision {
    /// Send as built.
    Proceed,
    /// Append these messages after the built ones. Recorded in the replay
    /// ledger (`INJECTION_LLM_HOOK`) so the round stays byte-exact replayable
    /// and visible in request_log.
    Append(Vec<LlmMessage>),
    /// Do not call the LLM this round; the turn ends with `reason` shown to
    /// the user (mirrors the e-stop mid-call handling).
    Block { reason: String },
}

/// Post-LLM decision.
#[derive(Debug, Clone)]
pub enum LlmResponseDecision {
    /// Accept the (current) response.
    Allow,
    /// Replace the response downstream consumers see (observer event, usage
    /// recording, tool execution). Later hooks see the replaced value.
    Replace(LlmResponse),
    /// Re-call the LLM with `reason` appended as a feedback message
    /// (regeneration with guidance). Bounded by [`MAX_LLM_HOOK_RETRIES`].
    Retry { reason: String },
    /// Abort the turn with `reason` shown to the user.
    Block { reason: String },
}

/// An LLM-call-level hook. Defaults are permissive — an observer-only hook
/// (neither method overridden) can never change behavior.
#[async_trait]
pub trait LlmHook: Send + Sync {
    /// Diagnostic name (logs / K2 hooks.json wiring).
    fn name(&self) -> String {
        "unnamed-llm-hook".to_string()
    }

    /// Runs after messages are built (nudges included), BEFORE the LlmRequest
    /// observer event — so appended messages land in request_log.
    async fn pre_llm_call(&self, _call: &HookLlmCall, _messages: &[LlmMessage]) -> LlmRequestDecision {
        LlmRequestDecision::Proceed
    }

    /// Runs after the response (and after the built-in error recovery), BEFORE
    /// the LlmResponse observer event — so downstream sees the final decision.
    async fn post_llm_call(&self, _call: &HookLlmCall, _response: &LlmResponse) -> LlmResponseDecision {
        LlmResponseDecision::Allow
    }
}

/// Ordered LLM-hook registry. Same snapshot-then-await discipline as
/// [`ToolHookManager`].
#[derive(Clone, Default)]
pub struct LlmHookManager {
    hooks: Vec<Arc<dyn LlmHook>>,
}

impl LlmHookManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, hook: Arc<dyn LlmHook>) {
        self.hooks.push(hook);
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn snapshot(&self) -> Vec<Arc<dyn LlmHook>> {
        self.hooks.clone()
    }
}

/// Run pre-LLM hooks. Folds decisions: `Append`s concatenate (in hook order),
/// the first `Block` short-circuits (later hooks don't run).
/// Returns `Err(reason)` when blocked, `Ok(appended)` otherwise (possibly
/// empty — call only when the manager is non-empty, or pay one no-op pass).
pub async fn run_llm_pre_hooks(
    hooks: &[Arc<dyn LlmHook>],
    call: &HookLlmCall,
    messages: &[LlmMessage],
) -> Result<Vec<LlmMessage>, String> {
    let mut appended: Vec<LlmMessage> = Vec::new();
    for hook in hooks {
        match hook.pre_llm_call(call, messages).await {
            LlmRequestDecision::Proceed => {}
            LlmRequestDecision::Append(mut extra) => {
                tracing::info!(
                    "[hooks] llm-pre '{}' appended {} message(s) (round {})",
                    hook.name(),
                    extra.len(),
                    call.round
                );
                appended.append(&mut extra);
            }
            LlmRequestDecision::Block { reason } => {
                tracing::warn!(
                    "[hooks] llm-pre '{}' blocked LLM call (round {}): {}",
                    hook.name(),
                    call.round,
                    reason
                );
                return Err(reason);
            }
        }
    }
    Ok(appended)
}

/// Folded outcome of the post-LLM hook chain. `Allow` carries the final
/// (possibly replaced) response.
#[derive(Debug, Clone)]
pub enum PostLlmOutcome {
    Allow(LlmResponse),
    Retry { reason: String },
    Block { reason: String },
}

/// Run post-LLM hooks as a pipeline: each hook sees the current (possibly
/// replaced) response; the first `Retry`/`Block` short-circuits.
pub async fn run_llm_post_hooks(
    hooks: &[Arc<dyn LlmHook>],
    call: &HookLlmCall,
    response: LlmResponse,
) -> PostLlmOutcome {
    let mut current = response;
    for hook in hooks {
        match hook.post_llm_call(call, &current).await {
            LlmResponseDecision::Allow => {}
            LlmResponseDecision::Replace(new) => {
                tracing::info!(
                    "[hooks] llm-post '{}' replaced response (round {})",
                    hook.name(),
                    call.round
                );
                current = new;
            }
            LlmResponseDecision::Retry { reason } => {
                tracing::warn!(
                    "[hooks] llm-post '{}' demands retry (round {}): {}",
                    hook.name(),
                    call.round,
                    reason
                );
                return PostLlmOutcome::Retry { reason };
            }
            LlmResponseDecision::Block { reason } => {
                tracing::warn!(
                    "[hooks] llm-post '{}' blocked response (round {}): {}",
                    hook.name(),
                    call.round,
                    reason
                );
                return PostLlmOutcome::Block { reason };
            }
        }
    }
    PostLlmOutcome::Allow(current)
}

// ---------------------------------------------------------------------------
// Prompt/turn lifecycle hooks (K2 — CC SessionStart/UserPromptSubmit/Stop 桥)
// ---------------------------------------------------------------------------

/// Per-turn safety cap for `TurnEndDecision::Continue` demands (CC Stop-hook
/// "block stopping"). Exhausted → the turn stops anyway with a loud warn
/// (fail-open, same discipline as [`MAX_LLM_HOOK_RETRIES`]: a buggy hook must
/// not be able to keep a session answering forever).
pub const MAX_TURN_END_CONTINUES: u32 = 2;

/// Snapshot of an arriving user prompt (BEFORE it enters instance history —
/// a blocked prompt is never seen by the model, matching CC's
/// UserPromptSubmit block semantics).
#[derive(Debug, Clone)]
pub struct HookPrompt {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    /// The raw user message text.
    pub prompt: String,
}

/// User-prompt decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptDecision {
    /// Enqueue the prompt normally.
    Allow,
    /// Reject the prompt. `reason` surfaces to the user; the message never
    /// enters history and no LLM call is made this turn.
    Block { reason: String },
}

/// Snapshot of an accepted final answer, right before the turn ends.
#[derive(Debug, Clone)]
pub struct HookTurnEnd {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    /// The final assistant content about to be delivered.
    pub final_content: String,
    /// True when this turn's stop was already blocked once before (CC's
    /// `stop_hook_active` — scripts use it to avoid infinite loops).
    pub stop_hook_active: bool,
}

/// Turn-end decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEndDecision {
    /// Deliver the final answer and end the turn.
    Stop,
    /// Block stopping: inject `feedback` as a user message and give the
    /// model one more round. Bounded by [`MAX_TURN_END_CONTINUES`] in the
    /// loop (exhausted → stop anyway, fail-open).
    Continue { feedback: String },
}

/// A prompt/turn lifecycle hook (K2). The CC dialect bridge is the primary
/// consumer; defaults are permissive so an observer implements only what it
/// needs.
#[async_trait]
pub trait LifecycleHook: Send + Sync {
    /// Diagnostic name (logs).
    fn name(&self) -> String {
        "unnamed-lifecycle-hook".to_string()
    }

    /// User prompt arrived — runs in `run_with_trace` BEFORE
    /// `add_user_message`. Ordered; the first `Block` short-circuits.
    /// Note: CC's SessionStart event has no dedicated point here — a bridge
    /// fires it itself on first sight of a session inside this callback.
    async fn on_user_prompt(&self, _prompt: &HookPrompt) -> PromptDecision {
        PromptDecision::Allow
    }

    /// Final answer accepted — runs after the assistant message is recorded
    /// in history, BEFORE the turn's Done event. The first `Continue` wins.
    async fn on_turn_end(&self, _end: &HookTurnEnd) -> TurnEndDecision {
        TurnEndDecision::Stop
    }
}

/// Ordered lifecycle-hook registry. Same snapshot-then-await discipline as
/// [`ToolHookManager`].
#[derive(Clone, Default)]
pub struct LifecycleHookManager {
    hooks: Vec<Arc<dyn LifecycleHook>>,
}

impl LifecycleHookManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, hook: Arc<dyn LifecycleHook>) {
        self.hooks.push(hook)
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn snapshot(&self) -> Vec<Arc<dyn LifecycleHook>> {
        self.hooks.clone()
    }
}

/// Run user-prompt hooks in order. `Some(reason)` = a hook blocked the prompt
/// (first Block wins, later hooks don't run); `None` = proceed.
pub async fn run_user_prompt_hooks(
    hooks: &[Arc<dyn LifecycleHook>],
    prompt: &HookPrompt,
) -> Option<String> {
    for (i, hook) in hooks.iter().enumerate() {
        match hook.on_user_prompt(prompt).await {
            PromptDecision::Allow => continue,
            PromptDecision::Block { reason } => {
                tracing::warn!(
                    "[hooks] prompt '{}' blocked prompt for session '{}' (hook {}/{})",
                    hook.name(),
                    prompt.session_key,
                    i + 1,
                    hooks.len()
                );
                return Some(reason);
            }
        }
    }
    None
}

/// Run turn-end hooks in order. The first `Continue` short-circuits (CC: any
/// hook may veto stopping); `Stop` when all agree (or none demand more).
pub async fn run_turn_end_hooks(
    hooks: &[Arc<dyn LifecycleHook>],
    end: &HookTurnEnd,
) -> TurnEndDecision {
    for hook in hooks {
        match hook.on_turn_end(end).await {
            TurnEndDecision::Stop => {}
            TurnEndDecision::Continue { feedback } => {
                tracing::warn!(
                    "[hooks] turn-end '{}' blocked stopping for session '{}' (feedback: {} bytes)",
                    hook.name(),
                    end.session_key,
                    feedback.len()
                );
                return TurnEndDecision::Continue { feedback };
            }
        }
    }
    TurnEndDecision::Stop
}

#[cfg(test)]
mod tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
