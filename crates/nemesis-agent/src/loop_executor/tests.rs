//! `loop_executor` 存活类型测试（2026-09-24 自文件内 `#[cfg(test)]` 迁出——
//! a5a6e746 折叠的 11 用例原样搬移，纪律门 `check-inline-tests.sh` 要求测试
//! 独立成文件；生产文件只留 `#[cfg(test)] mod tests;` 声明）。
//!
//! 覆盖：`ToolResult` 构造家族 6 例 + `ObserverEvent` → conversation 事件
//! 映射 5 例。

use super::*;

#[test]
fn test_tool_result_simple() {
    let result = ToolResult::simple("hello".to_string());
    assert_eq!(result.for_llm, "hello");
    assert!(result.for_user.is_empty());
    assert!(result.silent);
    assert!(!result.is_async);
    assert!(result.err.is_none());
}

#[test]
fn test_tool_result_for_llm_only() {
    let result = ToolResult::for_llm_only("internal data".to_string());
    assert_eq!(result.for_llm, "internal data");
    assert!(result.for_user.is_empty());
    assert!(result.silent);
}

#[test]
fn test_tool_result_async() {
    let result = ToolResult::async_result(
        "task-123".to_string(),
        "Processing your request...".to_string(),
    );
    assert!(result.is_async);
    assert_eq!(result.task_id, "task-123");
    assert_eq!(result.for_user, "Processing your request...");
    assert!(!result.silent);
}

#[test]
fn test_tool_result_error() {
    let result = ToolResult::error("Something went wrong".to_string());
    assert!(result.err.is_some());
    assert!(result.for_llm.contains("Something went wrong"));
}

#[test]
fn test_tool_result_default() {
    let result = ToolResult::default();
    assert!(result.for_llm.is_empty());
    assert!(result.for_user.is_empty());
    assert!(!result.is_async);
    assert!(result.task_id.is_empty());
    assert!(result.silent);
    assert!(result.err.is_none());
}

#[test]
fn test_tool_result_from_async_extra() {
    let result = ToolResult::async_result("task-42".to_string(), "waiting...".to_string());
    assert!(result.is_async);
    assert_eq!(result.task_id, "task-42");
    assert_eq!(result.for_user, "waiting...");
}

#[test]
fn test_observer_event_conversation_start() {
    let event = ObserverEvent::ConversationStart {
        trace_id: "t1".to_string(),
        session_key: "s1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        sender_id: "user1".to_string(),
        content: "hello".to_string(),
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(
        conv_event.event_type,
        nemesis_observer::EventType::ConversationStart
    );
}

#[test]
fn test_observer_event_conversation_end() {
    let event = ObserverEvent::ConversationEnd {
        trace_id: "t1".to_string(),
        session_key: "s1".to_string(),
        total_rounds: 3,
        duration_ms: 1500,
        content: "response".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(
        conv_event.event_type,
        nemesis_observer::EventType::ConversationEnd
    );
}

#[test]
fn test_observer_event_llm_request() {
    let event = ObserverEvent::LlmRequest {
        trace_id: "t1".to_string(),
        round: 1,
        model: "gpt-4".to_string(),
        messages: vec![],
        tools: vec![],
        messages_count: 0,
        tools_count: 0,
        provider_name: String::new(),
        api_key: String::new(),
        api_base: String::new(),
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(
        conv_event.event_type,
        nemesis_observer::EventType::LlmRequest
    );
}

#[test]
fn test_observer_event_llm_response() {
    let event = ObserverEvent::LlmResponse {
        trace_id: "t1".to_string(),
        round: 1,
        duration_ms: 200,
        has_tool_calls: true,
        content: "response text".to_string(),
        tool_calls: vec![],
        tool_calls_count: 0,
        finish_reason: Some("stop".to_string()),
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(
        conv_event.event_type,
        nemesis_observer::EventType::LlmResponse
    );
}

#[test]
fn test_observer_event_tool_call() {
    let event = ObserverEvent::ToolCall {
        trace_id: "t1".to_string(),
        tool_name: "search".to_string(),
        success: true,
        duration_ms: 50,
        round: 1,
        arguments: "{}".to_string(),
        result: "ok".to_string(),
    };
    let conv_event = event.to_conversation_event();
    assert_eq!(conv_event.event_type, nemesis_observer::EventType::ToolCall);
}
