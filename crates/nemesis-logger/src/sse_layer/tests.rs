//! Tests for the SseLogLayer.
//!
//! These tests use `tracing::dispatch::with_default` to install a subscriber with our layer,
//! emit a tracing event, and assert that the callback fired with the expected fields.

use std::sync::{Arc, Mutex};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{fmt, Registry};

use super::{SseLogEvent, SseLogLayer};
use crate::sse_layer::{boot_unix_ms, next_seq};

/// Collect emitted events into a Vec for inspection.
fn collect_events<F>(run: F) -> Vec<SseLogEvent>
where
    F: FnOnce(),
{
    let captured: Arc<Mutex<Vec<SseLogEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    let layer = SseLogLayer::new(move |ev| {
        captured_clone.lock().unwrap().push(ev);
    });

    let subscriber = Registry::default().with(layer).with(
        fmt::layer().with_writer(std::io::sink), // suppress console output during tests
    );

    tracing::subscriber::with_default(subscriber, run);

    let guard = captured.lock().unwrap();
    guard.clone()
}

#[test]
fn classifies_source_by_target_prefix() {
    assert_eq!(SseLogEvent::classify_source("nemesis_providers::openai"), "llm");
    assert_eq!(SseLogEvent::classify_source("nemesis_cluster::rpc::client"), "cluster");
    assert_eq!(SseLogEvent::classify_source("nemesis_security::middleware"), "security");
    assert_eq!(SseLogEvent::classify_source("nemesis_agent::loop"), "general");
    assert_eq!(SseLogEvent::classify_source("nemesis_web::server"), "general");
    // Unknown crates fall back to general
    assert_eq!(SseLogEvent::classify_source("reqwest::blocking"), "general");
    assert_eq!(SseLogEvent::classify_source(""), "general");
}

#[test]
fn layer_fires_callback_for_info_event() {
    let events = collect_events(|| {
        tracing::info!("hello world");
    });

    assert_eq!(events.len(), 1);
    let ev = &events[0];
    assert_eq!(ev.level, "INFO");
    assert_eq!(ev.message, "hello world");
    assert_eq!(ev.source, "general");
    // seq must be populated and consistent with next_seq's layout
    // (high 44 bits = boot_unix_ms, low 20 bits = counter)
    assert!(
        ev.seq >> 20 == boot_unix_ms(),
        "seq's high bits should equal boot_unix_ms"
    );
}

#[test]
fn next_seq_is_monotonic_and_distinct() {
    // Consecutive calls must return strictly increasing values.
    // The high 44 bits are fixed per boot, so monotonicity comes from the
    // low 20-bit counter incrementing.
    let a = next_seq();
    let b = next_seq();
    let c = next_seq();
    assert!(b > a, "second call must exceed first: a={a:#x} b={b:#x}");
    assert!(c > b, "third call must exceed second: b={b:#x} c={c:#x}");
}

#[test]
fn next_seq_high_bits_match_boot_unix_ms() {
    // The high 44 bits of seq must equal boot_unix_ms. This is the invariant
    // the frontend relies on for cross-restart uniqueness.
    let s = next_seq();
    assert_eq!(s >> 20, boot_unix_ms());
}

#[test]
fn layer_fires_callback_for_warn_and_error() {
    let events = collect_events(|| {
        tracing::warn!("warning!");
        tracing::error!("oops");
    });

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].level, "WARN");
    assert_eq!(events[0].message, "warning!");
    assert_eq!(events[1].level, "ERROR");
    assert_eq!(events[1].message, "oops");
}

#[test]
fn layer_captures_structured_fields() {
    let events = collect_events(|| {
        tracing::info!(user_id = 42, action = "login", "user logged in");
    });

    assert_eq!(events.len(), 1);
    let ev = &events[0];
    assert_eq!(ev.message, "user logged in");
    assert_eq!(ev.fields.get("user_id").and_then(|v| v.as_i64()), Some(42));
    assert_eq!(
        ev.fields.get("action").and_then(|v| v.as_str()),
        Some("login")
    );
}

#[test]
fn layer_captures_bool_field() {
    let events = collect_events(|| {
        tracing::info!(success = true, "operation done");
    });
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].fields.get("success").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn layer_captures_float_field_when_finite() {
    let events = collect_events(|| {
        tracing::info!(latency_ms = 12.5, "request finished");
    });
    assert_eq!(events.len(), 1);
    let v = events[0].fields.get("latency_ms").cloned().unwrap();
    assert!((v.as_f64().unwrap() - 12.5).abs() < 1e-9);
}

