//! S1 coverage batch (2026-08-26): nemesis-heartbeat remaining gaps.
//!
//! The spawned-task "static twin" functions (`execute_heartbeat_tick`,
//! `build_prompt_from_workspace`, `is_heartbeat_file_empty_static`,
//! `create_default_heartbeat_template_static`, `parse_last_channel_static`,
//! `send_response_static`) are only reached indirectly through the spawned
//! interval loop, so several of their arms (skip-file double-check, no-handler,
//! async/error message selection, template write failure, invalid channel
//! formats) never fire in the real-timer tests. This module calls them
//! directly — they are private to `service` but visible from this child
//! module — and additionally installs a thread-local tracing subscriber so
//! the lazy `tracing::*!` macro field arguments (never evaluated without an
//! enabled subscriber) execute too.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn s1_subscriber() -> impl tracing::Subscriber + Send + Sync + 'static {
    // TRACE sink to /dev/null: only exists to make tracing evaluate event
    // field arguments. Thread-local via with_default, no global state.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(std::io::sink)
        .finish()
}

struct S1Bus {
    sent: Arc<Mutex<Vec<(String, String, String)>>>,
}
impl MessageBus for S1Bus {
    fn publish_outbound(&self, channel: String, chat_id: String, content: String) {
        self.sent.lock().push((channel, chat_id, content));
    }
}

struct S1State {
    last_channel: String,
}
impl StateManager for S1State {
    fn get_last_channel(&self) -> String {
        self.last_channel.clone()
    }
}

fn s1_ws(dir: &tempfile::TempDir) -> Option<String> {
    Some(dir.path().to_string_lossy().to_string())
}

fn s1_args() -> (
    Arc<Mutex<Option<String>>>,
    Arc<Mutex<chrono::DateTime<Local>>>,
    Arc<AtomicU64>,
) {
    (
        Arc::new(Mutex::new(None)),
        Arc::new(Mutex::new(Local::now())),
        Arc::new(AtomicU64::new(0)),
    )
}

fn s1_counting_handler() -> (Option<SharedHeartbeatHandler>, Arc<AtomicU64>) {
    let called = Arc::new(AtomicU64::new(0));
    let c2 = called.clone();
    (
        Some(Arc::new(move |_p, _c, _ch| {
            c2.fetch_add(1, Ordering::SeqCst);
            None
        })),
        called,
    )
}

// ---------------------------------------------------------------------------
// execute_heartbeat_tick early-return arms
// ---------------------------------------------------------------------------

#[test]
fn test_s1_tick_skip_file_blocks_execution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "- Task\n").unwrap();
    let skip = dir.path().join("BOOTSTRAP.md");
    std::fs::write(&skip, "bootstrap on").unwrap();

    let (handler, called) = s1_counting_handler();
    let (_, last_beat, beat_count) = s1_args();
    let skip_file = Arc::new(Mutex::new(Some(skip.to_string_lossy().to_string())));

    tracing::subscriber::with_default(s1_subscriber(), || {
        execute_heartbeat_tick(
            &handler,
            &s1_ws(&dir),
            &None,
            &None,
            &skip_file,
            &last_beat,
            &beat_count,
        );
    });

    assert_eq!(called.load(Ordering::SeqCst), 0, "skip file must block the tick");
    assert_eq!(
        beat_count.load(Ordering::SeqCst),
        0,
        "skip returns before beat tracking advances"
    );
}

#[test]
fn test_s1_tick_empty_prompt_returns_early() {
    // No workspace → prompt empty → tick returns before beat tracking.
    let (handler, called) = s1_counting_handler();
    let (skip_file, last_beat, beat_count) = s1_args();
    let workspace: Option<String> = None;
    let bus: Option<Arc<dyn MessageBus>> = None;

    tracing::subscriber::with_default(s1_subscriber(), || {
        execute_heartbeat_tick(
            &handler,
            &workspace,
            &bus,
            &None,
            &skip_file,
            &last_beat,
            &beat_count,
        );
    });

    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert_eq!(beat_count.load(Ordering::SeqCst), 0);
}

#[test]
fn test_s1_tick_comments_only_prompt_returns_early() {
    // HEARTBEAT.md exists but is comments-only → prompt empty → early return.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "# only\n## comments\n").unwrap();

    let (handler, called) = s1_counting_handler();
    let (skip_file, last_beat, beat_count) = s1_args();
    let bus: Option<Arc<dyn MessageBus>> = None;

    tracing::subscriber::with_default(s1_subscriber(), || {
        execute_heartbeat_tick(
            &handler,
            &s1_ws(&dir),
            &bus,
            &None,
            &skip_file,
            &last_beat,
            &beat_count,
        );
    });

    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert_eq!(beat_count.load(Ordering::SeqCst), 0);
}

