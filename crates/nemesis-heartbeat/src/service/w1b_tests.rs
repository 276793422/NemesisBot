//! W1b batch (Phase 3 batch 16): nemesis-heartbeat gap tests.
//!
//! Targets the uncovered surface of `service.rs` found by auditing the
//! existing 74-test suite:
//! - `stop()` without start, handler overwrite, both-messages-empty branch
//! - handler argument contract (parsed prompt/channel/chat_id)
//! - `HeartbeatConfig::new` boundary values (4→5 clamp, 5 stays)
//! - non-UTF8 HEARTBEAT.md lossy read, template write failure no-panic
//! - tick branches: is_error / is_async publish nothing but advance beat
//!   tracking
//! - BUG #17 regression: start() used to MOVE the handler into the spawned
//!   task, so a stop()+start() cycle silently dropped it (ticks then logged
//!   "handler not configured" forever)

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Local mocks (sibling `tests` module's MockBus/MockState are private there).
struct W1bBus {
    sent: Arc<Mutex<Vec<(String, String, String)>>>,
}
impl W1bBus {
    fn new() -> (Self, Arc<Mutex<Vec<(String, String, String)>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        (Self { sent: sent.clone() }, sent)
    }
}
impl MessageBus for W1bBus {
    fn publish_outbound(&self, channel: String, chat_id: String, content: String) {
        self.sent.lock().push((channel, chat_id, content));
    }
}

struct W1bState {
    last_channel: String,
}
impl StateManager for W1bState {
    fn get_last_channel(&self) -> String {
        self.last_channel.clone()
    }
}

fn ws_config(dir: &tempfile::TempDir) -> HeartbeatConfig {
    HeartbeatConfig {
        interval: Duration::from_millis(100),
        enabled: true,
        workspace: Some(dir.path().to_string_lossy().to_string()),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    }
}

fn write_tasks(dir: &tempfile::TempDir, tasks: &str) {
    std::fs::write(dir.path().join("HEARTBEAT.md"), tasks).unwrap();
}

#[test]
fn test_w1b_stop_without_start_no_panic() {
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    svc.stop(); // handle is None — must be a no-op, not a panic
    assert!(!svc.is_running());
    // Double stop is also safe.
    svc.stop();
}

#[test]
fn test_w1b_set_handler_overwrite_second_wins() {
    let dir = tempfile::tempdir().unwrap();
    write_tasks(&dir, "- Task\n");

    let first = Arc::new(AtomicBool::new(false));
    let second = Arc::new(AtomicBool::new(false));
    let svc = HeartbeatService::new(ws_config(&dir));

    let f = first.clone();
    svc.set_handler(Box::new(move |_p, _c, _ch| {
        f.store(true, Ordering::SeqCst);
        None
    }));
    let s = second.clone();
    svc.set_handler(Box::new(move |_p, _c, _ch| {
        s.store(true, Ordering::SeqCst);
        None
    }));

    svc.execute_heartbeat();
    assert!(
        !first.load(Ordering::SeqCst),
        "first handler must be replaced"
    );
    assert!(second.load(Ordering::SeqCst), "second handler wins");
}

#[test]
fn test_w1b_execute_heartbeat_both_messages_empty_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_tasks(&dir, "- Task\n");
    let svc = HeartbeatService::new(ws_config(&dir));

    let (mock_bus, sent) = W1bBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(W1bState {
        last_channel: "telegram:77".to_string(),
    }));
    svc.set_handler(Box::new(|_p, _c, _ch| {
        Some(HeartbeatResult {
            is_error: false,
            is_async: false,
            silent: false,
            for_user: String::new(),
            for_llm: String::new(),
        })
    }));

    svc.execute_heartbeat();
    assert!(
        sent.lock().is_empty(),
        "empty for_user AND for_llm must publish nothing"
    );
}

