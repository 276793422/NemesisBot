use super::*;
use chrono::Local;
use nemesis_agent::request_logger::{DetailLevel, LoggingConfig};
use nemesis_agent::request_logger_observer::{
    ConversationEndData, ConversationEvent, ConversationStartData, EventData, EventType,
    LLMRequestEventData, LLMResponseEventData, ToolCallEventData,
};
use tempfile::TempDir;

fn test_config() -> LoggingConfig {
    LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Full,
        log_dir: "logs/cluster_logs".to_string(),
        save_raw: false,
    }
}

fn raw_test_config() -> LoggingConfig {
    LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Full,
        log_dir: "logs/cluster_logs".to_string(),
        save_raw: true,
    }
}

fn make_start_event(trace: &str, content: &str) -> ConversationEvent {
    ConversationEvent {
        event_type: EventType::ConversationStart,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::ConversationStart(ConversationStartData {
            session_key: "cluster:test".to_string(),
            channel: "cluster".to_string(),
            chat_id: "chat1".to_string(),
            sender_id: "node-a".to_string(),
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
            messages_count: 2,
            tools_count: 1,
            messages: vec![
                serde_json::json!({"role": "system", "content": "You are helpful"}),
                serde_json::json!({"role": "user", "content": "Hello from remote"}),
            ],
            tools: vec![serde_json::json!({"type": "function", "function": {"name": "calc"}})],
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
            duration_ms: 1200,
            content: "Hi there!".to_string(),
            tool_calls_count: 0,
            finish_reason: "stop".to_string(),
            tool_calls: vec![],
            prompt_tokens: 50,
            completion_tokens: 10,
            total_tokens: 60,
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
            duration_ms: 80,
            error: if success { String::new() } else { "boom".to_string() },
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
            session_key: "cluster:test".to_string(),
            channel: "cluster".to_string(),
            chat_id: "chat1".to_string(),
            total_rounds: rounds,
            total_duration_ms: 2000,
            content: "Final cluster response.".to_string(),
            is_error: false,
        }),
    }
}

// ---------------------------------------------------------------------------
// Path layout tests
// ---------------------------------------------------------------------------

#[test]
fn path_uses_device_id_subdir_when_task_context_set() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    observer.set_task_context("task-123".to_string(), "node-A".to_string());
    observer.dispatch(&make_start_event("trace-1", "Hello"));

    // Expected: workspace/logs/cluster_logs/node-A/{ts}_task-123/
    let device_dir = tmp.path().join("logs").join("cluster_logs").join("node-A");
    assert!(device_dir.exists(), "device_id subdir should exist");

    let sessions: Vec<_> = std::fs::read_dir(&device_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .collect();
    assert_eq!(sessions.len(), 1, "exactly one session dir expected");

    let session_name = sessions[0].file_name().to_string_lossy().to_string();
    assert!(
        session_name.ends_with("_task-123"),
        "session name should end with sanitized task_id, got: {}",
        session_name
    );
}

#[test]
fn path_falls_back_to_unknown_when_no_task_context() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    // No set_task_context call — should fall back to _unknown
    observer.dispatch(&make_start_event("trace-no-ctx", "Hello"));

    let unknown_dir = tmp.path().join("logs").join("cluster_logs").join("_unknown");
    assert!(unknown_dir.exists(), "_unknown subdir should exist when no task context");

    // Without task context, session_name is None → RequestLogger generates a random one
    let sessions: Vec<_> = std::fs::read_dir(&unknown_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .collect();
    assert_eq!(sessions.len(), 1, "one session dir expected");
}

#[test]
fn empty_device_id_falls_back_to_unknown() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    observer.set_task_context("task-x".to_string(), "".to_string());
    observer.dispatch(&make_start_event("trace-empty-dev", "Hi"));

    let unknown_dir = tmp.path().join("logs").join("cluster_logs").join("_unknown");
    assert!(unknown_dir.exists(), "empty device_id should fall back to _unknown");
}

#[test]
fn unsafe_chars_in_device_id_and_task_id_are_sanitized() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    // Windows-unsafe chars in both fields
    observer.set_task_context("task<bad>/name".to_string(), "node\\B:z".to_string());
    observer.dispatch(&make_start_event("trace-bad", "Hello"));

    // device_id should not contain \\ or : — sanitized to _
    let bad_device_dir = tmp.path().join("logs").join("cluster_logs").join("node\\B:z");
    assert!(!bad_device_dir.exists(), "raw unsafe device_id dir should not exist");

    // Find what device dir was actually created
    let cluster_logs = tmp.path().join("logs").join("cluster_logs");
    let device_dirs: Vec<_> = std::fs::read_dir(&cluster_logs)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(device_dirs.len(), 1, "exactly one sanitized device dir");
    let device_name = &device_dirs[0];
    assert!(!device_name.contains('\\'), "no backslash in device dir name");
    assert!(!device_name.contains(':'), "no colon in device dir name");

    // Check session name has no unsafe chars either
    let session_dirs: Vec<_> = std::fs::read_dir(cluster_logs.join(device_name))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(session_dirs.len(), 1);
    let session_name = &session_dirs[0];
    assert!(!session_name.contains('<'), "no < in session name");
    assert!(!session_name.contains('>'), "no > in session name");
    assert!(!session_name.contains('/'), "no / in session name");
}

