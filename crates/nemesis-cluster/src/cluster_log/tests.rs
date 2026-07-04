use super::*;
use std::fs;

// Test that write doesn't panic when called before initialization
#[test]
fn test_write_cluster_log_before_init_is_noop() {
    write_cluster_log("test_event", serde_json::json!({"key": "value"}));
    // If we got here, the no-op behavior worked
}

// Test basic writer functionality
#[test]
fn test_cluster_log_writer_basic() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path();
    let writer = ClusterLogWriter::new(log_dir.to_path_buf());

    writer.write_entry("test_event", serde_json::json!({"msg": "test"}));

    let now = chrono::Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    assert!(log_file.exists(), "Log file should be created");

    let content = fs::read_to_string(&log_file).unwrap();
    assert!(
        content.contains("test_event"),
        "Log should contain event name"
    );
    assert!(content.contains("test"), "Log should contain message");

    let _ = fs::remove_dir_all(log_dir);
}

// Test multiple writes append to file
#[test]
fn test_cluster_log_writer_multiple_entries() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path();
    let writer = ClusterLogWriter::new(log_dir.to_path_buf());

    writer.write_entry("event1", serde_json::json!({"count": 1}));
    writer.write_entry("event2", serde_json::json!({"count": 2}));
    writer.write_entry("event3", serde_json::json!({"count": 3}));

    let now = chrono::Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let content = fs::read_to_string(&log_file).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 3, "Should have 3 log entries");

    let _ = fs::remove_dir_all(log_dir);
}

// Test JSON structure
#[test]
fn test_cluster_log_writer_json_structure() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path();
    let writer = ClusterLogWriter::new(log_dir.to_path_buf());

    writer.write_entry(
        "custom_event",
        serde_json::json!({"user_id": "123", "action": "login"}),
    );

    let now = chrono::Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let content = fs::read_to_string(&log_file).unwrap();
    let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();

    assert_eq!(entry["event"], "custom_event");
    assert_eq!(entry["user_id"], "123");
    assert_eq!(entry["action"], "login");
    assert!(entry.get("ts").is_some(), "Should have timestamp");

    let _ = fs::remove_dir_all(log_dir);
}

// Test hook functionality
#[test]
fn test_cluster_log_hook() {
    let hook_called: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let hook_called_clone = hook_called.clone();

    set_cluster_log_hook(Arc::new(move |event: &str, _fields: &serde_json::Value| {
        if event == "test_event" {
            let mut called = hook_called_clone.lock();
            *called = true;
        }
    }));

    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path();
    let writer = ClusterLogWriter::new(log_dir.to_path_buf());

    writer.write_entry("test_event", serde_json::json!({"msg": "hook_test"}));

    let called = hook_called.lock();
    assert!(*called, "Hook should be called");

    let _ = fs::remove_dir_all(log_dir);
}

// Test complex JSON structures
#[test]
fn test_cluster_log_writer_complex_json() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path();
    let writer = ClusterLogWriter::new(log_dir.to_path_buf());

    writer.write_entry(
        "complex_event",
        serde_json::json!({
            "user": {"id": "123", "name": "test"},
            "action": "login",
            "metadata": {"ip": "127.0.0.1"}
        }),
    );

    let now = chrono::Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let content = fs::read_to_string(&log_file).unwrap();
    let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();

    assert_eq!(entry["user"]["id"], "123");
    assert_eq!(entry["user"]["name"], "test");
    assert_eq!(entry["metadata"]["ip"], "127.0.0.1");

    let _ = fs::remove_dir_all(log_dir);
}

// Test file creation in nested directories
#[test]
fn test_cluster_log_writer_nested_directory() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path().join("nested/logs/path");
    let writer = ClusterLogWriter::new(log_dir.to_path_buf());

    writer.write_entry("test", serde_json::json!({"msg": "nested_dir"}));

    let now = chrono::Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    assert!(log_file.exists(), "Should create nested directories");

    let _ = fs::remove_dir_all(temp_dir.path());
}

// Test concurrent access
#[test]
fn test_cluster_log_writer_concurrent() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path().to_path_buf();
    let writer = Arc::new(ClusterLogWriter::new(log_dir.clone()));

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let writer_clone = Arc::clone(&writer);
            std::thread::spawn(move || {
                writer_clone.write_entry(
                    &format!("thread_event_{}", i),
                    serde_json::json!({"thread": i}),
                );
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    let now = chrono::Local::now();
    let date_str = now.format("%Y-%m-%d").to_string();
    let log_file = log_dir.join(format!("cluster_{}.log", date_str));

    let content = fs::read_to_string(&log_file).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 10, "All concurrent writes should succeed");

    let _ = fs::remove_dir_all(log_dir);
}

// Test try_init returns false after first init
#[test]
fn test_try_init_cluster_log_already_initialized() {
    // Note: This test may fail if run after other tests that already initialized the log
    // In that case, it actually demonstrates that the global state is working correctly
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    let first_result = try_init_cluster_log(log_dir);
    let second_result = try_init_cluster_log(log_dir);

    if first_result {
        assert!(!second_result, "Second init should return false");
    } else {
        // Log was already initialized by previous test, this is expected
        assert!(
            !second_result,
            "Should still return false when already initialized"
        );
    }

    let _ = fs::remove_dir_all(log_dir);
}

// Test that init_cluster_log panics on double init
// Marked `serial` because CLUSTER_LOG is a process-wide OnceLock — if any
// other test in the same `cargo test` invocation initializes it first,
// this test's "first" init would itself panic (outside catch_unwind).
// serial_test forces this test to run alone, avoiding the race.
#[test]
#[serial_test::serial]
fn test_init_cluster_log_panics_on_double_init() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let log_dir = temp_dir.path();

    init_cluster_log(log_dir);

    // This should panic
    let result = std::panic::catch_unwind(|| {
        init_cluster_log(log_dir);
    });

    assert!(result.is_err(), "Second init should panic");

    let _ = fs::remove_dir_all(log_dir);
}