#[test]
fn test_s1_tick_handler_not_configured() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "- Task\n").unwrap();

    let handler: Option<SharedHeartbeatHandler> = None;
    let (skip_file, last_beat, beat_count) = s1_args();
    let bus: Option<Arc<dyn MessageBus>> = None;

    tracing::subscriber::with_default(s1_subscriber(), || {
        execute_heartbeat_tick(
            &handler,
            &s1_ws(&dir),
            &bus,
            &None,
            &skip_file,
            &last_beat,
            &beat_count,
        );
    });

    // Beat tracking (step 3.5) advances BEFORE the handler call (step 4), so
    // the count is 1 even though the handler is missing.
    assert_eq!(beat_count.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// execute_heartbeat_tick result-dispatch arms
// ---------------------------------------------------------------------------

#[test]
fn test_s1_tick_async_result_publishes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "- Task\n").unwrap();

    let sent = Arc::new(Mutex::new(Vec::new()));
    let bus: Option<Arc<dyn MessageBus>> = Some(Arc::new(S1Bus { sent: sent.clone() }));
    let state: Option<Arc<dyn StateManager>> = Some(Arc::new(S1State {
        last_channel: "telegram:7".to_string(),
    }));
    let handler: Option<SharedHeartbeatHandler> = Some(Arc::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: true,
            silent: false,
            for_user: "NOT SENT".to_string(),
            for_llm: "spawned subagent".to_string(),
        })
    }));
    let (skip_file, last_beat, beat_count) = s1_args();

    tracing::subscriber::with_default(s1_subscriber(), || {
        execute_heartbeat_tick(
            &handler,
            &s1_ws(&dir),
            &bus,
            &state,
            &skip_file,
            &last_beat,
            &beat_count,
        );
    });

    assert!(sent.lock().is_empty(), "async results must not publish");
    assert_eq!(beat_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_s1_tick_falls_back_to_for_llm_message() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "- Task\n").unwrap();

    let sent = Arc::new(Mutex::new(Vec::new()));
    let bus: Option<Arc<dyn MessageBus>> = Some(Arc::new(S1Bus { sent: sent.clone() }));
    let state: Option<Arc<dyn StateManager>> = Some(Arc::new(S1State {
        last_channel: "telegram:7".to_string(),
    }));
    let handler: Option<SharedHeartbeatHandler> = Some(Arc::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: false,
            for_user: String::new(),
            for_llm: "LLM-only content".to_string(),
        })
    }));
    let (skip_file, last_beat, beat_count) = s1_args();

    execute_heartbeat_tick(
        &handler,
        &s1_ws(&dir),
        &bus,
        &state,
        &skip_file,
        &last_beat,
        &beat_count,
    );

    let sent_lock = sent.lock();
    assert_eq!(sent_lock.len(), 1);
    assert_eq!(
        &sent_lock[0],
        &(
            "telegram".to_string(),
            "7".to_string(),
            "LLM-only content".to_string()
        )
    );
}

#[test]
fn test_s1_tick_both_messages_empty_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "- Task\n").unwrap();

    let sent = Arc::new(Mutex::new(Vec::new()));
    let bus: Option<Arc<dyn MessageBus>> = Some(Arc::new(S1Bus { sent: sent.clone() }));
    let state: Option<Arc<dyn StateManager>> = Some(Arc::new(S1State {
        last_channel: "telegram:7".to_string(),
    }));
    let handler: Option<SharedHeartbeatHandler> = Some(Arc::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: false,
            for_user: String::new(),
            for_llm: String::new(),
        })
    }));
    let (skip_file, last_beat, beat_count) = s1_args();

    execute_heartbeat_tick(
        &handler,
        &s1_ws(&dir),
        &bus,
        &state,
        &skip_file,
        &last_beat,
        &beat_count,
    );

    assert!(sent.lock().is_empty(), "empty response must not publish");
    assert_eq!(beat_count.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// Method-version async arm with lazy tracing field (service.rs:535)
// ---------------------------------------------------------------------------

#[test]
fn test_s1_execute_heartbeat_async_logs_field() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "- Task\n").unwrap();

    let svc = HeartbeatService::new(HeartbeatConfig {
        enabled: true,
        workspace: s1_ws(&dir),
        ..Default::default()
    });
    svc.set_handler(Box::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: true,
            silent: false,
            for_user: String::new(),
            for_llm: "spawned task-9".to_string(),
        })
    }));

    // The subscriber makes tracing evaluate `message = result.for_llm.as_str()`.
    tracing::subscriber::with_default(s1_subscriber(), || svc.execute_heartbeat());
}

// ---------------------------------------------------------------------------
// build_prompt_from_workspace / template creation arms
// ---------------------------------------------------------------------------