// ---------------------------------------------------------------------------
// Task context lifecycle tests
// ---------------------------------------------------------------------------

#[test]
fn clear_task_context_prevents_next_event_from_inheriting_it() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    observer.set_task_context("task-1".to_string(), "node-A".to_string());
    observer.dispatch(&make_start_event("trace-1", "First"));
    observer.clear_task_context();

    // New event without re-setting context should go to _unknown
    observer.dispatch(&make_start_event("trace-2", "Second"));

    let dir_a = tmp.path().join("logs").join("cluster_logs").join("node-A");
    let dir_unknown = tmp.path().join("logs").join("cluster_logs").join("_unknown");
    assert!(dir_a.exists(), "first task logged under node-A");
    assert!(dir_unknown.exists(), "second task fell back to _unknown");
}

#[test]
fn set_task_context_overwrites_previous_context() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    observer.set_task_context("task-1".to_string(), "node-A".to_string());
    observer.dispatch(&make_start_event("trace-1", "First"));

    observer.set_task_context("task-2".to_string(), "node-B".to_string());
    observer.dispatch(&make_start_event("trace-2", "Second"));

    let dir_a = tmp.path().join("logs").join("cluster_logs").join("node-A");
    let dir_b = tmp.path().join("logs").join("cluster_logs").join("node-B");
    assert!(dir_a.exists(), "first call logged under node-A");
    assert!(dir_b.exists(), "second call switched to node-B");
}

// ---------------------------------------------------------------------------
// Full lifecycle tests
// ---------------------------------------------------------------------------

#[test]
fn full_lifecycle_creates_all_expected_files() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    observer.set_task_context("task-full".to_string(), "node-A".to_string());

    let trace = "trace-full";
    observer.dispatch(&make_start_event(trace, "Calculate 2+2"));
    assert_eq!(observer.active_count(), 1);

    observer.dispatch(&make_llm_request_event(trace, 1));
    observer.dispatch(&make_llm_response_event(trace, 1));
    observer.dispatch(&make_tool_call_event(trace, 1, "calc", true));
    observer.dispatch(&make_llm_request_event(trace, 2));
    observer.dispatch(&make_llm_response_event(trace, 2));
    observer.dispatch(&make_end_event(trace, 2));
    assert_eq!(observer.active_count(), 0);

    // Verify session dir was created with files inside
    let device_dir = tmp.path().join("logs").join("cluster_logs").join("node-A");
    let session_dirs: Vec<_> = std::fs::read_dir(&device_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .collect();
    assert_eq!(session_dirs.len(), 1);

    let session_path = session_dirs[0].path();
    let files: Vec<_> = std::fs::read_dir(&session_path)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(!files.is_empty(), "session dir should contain log files");
}

#[test]
fn raw_mode_writes_raw_envelope_and_response() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(raw_test_config(), tmp.path());

    observer.set_task_context("task-raw".to_string(), "node-A".to_string());

    let trace = "trace-raw";
    observer.dispatch(&make_start_event(trace, "Hello"));

    // Response with raw_response_body present
    observer.dispatch(&make_llm_request_event(trace, 1));
    observer.dispatch(&ConversationEvent {
        event_type: EventType::LLMResponse,
        trace_id: trace.to_string(),
        timestamp: Local::now(),
        data: EventData::LLMResponse(LLMResponseEventData {
            round: 1,
            duration_ms: 200,
            content: "Raw reply".to_string(),
            tool_calls_count: 0,
            finish_reason: "stop".to_string(),
            tool_calls: vec![],
            prompt_tokens: 5,
            completion_tokens: 5,
            total_tokens: 10,
            cached_tokens: Some(3),
            raw_request_body: None,
            raw_response_body: Some(r#"{"id":"resp_1","choices":[]} "#.to_string()),
        }),
    });

    observer.dispatch(&make_end_event(trace, 1));

    let device_dir = tmp.path().join("logs").join("cluster_logs").join("node-A");
    let session_dirs: Vec<_> = std::fs::read_dir(&device_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .collect();
    assert_eq!(session_dirs.len(), 1);

    // In raw mode there should be request/response files
    let files: Vec<_> = std::fs::read_dir(session_dirs[0].path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        files.iter().any(|f| f.contains("request")),
        "raw mode should write a request file, got: {:?}",
        files
    );
    assert!(
        files.iter().any(|f| f.contains("response")),
        "raw mode should write a response file, got: {:?}",
        files
    );
}

#[test]
fn disabled_config_creates_nothing() {
    let config = LoggingConfig {
        enabled: false,
        detail_level: DetailLevel::Full,
        log_dir: "logs/cluster_logs".to_string(),
        save_raw: false,
    };
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(config, tmp.path());

    observer.set_task_context("task-disabled".to_string(), "node-A".to_string());
    observer.dispatch(&make_start_event("trace-disabled", "Hello"));

    let logs_dir = tmp.path().join("logs");
    assert!(!logs_dir.exists(), "disabled config should create no dirs");
    assert_eq!(observer.active_count(), 0);
}

#[test]
fn unknown_trace_events_are_ignored() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    // These reference a trace that was never started
    observer.dispatch(&make_llm_request_event("unknown-trace", 1));
    observer.dispatch(&make_llm_response_event("unknown-trace", 1));
    observer.dispatch(&make_tool_call_event("unknown-trace", 1, "tool", true));
    observer.dispatch(&make_end_event("unknown-trace", 1));

    assert_eq!(observer.active_count(), 0);

    let logs_dir = tmp.path().join("logs");
    assert!(!logs_dir.exists(), "no dirs created for unknown traces");
}

