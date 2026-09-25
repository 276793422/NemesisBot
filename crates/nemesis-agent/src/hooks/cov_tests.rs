// hooks.rs 覆盖率补充测试（post-failure 管道 / 默认委托 / Replace·Append·
// Block 日志臂 / lifecycle Block·Continue 臂 / MetricsPipelinePlugin 计时
// 包装与 Default）。
//
// 现有 tests/s9_tests 走 handle_tool_call 主路径；本文件直接驱动 runner
// 纯函数，钉住各日志臂与 fail-open 语义。

use std::sync::Arc;

use async_trait::async_trait;

use super::{
    HookDecision, HookLlmCall, HookPrompt, HookToolCall, HookTurnEnd, LlmHook, LlmRequestDecision,
    LlmResponseDecision, MetricsPipelinePlugin, PostHookAction, PostLlmOutcome, ToolHook,
    run_llm_post_hooks, run_llm_pre_hooks, run_post_failure_hooks, run_post_hooks, run_pre_hooks,
    run_turn_end_hooks, run_user_prompt_hooks,
};
use crate::r#loop::{LlmMessage, LlmResponse};

fn call() -> HookToolCall {
    HookToolCall {
        name: "exec".to_string(),
        arguments: "{}".to_string(),
        channel: "web".to_string(),
        chat_id: "cov".to_string(),
        session_key: "agent:main:session:covhooks".to_string(),
    }
}

fn llm_response() -> LlmResponse {
    LlmResponse {
        content: "cov-answer".to_string(),
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
        session_key: "agent:main:session:covhooks".to_string(),
        round: 1,
    }
}

// ---------------------------------------------------------------------------
// ToolHook fixtures
// ---------------------------------------------------------------------------

/// 默认实现钩（不 override 任何方法）——走 post_tool_use_failure 默认委托。
struct PlainHook;

#[async_trait]
impl ToolHook for PlainHook {
    fn name(&self) -> String {
        "plain".to_string()
    }
}

/// post 失败变体：Replace（触发 run_post_failure_hooks 的替换臂）。
struct ReplaceOnFailure {
    body: String,
}

#[async_trait]
impl ToolHook for ReplaceOnFailure {
    fn name(&self) -> String {
        "replace-on-failure".to_string()
    }
    async fn post_tool_use_failure(&self, _call: &HookToolCall, err: &str) -> PostHookAction {
        let body = &self.body;
        PostHookAction::Replace(format!("[{body}] orig={err}"))
    }
}

struct BlockPre {
    reason: &'static str,
}

#[async_trait]
impl ToolHook for BlockPre {
    fn name(&self) -> String {
        "block-pre".to_string()
    }
    async fn pre_tool_use(&self, _call: &HookToolCall) -> HookDecision {
        HookDecision::Block {
            reason: self.reason.to_string(),
        }
    }
}

struct ReplacePost;

#[async_trait]
impl ToolHook for ReplacePost {
    fn name(&self) -> String {
        "replace-post".to_string()
    }
    async fn post_tool_use(&self, _call: &HookToolCall, result: &str) -> PostHookAction {
        PostHookAction::Replace(format!("{result}!"))
    }
}

// ---------------------------------------------------------------------------
// post_tool_use_failure 默认委托 + run_post_failure_hooks 管道
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_failure_default_delegates_and_replace_pipeline() {
    // 默认委托：错误文本包成 "Tool error: {err}" 走 post_tool_use。
    let err = run_post_failure_hooks(&[Arc::new(PlainHook)], &call(), "boom").await;
    assert!(err.contains("Tool error: boom"), "err: {err}");

    // Replace 臂：注意失败变体每个钩子收到的是**原始 err**（非前钩输出，
    // 与 run_post_hooks 的链式语义不同——`err` 参数不随 current 传递）。
    let replaced = run_post_failure_hooks(
        &[
            Arc::new(ReplaceOnFailure {
                body: "first".to_string(),
            }),
            Arc::new(ReplaceOnFailure {
                body: "second".to_string(),
            }),
        ],
        &call(),
        "boom",
    )
    .await;
    assert_eq!(replaced, "[second] orig=boom");
}

#[tokio::test]
async fn pre_block_and_post_replace_arms() {
    let reason = run_pre_hooks(&[Arc::new(BlockPre { reason: "no exec" })], &call())
        .await
        .expect("first block wins");
    assert_eq!(reason, "no exec");
    assert!(run_pre_hooks(&[], &call()).await.is_none());

    let out = run_post_hooks(&[Arc::new(ReplacePost)], &call(), "base".to_string()).await;
    assert_eq!(out, "base!");
}

// ---------------------------------------------------------------------------
// LLM 钩子：Append / Block / Replace / Retry / Block
// ---------------------------------------------------------------------------

struct AppendPre(Vec<LlmMessage>);

#[async_trait]
impl LlmHook for AppendPre {
    fn name(&self) -> String {
        "append-pre".to_string()
    }
    async fn pre_llm_call(
        &self,
        _call: &HookLlmCall,
        _messages: &[LlmMessage],
    ) -> LlmRequestDecision {
        LlmRequestDecision::Append(self.0.clone())
    }
}

struct BlockPreLlm;