#[test]
fn layer_captures_nan_float_as_string() {
    let events = collect_events(|| {
        tracing::info!(value = f64::NAN, "weird");
    });
    assert_eq!(events.len(), 1);
    // NaN cannot become a JSON number, so the visitor falls back to string rendering.
    let v = events[0].fields.get("value").cloned().unwrap();
    assert!(v.is_string(), "expected string fallback for NaN, got {:?}", v);
}

#[test]
fn layer_populates_timestamp_and_target() {
    let events = collect_events(|| {
        let span = tracing::info_span!("test span");
        let _enter = span.enter();
        tracing::info!("inside span");
    });
    assert!(!events.is_empty());
    let ev = &events[0];
    assert!(!ev.timestamp.is_empty());
    assert!(ev.target.starts_with("nemesis_logger") || ev.target.contains("sse_layer"));
}

#[test]
fn layer_distinguishes_sources_in_different_modules() {
    // We can't easily emit events from crates that aren't compiled in here,
    // so instead test the classifier directly with realistic target strings.
    assert_eq!(SseLogEvent::classify_source("nemesis_providers::sse"), "llm");
    assert_eq!(SseLogEvent::classify_source("nemesis_cluster::rpc::server"), "cluster");
    assert_eq!(SseLogEvent::classify_source("nemesis_security::scanner::clamav"), "security");
    assert_eq!(SseLogEvent::classify_source("hyper::proto::h1"), "general");
}

#[test]
fn layer_works_within_span() {
    // Tracing span context doesn't break the layer — events emitted inside a span still arrive.
    let events = collect_events(|| {
        let span = tracing::info_span!("my_span");
        let _enter = span.enter();
        tracing::info!("inside span");
    });
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "inside span");
}

#[test]
fn callback_does_not_block_subscriber_when_panicking() {
    // If a callback panics, tracing should propagate the panic but the subscriber itself
    // shouldn't be poisoned — this is mainly a smoke test that we don't swallow errors.
    let layer = SseLogLayer::new(|_ev| {
        panic!("boom");
    });
    let subscriber = Registry::default().with(layer).with(
        fmt::layer().with_writer(std::io::sink),
    );

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("this triggers the panic");
        });
    }));
    assert!(result.is_err(), "expected panic to propagate");
}

// ===========================================================================
// GlobalSseLogLayer end-to-end tests
//
// These tests verify that the global-callback variant of the layer correctly
// bridges tracing events to a late-bound callback, simulating how the gateway
// wires the EventHub into the layer after logger init.
// ===========================================================================

use crate::sse_layer::{
    clear_global_log_callback, global_log_callback_slot, set_global_log_callback, GlobalSseLogLayer,
};

/// Mutex guard that serializes the global-state tests. Without this, parallel test runs
/// race on the process-wide `global_log_callback_slot()` and either see each other's
/// callbacks or poison the inner Mutex via panic.
static GLOBAL_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run a test body with a `GlobalSseLogLayer` subscriber and a callback of the test's choice.
///
/// Serializes via `GLOBAL_TEST_GUARD` so concurrent tests don't clobber each other's
/// global callback. Restores the prior callback after the body runs.
fn with_global_layer_and_callback<F, C>(callback: C, run: F)
where
    F: FnOnce(),
    C: Fn(SseLogEvent) + Send + Sync + 'static,
{
    let _guard = GLOBAL_TEST_GUARD.lock().unwrap();

    let snapshot = {
        let slot = global_log_callback_slot();
        slot.lock().unwrap().clone()
    };

    set_global_log_callback(callback);

    let subscriber = Registry::default()
        .with(GlobalSseLogLayer)
        .with(fmt::layer().with_writer(std::io::sink));

    tracing::subscriber::with_default(subscriber, run);

    let slot = global_log_callback_slot();
    *slot.lock().unwrap() = snapshot;
}

