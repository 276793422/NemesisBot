//! Tests for `crate::hooks` (K1a — U14 seventh batch).
//!
//! Acceptance mapping (goal §二 第七批 K1a):
//! - ① pre 能拦下工具调用 → `handle_tool_call_pre_blocks`（真 dispatch 路径，
//!   且断言工具本体未执行）
//! - ② post 能观察/改写 result → `handle_tool_call_post_replaces` /
//!   `post_pipeline_each_hook_sees_current_result`
//! - ③ security/Forge 行为不变 → `handle_tool_call_no_hooks_unchanged`
//!   （无钩子路径结果字节不变）+ 既有全量回归
//! - ④ 旁路覆盖 → loop_executor.rs 为 legacy 零生产构造（已留指针注释，
//!   见 hooks.rs 模块文档「覆盖范围与旁路」）；U5 只读并行批内部逐个调
//!   `handle_tool_call`，自动被同一插入点覆盖（loop/tests.rs U5 并发测试）。
//!
//! 另外钉住文档化的不对称语义：pre 对**每次分发尝试**都跑（含未知工具名，
//! `handle_tool_call_pre_fires_for_unknown_tool`），post 仅在工具真实执行
//! 后跑（`handle_tool_call_post_skipped_for_unknown_tool`）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use super::{
    HookDecision, HookLlmCall, HookToolCall, LlmHook, LlmHookManager, LlmRequestDecision,
    LlmResponseDecision, MAX_LLM_HOOK_RETRIES, PostHookAction, PostLlmOutcome, ToolHook,
    ToolHookManager, run_llm_post_hooks, run_llm_pre_hooks, run_post_hooks, run_pre_hooks,
};
use crate::context::RequestContext;
use crate::instance::AgentInstance;
use crate::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse, Tool};
use crate::types::{AgentConfig, ChatOptions, ToolCallInfo};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Provider that must never be called — these tests exercise
/// `handle_tool_call`, which never touches the LLM.
struct NoopProvider;

#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("NoopProvider must not be called by handle_tool_call".to_string())
    }
}

/// Tool whose execute() returns a fixed body and flips a flag — the flag lets
/// tests prove a blocked dispatch never reached the tool body.
struct MarkerTool {
    executed: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for MarkerTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        self.executed.store(true, Ordering::SeqCst);
        Ok("original-result".to_string())
    }
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["marker".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn loop_with_marker_tool(executed: Arc<AtomicBool>) -> AgentLoop {
    let mut lp = AgentLoop::new(Box::new(NoopProvider), test_config());
    lp.register_tool("marker".to_string(), Box::new(MarkerTool { executed }));
    lp
}

fn marker_call() -> ToolCallInfo {
    ToolCallInfo {
        id: "call-1".to_string(),
        name: "marker".to_string(),
        arguments: "{}".to_string(),
    }
}

fn test_context() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "session1")
}

fn hook_call() -> HookToolCall {
    HookToolCall {
        name: "marker".to_string(),
        arguments: "{}".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: "session1".to_string(),
    }
}

/// Hook that blocks everything with a fixed reason.
struct BlockHook {
    reason: String,
}

#[async_trait]
impl ToolHook for BlockHook {
    fn name(&self) -> String {
        "block-hook".to_string()
    }

    async fn pre_tool_use(&self, _call: &HookToolCall) -> HookDecision {
        HookDecision::Block {
            reason: self.reason.clone(),
        }
    }
}

/// Hook that replaces every post result with a fixed string.
struct ReplaceHook {
    replacement: String,
}

#[async_trait]
impl ToolHook for ReplaceHook {
    fn name(&self) -> String {
        "replace-hook".to_string()
    }

    async fn post_tool_use(&self, _call: &HookToolCall, _result: &str) -> PostHookAction {
        PostHookAction::Replace(self.replacement.clone())
    }
}

/// Post hook that appends a tag to the CURRENT result — the append chain
/// proves each hook sees the output of the previous one (pipeline, not
/// parallel-observing-original).
struct AppendHook {
    tag: String,
}

#[async_trait]
impl ToolHook for AppendHook {
    fn name(&self) -> String {
        format!("append-{}", self.tag)
    }

    async fn post_tool_use(&self, _call: &HookToolCall, result: &str) -> PostHookAction {
        PostHookAction::Replace(format!("{}{}", result, self.tag))
    }
}