#[async_trait]
impl LlmHook for BlockPreLlm {
    fn name(&self) -> String {
        "block-pre-llm".to_string()
    }
    async fn pre_llm_call(
        &self,
        _call: &HookLlmCall,
        _messages: &[LlmMessage],
    ) -> LlmRequestDecision {
        LlmRequestDecision::Block {
            reason: "budget exhausted".to_string(),
        }
    }
}

enum PostKind {
    Replace,
    Retry,
    Block,
}

struct PostLlm(PostKind);

#[async_trait]
impl LlmHook for PostLlm {
    fn name(&self) -> String {
        "post-llm".to_string()
    }
    async fn post_llm_call(
        &self,
        _call: &HookLlmCall,
        _response: &LlmResponse,
    ) -> LlmResponseDecision {
        match self.0 {
            PostKind::Replace => LlmResponseDecision::Replace(LlmResponse {
                content: "rewritten".to_string(),
                ..llm_response()
            }),
            PostKind::Retry => LlmResponseDecision::Retry {
                reason: "bad format".to_string(),
            },
            PostKind::Block => LlmResponseDecision::Block {
                reason: "policy violation".to_string(),
            },
        }
    }
}

#[tokio::test]
async fn llm_pre_hooks_append_then_block() {
    let nudge = LlmMessage {
        role: "user".to_string(),
        content: "stay disciplined".to_string(),
        tool_call_id: None,
        tool_calls: None,
        reasoning_content: None,
        images: Vec::new(),
    };
    let appended = run_llm_pre_hooks(
        &[Arc::new(AppendPre(vec![nudge.clone()]))],
        &llm_call(),
        &[],
    )
    .await
    .expect("append does not block");
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].content, "stay disciplined");

    // Block 短路：后续钩子不跑。
    let err = run_llm_pre_hooks(
        &[Arc::new(BlockPreLlm), Arc::new(AppendPre(vec![nudge]))],
        &llm_call(),
        &[],
    )
    .await
    .expect_err("block short-circuits");
    assert_eq!(err, "budget exhausted");
}

#[tokio::test]
async fn llm_post_hooks_replace_retry_block_arms() {
    let (c, hlc) = (llm_call(), llm_response());

    match run_llm_post_hooks(&[Arc::new(PostLlm(PostKind::Replace))], &c, hlc.clone()).await {
        PostLlmOutcome::Allow(r) => assert_eq!(r.content, "rewritten"),
        other => panic!("expected allow, got {other:?}"),
    }
    match run_llm_post_hooks(&[Arc::new(PostLlm(PostKind::Retry))], &c, hlc.clone()).await {
        PostLlmOutcome::Retry { reason } => assert_eq!(reason, "bad format"),
        other => panic!("expected retry, got {other:?}"),
    }
    match run_llm_post_hooks(&[Arc::new(PostLlm(PostKind::Block))], &c, hlc).await {
        PostLlmOutcome::Block { reason } => assert_eq!(reason, "policy violation"),
        other => panic!("expected block, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// lifecycle：user-prompt Block / turn-end Continue
// ---------------------------------------------------------------------------

struct PromptBlocker;

#[async_trait]
impl super::LifecycleHook for PromptBlocker {
    fn name(&self) -> String {
        "prompt-blocker".to_string()
    }
    async fn on_user_prompt(&self, _prompt: &HookPrompt) -> super::PromptDecision {
        super::PromptDecision::Block {
            reason: "friday freeze".to_string(),
        }
    }
}

struct TurnEndContinuer;

#[async_trait]
impl super::LifecycleHook for TurnEndContinuer {
    fn name(&self) -> String {
        "turn-end-continuer".to_string()
    }
    async fn on_turn_end(&self, _end: &HookTurnEnd) -> super::TurnEndDecision {
        super::TurnEndDecision::Continue {
            feedback: "finish the todo list first".to_string(),
        }
    }
}

#[tokio::test]
async fn prompt_block_and_turn_end_continue_arms() {
    let prompt = HookPrompt {
        session_key: "agent:main:session:covhooks".to_string(),
        channel: "web".to_string(),
        chat_id: "cov".to_string(),
        prompt: "hello".to_string(),
    };
    let reason = run_user_prompt_hooks(&[Arc::new(PromptBlocker)], &prompt)
        .await
        .expect("block surfaces reason");
    assert_eq!(reason, "friday freeze");

    let end = HookTurnEnd {
        session_key: "agent:main:session:covhooks".to_string(),
        channel: "web".to_string(),
        chat_id: "cov".to_string(),
        final_content: "done".to_string(),
        stop_hook_active: false,
    };
    match run_turn_end_hooks(&[Arc::new(TurnEndContinuer)], &end).await {
        super::TurnEndDecision::Continue { feedback } => {
            assert_eq!(feedback, "finish the todo list first")
        }
        other => panic!("expected continue, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// MetricsPipelinePlugin：Default + around 计时（enabled / disabled）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn metrics_pipeline_around_wraps_and_passes_through() {
    let enabled = MetricsPipelinePlugin::default();
    assert!(enabled.is_enabled());

    let next = Arc::new(|call: HookToolCall| {
        Box::pin(async move { format!("ran:{}", call.name) })
            as std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
    });

    let out = enabled.around_tool_use(call(), next.clone()).await;
    assert_eq!(out, "ran:exec");

    enabled.set_enabled(false);
    assert!(!enabled.is_enabled());
    let out = enabled.around_tool_use(call(), next).await;
    assert_eq!(out, "ran:exec");
}