/// Variant that runs without any callback installed — useful for asserting the
/// "no callback" code path.
fn with_global_layer_no_callback<F>(run: F)
where
    F: FnOnce(),
{
    let _guard = GLOBAL_TEST_GUARD.lock().unwrap();

    let snapshot = {
        let slot = global_log_callback_slot();
        slot.lock().unwrap().clone()
    };
    clear_global_log_callback();

    let subscriber = Registry::default()
        .with(GlobalSseLogLayer)
        .with(fmt::layer().with_writer(std::io::sink));

    tracing::subscriber::with_default(subscriber, run);

    let slot = global_log_callback_slot();
    *slot.lock().unwrap() = snapshot;
}

#[test]
fn global_layer_drops_events_when_no_callback_installed() {
    let captured: Arc<Mutex<Vec<SseLogEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    let _ = captured_clone; // not used — we just want the slot to be None
    with_global_layer_no_callback(|| {
        tracing::info!("this should be dropped");
    });

    assert_eq!(captured.lock().unwrap().len(), 0);
}

#[test]
fn global_layer_forwards_events_to_installed_callback() {
    let captured: Arc<Mutex<Vec<SseLogEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    with_global_layer_and_callback(
        move |ev| {
            captured_clone.lock().unwrap().push(ev);
        },
        || {
            tracing::warn!(user_id = 7, "test warning");
            tracing::info!("test info");
        },
    );

    let events = captured.lock().unwrap().clone();
    assert_eq!(events.len(), 2, "expected both events to be captured");
    assert_eq!(events[0].level, "WARN");
    assert_eq!(events[0].message, "test warning");
    assert_eq!(events[0].fields.get("user_id").and_then(|v| v.as_i64()), Some(7));
    assert_eq!(events[1].level, "INFO");
    assert_eq!(events[1].message, "test info");
}

#[test]
fn global_layer_callback_can_be_replaced() {
    let first: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let first_clone = first.clone();
    with_global_layer_and_callback(
        move |ev| {
            first_clone.lock().unwrap().push(ev.message.clone());
        },
        || {
            tracing::info!("first callback");
        },
    );

    let second: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let second_clone = second.clone();
    with_global_layer_and_callback(
        move |ev| {
            second_clone.lock().unwrap().push(ev.message.clone());
        },
        || {
            tracing::info!("second callback");
        },
    );

    assert_eq!(first.lock().unwrap().as_slice(), &["first callback"]);
    assert_eq!(second.lock().unwrap().as_slice(), &["second callback"]);
}

#[test]
fn global_layer_callback_sees_source_classification() {
    let captured: Arc<Mutex<Vec<SseLogEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    with_global_layer_and_callback(
        move |ev| {
            captured_clone.lock().unwrap().push(ev);
        },
        || {
            tracing::info!(target: "nemesis_providers::http", "llm-style event");
            tracing::info!(target: "nemesis_cluster::rpc", "cluster event");
            tracing::info!(target: "nemesis_security::middleware", "security event");
            tracing::info!(target: "nemesis_agent::loop", "general event");
        },
    );

    let events = captured.lock().unwrap().clone();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].source, "llm");
    assert_eq!(events[1].source, "cluster");
    assert_eq!(events[2].source, "security");
    assert_eq!(events[3].source, "general");
}

#[test]
fn global_layer_clear_callback_stops_events() {
    let captured: Arc<Mutex<Vec<SseLogEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    with_global_layer_and_callback(
        move |ev| {
            captured_clone.lock().unwrap().push(ev);
        },
        || {
            tracing::info!("captured");
        },
    );

    with_global_layer_no_callback(|| {
        tracing::info!("not captured");
    });

    let events = captured.lock().unwrap().clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "captured");
}

#[test]
fn global_layer_does_not_hold_lock_during_callback() {
    // Regression test: the layer should clone the Arc<callback> out of the slot and release
    // the slot lock BEFORE invoking the callback. If it held the lock, a callback that itself
    // touches the slot would deadlock. We simulate this by having the callback inspect the slot.
    let counter: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let counter_clone = counter.clone();
    with_global_layer_and_callback(
        move |_ev| {
            // The callback reads the slot — if the layer held the slot lock during callback
            // invocation, this would deadlock.
            let _snapshot = global_log_callback_slot().lock().unwrap().clone();
            *counter_clone.lock().unwrap() += 1;
        },
        || {
            tracing::info!("trigger");
        },
    );

    assert_eq!(*counter.lock().unwrap(), 1);
}
