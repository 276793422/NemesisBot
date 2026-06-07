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
struct ClusterLogWriter {
    inner: Mutex<ClusterLogInner>,
}

#[derive(Debug)]
struct ClusterLogInner {
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
        let now = chrono::Utc::now();
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