// ---------------------------------------------------------------------------
// Unit tests: manager + run fns
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pre_first_block_wins() {
    let hooks: Vec<Arc<dyn ToolHook>> = vec![
        Arc::new(BlockHook {
            reason: "first-reason".to_string(),
        }),
        Arc::new(BlockHook {
            reason: "second-reason".to_string(),
        }),
    ];
    let blocked = run_pre_hooks(&hooks, &hook_call()).await;
    assert_eq!(blocked.as_deref(), Some("first-reason"));
}

#[tokio::test]
async fn pre_all_allow_returns_none() {
    let hooks: Vec<Arc<dyn ToolHook>> = vec![];
    assert!(run_pre_hooks(&hooks, &hook_call()).await.is_none());
}

/// A hook implementing neither method (pure observer intent) must default to
/// Allow/Continue — registering an observer can never change behavior.
#[tokio::test]
async fn trait_defaults_are_permissive() {
    struct BareHook;
    #[async_trait]
    impl ToolHook for BareHook {}

    let bare = BareHook;
    assert_eq!(bare.name(), "unnamed-hook");

    let hooks: Vec<Arc<dyn ToolHook>> = vec![Arc::new(BareHook)];
    assert!(run_pre_hooks(&hooks, &hook_call()).await.is_none());
    assert_eq!(
        run_post_hooks(&hooks, &hook_call(), "r".to_string()).await,
        "r"
    );
}

#[tokio::test]
async fn post_pipeline_each_hook_sees_current_result() {
    let hooks: Vec<Arc<dyn ToolHook>> = vec![
        Arc::new(AppendHook {
            tag: "+B".to_string(),
        }),
        Arc::new(AppendHook {
            tag: "+C".to_string(),
        }),
    ];
    let out = run_post_hooks(&hooks, &hook_call(), "A".to_string()).await;
    assert_eq!(out, "A+B+C");
}

#[tokio::test]
async fn manager_add_snapshot_len() {
    let mut mgr = ToolHookManager::new();
    assert!(mgr.is_empty());
    mgr.add(Arc::new(BlockHook {
        reason: "r".to_string(),
    }));
    mgr.add(Arc::new(ReplaceHook {
        replacement: "x".to_string(),
    }));
    assert_eq!(mgr.len(), 2);
    assert_eq!(mgr.snapshot().len(), 2);
    // Snapshot is independent of later mutations (the point of snapshotting
    // before an await — see ToolHookManager doc).
    let snap = mgr.snapshot();
    mgr.add(Arc::new(BlockHook {
        reason: "r2".to_string(),
    }));
    assert_eq!(snap.len(), 2);
    assert_eq!(mgr.len(), 3);
}

// ---------------------------------------------------------------------------
// Integration: through AgentLoop::handle_tool_call (the real dispatch path)
// ---------------------------------------------------------------------------

/// Acceptance ①: a registered pre hook intercepts the dispatch — the blocked
/// result carries the ⛔ HOOK BLOCKED marker and the tool body never ran.
#[tokio::test]
async fn handle_tool_call_pre_blocks() {
    let executed = Arc::new(AtomicBool::new(false));
    let lp = loop_with_marker_tool(executed.clone());
    lp.add_tool_hook(Arc::new(BlockHook {
        reason: "denied by policy X".to_string(),
    }));

    let result = lp.handle_tool_call(&marker_call(), &test_context()).await;

    assert!(
        result.starts_with("⛔ HOOK BLOCKED:"),
        "blocked marker missing: {result}"
    );
    assert!(
        result.contains("denied by policy X"),
        "hook reason not surfaced: {result}"
    );
    assert!(
        !executed.load(Ordering::SeqCst),
        "blocked dispatch must not execute the tool body"
    );
}

/// Acceptance ②: a post hook can rewrite the result the model sees.
#[tokio::test]
async fn handle_tool_call_post_replaces() {
    let executed = Arc::new(AtomicBool::new(false));
    let lp = loop_with_marker_tool(executed.clone());
    lp.add_tool_hook(Arc::new(ReplaceHook {
        replacement: "sanitized-result".to_string(),
    }));

    let result = lp.handle_tool_call(&marker_call(), &test_context()).await;

    assert!(executed.load(Ordering::SeqCst));
    assert_eq!(result, "sanitized-result");
}