#[test]
fn test_s1_build_prompt_static_none_workspace() {
    assert_eq!(build_prompt_from_workspace(&None), "");
}

#[test]
fn test_s1_build_prompt_static_comments_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "# a\n## b\n\n").unwrap();
    let ws = s1_ws(&dir);

    tracing::subscriber::with_default(s1_subscriber(), || {
        assert_eq!(build_prompt_from_workspace(&ws), "");
    });
}

#[test]
fn test_s1_build_prompt_static_missing_file_creates_template() {
    let dir = tempfile::tempdir().unwrap();
    let ws = s1_ws(&dir);
    assert!(!dir.path().join("HEARTBEAT.md").exists());

    tracing::subscriber::with_default(s1_subscriber(), || {
        assert_eq!(build_prompt_from_workspace(&ws), "");
    });

    let template = dir.path().join("HEARTBEAT.md");
    assert!(template.exists(), "missing file must create the template");
    let content = std::fs::read_to_string(&template).unwrap();
    assert!(content.contains("Heartbeat Check List"));

    // Second read now has real (non-comment) content → prompt is built.
    let prompt2 = build_prompt_from_workspace(&ws);
    assert!(prompt2.contains("Heartbeat Check"));
    assert!(prompt2.contains("Current time:"));
}

#[test]
fn test_s1_create_default_template_static_write_failure_no_panic() {
    // Parent directory does not exist → exists() is false but fs::write
    // fails: the warn arm, not a panic.
    let dir = tempfile::tempdir().unwrap();
    let ws = dir
        .path()
        .join("never_created_subdir")
        .to_string_lossy()
        .to_string();

    tracing::subscriber::with_default(s1_subscriber(), || {
        create_default_heartbeat_template_static(&ws);
    });

    assert!(!dir.path().join("never_created_subdir").join("HEARTBEAT.md").exists());
}

#[test]
fn test_s1_create_default_template_static_existing_file_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("HEARTBEAT.md"), "custom content").unwrap();
    let ws = dir.path().to_string_lossy().to_string();

    create_default_heartbeat_template_static(&ws);

    let content = std::fs::read_to_string(dir.path().join("HEARTBEAT.md")).unwrap();
    assert_eq!(content, "custom content", "existing file must not be overwritten");
}

#[test]
fn test_s1_is_heartbeat_file_empty_static_both_paths() {
    assert!(is_heartbeat_file_empty_static(b"# a\n\n   \n"));
    assert!(is_heartbeat_file_empty_static(b""));
    assert!(!is_heartbeat_file_empty_static(b"# a\nreal line\n"));
}

// ---------------------------------------------------------------------------
// parse_last_channel_static arms
// ---------------------------------------------------------------------------

#[test]
fn test_s1_parse_last_channel_static_arms() {
    fn as_str(input: &str) -> (String, String) {
        parse_last_channel_static(input)
    }
    let cases = [
        ("", ("", "")),
        ("nocolon", ("", "")),
        (":", ("", "")),
        ("telegram:", ("", "")),
        ("system:1", ("", "")),
        ("rpc:1", ("", "")),
        ("cluster:1", ("", "")),
        ("internal:1", ("", "")),
        ("telegram:42", ("telegram", "42")),
    ];
    for (input, want) in cases {
        let (p, u) = as_str(input);
        assert_eq!((p.as_str(), u.as_str()), want, "input {input:?}");
    }
}

// ---------------------------------------------------------------------------
// send_response_static arms
// ---------------------------------------------------------------------------

#[test]
fn test_s1_send_response_static_all_reject_arms_then_success() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let bus: Option<Arc<dyn MessageBus>> = Some(Arc::new(S1Bus { sent: sent.clone() }));
    let good_state: Option<Arc<dyn StateManager>> = Some(Arc::new(S1State {
        last_channel: "telegram:1".to_string(),
    }));

    // No bus configured.
    send_response_static(&None, &good_state, "x");
    // No state manager configured.
    send_response_static(&bus, &None, "x");
    // Empty last channel recorded.
    send_response_static(
        &bus,
        &Some(Arc::new(S1State {
            last_channel: String::new(),
        })),
        "x",
    );
    // Last channel recorded but unparseable (no colon → empty platform).
    send_response_static(
        &bus,
        &Some(Arc::new(S1State {
            last_channel: "malformed-no-colon".to_string(),
        })),
        "x",
    );

    assert!(sent.lock().is_empty(), "all four reject arms must not publish");

    // Happy path still publishes.
    send_response_static(&bus, &good_state, "hello");
    let sent_lock = sent.lock();
    assert_eq!(sent_lock.len(), 1);
    assert_eq!(
        &sent_lock[0],
        &(
            "telegram".to_string(),
            "1".to_string(),
            "hello".to_string()
        )
    );
}
