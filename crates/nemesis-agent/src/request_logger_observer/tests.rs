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

// ===========================================================================
// Coverage gap tests — convert_event, save_raw mode, zero-duration fallback,
// and the Observer trait impl.
// ===========================================================================

use std::collections::HashMap;
use std::time::Duration;

fn src_event(
    event_type: nemesis_observer::EventType,
    trace: &str,
    data: nemesis_observer::EventData,
) -> nemesis_observer::ConversationEvent {
    nemesis_observer::ConversationEvent {
        event_type,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data,
    }
}

#[test]
fn convert_event_conversation_start() {
    let src = src_event(
        nemesis_observer::EventType::ConversationStart,
        "t",
        nemesis_observer::EventData::ConversationStart(nemesis_observer::ConversationStartData {
            session_key: "s:k".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            sender_id: "u1".to_string(),
            content: "hi".to_string(),
        }),
    );
    let e = convert_event(&src).expect("start should convert");
    assert_eq!(e.event_type, EventType::ConversationStart);
    match e.data {
        EventData::ConversationStart(d) => {
            assert_eq!(d.channel, "web");
            assert_eq!(d.content, "hi");
        }
        _ => panic!("expected ConversationStart"),
    }
}

#[test]
fn convert_event_conversation_end_with_and_without_error() {
    let mut src = src_event(
        nemesis_observer::EventType::ConversationEnd,
        "t",
        nemesis_observer::EventData::ConversationEnd(nemesis_observer::ConversationEndData {
            session_key: "s".to_string(),
            channel: "web".to_string(),
            chat_id: "c".to_string(),
            total_rounds: 3,
            total_duration: Duration::from_millis(2500),
            content: "done".to_string(),
            error: None,
        }),
    );

    // Without error.
    let e = convert_event(&src).expect("end should convert");
    assert_eq!(e.event_type, EventType::ConversationEnd);
    match e.data {
        EventData::ConversationEnd(ref d) => {
            assert_eq!(d.total_rounds, 3);
            assert_eq!(d.total_duration_ms, 2500);
            assert!(!d.is_error);
        }
        _ => panic!("expected ConversationEnd"),
    }

    // With error → is_error true.
    if let nemesis_observer::EventData::ConversationEnd(ref mut d) = src.data {
        d.error = Some("boom".to_string());
    }
    let e2 = convert_event(&src).expect("end should convert");
    match e2.data {
        EventData::ConversationEnd(d) => assert!(d.is_error),
        _ => panic!("expected ConversationEnd"),
    }
}

#[test]
fn convert_event_llm_request() {
    let src = src_event(
        nemesis_observer::EventType::LlmRequest,
        "t",
        nemesis_observer::EventData::LlmRequest(nemesis_observer::LlmRequestData {
            round: 2,
            model: "gpt".to_string(),
            provider_name: "openai".to_string(),
            api_key: "k".to_string(),
            api_base: "b".to_string(),
            http_headers: HashMap::new(),
            full_config: None,
            messages: vec![serde_json::json!({"role": "system"})],
            tools: vec![serde_json::json!({"name": "t"})],
            messages_count: 1,
            tools_count: 1,
        }),
    );
    let e = convert_event(&src).expect("request should convert");
    assert_eq!(e.event_type, EventType::LLMRequest);
    match e.data {
        EventData::LLMRequest(d) => {
            assert_eq!(d.round, 2);
            assert_eq!(d.model, "gpt");
            assert_eq!(d.messages_count, 1);
            assert_eq!(d.tools_count, 1);
        }
        _ => panic!("expected LLMRequest"),
    }
}

#[test]
fn convert_event_llm_response_with_usage() {
    let src = src_event(
        nemesis_observer::EventType::LlmResponse,
        "t",
        nemesis_observer::EventData::LlmResponse(nemesis_observer::LlmResponseData {
            round: 1,
            duration: Duration::from_millis(500),
            content: "answer".to_string(),
            tool_calls: vec![],
            tool_calls_count: 0,
            usage: Some(nemesis_observer::UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                cached_tokens: Some(3),
                cache_creation_tokens: None,
                cache_read_tokens: None,
            }),
            finish_reason: Some("stop".to_string()),
            raw_request_body: None,
            raw_response_body: None,
        }),
    );
    let e = convert_event(&src).expect("response should convert");
    assert_eq!(e.event_type, EventType::LLMResponse);
    match e.data {
        EventData::LLMResponse(d) => {
            assert_eq!(d.duration_ms, 500);
            assert_eq!(d.finish_reason, "stop");
            assert_eq!(d.total_tokens, 15);
            assert_eq!(d.cached_tokens, Some(3));
        }
        _ => panic!("expected LLMResponse"),
    }
}

#[test]
fn convert_event_llm_response_without_usage_uses_defaults() {
    let src = src_event(
        nemesis_observer::EventType::LlmResponse,
        "t",
        nemesis_observer::EventData::LlmResponse(nemesis_observer::LlmResponseData {
            round: 1,
            duration: Duration::from_millis(100),
            content: "x".to_string(),
            tool_calls: vec![],
            tool_calls_count: 0,
            usage: None,
            finish_reason: None,
            raw_request_body: None,
            raw_response_body: None,
        }),
    );
    let e = convert_event(&src).expect("response should convert");
    match e.data {
        EventData::LLMResponse(d) => {
            assert_eq!(d.finish_reason, ""); // unwrap_or_default
            assert_eq!(d.total_tokens, 0); // usage None → 0
            assert_eq!(d.cached_tokens, None);
        }
        _ => panic!("expected LLMResponse"),
    }
}