/// Acceptance ③: with no hooks registered the result is byte-identical to
/// the pre-K1a behavior (zero-regression guarantee).
#[tokio::test]
async fn handle_tool_call_no_hooks_unchanged() {
    let executed = Arc::new(AtomicBool::new(false));
    let lp = loop_with_marker_tool(executed.clone());

    let result = lp.handle_tool_call(&marker_call(), &test_context()).await;

    assert_eq!(result, "original-result");
}

/// Documented asymmetry: pre hooks fire on EVERY dispatch attempt, including
/// unknown tool names (a hook may deny what the model tried to call).
#[tokio::test]
async fn handle_tool_call_pre_fires_for_unknown_tool() {
    let lp = loop_with_marker_tool(Arc::new(AtomicBool::new(false)));
    lp.add_tool_hook(Arc::new(BlockHook {
        reason: "unknown-tool-denied".to_string(),
    }));
    let call = ToolCallInfo {
        id: "call-2".to_string(),
        name: "does_not_exist".to_string(),
        arguments: "{}".to_string(),
    };

    let result = lp.handle_tool_call(&call, &test_context()).await;

    assert!(
        result.starts_with("⛔ HOOK BLOCKED:"),
        "pre hook must fire for unknown tools: {result}"
    );
}

/// Documented asymmetry: post hooks only run when the tool actually executed
/// (Pre/Post pairing) — the unknown-tool error path is NOT rewritten.
#[tokio::test]
async fn handle_tool_call_post_skipped_for_unknown_tool() {
    let lp = loop_with_marker_tool(Arc::new(AtomicBool::new(false)));
    lp.add_tool_hook(Arc::new(ReplaceHook {
        replacement: "should-not-appear".to_string(),
    }));
    let call = ToolCallInfo {
        id: "call-3".to_string(),
        name: "does_not_exist".to_string(),
        arguments: "{}".to_string(),
    };

    let result = lp.handle_tool_call(&call, &test_context()).await;

    assert_eq!(result, "Error: Unknown tool 'does_not_exist'");
}

/// A bare observer hook (defaults only) registered on the live loop changes
/// nothing — the result passes through untouched.
#[tokio::test]
async fn handle_tool_call_bare_observer_is_noop() {
    struct BareHook;
    #[async_trait]
    impl ToolHook for BareHook {}

    let executed = Arc::new(AtomicBool::new(false));
    let lp = loop_with_marker_tool(executed.clone());
    lp.add_tool_hook(Arc::new(BareHook));

    let result = lp.handle_tool_call(&marker_call(), &test_context()).await;

    assert!(executed.load(Ordering::SeqCst));
    assert_eq!(result, "original-result");
}

// ===========================================================================
// K1b — LLM-call-level hooks
// ===========================================================================
//
// Acceptance mapping (goal §二 第七批 K1b):
// - ①请求前注入的提醒出现在该轮 messages → llm_pre_append_reaches_provider
//   （observer LlmRequest 事件用同一份 messages 构建，结构上 request_log
//   即可见）
// - ②响应检查可要求重试/放行 → llm_post_retry_recalls_with_feedback（重呼
//   带反馈消息）/ llm_post_replace_downstream / llm_post_block_terminates /
//   llm_retry_budget_exhausted_fail_open
// - ③无钩子注册时字节不变 → llm_no_hooks_single_unchanged_call（空链零执行）
//   + 既有全量回归。

/// Shared recording state — the handle clones the Arc so tests can inspect
/// calls after the loop consumed the provider Box.
#[derive(Clone)]
struct Recorder(Arc<RecorderState>);

struct RecorderState {
    calls: std::sync::Mutex<Vec<Vec<LlmMessage>>>,
    script: std::sync::Mutex<Vec<LlmResponse>>,
}

/// Provider that records every call's messages and plays back a script.
struct RecordingProvider {
    state: Arc<RecorderState>,
}

/// Build the (provider, recorder) pair — `recorder` observes what the
/// provider saw.
fn recording_provider(script: Vec<LlmResponse>) -> (Box<RecordingProvider>, Recorder) {
    let state = Arc::new(RecorderState {
        calls: std::sync::Mutex::new(Vec::new()),
        script: std::sync::Mutex::new(script),
    });
    (
        Box::new(RecordingProvider {
            state: state.clone(),
        }),
        Recorder(state),
    )
}

