//! S9 覆盖率批次：hooks.rs 剩余未覆盖行——全部是 tracing 宏参数表达式行
//! （177-180/200/202-203/330-331/339/373/381/390/523/525-526/547/549）。
//! 无 subscriber 时这些行不求值；`capture_logs()` 装上后跑各 dispatch 路径。

use super::*;
use crate::test_support::capture_logs;
use async_trait::async_trait;

// ---------- tool hooks ----------

struct BlockPreHook;
#[async_trait]
impl ToolHook for BlockPreHook {
    fn name(&self) -> String {
        "s9-block-pre".to_string()
    }
    async fn pre_tool_use(&self, _call: &HookToolCall) -> HookDecision {
        HookDecision::Block {
            reason: "policy says no".to_string(),
        }
    }
}

struct ReplacePostHook;
#[async_trait]
impl ToolHook for ReplacePostHook {
    fn name(&self) -> String {
        "s9-replace-post".to_string()
    }
    async fn post_tool_use(&self, _call: &HookToolCall, _result: &str) -> PostHookAction {
        PostHookAction::Replace("sanitized".to_string())
    }
}

fn tool_call() -> HookToolCall {
    HookToolCall {
        name: "exec".to_string(),
        arguments: "{}".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: "web:s9".to_string(),
    }
}

#[tokio::test]
async fn pre_hook_block_logs_warn_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn ToolHook>> = vec![Arc::new(BlockPreHook)];
    let out = run_pre_hooks(&hooks, &tool_call()).await;
    assert_eq!(out.as_deref(), Some("policy says no"));
}

#[tokio::test]
async fn post_hook_replace_logs_info_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn ToolHook>> = vec![Arc::new(ReplacePostHook)];
    let out = run_post_hooks(&hooks, &tool_call(), "raw secret".to_string()).await;
    assert_eq!(out, "sanitized");
}

// ---------- llm hooks ----------

struct AppendLlmHook;
#[async_trait]
impl LlmHook for AppendLlmHook {
    fn name(&self) -> String {
        "s9-append-llm".to_string()
    }
    async fn pre_llm_call(
        &self,
        _call: &HookLlmCall,
        _messages: &[LlmMessage],
    ) -> LlmRequestDecision {
        LlmRequestDecision::Append(vec![LlmMessage {
            role: "system".to_string(),
            content: "reminder".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }])
    }
}

struct BlockLlmHook;
#[async_trait]
impl LlmHook for BlockLlmHook {
    fn name(&self) -> String {
        "s9-block-llm".to_string()
    }
    async fn pre_llm_call(
        &self,
        _call: &HookLlmCall,
        _messages: &[LlmMessage],
    ) -> LlmRequestDecision {
        LlmRequestDecision::Block {
            reason: "stop".to_string(),
        }
    }
}

struct RetryLlmHook;
#[async_trait]
impl LlmHook for RetryLlmHook {
    fn name(&self) -> String {
        "s9-retry-llm".to_string()
    }
    async fn post_llm_call(&self, _call: &HookLlmCall, _resp: &LlmResponse) -> LlmResponseDecision {
        LlmResponseDecision::Retry {
            reason: "redo".to_string(),
        }
    }
}

struct BlockPostLlmHook;
#[async_trait]
impl LlmHook for BlockPostLlmHook {
    fn name(&self) -> String {
        "s9-block-post-llm".to_string()
    }
    async fn post_llm_call(&self, _call: &HookLlmCall, _resp: &LlmResponse) -> LlmResponseDecision {
        LlmResponseDecision::Block {
            reason: "abort".to_string(),
        }
    }
}

struct ReplaceLlmHook;
#[async_trait]
impl LlmHook for ReplaceLlmHook {
    fn name(&self) -> String {
        "s9-replace-llm".to_string()
    }
    async fn post_llm_call(&self, _call: &HookLlmCall, _resp: &LlmResponse) -> LlmResponseDecision {
        LlmResponseDecision::Replace(empty_response("replaced"))
    }
}

fn empty_response(content: &str) -> LlmResponse {
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

fn llm_call() -> HookLlmCall {
    HookLlmCall {
        model: "test-model".to_string(),
        session_key: "web:s9".to_string(),
        round: 2,
    }
}

#[tokio::test]
async fn llm_hook_append_logs_info_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn LlmHook>> = vec![Arc::new(AppendLlmHook)];
    let out = run_llm_pre_hooks(&hooks, &llm_call(), &[]).await.unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].content, "reminder");
}

#[tokio::test]
async fn llm_hook_block_logs_warn_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn LlmHook>> = vec![Arc::new(BlockLlmHook)];
    let out = run_llm_pre_hooks(&hooks, &llm_call(), &[]).await;
    assert_eq!(out.unwrap_err(), "stop");
}

#[tokio::test]
async fn llm_hook_post_replace_logs_info_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn LlmHook>> = vec![Arc::new(ReplaceLlmHook)];
    let out = run_llm_post_hooks(&hooks, &llm_call(), empty_response("orig")).await;
    match out {
        PostLlmOutcome::Allow(r) => assert_eq!(r.content, "replaced"),
        other => panic!("expected Allow, got {:?}", other),
    }
}

#[tokio::test]
async fn llm_hook_post_retry_logs_warn_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn LlmHook>> = vec![Arc::new(RetryLlmHook)];
    let out = run_llm_post_hooks(&hooks, &llm_call(), empty_response("orig")).await;
    assert!(matches!(out, PostLlmOutcome::Retry { .. }));
}

#[tokio::test]
async fn llm_hook_post_block_logs_warn_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn LlmHook>> = vec![Arc::new(BlockPostLlmHook)];
    let out = run_llm_post_hooks(&hooks, &llm_call(), empty_response("orig")).await;
    assert!(matches!(out, PostLlmOutcome::Block { .. }));
}

// ---------- lifecycle hooks ----------

struct BlockPromptHook;
#[async_trait]
impl LifecycleHook for BlockPromptHook {
    fn name(&self) -> String {
        "s9-block-prompt".to_string()
    }
    async fn on_user_prompt(&self, _prompt: &HookPrompt) -> PromptDecision {
        PromptDecision::Block {
            reason: "not now".to_string(),
        }
    }
}

struct ContinueTurnEndHook;
#[async_trait]
impl LifecycleHook for ContinueTurnEndHook {
    fn name(&self) -> String {
        "s9-continue-end".to_string()
    }
    async fn on_turn_end(&self, _end: &HookTurnEnd) -> TurnEndDecision {
        TurnEndDecision::Continue {
            feedback: "keep going".to_string(),
        }
    }
}

#[tokio::test]
async fn prompt_hook_block_logs_warn_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn LifecycleHook>> = vec![Arc::new(BlockPromptHook)];
    let prompt = HookPrompt {
        session_key: "web:s9".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        prompt: "hello".to_string(),
    };
    let out = run_user_prompt_hooks(&hooks, &prompt).await;
    assert_eq!(out.as_deref(), Some("not now"));
}

#[tokio::test]
async fn turn_end_hook_continue_logs_warn_fields() {
    let _logs = capture_logs();
    let hooks: Vec<Arc<dyn LifecycleHook>> = vec![Arc::new(ContinueTurnEndHook)];
    let end = HookTurnEnd {
        session_key: "web:s9".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        final_content: "done".to_string(),
        stop_hook_active: false,
    };
    let out = run_turn_end_hooks(&hooks, &end).await;
    match out {
        TurnEndDecision::Continue { feedback } => assert_eq!(feedback, "keep going"),
        other => panic!("expected Continue, got {:?}", other),
    }
}
