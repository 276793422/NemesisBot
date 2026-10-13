//! M4-B（A13）：空内容撞 max_tokens cap 的提前停轮守卫测试。
//!
//! 器官 6（[`AgentLoop::handle_length_continuation`]）直测：空产出撞 cap
//! ≠ 大文件被截断，继续喂 continue 提示只会每轮烧一次全量 prefill。守卫
//! 语义：第 1 次按截断续写放行（偶发瞬态），连续第 2 次提前 Stop（诚实
//! 报错）；非空内容撞 cap 与正常完成（below-cap）都清零空产出计数。

use super::*;
use crate::loop_executor::ObserverUsageInfo;
use crate::{ChatOptions, ToolDefinition};

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

/// 空 provider——器官 6 直测不发 LLM 请求，占位即可。
struct NoopProvider;
#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("noop".to_string())
    }
}

/// completion_tokens 恰好顶到默认 max_tokens cap（8192）——截断信号。
fn cap_usage() -> Option<ObserverUsageInfo> {
    Some(ObserverUsageInfo {
        prompt_tokens: 100,
        completion_tokens: 8192,
        total_tokens: 8292,
        cached_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
    })
}

fn below_cap_usage() -> Option<ObserverUsageInfo> {
    Some(ObserverUsageInfo {
        prompt_tokens: 100,
        completion_tokens: 10,
        total_tokens: 110,
        cached_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
    })
}

fn empty_at_cap() -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: String::new(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: cap_usage(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn text_at_cap(text: &str) -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: text.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: cap_usage(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn text_below_cap(text: &str) -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: text.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: below_cap_usage(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn fresh_state() -> TurnState {
    TurnState {
        turns_used: 0,
        validation_failures: 0,
        length_continuations: 0,
        empty_length_continuations: 0,
        grace_round: false,
        turn_guard: crate::turn_guard::TurnGuard::new(),
        degenerate_nudge_pending: None,
        repetition_nudge_pending: None,
        steer_escape_used: false,
        terminal_reason: None,
        turn_end_continues: 0,
        canvas_retries: 0,
        log_boundaries: false,
        chat_opts: crate::types::ChatOptions {
            max_tokens: Some(8192),
            ..Default::default()
        },
    }
}

/// 第 1 次空产出撞 cap：按偶发瞬态放行截断续写（不是停轮）。
#[test]
fn empty_at_cap_first_round_continues() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    let flow = agent_loop.handle_length_continuation(&instance, &context, &empty_at_cap(), &mut st);

    assert!(matches!(flow, Some(TurnFlow::Continue)));
    assert_eq!(st.empty_length_continuations, 1);
    // 走的是既有截断续写分支（continue 提示已追加）。
    assert_eq!(st.length_continuations, 1);
}

/// 连续第 2 次空产出撞 cap：提前 Stop（诚实报错），不再烧剩余续写预算。
#[test]
fn empty_at_cap_second_round_stops_with_honest_error() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    let _ = agent_loop.handle_length_continuation(&instance, &context, &empty_at_cap(), &mut st);
    let flow = agent_loop.handle_length_continuation(&instance, &context, &empty_at_cap(), &mut st);

    match flow {
        Some(TurnFlow::Stop(AgentEvent::Error(msg))) => {
            assert!(msg.contains("空输出"), "报错应点明空输出：{msg}");
            assert!(msg.contains("提前停止"), "报错应说明提前停轮：{msg}");
        }
        other => panic!(
            "expected Stop(Error), got {}",
            match other {
                Some(TurnFlow::Continue) => "Continue".to_string(),
                Some(TurnFlow::Stop(_)) => "Stop(non-Error)".to_string(),
                None => "None".to_string(),
            }
        ),
    }
    assert_eq!(st.empty_length_continuations, 2);
    // 提前停轮：5 格续写预算仍有余量（不是烧穿 length-continuation）。
    assert!(st.length_continuations < MAX_LENGTH_CONTINUATIONS);
}

/// 非空内容撞 cap：正常截断续写，且空产出计数清零（瞬态已自愈）。
#[test]
fn nonempty_at_cap_resets_empty_counter() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    let _ = agent_loop.handle_length_continuation(&instance, &context, &empty_at_cap(), &mut st);
    let flow = agent_loop.handle_length_continuation(
        &instance,
        &context,
        &text_at_cap("partial file content that got cut"),
        &mut st,
    );

    assert!(matches!(flow, Some(TurnFlow::Continue)));
    assert_eq!(st.empty_length_continuations, 0);
    assert_eq!(st.length_continuations, 2);
}

/// 正常完成（below-cap）：两个计数器都清零。
#[test]
fn normal_completion_resets_both_counters() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    let _ = agent_loop.handle_length_continuation(&instance, &context, &empty_at_cap(), &mut st);
    let flow = agent_loop.handle_length_continuation(
        &instance,
        &context,
        &text_below_cap("all done, final answer"),
        &mut st,
    );

    assert!(flow.is_none());
    assert_eq!(st.length_continuations, 0);
    assert_eq!(st.empty_length_continuations, 0);
}