impl Recorder {
    fn calls(&self) -> std::sync::MutexGuard<'_, Vec<Vec<LlmMessage>>> {
        self.0.calls.lock().unwrap()
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.state.calls.lock().unwrap().push(messages);
        let mut script = self.state.script.lock().unwrap();
        Ok(if script.is_empty() {
            scripted_resp("script-exhausted")
        } else {
            script.remove(0)
        })
    }
}

fn scripted_resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn llm_hook_call() -> HookLlmCall {
    HookLlmCall {
        model: "test-model".to_string(),
        session_key: "session1".to_string(),
        round: 1,
    }
}

fn sys_msg(content: &str) -> LlmMessage {
    LlmMessage {
        role: "system".to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }
}

/// Pre hook appending a fixed reminder.
struct AppendReminderHook {
    text: String,
}

#[async_trait]
impl LlmHook for AppendReminderHook {
    fn name(&self) -> String {
        "append-reminder".to_string()
    }

    async fn pre_llm_call(
        &self,
        _call: &HookLlmCall,
        _messages: &[LlmMessage],
    ) -> LlmRequestDecision {
        LlmRequestDecision::Append(vec![sys_msg(&self.text)])
    }
}

/// Pre hook blocking the call outright.
struct BlockLlmCallHook {
    reason: String,
}

#[async_trait]
impl LlmHook for BlockLlmCallHook {
    async fn pre_llm_call(
        &self,
        _call: &HookLlmCall,
        _messages: &[LlmMessage],
    ) -> LlmRequestDecision {
        LlmRequestDecision::Block {
            reason: self.reason.clone(),
        }
    }
}

/// Post hook demanding a retry whenever the response carries a marker.
struct RetryOnMarkerHook {
    marker: String,
}

#[async_trait]
impl LlmHook for RetryOnMarkerHook {
    async fn post_llm_call(
        &self,
        _call: &HookLlmCall,
        response: &LlmResponse,
    ) -> LlmResponseDecision {
        if response.content.contains(&self.marker) {
            LlmResponseDecision::Retry {
                reason: format!("response contained '{}', regenerate", self.marker),
            }
        } else {
            LlmResponseDecision::Allow
        }
    }
}

/// Post hook that always demands a retry (budget test).
struct AlwaysRetryHook;

#[async_trait]
impl LlmHook for AlwaysRetryHook {
    async fn post_llm_call(
        &self,
        _call: &HookLlmCall,
        _response: &LlmResponse,
    ) -> LlmResponseDecision {
        LlmResponseDecision::Retry {
            reason: "never good enough".to_string(),
        }
    }
}

/// Post hook replacing the response content.
struct ReplaceResponseHook {
    to: String,
}

#[async_trait]
impl LlmHook for ReplaceResponseHook {
    async fn post_llm_call(
        &self,
        _call: &HookLlmCall,
        _response: &LlmResponse,
    ) -> LlmResponseDecision {
        LlmResponseDecision::Replace(scripted_resp(&self.to))
    }
}

/// Post hook blocking the response (turn termination).
struct BlockResponseHook {
    reason: String,
}

#[async_trait]
impl LlmHook for BlockResponseHook {
    async fn post_llm_call(
        &self,
        _call: &HookLlmCall,
        _response: &LlmResponse,
    ) -> LlmResponseDecision {
        LlmResponseDecision::Block {
            reason: self.reason.clone(),
        }
    }
}

