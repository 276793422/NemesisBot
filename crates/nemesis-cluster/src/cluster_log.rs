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
            // 非 object 调用方（错误地传了 string/number 等）不 panic——包一层
            // {"payload": fields} 落盘（集群完备性加固：日志写入
            // 是观测面，绝不能因调用方类型错误把生产进程打崩）。
            if !fields.is_object() {
                fields = serde_json::json!({ "payload": fields });
            }
            let obj = fields.as_object_mut().expect("just wrapped as object");
            obj.insert("ts".into(), serde_json::Value::String(now.to_rfc3339()));
            obj.insert("event".into(), serde_json::Value::String(event.to_string()));
            serde_json::to_string(&fields).unwrap_or_default()
        };

        let mut inner = self.inner.lock();

        // Rotate file if the date changed — or retry the open after a
        // previous failure（BUG-20261112-106）：旧实现先落 current_date 再开
        // 文件，一次打开失败（AV/索引器瞬时锁、磁盘满、目录创建失败）=
        // 当天全部后续条目静默丢弃直到次日——派发审计面假完整（本文件
        // 注释引用的实盘核对事故正依赖此文件的完整性）。现在打开失败
        // 不更新 current_date：file 为 None 即在下一条重试（瞬时锁自愈；
        // 持续失败 = 每条 warn 诚实可见，观测面绝不静默）。
        if inner.current_date != date_str || inner.file.is_none() {
            let filename = format!("cluster_{}.log", date_str);
            let path = inner.log_dir.join(&filename);

            if let Some(parent) = path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                tracing::warn!(
                    dir = %parent.display(),
                    error = %e,
                    "[ClusterLog] Failed to create log directory"
                );
            }

            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(f) => {
                    inner.file = Some(f);
                    inner.current_date = date_str.clone();
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "[ClusterLog] Failed to open log file (will retry on next entry)"
                    );
                    inner.file = None;
                    return;
                }
            }
        }

        // Write line.
        if let Some(ref mut file) = inner.file
            && let Err(e) = writeln!(file, "{}", entry)
        {
            tracing::warn!(error = %e, "[ClusterLog] Failed to write log entry");
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

/// Normalize an RPC action name for the audit trail (`rpc_call` rows).
///
/// The wire protocol itself is deliberately dual-form: `KnownAction::as_str()`
/// is PascalCase (`PeerChat`/`Ping`/`PeerChatCallback`) while `ActionType::Custom`
/// strings are lowercase (`peer_chat`/`transfer_begin`/`nb_bus`) — that contract
/// is frozen (deserialization matches exact strings). The audit trail, however,
/// renders every action lowercase so log forensics needs a single grep form
/// (实盘核对实证：按小写 `peer_chat` 检索漏掉 85 条 `PeerChat`
/// 派发行，险些误判「派发审计缺失」）。
///
/// Historical note: log files written before keep the raw form
/// (PascalCase for Known actions) — auditing old files still needs both forms.
pub fn audit_action_name(action: &str) -> String {
    action.to_ascii_lowercase()
}

#[cfg(test)]
mod tests;