#[test]
fn test_w1b_execute_heartbeat_handler_receives_prompt_and_parsed_channel() {
    let dir = tempfile::tempdir().unwrap();
    write_tasks(&dir, "- Unique task marker XYZ\n");
    let svc = HeartbeatService::new(ws_config(&dir));

    let captured: Arc<Mutex<Option<(String, String, String)>>> = Arc::new(Mutex::new(None));
    let cap2 = captured.clone();
    svc.set_state_manager(Arc::new(W1bState {
        last_channel: "telegram:42".to_string(),
    }));
    svc.set_handler(Box::new(move |prompt, channel, chat_id| {
        *cap2.lock() = Some((prompt, channel, chat_id));
        None
    }));

    svc.execute_heartbeat();
    let (prompt, channel, chat_id) = captured.lock().clone().expect("handler called");
    assert!(prompt.contains("Heartbeat Check"), "prompt has header");
    assert!(prompt.contains("Current time:"), "prompt has time block");
    assert!(
        prompt.contains("Unique task marker XYZ"),
        "prompt embeds HEARTBEAT.md content"
    );
    assert_eq!(channel, "telegram");
    assert_eq!(chat_id, "42");
}

#[test]
fn test_w1b_execute_heartbeat_internal_channel_passes_empty_to_handler() {
    let dir = tempfile::tempdir().unwrap();
    write_tasks(&dir, "- Task\n");
    let svc = HeartbeatService::new(ws_config(&dir));

    let captured: Arc<Mutex<Option<(String, String, String)>>> = Arc::new(Mutex::new(None));
    let cap2 = captured.clone();
    svc.set_state_manager(Arc::new(W1bState {
        last_channel: "system:123".to_string(), // internal → filtered
    }));
    svc.set_handler(Box::new(move |prompt, channel, chat_id| {
        *cap2.lock() = Some((prompt, channel, chat_id));
        None
    }));

    svc.execute_heartbeat();
    let (_, channel, chat_id) = captured.lock().clone().expect("handler called");
    assert_eq!(channel, "", "internal platform parses to empty");
    assert_eq!(chat_id, "");
}

#[test]
fn test_w1b_status_interval_secs_and_running_flag() {
    let dir = tempfile::tempdir().unwrap();
    // HeartbeatConfig::new(15) → 15min = 900s.
    let svc = HeartbeatService::new(HeartbeatConfig::new(
        15,
        true,
        dir.path().to_string_lossy().to_string(),
    ));
    assert_eq!(svc.status()["interval_secs"], serde_json::json!(900));
    assert_eq!(svc.status()["running"], serde_json::json!(false));
    assert_eq!(svc.status()["enabled"], serde_json::json!(true));
}

#[test]
fn test_w1b_config_interval_boundaries() {
    // 4 → clamped up to the 5-minute floor; 5 stays exactly 5; 6 passes.
    assert_eq!(
        HeartbeatConfig::new(4, true, "/tmp/x".into()).interval,
        Duration::from_secs(5 * 60)
    );
    assert_eq!(
        HeartbeatConfig::new(5, true, "/tmp/x".into()).interval,
        Duration::from_secs(5 * 60)
    );
    assert_eq!(
        HeartbeatConfig::new(6, true, "/tmp/x".into()).interval,
        Duration::from_secs(6 * 60)
    );
    assert_eq!(
        HeartbeatConfig::new(1, true, "/tmp/x".into()).interval,
        Duration::from_secs(5 * 60)
    );
}

#[test]
fn test_w1b_build_prompt_non_utf8_lossy() {
    // Invalid UTF-8 bytes in HEARTBEAT.md must not panic; lossy replacement
    // keeps the (non-comment) line so the prompt is still built.
    let dir = tempfile::tempdir().unwrap();
    let mut raw: Vec<u8> = b"- task \xFF\xFE ends\n".to_vec();
    raw.extend_from_slice(b"- plain task\n");
    std::fs::write(dir.path().join("HEARTBEAT.md"), raw).unwrap();

    let svc = HeartbeatService::new(ws_config(&dir));
    let prompt = svc.build_prompt();
    assert!(prompt.contains("Heartbeat Check"));
    assert!(prompt.contains("plain task"));
    assert!(
        prompt.contains("\u{FFFD}"),
        "lossy replacement char present"
    );
}