fn first_done(events: &[crate::types::AgentEvent]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            crate::types::AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

// --- unit: fold semantics ---------------------------------------------------

#[tokio::test]
async fn llm_pre_hooks_concatenate_appends_in_order() {
    let hooks: Vec<Arc<dyn LlmHook>> = vec![
        Arc::new(AppendReminderHook {
            text: "first".to_string(),
        }),
        Arc::new(AppendReminderHook {
            text: "second".to_string(),
        }),
    ];
    let out = run_llm_pre_hooks(&hooks, &llm_hook_call(), &[]).await;
    assert!(out.is_ok());
    let appended = out.unwrap();
    assert_eq!(appended.len(), 2);
    assert_eq!(appended[0].content, "first");
    assert_eq!(appended[1].content, "second");
}

#[tokio::test]
async fn llm_pre_hook_block_short_circuits() {
    let hooks: Vec<Arc<dyn LlmHook>> = vec![
        Arc::new(AppendReminderHook {
            text: "never-appended".to_string(),
        }),
        Arc::new(BlockLlmCallHook {
            reason: "no-llm-for-you".to_string(),
        }),
    ];
    // Block is the SECOND hook — the first hook's append already ran, but the
    // aggregate is Err (caller discards everything on block).
    let out = run_llm_pre_hooks(&hooks, &llm_hook_call(), &[]).await;
    assert_eq!(out.unwrap_err(), "no-llm-for-you");
}

#[tokio::test]
async fn llm_post_replace_pipelines_into_later_hooks() {
    struct ProbeHook {
        seen: std::sync::Mutex<Vec<String>>,
    }
    #[async_trait]
    impl LlmHook for ProbeHook {
        async fn post_llm_call(&self, _c: &HookLlmCall, r: &LlmResponse) -> LlmResponseDecision {
            self.seen.lock().unwrap().push(r.content.clone());
            LlmResponseDecision::Allow
        }
    }

    let probe = Arc::new(ProbeHook {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let hooks: Vec<Arc<dyn LlmHook>> = vec![
        Arc::new(ReplaceResponseHook {
            to: "replaced".to_string(),
        }),
        probe.clone(),
    ];
    let out = run_llm_post_hooks(&hooks, &llm_hook_call(), scripted_resp("original")).await;
    match out {
        PostLlmOutcome::Allow(r) => assert_eq!(r.content, "replaced"),
        other => panic!("expected Allow, got {other:?}"),
    }
    // The later hook saw the REPLACED value (pipeline).
    assert_eq!(*probe.seen.lock().unwrap(), vec!["replaced".to_string()]);
}

#[tokio::test]
async fn llm_post_retry_short_circuits() {
    let hooks: Vec<Arc<dyn LlmHook>> = vec![Arc::new(AlwaysRetryHook)];
    let out = run_llm_post_hooks(&hooks, &llm_hook_call(), scripted_resp("x")).await;
    assert!(matches!(out, PostLlmOutcome::Retry { .. }));
}

#[tokio::test]
async fn llm_hook_manager_basics() {
    let mut mgr = LlmHookManager::new();
    assert!(mgr.is_empty());
    mgr.add(Arc::new(AlwaysRetryHook));
    assert_eq!(mgr.len(), 1);
    assert_eq!(mgr.snapshot().len(), 1);
}

// --- integration: through AgentLoop::run (production run_llm_loop path) ------

/// Acceptance ①: a pre-LLM hook's appended reminder reaches the provider as
/// part of that round's messages (request_log is built from the same vec).
#[tokio::test]
async fn llm_pre_append_reaches_provider() {
    let (provider, recorder) = recording_provider(vec![scripted_resp("ok")]);
    let lp = AgentLoop::new(provider, test_config());
    lp.add_llm_hook(Arc::new(AppendReminderHook {
        text: "# Reminder: cite file paths".to_string(),
    }));

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "Hi", &test_context()).await;

    assert_eq!(first_done(&events), "ok");
    let calls = recorder.calls();
    assert_eq!(calls.len(), 1);
    let last = calls[0].last().expect("at least one message");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "# Reminder: cite file paths");
}

/// Pre-LLM Block: the provider is never called; the user sees the blocked
/// reason in the turn's Done event.
#[tokio::test]
async fn llm_pre_block_aborts_turn_before_call() {
    let (provider, recorder) = recording_provider(vec![scripted_resp("never")]);
    let lp = AgentLoop::new(provider, test_config());
    lp.add_llm_hook(Arc::new(BlockLlmCallHook {
        reason: "off-topic guard".to_string(),
    }));

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "Hi", &test_context()).await;

    let done = first_done(&events);
    assert!(done.contains("⛔ HOOK BLOCKED"), "done={done}");
    assert!(done.contains("off-topic guard"), "done={done}");
    assert!(
        recorder.calls().is_empty(),
        "blocked LLM call must not reach the provider"
    );
}

