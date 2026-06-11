//! Cluster log writer — thread-safe JSONL logger with daily rotation.
//!
//! Writes one JSON object per line to `cluster_YYYY-MM-DD.log` files.
//! Global singleton via `OnceLock`, initialized once by `init_cluster_log()`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

/// Global cluster log writer singleton.
static CLUSTER_LOG: OnceLock<ClusterLogWriter> = OnceLock::new();

/// Global publish hook — called after every log entry is written to disk.
/// Set via `set_cluster_log_hook()`. Used by the SSE bridge to push events
/// to the Dashboard in real-time.
static CLUSTER_LOG_HOOK: OnceLock<Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>> =
    OnceLock::new();

/// Set the global publish hook.
///
/// Called once during gateway startup to bridge cluster log events to the SSE
/// event hub. If called more than once, subsequent calls are silently ignored.
pub fn set_cluster_log_hook(hook: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>) {
    let _ = CLUSTER_LOG_HOOK.set(hook);
}

/// Thread-safe JSONL log writer with daily file rotation.
#[derive(Debug)]
pub struct ClusterLogWriter {
    inner: Mutex<ClusterLogInner>,
}

#[derive(Debug)]
pub struct ClusterLogInner {
    log_dir: PathBuf,
    current_date: String,
    file: Option<File>,
}

impl ClusterLogWriter {
    fn new(log_dir: PathBuf) -> Self {
        Self {
            inner: Mutex::new(ClusterLogInner {
                log_dir,
                current_date: String::new(),
                file: None,
            }),
        }
    }

    fn write_entry(&self, event: &str, mut fields: serde_json::Value) {
        let now = chrono::Local::now();
        let date_str = now.format("%Y-%m-%d").to_string();

        // Build the log entry.
        let entry = {
            let obj = fields.as_object_mut().expect("fields must be a JSON object");
            obj.insert("ts".into(), serde_json::Value::String(now.to_rfc3339()));
            obj.insert("event".into(), serde_json::Value::String(event.to_string()));
            serde_json::to_string(&fields).unwrap_or_default()
        };

        let mut inner = self.inner.lock();

        // Rotate file if date changed.
        if inner.current_date != date_str {
            inner.current_date = date_str.clone();
            let filename = format!("cluster_{}.log", date_str);
            let path = inner.log_dir.join(&filename);

            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => {
                    inner.file = Some(f);
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "[ClusterLog] Failed to open log file"
                    );
                    inner.file = None;
                    return;
                }
            }
        }

        // Write line.
        if let Some(ref mut file) = inner.file {
            if let Err(e) = writeln!(file, "{}", entry) {
                tracing::warn!(error = %e, "[ClusterLog] Failed to write log entry");
            }
        }

        // Drop the inner lock before calling the hook to avoid potential deadlocks
        // if the hook does anything that touches the cluster log (e.g. publishes
        // an SSE event that triggers another log write).
        drop(inner);

        // Fire the publish hook if one is registered.
        if let Some(hook) = CLUSTER_LOG_HOOK.get() {
            hook(event, &fields);
        }
    }
}

/// Initialize the global cluster log writer.
///
/// Must be called once during startup. Panics if called more than once.
pub fn init_cluster_log(log_dir: &Path) {
    CLUSTER_LOG
        .set(ClusterLogWriter::new(log_dir.to_path_buf()))
        .expect("init_cluster_log called more than once");
    tracing::info!(
        dir = %log_dir.display(),
        "[ClusterLog] Initialized"
    );
}

/// Try to initialize the global cluster log writer.
///
/// Returns `true` if this call performed the initialization, `false` if already initialized.
pub fn try_init_cluster_log(log_dir: &Path) -> bool {
    CLUSTER_LOG
        .set(ClusterLogWriter::new(log_dir.to_path_buf()))
        .is_ok()
}

/// Write a cluster log entry.
///
/// If the log writer has not been initialized, this is a no-op (silently ignored).
///
/// `fields` must be a JSON object. The function adds `ts` and `event` automatically.
pub fn write_cluster_log(event: &str, fields: serde_json::Value) {
    if let Some(writer) = CLUSTER_LOG.get() {
        writer.write_entry(event, fields);
    }
}

#[cfg(test)]
mod tests {
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
        assert!(content.contains("test_event"), "Log should contain event name");
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

        writer.write_entry("custom_event", serde_json::json!({"user_id": "123", "action": "login"}));

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
            assert!(!second_result, "Should still return false when already initialized");
        }

        let _ = fs::remove_dir_all(log_dir);
    }

    // Test that init_cluster_log panics on double init
    #[test]
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
}
