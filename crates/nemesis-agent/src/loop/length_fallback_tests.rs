//! B1（2026-10-18 B 级清零）：无 usage 时 finish_reason 截断回退测试。
//!
//! 器官 6（[`AgentLoop::handle_length_continuation`]）的截断判定此前只认
//! usage 数值（completion_tokens ≥ cap）；上游剥离 usage 的代理/流形态下
//! 截断响应被直接放行给用户（半截答案无提示）。补回落：usage 缺失时以
//! `finish_reason == "length"`（provider 权威截断信号）判定；有 usage 时
//! 数值优先（finish_reason 不覆盖数值）；CLI lane 恒 "stop"/"tool_calls"
//! （子进程吞截断信息），回落对它们诚实无效。

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
        tool_fence_profession: None,
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

fn usage_at(cap: i64) -> Option<ObserverUsageInfo> {
    Some(ObserverUsageInfo {
        prompt_tokens: 100,
        completion_tokens: cap,
        total_tokens: 100 + cap,
        cached_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
    })
}

fn response(
    finish_reason: Option<&str>,
    content: &str,
    usage: Option<ObserverUsageInfo>,
) -> LlmResponse {
    LlmResponse {
        finish_reason: finish_reason.map(|s| s.to_string()),
        escalated_to: None,
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage,
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

/// usage 缺失 + finish_reason="length" + 有正文 → 回落判定命中，续写
/// （continue 提示已追加，计数进位）。
#[test]
fn no_usage_length_finish_reason_triggers_continuation() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    let resp = response(Some("length"), "partial answer that got cut", None);
    let flow = agent_loop.handle_length_continuation(&instance, &context, &resp, &mut st);

    assert!(
        matches!(flow, Some(TurnFlow::Continue)),
        "截断经 finish_reason 回落被识别"
    );
    assert_eq!(st.length_continuations, 1, "续写预算已进位");
    // 半截正文已前置进历史（续行语义的消息结构承载）。
    let history = instance.get_history();
    assert!(
        history
            .iter()
            .any(|t| t.role == "assistant" && t.content.contains("partial answer")),
        "截断 assistant 前置进历史"
    );
}

/// usage 缺失 + finish_reason="stop" → 完整响应，不触发（回落不越权）。
#[test]
fn no_usage_stop_finish_reason_not_triggered() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    let resp = response(Some("stop"), "complete answer", None);
    let flow = agent_loop.handle_length_continuation(&instance, &context, &resp, &mut st);

    assert!(flow.is_none(), "stop 不是截断信号");
    assert_eq!(st.length_continuations, 0);
}

/// usage 在场时数值优先：completion < cap 但 finish_reason="length" → 不
/// 触发（数值判定为准，finish_reason 不覆盖数值——两信号冲突时信数值）。
#[test]
fn usage_present_numeric_wins_over_length_finish_reason() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    let resp = response(Some("length"), "short answer", usage_at(10));
    let flow = agent_loop.handle_length_continuation(&instance, &context, &resp, &mut st);

    assert!(flow.is_none(), "completion 10 < cap 8192，数值判定未截断");
    assert_eq!(st.length_continuations, 0);
}

/// usage 在场 + 数值顶 cap → 既有数值通路不受回落影响（回归锁）。
#[test]
fn usage_at_cap_still_continues() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    let resp = response(Some("stop"), "truncated content", usage_at(8192));
    let flow = agent_loop.handle_length_continuation(&instance, &context, &resp, &mut st);

    assert!(
        matches!(flow, Some(TurnFlow::Continue)),
        "数值顶 cap 照常续写"
    );
}

/// 回落通路同样受 MAX_LENGTH_CONTINUATIONS 封顶：连续 length（无 usage）
/// 第 6 次判定 → 预算耗尽诚实报错（不无限续写）。
#[test]
fn no_usage_fallback_respects_continuation_budget() {
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let mut st = fresh_state();

    for i in 1..=5 {
        let resp = response(Some("length"), &format!("chunk {i}"), None);
        let flow = agent_loop.handle_length_continuation(&instance, &context, &resp, &mut st);
        assert!(
            matches!(flow, Some(TurnFlow::Continue)),
            "第 {i} 次仍在续写预算内"
        );
    }

    let resp = response(Some("length"), "chunk 6", None);
    let flow = agent_loop.handle_length_continuation(&instance, &context, &resp, &mut st);
    match flow {
        Some(TurnFlow::Stop(AgentEvent::Error(msg))) => {
            assert!(
                msg.contains("max_tokens"),
                "预算耗尽报错应点名 max_tokens：{msg}"
            );
        }
        other => panic!(
            "expected Stop(Error), got {}",
            match other {
                Some(TurnFlow::Continue) => "Continue".to_string(),
                Some(TurnFlow::Stop(_)) => "Stop(other)".to_string(),
                None => "None".to_string(),
            }
        ),
    }
}