/// Acceptance ② (retry): a response checker demands a retry for a marked
/// response — the loop re-calls the LLM with the hook feedback appended and
/// finishes with the regenerated answer.
#[tokio::test]
async fn llm_post_retry_recalls_with_feedback() {
    let (provider, recorder) = recording_provider(vec![
        scripted_resp("bad answer [REGEN]"),
        scripted_resp("good answer"),
    ]);
    let lp = AgentLoop::new(provider, test_config());
    lp.add_llm_hook(Arc::new(RetryOnMarkerHook {
        marker: "[REGEN]".to_string(),
    }));

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "Hi", &test_context()).await;

    assert_eq!(first_done(&events), "good answer");
    let calls = recorder.calls();
    assert_eq!(calls.len(), 2, "retry must re-call the LLM once");
    let second = &calls[1];
    assert!(
        second
            .iter()
            .any(|m| m.role == "system" && m.content.starts_with("# Hook feedback:")),
        "retry call must carry the hook feedback message"
    );
}

/// Acceptance ② (replace): the downstream (Done event) sees the replaced
/// response, not the original.
#[tokio::test]
async fn llm_post_replace_downstream() {
    let (provider, recorder) = recording_provider(vec![scripted_resp("original")]);
    let lp = AgentLoop::new(provider, test_config());
    lp.add_llm_hook(Arc::new(ReplaceResponseHook {
        to: "sanitized answer".to_string(),
    }));

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "Hi", &test_context()).await;

    assert_eq!(first_done(&events), "sanitized answer");
    assert_eq!(recorder.calls().len(), 1);
}

/// Acceptance ② (block): a response blocker terminates the turn with the
/// reason surfaced to the user.
#[tokio::test]
async fn llm_post_block_terminates_turn() {
    let (provider, _recorder) = recording_provider(vec![scripted_resp("original")]);
    let lp = AgentLoop::new(provider, test_config());
    lp.add_llm_hook(Arc::new(BlockResponseHook {
        reason: "unsafe conclusion".to_string(),
    }));

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "Hi", &test_context()).await;

    let done = first_done(&events);
    assert!(done.contains("⛔ HOOK BLOCKED"), "done={done}");
    assert!(done.contains("unsafe conclusion"), "done={done}");
}

/// Retry budget: an always-retry hook gets exactly MAX_LLM_HOOK_RETRIES
/// re-calls; after the budget is spent the LAST obtained response is allowed
/// through (fail-open — a buggy hook must not deadlock the turn).
#[tokio::test]
async fn llm_retry_budget_exhausted_fail_open() {
    let (provider, recorder) = recording_provider(vec![
        scripted_resp("r1"),
        scripted_resp("r2"),
        scripted_resp("r3"),
    ]);
    let lp = AgentLoop::new(provider, test_config());
    lp.add_llm_hook(Arc::new(AlwaysRetryHook));

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "Hi", &test_context()).await;

    assert_eq!(
        recorder.calls().len(),
        1 + MAX_LLM_HOOK_RETRIES as usize,
        "initial call + budgeted retries, no more"
    );
    assert_eq!(
        first_done(&events),
        "r3",
        "last obtained response is allowed"
    );
}

/// Acceptance ③: no hooks registered → exactly one provider call, and the
/// message stream carries no hook artifacts (byte-compat: the hook block is
/// skipped entirely when the chain is empty).
#[tokio::test]
async fn llm_no_hooks_single_unchanged_call() {
    let (provider, recorder) = recording_provider(vec![scripted_resp("plain")]);
    let lp = AgentLoop::new(provider, test_config());

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "Hi", &test_context()).await;

    assert_eq!(first_done(&events), "plain");
    let calls = recorder.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        !calls[0]
            .iter()
            .any(|m| m.content.contains("Hook feedback") || m.content.contains("Reminder:")),
        "no hook artifacts may appear without registered hooks"
    );
}

// ---------------------------------------------------------------------------
// K1b lifecycle entry points: run_user_prompt_hooks / run_turn_end_hooks
// (4b layer-1 gap fill — these two run_* fns had zero coverage while the
// LlmHook pre/post family above was fully tested).
// ---------------------------------------------------------------------------

use super::{
    HookPrompt, HookTurnEnd, LifecycleHook, PromptDecision, TurnEndDecision, run_turn_end_hooks,
    run_user_prompt_hooks,
};

struct PromptHook {
    name: &'static str,
    block: Option<String>,
    called: AtomicBool,
}

