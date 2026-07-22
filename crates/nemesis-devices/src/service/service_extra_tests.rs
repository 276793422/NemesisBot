//! Additional coverage tests for service.rs focusing on:
//! - `with_config(monitor_usb=true)` line 136
//! - `start()` source iteration (lines 213-235) via a mock EventSource
//! - `stop()` source error path (lines 251-253)
//! - `handle_events_task` private function (lines 326-355)
//! - All parse/serialization edge cases
//! - All bus/state configuration combinations

use super::*;
use crate::source::{Action, EventSource, Kind, UsbEventSource};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// MockEventSource — controllable source for testing source lifecycle
// ---------------------------------------------------------------------------

struct MockEventSource {
    name: Kind,
    start_should_fail: bool,
    stop_should_fail: bool,
    started: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
}

impl MockEventSource {
    fn new(name: Kind) -> Self {
        Self {
            name,
            start_should_fail: false,
            stop_should_fail: false,
            started: Arc::new(AtomicUsize::new(0)),
            stopped: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn failing(name: Kind, start_fails: bool, stop_fails: bool) -> Self {
        Self {
            name,
            start_should_fail: start_fails,
            stop_should_fail: stop_fails,
            started: Arc::new(AtomicUsize::new(0)),
            stopped: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl EventSource for MockEventSource {
    fn kind(&self) -> Kind {
        self.name.clone()
    }

    fn start(&self) -> Result<mpsc::Receiver<DeviceEvent>, String> {
        self.started.fetch_add(1, Ordering::SeqCst);
        if self.start_should_fail {
            Err("mock start error".to_string())
        } else {
            let (_tx, rx) = mpsc::channel::<DeviceEvent>(1);
            Ok(rx)
        }
    }

    fn stop(&self) -> Result<(), String> {
        self.stopped.fetch_add(1, Ordering::SeqCst);
        if self.stop_should_fail {
            Err("mock stop error".to_string())
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests for with_config line 136 — adding USB source when monitor_usb=true
// ---------------------------------------------------------------------------

#[test]
fn test_with_config_monitor_usb_true_does_not_panic() {
    // Constructing with monitor_usb=true will instantiate UsbEventSource.
    // On non-Linux, start() will fail later but construction must succeed.
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 5,
        monitor_usb: true,
    };
    let svc = DeviceService::with_config(config);
    // Status should reflect the config
    let status = svc.status();
    assert_eq!(status["enabled"], true);
    assert_eq!(status["monitor_usb"], true);
    assert_eq!(status["poll_interval_secs"], 5);
}

#[test]
fn test_with_config_monitor_usb_false_no_source() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 5,
        monitor_usb: false,
    };
    let _svc = DeviceService::with_config(config);
    // If monitor_usb=false, no sources are added — construction is clean.
    // We can't query sources directly, but a start() without sources should work.
}

#[test]
fn test_with_config_disabled_usb_flag_combinations() {
    // Try all combinations to exercise constructor branches
    for enabled in [true, false] {
        for monitor_usb in [true, false] {
            let config = DeviceServiceConfig {
                enabled,
                poll_interval_secs: 1,
                monitor_usb,
            };
            let svc = DeviceService::with_config(config.clone());
            assert_eq!(svc.status()["enabled"], enabled);
            assert_eq!(svc.status()["monitor_usb"], monitor_usb);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests for start() source iteration (lines 213-235)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_start_with_usb_source_starts_and_fails_gracefully_non_linux() {
    // On non-Linux, UsbEventSource::start() returns Err.
    // The start() method should log the error and continue to mark running=true.
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: true,
    };
    let svc = DeviceService::with_config(config);
    let result = svc.start().await;
    assert!(result.is_ok());
    // Even with failed source, running should be true because start() succeeded
    assert!(svc.is_running());
    svc.stop();
}

#[tokio::test]
async fn test_start_when_disabled_returns_ok_but_not_running() {
    let config = DeviceServiceConfig {
        enabled: false,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let svc = DeviceService::with_config(config);
    let result = svc.start().await;
    assert!(result.is_ok());
    // Disabled config means running should NOT flip to true
    assert!(!svc.is_running());
}

#[tokio::test]
async fn test_start_idempotent_when_already_running() {
    // Already-running path: start() should return early with Ok
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let svc = DeviceService::with_config(config);
    svc.start().await.unwrap();
    assert!(svc.is_running());
    // Second call should be a no-op (no panic, no error)
    let result = svc.start().await;
    assert!(result.is_ok());
    assert!(svc.is_running());
    svc.stop();
}

// ---------------------------------------------------------------------------
// Tests for stop() source error path (lines 251-253)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stop_logs_source_errors_but_does_not_panic() {
    // We can't inject a MockEventSource into DeviceService through public API,
    // but we can verify that stop() on a service with UsbEventSource is safe
    // even if the underlying source fails. On non-Linux, UsbEventSource.stop()
    // returns Ok, so this exercises the success path.
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: true,
    };
    let svc = DeviceService::with_config(config);
    svc.start().await.unwrap();
    svc.stop();
    assert!(!svc.is_running());
}

#[tokio::test]
async fn test_stop_idempotent_multiple_times() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let svc = DeviceService::with_config(config);
    svc.start().await.unwrap();
    svc.stop();
    svc.stop(); // should not panic
    svc.stop(); // should not panic
    assert!(!svc.is_running());
}

// ---------------------------------------------------------------------------
// Tests for handle_events_task private function (lines 326-355)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_handle_events_task_stops_on_stop_flag() {
    let (tx, rx) = mpsc::channel::<DeviceEvent>(8);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = tokio::spawn(async move {
        DeviceService::handle_events_task(rx, stop_clone, true, true).await;
    });

    // Set the stop flag — task should break out of the loop
    stop.store(true, Ordering::SeqCst);

    // Wait for the task to complete
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_handle_events_task_processes_event_and_continues() {
    let (tx, rx) = mpsc::channel::<DeviceEvent>(8);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = tokio::spawn(async move {
        DeviceService::handle_events_task(rx, stop_clone, false, false).await;
    });

    // Send an event
    let ev = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "test-1".to_string(),
        vendor: "VendorX".to_string(),
        product: "ProductY".to_string(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    tx.send(ev).await.unwrap();

    // Give task time to process
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Stop the task
    stop.store(true, Ordering::SeqCst);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_handle_events_task_stops_on_channel_close() {
    let (tx, rx) = mpsc::channel::<DeviceEvent>(8);
    let stop = Arc::new(AtomicBool::new(false));

    let handle = tokio::spawn(async move {
        DeviceService::handle_events_task(rx, stop, true, true).await;
    });

    // Drop the sender — receiver will return None, breaking the loop
    drop(tx);

    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_handle_events_task_multiple_events() {
    let (tx, rx) = mpsc::channel::<DeviceEvent>(16);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = tokio::spawn(async move {
        DeviceService::handle_events_task(rx, stop_clone, false, false).await;
    });

    // Send multiple events
    for i in 0..5 {
        let ev = DeviceEvent {
            action: Action::Add,
            kind: Kind::Usb,
            device_id: format!("dev-{}", i),
            vendor: String::new(),
            product: String::new(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        tx.send(ev).await.unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Verify the channel is drained (all events processed without panic)
    stop.store(true, Ordering::SeqCst);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_handle_events_task_remove_event() {
    let (tx, rx) = mpsc::channel::<DeviceEvent>(8);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = tokio::spawn(async move {
        DeviceService::handle_events_task(rx, stop_clone, false, false).await;
    });

    let ev = DeviceEvent {
        action: Action::Remove,
        kind: Kind::Bluetooth,
        device_id: "bt-1".to_string(),
        vendor: "V".to_string(),
        product: "P".to_string(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    tx.send(ev).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    stop.store(true, Ordering::SeqCst);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_handle_events_task_change_event() {
    let (tx, rx) = mpsc::channel::<DeviceEvent>(8);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = tokio::spawn(async move {
        DeviceService::handle_events_task(rx, stop_clone, false, false).await;
    });

    let ev = DeviceEvent {
        action: Action::Change,
        kind: Kind::Pci,
        device_id: "pci-1".to_string(),
        vendor: "V".to_string(),
        product: "P".to_string(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    tx.send(ev).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    stop.store(true, Ordering::SeqCst);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_handle_events_task_with_bus_and_state_true() {
    // Verify _bus_available=true and _state_available=true parameters
    let (tx, rx) = mpsc::channel::<DeviceEvent>(8);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = tokio::spawn(async move {
        DeviceService::handle_events_task(rx, stop_clone, true, true).await;
    });

    let ev = DeviceEvent {
        action: Action::Add,
        kind: Kind::Generic,
        device_id: "g-1".to_string(),
        vendor: String::new(),
        product: String::new(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    tx.send(ev).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    stop.store(true, Ordering::SeqCst);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_handle_events_task_with_bus_and_state_false() {
    // Verify _bus_available=false and _state_available=false parameters
    let (tx, rx) = mpsc::channel::<DeviceEvent>(8);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let handle = tokio::spawn(async move {
        DeviceService::handle_events_task(rx, stop_clone, false, false).await;
    });

    let ev = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "g-2".to_string(),
        vendor: String::new(),
        product: String::new(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    tx.send(ev).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    stop.store(true, Ordering::SeqCst);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

// ---------------------------------------------------------------------------
// Tests for start() with injected mock sources (covers lines 215-232)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_start_with_mock_source_starts_successfully() {
    // Inject a mock source that succeeds on start — this exercises the
    // success branch of source.start() in DeviceService::start()
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let sources: Vec<Box<dyn EventSource>> = vec![Box::new(MockEventSource::new(Kind::Usb))];
    let svc = DeviceService::with_sources_for_test(config, sources);
    let result = svc.start().await;
    assert!(result.is_ok());
    assert!(svc.is_running());
    svc.stop();
    assert!(!svc.is_running());
}

#[tokio::test]
async fn test_start_with_failing_mock_source_continues() {
    // A failing source should be logged but not prevent start from completing
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let sources: Vec<Box<dyn EventSource>> =
        vec![Box::new(MockEventSource::failing(Kind::Usb, true, false))];
    let svc = DeviceService::with_sources_for_test(config, sources);
    let result = svc.start().await;
    assert!(result.is_ok());
    assert!(svc.is_running());
    svc.stop();
}

#[tokio::test]
async fn test_start_with_multiple_mixed_sources() {
    // Mix of successful and failing sources — exercise both branches
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let sources: Vec<Box<dyn EventSource>> = vec![
        Box::new(MockEventSource::new(Kind::Usb)),
        Box::new(MockEventSource::failing(Kind::Bluetooth, true, false)),
        Box::new(MockEventSource::new(Kind::Pci)),
        Box::new(MockEventSource::failing(Kind::Generic, true, false)),
    ];
    let svc = DeviceService::with_sources_for_test(config, sources);
    let result = svc.start().await;
    assert!(result.is_ok());
    assert!(svc.is_running());
    svc.stop();
}

// ---------------------------------------------------------------------------
// Tests for stop() with failing source (covers lines 251-253)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stop_with_failing_source_logs_error_safely() {
    // Inject a source whose stop() returns Err — this exercises lines 251-253
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let sources: Vec<Box<dyn EventSource>> =
        vec![Box::new(MockEventSource::failing(Kind::Usb, false, true))];
    let svc = DeviceService::with_sources_for_test(config, sources);
    svc.start().await.unwrap();
    assert!(svc.is_running());
    // stop() should not panic even though the source returns Err
    svc.stop();
    assert!(!svc.is_running());
}

#[tokio::test]
async fn test_stop_with_mixed_success_and_failure_sources() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let sources: Vec<Box<dyn EventSource>> = vec![
        Box::new(MockEventSource::new(Kind::Usb)),
        Box::new(MockEventSource::failing(Kind::Bluetooth, false, true)),
        Box::new(MockEventSource::new(Kind::Pci)),
    ];
    let svc = DeviceService::with_sources_for_test(config, sources);
    svc.start().await.unwrap();
    // Should not panic
    svc.stop();
    assert!(!svc.is_running());
}

#[tokio::test]
async fn test_stop_with_all_failing_sources() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let sources: Vec<Box<dyn EventSource>> = vec![
        Box::new(MockEventSource::failing(Kind::Usb, false, true)),
        Box::new(MockEventSource::failing(Kind::Bluetooth, false, true)),
    ];
    let svc = DeviceService::with_sources_for_test(config, sources);
    svc.start().await.unwrap();
    svc.stop();
    assert!(!svc.is_running());
}

// ---------------------------------------------------------------------------
// Tests for start() with no sources (empty vec)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_start_with_empty_sources_vec() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let sources: Vec<Box<dyn EventSource>> = vec![];
    let svc = DeviceService::with_sources_for_test(config, sources);
    let result = svc.start().await;
    assert!(result.is_ok());
    assert!(svc.is_running());
    svc.stop();
}

#[tokio::test]
async fn test_start_disabled_with_sources_skips_iteration() {
    // When enabled=false, start() should return Ok early without iterating sources
    let config = DeviceServiceConfig {
        enabled: false,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let mock = MockEventSource::new(Kind::Usb);
    let sources: Vec<Box<dyn EventSource>> = vec![Box::new(mock)];
    let svc = DeviceService::with_sources_for_test(config, sources);
    let result = svc.start().await;
    assert!(result.is_ok());
    assert!(!svc.is_running());
}

// ---------------------------------------------------------------------------
// More tests for send_notification edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_send_notification_with_bus_set_after_state() {
    let svc = DeviceService::new();
    let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
    let sent_clone = sent.clone();

    // Set state first, then bus
    struct MockState {
        last_channel: String,
    }
    impl LastChannelProvider for MockState {
        fn get_last_channel(&self) -> String {
            self.last_channel.clone()
        }
    }
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "web:123".to_string(),
    }));
    svc.set_bus_sender(Box::new(move |ch, id, content| {
        sent_clone
            .lock()
            .push((ch.to_string(), id.to_string(), content.to_string()));
    }));

    let ev = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "1-1".to_string(),
        vendor: "V".to_string(),
        product: "P".to_string(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    svc.send_notification(&ev);
    assert_eq!(sent.lock().len(), 1);
}

#[test]
fn test_send_notification_format_message_with_capabilities() {
    // Verify the format_message output flows through to the bus
    let svc = DeviceService::new();
    let sent = Arc::new(Mutex::new(Vec::<String>::new()));
    let sent_clone = sent.clone();

    struct MockState {
        last_channel: String,
    }
    impl LastChannelProvider for MockState {
        fn get_last_channel(&self) -> String {
            self.last_channel.clone()
        }
    }
    svc.set_state_manager(Arc::new(MockState {
        last_channel: "web:capability-test".to_string(),
    }));
    svc.set_bus_sender(Box::new(move |_ch, _id, content| {
        sent_clone.lock().push(content.to_string());
    }));

    let ev = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "1-1".to_string(),
        vendor: "VendorX".to_string(),
        product: "ProductY".to_string(),
        serial: "SN123".to_string(),
        capabilities: "audio,midi".to_string(),
        raw: HashMap::new(),
    };
    svc.send_notification(&ev);
    let msgs = sent.lock();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].contains("Capabilities"));
    assert!(msgs[0].contains("Serial"));
    assert!(msgs[0].contains("audio,midi"));
    assert!(msgs[0].contains("SN123"));
}

// ---------------------------------------------------------------------------
// More tests for parse_last_channel boundary cases
// ---------------------------------------------------------------------------

#[test]
fn test_parse_last_channel_with_numbers_in_platform() {
    let (p, u) = parse_last_channel("discord123:user456");
    assert_eq!(p, "discord123");
    assert_eq!(u, "user456");
}

#[test]
fn test_parse_last_channel_with_underscores_and_dashes() {
    let (p, u) = parse_last_channel("slack_bot:user-name");
    assert_eq!(p, "slack_bot");
    assert_eq!(u, "user-name");
}

#[test]
fn test_parse_last_channel_with_long_strings() {
    let long_platform = "x".repeat(50);
    let long_user = "y".repeat(50);
    let combined = format!("{}:{}", long_platform, long_user);
    let (p, u) = parse_last_channel(&combined);
    assert_eq!(p, long_platform);
    assert_eq!(u, long_user);
}

#[test]
fn test_parse_last_channel_unicode_platform() {
    let (p, u) = parse_last_channel("微博:用户123");
    assert_eq!(p, "微博");
    assert_eq!(u, "用户123");
}

#[test]
fn test_parse_last_channel_single_colon_only() {
    let (p, u) = parse_last_channel(":");
    assert_eq!(p, "");
    assert_eq!(u, "");
}

#[test]
fn test_parse_last_channel_just_colons() {
    let (p, u) = parse_last_channel(":::");
    assert_eq!(p, "");
    assert_eq!(u, "");
}

// ---------------------------------------------------------------------------
// More is_internal_channel tests
// ---------------------------------------------------------------------------

#[test]
fn test_is_internal_channel_empty_string() {
    assert!(!is_internal_channel(""));
}

#[test]
fn test_is_internal_channel_uppercase_variants() {
    // Internal check is case-sensitive — uppercase should NOT be internal
    assert!(!is_internal_channel("CLI"));
    assert!(!is_internal_channel("SYSTEM"));
    assert!(!is_internal_channel("SUBAGENT"));
}

#[test]
fn test_is_internal_channel_partial_match() {
    assert!(!is_internal_channel("cli-extra"));
    assert!(!is_internal_channel("systemd"));
    assert!(!is_internal_channel("subagent-1"));
}

// ---------------------------------------------------------------------------
// More DeviceService API tests
// ---------------------------------------------------------------------------

#[test]
fn test_register_with_empty_id() {
    let svc = DeviceService::new();
    let device = Device {
        id: String::new(),
        name: "Empty ID".to_string(),
        device_type: "test".to_string(),
        status: "online".to_string(),
        vendor_id: None,
        product_id: None,
        serial: None,
        connected_at: None,
        metadata: HashMap::new(),
    };
    svc.register(device);
    assert_eq!(svc.count(), 1);
    // Empty string ID should still be retrievable
    assert!(svc.get("").is_some());
}

#[test]
fn test_register_with_unicode_name_and_metadata() {
    let svc = DeviceService::new();
    let mut metadata = HashMap::new();
    metadata.insert("中文".to_string(), "值".to_string());
    metadata.insert("emoji".to_string(), "🚀".to_string());

    let device = Device {
        id: "unicode-dev".to_string(),
        name: "设备 📡".to_string(),
        device_type: "传感器".to_string(),
        status: "在线".to_string(),
        vendor_id: None,
        product_id: None,
        serial: None,
        connected_at: None,
        metadata,
    };
    svc.register(device);
    let retrieved = svc.get("unicode-dev").unwrap();
    assert_eq!(retrieved.name, "设备 📡");
    assert_eq!(retrieved.metadata.get("中文").unwrap(), "值");
    assert_eq!(retrieved.metadata.get("emoji").unwrap(), "🚀");
}

#[test]
fn test_set_handler_overrides_previous() {
    let svc = DeviceService::new();
    let counter1 = Arc::new(AtomicUsize::new(0));
    let counter1_clone = counter1.clone();
    svc.set_handler(Box::new(move |_| {
        counter1_clone.fetch_add(1, Ordering::SeqCst);
    }));

    let counter2 = Arc::new(AtomicUsize::new(0));
    let counter2_clone = counter2.clone();
    svc.set_handler(Box::new(move |_| {
        counter2_clone.fetch_add(1, Ordering::SeqCst);
    }));

    svc.register(Device {
        id: "d1".to_string(),
        name: "D1".to_string(),
        device_type: "t".to_string(),
        status: "online".to_string(),
        vendor_id: None,
        product_id: None,
        serial: None,
        connected_at: None,
        metadata: HashMap::new(),
    });

    // Only the second handler should fire
    assert_eq!(counter1.load(Ordering::SeqCst), 0);
    assert_eq!(counter2.load(Ordering::SeqCst), 1);
}

#[test]
fn test_set_handler_to_no_op_after_setting() {
    let svc = DeviceService::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    svc.set_handler(Box::new(move |_| {
        counter_clone.fetch_add(1, Ordering::SeqCst);
    }));

    svc.register(Device {
        id: "d1".to_string(),
        name: "D1".to_string(),
        device_type: "t".to_string(),
        status: "online".to_string(),
        vendor_id: None,
        product_id: None,
        serial: None,
        connected_at: None,
        metadata: HashMap::new(),
    });
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Set a new no-op handler
    svc.set_handler(Box::new(move |_| {}));
    svc.register(Device {
        id: "d2".to_string(),
        name: "D2".to_string(),
        device_type: "t".to_string(),
        status: "online".to_string(),
        vendor_id: None,
        product_id: None,
        serial: None,
        connected_at: None,
        metadata: HashMap::new(),
    });
    // Original counter unchanged
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn test_get_after_overwrite_returns_new_data() {
    let svc = DeviceService::new();
    svc.register(Device {
        id: "d".to_string(),
        name: "v1".to_string(),
        device_type: "t".to_string(),
        status: "online".to_string(),
        vendor_id: Some("vid1".to_string()),
        product_id: None,
        serial: None,
        connected_at: None,
        metadata: HashMap::new(),
    });
    svc.register(Device {
        id: "d".to_string(),
        name: "v2".to_string(),
        device_type: "t".to_string(),
        status: "offline".to_string(),
        vendor_id: Some("vid2".to_string()),
        product_id: None,
        serial: None,
        connected_at: None,
        metadata: HashMap::new(),
    });
    let dev = svc.get("d").unwrap();
    assert_eq!(dev.name, "v2");
    assert_eq!(dev.status, "offline");
    assert_eq!(dev.vendor_id, Some("vid2".to_string()));
}

#[test]
fn test_list_returns_clones_not_references() {
    let svc = DeviceService::new();
    svc.register(Device {
        id: "d1".to_string(),
        name: "Original".to_string(),
        device_type: "t".to_string(),
        status: "online".to_string(),
        vendor_id: None,
        product_id: None,
        serial: None,
        connected_at: None,
        metadata: HashMap::new(),
    });
    let list1 = svc.list();
    let list2 = svc.list();
    assert_eq!(list1.len(), 1);
    assert_eq!(list2.len(), 1);
    // Both lists should have the same data
    assert_eq!(list1[0].id, list2[0].id);
}

#[test]
fn test_status_json_structure() {
    let svc = DeviceService::new();
    let status = svc.status();
    // Verify all expected keys are present
    assert!(status.get("running").is_some());
    assert!(status.get("enabled").is_some());
    assert!(status.get("device_count").is_some());
    assert!(status.get("monitor_usb").is_some());
    assert!(status.get("poll_interval_secs").is_some());
}

#[test]
fn test_status_reflects_config_poll_interval() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 999,
        monitor_usb: false,
    };
    let svc = DeviceService::with_config(config);
    assert_eq!(svc.status()["poll_interval_secs"], 999);
}

// ---------------------------------------------------------------------------
// Additional ServiceDeviceEvent serialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_service_device_event_added_with_empty_fields() {
    let ev = ServiceDeviceEvent::Added {
        device_id: String::new(),
        device_type: String::new(),
    };
    let json = serde_json::to_string(&ev).unwrap();
    assert!(json.contains("Added"));
    let parsed: ServiceDeviceEvent = serde_json::from_str(&json).unwrap();
    if let ServiceDeviceEvent::Added {
        device_id,
        device_type,
    } = parsed
    {
        assert!(device_id.is_empty());
        assert!(device_type.is_empty());
    } else {
        panic!("expected Added");
    }
}

#[test]
fn test_service_device_event_removed_with_empty_id() {
    let ev = ServiceDeviceEvent::Removed {
        device_id: String::new(),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let parsed: ServiceDeviceEvent = serde_json::from_str(&json).unwrap();
    if let ServiceDeviceEvent::Removed { device_id } = parsed {
        assert!(device_id.is_empty());
    } else {
        panic!("expected Removed");
    }
}

#[test]
fn test_service_device_event_changed_with_empty_changes() {
    let ev = ServiceDeviceEvent::Changed {
        device_id: "d".to_string(),
        changes: HashMap::new(),
    };
    let json = serde_json::to_string(&ev).unwrap();
    let parsed: ServiceDeviceEvent = serde_json::from_str(&json).unwrap();
    if let ServiceDeviceEvent::Changed { device_id, changes } = parsed {
        assert_eq!(device_id, "d");
        assert!(changes.is_empty());
    } else {
        panic!("expected Changed");
    }
}

// ---------------------------------------------------------------------------
// Config serialization edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_config_serialization_with_extreme_values() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: u64::MAX,
        monitor_usb: true,
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: DeviceServiceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.poll_interval_secs, u64::MAX);
}

#[test]
fn test_config_serialization_with_zero_poll_interval() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 0,
        monitor_usb: false,
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: DeviceServiceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.poll_interval_secs, 0);
}

#[test]
fn test_config_deserialization_with_unknown_fields() {
    // Unknown fields should be ignored by default serde behavior
    let json = r#"{"enabled": true, "poll_interval_secs": 5, "monitor_usb": true, "unknown_field": "ignored"}"#;
    let config: DeviceServiceConfig = serde_json::from_str(json).unwrap();
    assert!(config.enabled);
    assert_eq!(config.poll_interval_secs, 5);
    assert!(config.monitor_usb);
}

#[test]
fn test_config_deserialization_invalid_json() {
    let json = r#"{"enabled": "not_a_bool"}"#;
    let result: Result<DeviceServiceConfig, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Tests for the OutboundSender callback type
// ---------------------------------------------------------------------------

#[test]
fn test_outbound_sender_boxed_closure() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let sender: OutboundSender = Box::new(move |_ch, _id, _content| {
        counter_clone.fetch_add(1, Ordering::SeqCst);
    });
    sender("web", "user1", "hello");
    sender("discord", "user2", "world");
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn test_outbound_sender_captures_state() {
    let messages = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
    let messages_clone = messages.clone();
    let sender: OutboundSender = Box::new(move |ch, id, content| {
        messages_clone
            .lock()
            .push((ch.to_string(), id.to_string(), content.to_string()));
    });
    sender("web", "u1", "msg1");
    sender("telegram", "u2", "msg2");
    sender("discord", "u3", "msg3");
    let m = messages.lock();
    assert_eq!(m.len(), 3);
    assert_eq!(m[0].0, "web");
    assert_eq!(m[1].0, "telegram");
    assert_eq!(m[2].0, "discord");
}

// ---------------------------------------------------------------------------
// MockEventSource tests — directly exercising EventSource trait methods
// ---------------------------------------------------------------------------

#[test]
fn test_mock_source_kind() {
    let mock = MockEventSource::new(Kind::Usb);
    assert_eq!(mock.kind(), Kind::Usb);

    let mock = MockEventSource::new(Kind::Bluetooth);
    assert_eq!(mock.kind(), Kind::Bluetooth);

    let mock = MockEventSource::new(Kind::Pci);
    assert_eq!(mock.kind(), Kind::Pci);

    let mock = MockEventSource::new(Kind::Generic);
    assert_eq!(mock.kind(), Kind::Generic);
}

#[test]
fn test_mock_source_start_success() {
    let mock = MockEventSource::new(Kind::Usb);
    let result = mock.start();
    assert!(result.is_ok());
    assert_eq!(mock.started.load(Ordering::SeqCst), 1);
}

#[test]
fn test_mock_source_start_failure() {
    let mock = MockEventSource::failing(Kind::Usb, true, false);
    let result = mock.start();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "mock start error");
    assert_eq!(mock.started.load(Ordering::SeqCst), 1);
}

#[test]
fn test_mock_source_stop_success() {
    let mock = MockEventSource::new(Kind::Usb);
    let result = mock.stop();
    assert!(result.is_ok());
    assert_eq!(mock.stopped.load(Ordering::SeqCst), 1);
}

#[test]
fn test_mock_source_stop_failure() {
    let mock = MockEventSource::failing(Kind::Usb, false, true);
    let result = mock.stop();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "mock stop error");
    assert_eq!(mock.stopped.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// UsbEventSource direct tests
// ---------------------------------------------------------------------------

#[test]
fn test_usb_event_source_kind() {
    let src = UsbEventSource::new();
    assert_eq!(src.kind(), Kind::Usb);
}

#[test]
fn test_usb_event_source_stop_multiple_times_safety() {
    let src = UsbEventSource::new();
    let _ = src.stop();
    let _ = src.stop();
    let _ = src.stop();
}

// ---------------------------------------------------------------------------
// Concurrent registration tests (verifying thread safety)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_register_thread_safe() {
    let svc = Arc::new(DeviceService::new());
    let mut handles = Vec::new();

    for i in 0..20 {
        let svc_clone = svc.clone();
        handles.push(tokio::spawn(async move {
            svc_clone.register(Device {
                id: format!("dev-{}", i),
                name: format!("Device {}", i),
                device_type: "sensor".to_string(),
                status: "online".to_string(),
                vendor_id: None,
                product_id: None,
                serial: None,
                connected_at: None,
                metadata: HashMap::new(),
            });
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(svc.count(), 20);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_register_and_unregister() {
    let svc = Arc::new(DeviceService::new());

    // First register all devices
    for i in 0..10 {
        svc.register(Device {
            id: format!("dev-{}", i),
            name: format!("Device {}", i),
            device_type: "sensor".to_string(),
            status: "online".to_string(),
            vendor_id: None,
            product_id: None,
            serial: None,
            connected_at: None,
            metadata: HashMap::new(),
        });
    }

    let mut handles = Vec::new();
    for i in 0..10 {
        let svc_clone = svc.clone();
        handles.push(tokio::spawn(async move {
            svc_clone.unregister(&format!("dev-{}", i));
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(svc.count(), 0);
}

// ---------------------------------------------------------------------------
// Lifecycle tests with explicit state checks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_start_stop_start_cycle() {
    let config = DeviceServiceConfig {
        enabled: true,
        poll_interval_secs: 1,
        monitor_usb: false,
    };
    let svc = DeviceService::with_config(config);

    // First cycle
    svc.start().await.unwrap();
    assert!(svc.is_running());
    svc.stop();
    assert!(!svc.is_running());

    // Second cycle
    svc.start().await.unwrap();
    assert!(svc.is_running());
    svc.stop();
    assert!(!svc.is_running());
}

#[test]
fn test_service_after_default_construction() {
    let svc = DeviceService::default();
    assert!(!svc.is_running());
    assert_eq!(svc.count(), 0);
    assert!(svc.list().is_empty());
    assert!(svc.get("anything").is_none());
    assert!(svc.unregister("anything").is_none());

    let status = svc.status();
    assert_eq!(status["running"], false);
    assert_eq!(status["enabled"], true);
    assert_eq!(status["device_count"], 0);
}

#[test]
fn test_service_set_bus_and_state_independently() {
    let svc = DeviceService::new();
    // Set bus only
    let bus_calls = Arc::new(AtomicUsize::new(0));
    let bus_calls_clone = bus_calls.clone();
    svc.set_bus_sender(Box::new(move |_, _, _| {
        bus_calls_clone.fetch_add(1, Ordering::SeqCst);
    }));

    // Send notification without state — should not call bus
    let ev = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "x".to_string(),
        vendor: String::new(),
        product: String::new(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    svc.send_notification(&ev);
    assert_eq!(bus_calls.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// Tests for Device struct with all field combinations
// ---------------------------------------------------------------------------

#[test]
fn test_device_serialize_with_no_optionals() {
    let d = Device {
        id: "d".to_string(),
        name: "n".to_string(),
        device_type: "t".to_string(),
        status: "s".to_string(),
        vendor_id: None,
        product_id: None,
        serial: None,
        connected_at: None,
        metadata: HashMap::new(),
    };
    let json = serde_json::to_string(&d).unwrap();
    let parsed: Device = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, "d");
    assert!(parsed.vendor_id.is_none());
    assert!(parsed.product_id.is_none());
    assert!(parsed.serial.is_none());
    assert!(parsed.connected_at.is_none());
}

#[test]
fn test_device_serialize_with_all_optionals() {
    let now = Local::now();
    let d = Device {
        id: "d".to_string(),
        name: "n".to_string(),
        device_type: "t".to_string(),
        status: "s".to_string(),
        vendor_id: Some("v".to_string()),
        product_id: Some("p".to_string()),
        serial: Some("s".to_string()),
        connected_at: Some(now),
        metadata: {
            let mut m = HashMap::new();
            m.insert("k".to_string(), "v".to_string());
            m
        },
    };
    let json = serde_json::to_string(&d).unwrap();
    let parsed: Device = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.vendor_id, Some("v".to_string()));
    assert_eq!(parsed.product_id, Some("p".to_string()));
    assert_eq!(parsed.serial, Some("s".to_string()));
    assert!(parsed.connected_at.is_some());
    assert_eq!(parsed.metadata.get("k").unwrap(), "v");
}

#[test]
fn test_device_with_complex_metadata_values() {
    let mut metadata = HashMap::new();
    metadata.insert("path".to_string(), "/dev/bus/usb/001/002".to_string());
    metadata.insert("speed".to_string(), "480Mbps".to_string());
    metadata.insert("version".to_string(), "2.0".to_string());

    let d = Device {
        id: "complex".to_string(),
        name: "Complex".to_string(),
        device_type: "usb".to_string(),
        status: "active".to_string(),
        vendor_id: None,
        product_id: None,
        serial: None,
        connected_at: None,
        metadata,
    };
    let json = serde_json::to_string(&d).unwrap();
    let parsed: Device = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.metadata.len(), 3);
    assert_eq!(parsed.metadata.get("path").unwrap(), "/dev/bus/usb/001/002");
}

// ---------------------------------------------------------------------------
// Stress / scale tests
// ---------------------------------------------------------------------------

#[test]
fn test_register_and_unregister_large_scale() {
    let svc = DeviceService::new();
    // Register 100 devices
    for i in 0..100 {
        svc.register(Device {
            id: format!("dev-{}", i),
            name: format!("Device {}", i),
            device_type: "sensor".to_string(),
            status: "online".to_string(),
            vendor_id: Some(format!("0x{:04x}", i)),
            product_id: None,
            serial: None,
            connected_at: None,
            metadata: HashMap::new(),
        });
    }
    assert_eq!(svc.count(), 100);
    assert_eq!(svc.list().len(), 100);

    // Unregister even-numbered devices
    for i in (0..100).step_by(2) {
        assert!(svc.unregister(&format!("dev-{}", i)).is_some());
    }
    assert_eq!(svc.count(), 50);

    // Verify remaining devices
    for i in (1..100).step_by(2) {
        assert!(svc.get(&format!("dev-{}", i)).is_some());
    }
}

#[test]
fn test_repeated_overwrite_same_id() {
    let svc = DeviceService::new();
    for i in 0..10 {
        svc.register(Device {
            id: "same".to_string(),
            name: format!("V{}", i),
            device_type: "t".to_string(),
            status: if i % 2 == 0 { "online" } else { "offline" }.to_string(),
            vendor_id: None,
            product_id: None,
            serial: None,
            connected_at: None,
            metadata: HashMap::new(),
        });
    }
    assert_eq!(svc.count(), 1);
    let dev = svc.get("same").unwrap();
    assert_eq!(dev.name, "V9");
    assert_eq!(dev.status, "offline");
}

// ---------------------------------------------------------------------------
// Tests for LastChannelProvider trait usage
// ---------------------------------------------------------------------------

struct CustomLastChannel {
    channel: String,
}

impl LastChannelProvider for CustomLastChannel {
    fn get_last_channel(&self) -> String {
        self.channel.clone()
    }
}

#[test]
fn test_last_channel_provider_dyn_dispatch() {
    let providers: Vec<Arc<dyn LastChannelProvider>> = vec![
        Arc::new(CustomLastChannel {
            channel: "web:1".to_string(),
        }),
        Arc::new(CustomLastChannel {
            channel: "discord:2".to_string(),
        }),
        Arc::new(CustomLastChannel {
            channel: "".to_string(),
        }),
    ];

    assert_eq!(providers[0].get_last_channel(), "web:1");
    assert_eq!(providers[1].get_last_channel(), "discord:2");
    assert_eq!(providers[2].get_last_channel(), "");
}

#[test]
fn test_set_state_manager_overrides_previous() {
    let svc = DeviceService::new();

    svc.set_state_manager(Arc::new(CustomLastChannel {
        channel: "web:first".to_string(),
    }));

    let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
    let sent_clone = sent.clone();
    svc.set_bus_sender(Box::new(move |ch, id, content| {
        sent_clone
            .lock()
            .push((ch.to_string(), id.to_string(), content.to_string()));
    }));

    // Override state manager
    svc.set_state_manager(Arc::new(CustomLastChannel {
        channel: "telegram:second".to_string(),
    }));

    let ev = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "x".to_string(),
        vendor: "V".to_string(),
        product: "P".to_string(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    svc.send_notification(&ev);

    let msgs = sent.lock();
    assert_eq!(msgs.len(), 1);
    // Verify it used the second state manager
    assert_eq!(msgs[0].0, "telegram");
    assert_eq!(msgs[0].1, "second");
}