#[test]
fn test_w1b_create_default_template_write_failure_no_panic() {
    // Workspace on an invalid path (NUL byte): every fs op fails — template
    // creation must swallow the error instead of panicking.
    let svc = HeartbeatService::new(HeartbeatConfig {
        workspace: Some("bad\0path".to_string()),
        ..Default::default()
    });
    svc.create_default_heartbeat_template();
    let prompt = svc.build_prompt();
    assert!(prompt.is_empty(), "unreadable workspace → empty prompt");
}

#[test]
fn test_w1b_should_skip_reflects_file_appearance() {
    let dir = tempfile::tempdir().unwrap();
    let skip = dir.path().join("BOOTSTRAP.md");
    let svc = HeartbeatService::new(HeartbeatConfig::default());
    svc.set_skip_file(skip.to_string_lossy().to_string());
    assert!(!svc.should_skip());
    std::fs::write(&skip, "on").unwrap();
    assert!(svc.should_skip(), "skip flag is a live file check");
    std::fs::remove_file(&skip).unwrap();
    assert!(
        !svc.should_skip(),
        "and clears again when the file goes away"
    );
}

#[tokio::test]
async fn test_w1b_tick_error_and_async_results_publish_nothing_but_track_beats() {
    let dir = tempfile::tempdir().unwrap();
    write_tasks(&dir, "- Task\n");
    let svc = HeartbeatService::new(ws_config(&dir));

    let (mock_bus, sent) = W1bBus::new();
    svc.set_bus(Arc::new(mock_bus));
    svc.set_state_manager(Arc::new(W1bState {
        last_channel: "telegram:9".to_string(),
    }));

    // First tick returns is_error, later ticks is_async — both branches return
    // before the send step, and neither is `silent`, so this also proves the
    // branch order (error/async take precedence over delivery).
    let calls = Arc::new(AtomicU64::new(0));
    let c2 = calls.clone();
    svc.set_handler(Box::new(move |_p, _c, _ch| {
        let n = c2.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Some(HeartbeatResult {
                is_error: true,
                is_async: false,
                silent: false,
                for_user: "SHOULD NOT SEND (error)".to_string(),
                for_llm: "boom".to_string(),
            })
        } else {
            Some(HeartbeatResult {
                is_error: false,
                is_async: true,
                silent: false,
                for_user: "SHOULD NOT SEND (async)".to_string(),
                for_llm: "spawned".to_string(),
            })
        }
    }));

    let beat0 = svc.beat_count();
    svc.start().await.unwrap();
    // First beat lands ~1s in; a second one follows immediately (the interval
    // ticker's first tick completes right away), then every 100ms.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    svc.stop();

    assert!(
        sent.lock().is_empty(),
        "error and async branches must not publish"
    );
    assert!(
        svc.beat_count() >= beat0 + 2,
        "beat tracking advances even when the result is an error/async, got {}→{}",
        beat0,
        svc.beat_count()
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "handler ran for both branches"
    );
}

#[tokio::test]
async fn test_w1b_restart_after_stop_keeps_handler() {
    // BUG #17 regression: start() moves the handler into the spawned task, so
    // a stop()+start() cycle used to leave the service running with NO handler
    // (every tick just logged "handler not configured"). The handler must
    // survive a restart.
    let dir = tempfile::tempdir().unwrap();
    write_tasks(&dir, "- Task\n");
    let svc = HeartbeatService::new(ws_config(&dir));

    let called = Arc::new(AtomicU64::new(0));
    let c2 = called.clone();
    svc.set_handler(Box::new(move |_p, _c, _ch| {
        c2.fetch_add(1, Ordering::SeqCst);
        None
    }));

    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await; // first beat (~1s)
    svc.stop();
    let first_round = called.load(Ordering::SeqCst);
    assert!(first_round >= 1, "handler fired before stop");

    svc.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    svc.stop();

    assert!(
        called.load(Ordering::SeqCst) > first_round,
        "handler must still fire after a stop+start restart (got {} before, {} after)",
        first_round,
        called.load(Ordering::SeqCst)
    );
}