#[async_trait]
impl LifecycleHook for PromptHook {
    fn name(&self) -> String {
        self.name.to_string()
    }
    async fn on_user_prompt(&self, _p: &HookPrompt) -> PromptDecision {
        self.called.store(true, Ordering::SeqCst);
        match &self.block {
            Some(reason) => PromptDecision::Block {
                reason: reason.clone(),
            },
            None => PromptDecision::Allow,
        }
    }
}

struct TurnEndHook {
    name: &'static str,
    continue_feedback: Option<String>,
    called: AtomicBool,
}

#[async_trait]
impl LifecycleHook for TurnEndHook {
    fn name(&self) -> String {
        self.name.to_string()
    }
    async fn on_turn_end(&self, _e: &HookTurnEnd) -> TurnEndDecision {
        self.called.store(true, Ordering::SeqCst);
        match &self.continue_feedback {
            Some(feedback) => TurnEndDecision::Continue {
                feedback: feedback.clone(),
            },
            None => TurnEndDecision::Stop,
        }
    }
}

fn sample_prompt() -> HookPrompt {
    HookPrompt {
        session_key: "web:u1:c1".to_string(),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        prompt: "hello".to_string(),
    }
}

fn sample_turn_end() -> HookTurnEnd {
    HookTurnEnd {
        session_key: "web:u1:c1".to_string(),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        final_content: "done".to_string(),
        stop_hook_active: false,
    }
}