#[test]
fn convert_event_tool_call() {
    let mut args = HashMap::new();
    args.insert("q".to_string(), serde_json::json!("rust"));
    let src = src_event(
        nemesis_observer::EventType::ToolCall,
        "t",
        nemesis_observer::EventData::ToolCall(nemesis_observer::ToolCallData {
            tool_name: "search".to_string(),
            arguments: args,
            success: false,
            duration: Duration::from_millis(50),
            error: Some("timeout".to_string()),
            llm_round: 1,
            chain_pos: 0,
        }),
    );
    let e = convert_event(&src).expect("tool call should convert");
    assert_eq!(e.event_type, EventType::ToolCall);
    match e.data {
        EventData::ToolCall(d) => {
            assert_eq!(d.tool_name, "search");
            assert!(!d.success);
            assert_eq!(d.error, "timeout");
            assert!(d.arguments.contains("\"q\""));
        }
        _ => panic!("expected ToolCall"),
    }
}

fn raw_config() -> LoggingConfig {
    LoggingConfig {
        enabled: true,
        detail_level: crate::request_logger::DetailLevel::Full,
        log_dir: "logs/llm".to_string(),
        save_raw: true,
    }
}

/// Collect all filenames inside the (single) session directory under `log_dir`.
fn collect_session_files(log_dir: &Path) -> Vec<String> {
    let session: std::path::PathBuf = std::fs::read_dir(log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .map(|e| e.path())
        .expect("session dir exists");
    std::fs::read_dir(&session)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

#[test]
fn save_raw_mode_writes_raw_request_and_response_files() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(raw_config(), tmp.path());

    observer.on_event(&make_start_event("trace-raw", "hi"));
    observer.on_event(&ConversationEvent {
        event_type: EventType::LLMRequest,
        trace_id: "trace-raw".to_string(),
        timestamp: Local::now(),
        data: EventData::LLMRequest(LLMRequestEventData {
            round: 1,
            model: "gpt-4".to_string(),
            provider_name: "openai".to_string(),
            api_key: "k".to_string(),
            api_base: "b".to_string(),
            messages_count: 1,
            tools_count: 1,
            messages: vec![serde_json::json!({"role": "user", "content": "hi"})],
            tools: vec![serde_json::json!({"name": "t"})],
        }),
    });
    observer.on_event(&ConversationEvent {
        event_type: EventType::LLMResponse,
        trace_id: "trace-raw".to_string(),
        timestamp: Local::now(),
        data: EventData::LLMResponse(LLMResponseEventData {
            round: 1,
            duration_ms: 100,
            content: "ok".to_string(),
            tool_calls_count: 0,
            finish_reason: "stop".to_string(),
            tool_calls: vec![],
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cached_tokens: None,
            raw_request_body: None,
            raw_response_body: Some(r#"{"id":"x"}"#.to_string()),
        }),
    });
    observer.on_event(&make_end_event("trace-raw", 1));

    let log_dir = tmp.path().join("logs").join("llm");
    let names = collect_session_files(&log_dir);
    assert!(
        names.iter().any(|n| n.contains("AI.Request.raw.json")),
        "raw request file missing: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n.contains("AI.Response.raw.json")),
        "raw response file missing: {:?}",
        names
    );
}

#[test]
fn conversation_end_with_zero_duration_uses_start_time_fallback() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());
    observer.on_event(&make_start_event("trace-zero", "hi"));

    observer.on_event(&ConversationEvent {
        event_type: EventType::ConversationEnd,
        trace_id: "trace-zero".to_string(),
        timestamp: Local::now(),
        data: EventData::ConversationEnd(ConversationEndData {
            session_key: "test:chat1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            total_rounds: 1,
            total_duration_ms: 0, // triggers start_time fallback branch
            content: "done".to_string(),
            is_error: false,
        }),
    });
    assert_eq!(observer.active_count(), 0);
}

#[tokio::test]
async fn observer_trait_name_and_dispatch() {
    let tmp = TempDir::new().unwrap();
    let observer = RequestLoggerObserver::new(test_config(), tmp.path());

    // Trait impl name() (distinct from the inherent name()).
    assert_eq!(nemesis_observer::Observer::name(&observer), "request_logger");

    // Dispatch a conversation_start via the Observer trait → convert_event → internal on_event.
    let src = nemesis_observer::ConversationEvent {
        event_type: nemesis_observer::EventType::ConversationStart,
        trace_id: "trait-trace".to_string(),
        timestamp: Local::now(),
        data: nemesis_observer::EventData::ConversationStart(
            nemesis_observer::ConversationStartData {
                session_key: "s:k".to_string(),
                channel: "web".to_string(),
                chat_id: "chat1".to_string(),
                sender_id: "u".to_string(),
                content: "via trait".to_string(),
            },
        ),
    };
    nemesis_observer::Observer::on_event(&observer, src).await;
    assert_eq!(observer.active_count(), 1);

    // End via trait.
    let end = nemesis_observer::ConversationEvent {
        event_type: nemesis_observer::EventType::ConversationEnd,
        trace_id: "trait-trace".to_string(),
        timestamp: Local::now(),
        data: nemesis_observer::EventData::ConversationEnd(nemesis_observer::ConversationEndData {
            session_key: "s:k".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            total_rounds: 1,
            total_duration: Duration::from_millis(100),
            content: "y".to_string(),
            error: None,
        }),
    };
    nemesis_observer::Observer::on_event(&observer, end).await;
    assert_eq!(observer.active_count(), 0);
}
