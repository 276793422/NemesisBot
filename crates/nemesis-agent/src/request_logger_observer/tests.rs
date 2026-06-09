use super::*;
use tempfile::TempDir;

fn test_config() -> LoggingConfig {
    LoggingConfig {
        enabled: true,
        detail_level: crate::request_logger::DetailLevel::Full,
        log_dir: "logs/llm".to_string(),
        save_raw: false,
    }
}

fn make_start_event(trace: &str, content: &str) -> ConversationEvent {
    ConversationEvent {
        event_type: EventType::ConversationStart,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::ConversationStart(ConversationStartData {
            session_key: "test:chat1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            sender_id: "user1".to_string(),
            content: content.to_string(),
        }),
    }
}

fn make_llm_request_event(trace: &str, round: usize) -> ConversationEvent {
    ConversationEvent {
        event_type: EventType::LLMRequest,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::LLMRequest(LLMRequestEventData {
            round,
            model: "gpt-4".to_string(),
            provider_name: "openai".to_string(),
            api_key: "sk-test".to_string(),
            api_base: "https://api.openai.com".to_string(),
            messages_count: 5,
            tools_count: 3,
            messages: vec![
                serde_json::json!({"role": "system", "content": "You are helpful"}),
                serde_json::json!({"role": "user", "content": "Hello"}),
            ],
            tools: vec![
                serde_json::json!({"type": "function", "function": {"name": "test_tool"}}),
            ],
        }),
    }
}

fn make_llm_response_event(trace: &str, round: usize) -> ConversationEvent {
    ConversationEvent {
        event_type: EventType::LLMResponse,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::LLMResponse(LLMResponseEventData {
            round,
            duration_ms: 1500,
            content: "Hello!".to_string(),
            tool_calls_count: 0,
            finish_reason: "stop".to_string(),
            tool_calls: vec![],
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cached_tokens: None,
            raw_request_body: None,
            raw_response_body: None,
        }),
    }
}

fn make_tool_call_event(trace: &str, round: usize, tool: &str, success: bool) -> ConversationEvent {
    ConversationEvent {
        event_type: EventType::ToolCall,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::ToolCall(ToolCallEventData {
            tool_name: tool.to_string(),
            success,
            duration_ms: 100,
            error: if success { String::new() } else { "error".to_string() },
            llm_round: round,
            arguments: String::new(),
            result: String::new(),
        }),
    }
}

fn make_end_event(trace: &str, rounds: usize) -> ConversationEvent {
    ConversationEvent {
        event_type: EventType::ConversationEnd,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::ConversationEnd(ConversationEndData {
            session_key: "test:chat1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            total_rounds: rounds,
            total_duration_ms: 3000,
            content: "Final answer.".to_string(),
            is_error: false,
        }),
    }
}

#[test]
fn full_conversation_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    assert_eq!(observer.name(), "request_logger");

    // Start conversation
    observer.on_event(&make_start_event("trace-1", "Hello"));
    assert_eq!(observer.active_count(), 1);

    // LLM round
    observer.on_event(&make_llm_request_event("trace-1", 1));
    observer.on_event(&make_llm_response_event("trace-1", 1));

    // End conversation
    observer.on_event(&make_end_event("trace-1", 1));
    assert_eq!(observer.active_count(), 0);

    // Verify files were created in the session directory
    let log_dir = tmp.path().join("logs").join("llm");
    assert!(log_dir.exists());

    // There should be a session directory
    let session_dirs: Vec<_> = std::fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .collect();

    // At least one session dir
    assert!(!session_dirs.is_empty());
}

#[test]
fn tool_calls_are_logged_on_end() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    observer.on_event(&make_start_event("trace-2", "Do something"));

    // Round 1: tool call
    observer.on_event(&make_tool_call_event("trace-2", 1, "calculator", true));
    observer.on_event(&make_tool_call_event("trace-2", 1, "search", false));

    // End
    observer.on_event(&make_end_event("trace-2", 1));
    assert_eq!(observer.active_count(), 0);
}

#[test]
fn disabled_config_does_not_create_session() {
    let config = LoggingConfig {
        enabled: false,
        detail_level: crate::request_logger::DetailLevel::Full,
        log_dir: "logs/llm".to_string(),
        save_raw: false,
    };
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(config, tmp.path());

    observer.on_event(&make_start_event("trace-3", "Hello"));
    assert_eq!(observer.active_count(), 0); // Not tracked when disabled
}

#[test]
fn unknown_trace_ignored() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    // These events reference a trace that was never started
    observer.on_event(&make_llm_request_event("unknown-trace", 1));
    observer.on_event(&make_llm_response_event("unknown-trace", 1));
    observer.on_event(&make_tool_call_event("unknown-trace", 1, "tool", true));
    observer.on_event(&make_end_event("unknown-trace", 1));

    assert_eq!(observer.active_count(), 0);
}