#[tokio::test]
async fn user_prompt_hooks_all_allow_returns_none_and_runs_all() {
    let h1 = Arc::new(PromptHook {
        name: "p1",
        block: None,
        called: AtomicBool::new(false),
    });
    let h2 = Arc::new(PromptHook {
        name: "p2",
        block: None,
        called: AtomicBool::new(false),
    });
    let hooks: Vec<Arc<dyn LifecycleHook>> = vec![h1.clone(), h2.clone()];
    let verdict = run_user_prompt_hooks(&hooks, &sample_prompt()).await;
    assert_eq!(verdict, None, "all-allow must yield None (proceed)");
    assert!(h1.called.load(Ordering::SeqCst) && h2.called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn user_prompt_hooks_first_block_short_circuits() {
    let blocker = Arc::new(PromptHook {
        name: "blocker",
        block: Some("injected prompt".to_string()),
        called: AtomicBool::new(false),
    });
    let after = Arc::new(PromptHook {
        name: "after",
        block: None,
        called: AtomicBool::new(false),
    });
    let hooks: Vec<Arc<dyn LifecycleHook>> = vec![blocker.clone(), after.clone()];
    let verdict = run_user_prompt_hooks(&hooks, &sample_prompt()).await;
    assert_eq!(verdict.as_deref(), Some("injected prompt"));
    assert!(blocker.called.load(Ordering::SeqCst));
    assert!(
        !after.called.load(Ordering::SeqCst),
        "first Block must short-circuit; later hooks must not run"
    );
}

#[tokio::test]
async fn turn_end_hooks_all_stop_or_empty_yields_stop() {
    // Empty list → Stop (no hook demands more).
    let empty: Vec<Arc<dyn LifecycleHook>> = vec![];
    assert!(matches!(
        run_turn_end_hooks(&empty, &sample_turn_end()).await,
        TurnEndDecision::Stop
    ));
    // All Stop → Stop, every hook consulted.
    let h1 = Arc::new(TurnEndHook {
        name: "t1",
        continue_feedback: None,
        called: AtomicBool::new(false),
    });
    let h2 = Arc::new(TurnEndHook {
        name: "t2",
        continue_feedback: None,
        called: AtomicBool::new(false),
    });
    let hooks: Vec<Arc<dyn LifecycleHook>> = vec![h1.clone(), h2.clone()];
    assert!(matches!(
        run_turn_end_hooks(&hooks, &sample_turn_end()).await,
        TurnEndDecision::Stop
    ));
    assert!(h1.called.load(Ordering::SeqCst) && h2.called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn turn_end_hooks_first_continue_short_circuits_with_feedback() {
    let veto = Arc::new(TurnEndHook {
        name: "veto",
        continue_feedback: Some("please verify the fix".to_string()),
        called: AtomicBool::new(false),
    });
    let after = Arc::new(TurnEndHook {
        name: "after",
        continue_feedback: None,
        called: AtomicBool::new(false),
    });
    let hooks: Vec<Arc<dyn LifecycleHook>> = vec![veto.clone(), after.clone()];
    match run_turn_end_hooks(&hooks, &sample_turn_end()).await {
        TurnEndDecision::Continue { feedback } => {
            assert_eq!(feedback, "please verify the fix");
        }
        other => panic!("expected Continue, got {other:?}"),
    }
    assert!(veto.called.load(Ordering::SeqCst));
    assert!(
        !after.called.load(Ordering::SeqCst),
        "first Continue must short-circuit; later hooks must not run"
    );
}

// ---------------------------------------------------------------------------
// W3a: LlmHook / LifecycleHook 默认实现 + 两个 manager 的 len/is_empty
// ---------------------------------------------------------------------------

/// 什么都不覆盖的空实现：三个 trait 的默认方法都必须是「无害放行」。
#[tokio::test]
async fn llm_and_lifecycle_defaults_are_permissive() {
    struct BareAll;
    #[async_trait]
    impl ToolHook for BareAll {}
    #[async_trait]
    impl LlmHook for BareAll {}
    #[async_trait]
    impl crate::hooks::LifecycleHook for BareAll {}

    let b = BareAll;
    // LlmHook 默认 name / pre / post。
    assert_eq!(LlmHook::name(&b), "unnamed-llm-hook");
    let call = HookLlmCall {
        model: "m".to_string(),
        session_key: "s".to_string(),
        round: 1,
    };
    let resp = scripted_resp("ok");
    match b.pre_llm_call(&call, &[]).await {
        LlmRequestDecision::Proceed => {}
        other => panic!("default pre_llm_call must Proceed, got {other:?}"),
    }
    match b.post_llm_call(&call, &resp).await {
        LlmResponseDecision::Allow => {}
        other => panic!("default post_llm_call must Allow, got {other:?}"),
    }

    // LifecycleHook 默认 name / on_user_prompt / on_turn_end。
    assert_eq!(
        crate::hooks::LifecycleHook::name(&b),
        "unnamed-lifecycle-hook"
    );
    let prompt = crate::hooks::HookPrompt {
        session_key: "s".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        prompt: "hi".to_string(),
    };
    assert_eq!(
        b.on_user_prompt(&prompt).await,
        crate::hooks::PromptDecision::Allow
    );
    let end = crate::hooks::HookTurnEnd {
        session_key: "s".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        final_content: "done".to_string(),
        stop_hook_active: false,
    };
    assert_eq!(
        b.on_turn_end(&end).await,
        crate::hooks::TurnEndDecision::Stop
    );
}

/// LlmHookManager / LifecycleHookManager 的 add/len/is_empty/snapshot。
#[test]
fn llm_and_lifecycle_managers_add_len_snapshot() {
    struct BareLlm;
    #[async_trait]
    impl LlmHook for BareLlm {}
    struct BareLife;
    #[async_trait]
    impl crate::hooks::LifecycleHook for BareLife {}

    let mut lm = LlmHookManager::new();
    assert!(lm.is_empty());
    lm.add(std::sync::Arc::new(BareLlm));
    assert_eq!(lm.len(), 1);
    assert!(!lm.is_empty());
    assert_eq!(lm.snapshot().len(), 1);

    let mut lc = crate::hooks::LifecycleHookManager::new();
    assert!(lc.is_empty());
    lc.add(std::sync::Arc::new(BareLife));
    assert_eq!(lc.len(), 1);
    assert!(!lc.is_empty());
    assert_eq!(lc.snapshot().len(), 1);
}

/// run_user_prompt_hooks / run_turn_end_hooks 空钩子链：放行（None / Stop）。
#[tokio::test]
async fn user_prompt_and_turn_end_empty_chains_pass() {
    let prompt = crate::hooks::HookPrompt {
        session_key: "s".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        prompt: "hi".to_string(),
    };
    let end = crate::hooks::HookTurnEnd {
        session_key: "s".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        final_content: "done".to_string(),
        stop_hook_active: false,
    };
    assert!(
        crate::hooks::run_user_prompt_hooks(&[], &prompt)
            .await
            .is_none()
    );
    assert_eq!(
        crate::hooks::run_turn_end_hooks(&[], &end).await,
        crate::hooks::TurnEndDecision::Stop
    );
}