#[test]
fn two_concurrent_tasks_log_to_their_own_device_dirs() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    // Task 1 from node-A
    observer.set_task_context("task-1".to_string(), "node-A".to_string());
    observer.dispatch(&make_start_event("trace-a", "Hello from A"));
    assert_eq!(observer.active_count(), 1);

    // Task 2 from node-B (different device, different trace)
    observer.set_task_context("task-2".to_string(), "node-B".to_string());
    observer.dispatch(&make_start_event("trace-b", "Hello from B"));
    assert_eq!(observer.active_count(), 2);

    // Each completes
    observer.dispatch(&make_end_event("trace-a", 1));
    observer.dispatch(&make_end_event("trace-b", 1));
    assert_eq!(observer.active_count(), 0);

    let dir_a = tmp.path().join("logs").join("cluster_logs").join("node-A");
    let dir_b = tmp.path().join("logs").join("cluster_logs").join("node-B");
    assert!(dir_a.exists(), "node-A logged");
    assert!(dir_b.exists(), "node-B logged");

    // Each device dir should have exactly one session
    for dir in [dir_a, dir_b] {
        let sessions: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
            .collect();
        assert_eq!(sessions.len(), 1);
    }
}

// ---------------------------------------------------------------------------
// emit_conversation_start / emit_conversation_end helper tests
// (these verify the bug fix: cluster_agent.rs calls these helpers around
// run_with_trace/resume_execution because run_with_trace itself does not
// emit ConversationStart/ConversationEnd — without these helpers, the
// observer's `active` map never registers the trace_id and every
// subsequent LLM event is silently dropped)
// ---------------------------------------------------------------------------

#[test]
fn emit_conversation_start_makes_subsequent_llm_events_logged() {
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    observer.set_task_context("task-helper".to_string(), "node-A".to_string());
    observer.emit_conversation_start("trace-helper", "cluster", "task-helper", "node-A", "Hi");

    // Without emit_conversation_start this would have been dropped.
    observer.dispatch(&make_llm_request_event("trace-helper", 1));
    observer.dispatch(&make_llm_response_event("trace-helper", 1));

    assert_eq!(observer.active_count(), 1, "trace should be active after start");

    observer.emit_conversation_end("trace-helper", "cluster", "task-helper", 1, "Reply", false);
    assert_eq!(observer.active_count(), 0, "trace should be cleared after end");

    // Verify a session directory was created.
    let device_dir = tmp.path().join("logs").join("cluster_logs").join("node-A");
    let sessions: Vec<_> = std::fs::read_dir(&device_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
        .collect();
    assert_eq!(sessions.len(), 1, "one session dir expected");
}

#[test]
fn llm_events_without_prior_start_are_dropped() {
    // Regression test for the original bug: if emit_conversation_start is
    // not called, LLM events must NOT be logged (because there's no trace
    // entry in the `active` map). This documents the contract — if a future
    // refactor accidentally regresses the cluster_agent.rs call sites,
    // this test plus emit_conversation_start_makes_subsequent_llm_events_logged
    // together pinpoint the cause.
    let tmp = TempDir::new().unwrap();
    let observer = ClusterRequestLoggerObserver::new(test_config(), tmp.path());

    observer.set_task_context("task-no-start".to_string(), "node-A".to_string());

    // No emit_conversation_start call.
    observer.dispatch(&make_llm_request_event("trace-no-start", 1));
    observer.dispatch(&make_llm_response_event("trace-no-start", 1));

    assert_eq!(observer.active_count(), 0);

    let logs_dir = tmp.path().join("logs");
    assert!(!logs_dir.exists(), "without ConversationStart, nothing is written");
}