#[test]
fn multiple_concurrent_conversations() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    observer.on_event(&make_start_event("trace-a", "Hello A"));
    observer.on_event(&make_start_event("trace-b", "Hello B"));

    assert_eq!(observer.active_count(), 2);

    observer.on_event(&make_end_event("trace-a", 1));
    assert_eq!(observer.active_count(), 1);

    observer.on_event(&make_end_event("trace-b", 1));
    assert_eq!(observer.active_count(), 0);
}

#[test]
fn full_lifecycle_with_tool_calls_and_response() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    let trace = "trace-full";
    observer.on_event(&make_start_event(trace, "Calculate 2+2"));

    // Round 1: LLM request → response with tool calls
    observer.on_event(&make_llm_request_event(trace, 1));
    observer.on_event(&ConversationEvent {
        event_type: EventType::LLMResponse,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::LLMResponse(LLMResponseEventData {
            round: 1,
            duration_ms: 2000,
            content: "".to_string(),
            tool_calls_count: 2,
            finish_reason: "tool_calls".to_string(),
            tool_calls: vec![
                serde_json::json!({"id": "tc1", "function": {"name": "calculator", "arguments": "{\"expr\": \"2+2\"}"}}),
            ],
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cached_tokens: None,
            raw_request_body: None,
            raw_response_body: None,
        }),
    });

    // Tool call: success
    observer.on_event(&make_tool_call_event(trace, 1, "calculator", true));
    // Tool call: failure
    observer.on_event(&ConversationEvent {
        event_type: EventType::ToolCall,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::ToolCall(ToolCallEventData {
            tool_name: "search".to_string(),
            success: false,
            duration_ms: 500,
            error: "Network timeout".to_string(),
            llm_round: 1,
            arguments: "{\"query\": \"test\"}".to_string(),
            result: "Error: Network timeout".to_string(),
        }),
    });

    // Round 2: LLM request → response
    observer.on_event(&make_llm_request_event(trace, 2));
    observer.on_event(&make_llm_response_event(trace, 2));

    // End conversation with error
    observer.on_event(&ConversationEvent {
        event_type: EventType::ConversationEnd,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::ConversationEnd(ConversationEndData {
            session_key: "test:chat1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            total_rounds: 2,
            total_duration_ms: 5000,
            content: "The answer is 4.".to_string(),
            is_error: false,
        }),
    });

    assert_eq!(observer.active_count(), 0);
}

#[test]
fn conversation_end_with_error_flag() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    let trace = "trace-err";
    observer.on_event(&make_start_event(trace, "Do something"));

    observer.on_event(&ConversationEvent {
        event_type: EventType::ConversationEnd,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::ConversationEnd(ConversationEndData {
            session_key: "test:chat1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            total_rounds: 1,
            total_duration_ms: 1000,
            content: "Error: something went wrong".to_string(),
            is_error: true,
        }),
    });

    assert_eq!(observer.active_count(), 0);
}

#[test]
fn llm_response_with_tool_call_details() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    let trace = "trace-tc";
    observer.on_event(&make_start_event(trace, "Search for info"));
    observer.on_event(&make_llm_request_event(trace, 1));

    // Response with tool calls
    observer.on_event(&ConversationEvent {
        event_type: EventType::LLMResponse,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::LLMResponse(LLMResponseEventData {
            round: 1,
            duration_ms: 3000,
            content: "Let me search for that.".to_string(),
            tool_calls_count: 1,
            finish_reason: "tool_calls".to_string(),
            tool_calls: vec![
                serde_json::json!({
                    "id": "call_123",
                    "type": "function",
                    "function": {
                        "name": "web_search",
                        "arguments": "{\"query\": \"test query\"}"
                    }
                }),
            ],
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cached_tokens: None,
            raw_request_body: None,
            raw_response_body: None,
        }),
    });

    observer.on_event(&make_end_event(trace, 1));
    assert_eq!(observer.active_count(), 0);
}

#[test]
fn tool_call_with_arguments_and_result() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    let trace = "trace-args";
    observer.on_event(&make_start_event(trace, "List files"));
    observer.on_event(&make_llm_request_event(trace, 1));
    observer.on_event(&make_llm_response_event(trace, 1));

    // Tool call with full data
    observer.on_event(&ConversationEvent {
        event_type: EventType::ToolCall,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::ToolCall(ToolCallEventData {
            tool_name: "list_dir".to_string(),
            success: true,
            duration_ms: 50,
            error: String::new(),
            llm_round: 1,
            arguments: "{\"path\": \"/tmp\"}".to_string(),
            result: "file1.txt\nfile2.txt".to_string(),
        }),
    });

    observer.on_event(&make_end_event(trace, 1));
    assert_eq!(observer.active_count(), 0);
}
