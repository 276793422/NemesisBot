//! Diagnostic capture sink for the intermittent
//! "context error → session corruption → LLM no-response" bug.
//!
//! Goal: **zero overhead on the happy path, automatic evidence dump on
//! failure.** Normal turns only push lightweight records into an in-memory
//! ringbuffer (per `session_key`). When a failure signal fires (LLM retry
//! exhausted, context overflow, session overwrite detected, agent error
//! funnel), [`CaptureSink::flush`] writes the buffered evidence + the failure
//! payload to `{workspace}/logs/capture/{session_key}/{ts}_{signal}/`.
//!
//! This is **observability only** — it does not change control flow or
//! business logic. The captured evidence lets us locate the root cause next
//! time the bug reproduces (the original logs were deleted; the bug is not
//! currently reproducible).
//!
//! LLM request/response bodies are *not* written here —组 1 (request_logger
//! observer None-fallback) keeps `request_logs/` complete on failure. This
//! sink focuses on tool results, session-write timeline, and the full error
//! text. Both directories are correlated by `trace_id`.

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tracing::warn;

/// Max tool-call records kept per session in memory (ringbuffer bound).
const MAX_TOOLS: usize = 50;
/// Max session-write records kept per session in memory.
const MAX_SESSION_WRITES: usize = 200;

/// One captured tool call: full pre-truncation arguments + result.
///
/// `loop.rs` does NOT truncate tool results before they enter the context, so
/// this captures whatever bloated the context window (the suspected trigger).
#[derive(Clone, Serialize)]
pub struct ToolCapture {
    pub tool_name: String,
    pub arguments: String,
    pub result: String,
    pub success: bool,
    pub duration_ms: u64,
    pub error: String,
    pub llm_round: usize,
    pub ts: String,
}

/// One captured session-store write event (`set_history` / `add_message` /
/// `save` / `read_history`). `overwrite_detected` is set by the caller when
/// an incoming `set_history` shrinks the message count — i.e. an old snapshot
/// overwriting newer writes (the suspected `maybe_summarize` race).
#[derive(Clone, Serialize)]
pub struct SessionWriteCapture {
    pub writer: String,
    pub op: String,
    pub before_len: Option<usize>,
    pub after_len: Option<usize>,
    pub first_role: Option<String>,
    pub last_role: Option<String>,
    pub messages_hash: String,
    pub overwrite_detected: bool,
    pub ts: String,
}

#[derive(Default)]
struct CaptureBuffer {
    tools: Vec<ToolCapture>,
    session_writes: Vec<SessionWriteCapture>,
}

pub struct CaptureSink {
    workspace: PathBuf,
    enabled: bool,
    buffers: Mutex<HashMap<String, CaptureBuffer>>,
}

static GLOBAL: OnceLock<CaptureSink> = OnceLock::new();

impl CaptureSink {
    /// Initialize the global sink. Call once at gateway startup with the
    /// workspace path and the `debug.capture.enabled` flag. If never called,
    /// [`CaptureSink::global`] returns `None` and all capture calls are
    /// no-ops — so unconditional `CaptureSink::global()` call sites stay safe
    /// in tests/binaries that never init.
    pub fn init(workspace: PathBuf, enabled: bool) {
        let _ = GLOBAL.set(CaptureSink {
            workspace,
            enabled,
            buffers: Mutex::new(HashMap::new()),
        });
    }

    /// Whether capture is active (initialized + enabled). Callers may use this
    /// to skip hashing work when capture is off.
    #[inline]
    pub fn enabled() -> bool {
        GLOBAL.get().map(|s| s.enabled).unwrap_or(false)
    }

    #[inline]
    pub fn global() -> Option<&'static CaptureSink> {
        GLOBAL.get()
    }

    /// Record a tool call's full arguments + result (see [`ToolCapture`]).
    pub fn record_tool(&self, session_key: &str, mut tool: ToolCapture) {
        if !self.enabled {
            return;
        }
        if tool.ts.is_empty() {
            tool.ts = now_ts();
        }
        let mut bufs = self.buffers.lock().unwrap();
        let buf = bufs.entry(session_key.to_string()).or_default();
        buf.tools.push(tool);
        if buf.tools.len() > MAX_TOOLS {
            let drop_n = buf.tools.len() - MAX_TOOLS;
            buf.tools.drain(0..drop_n);
        }
    }

    /// Record a session-store write event (see [`SessionWriteCapture`]).
    pub fn record_session_write(&self, session_key: &str, mut rec: SessionWriteCapture) {
        if !self.enabled {
            return;
        }
        if rec.ts.is_empty() {
            rec.ts = now_ts();
        }
        let mut bufs = self.buffers.lock().unwrap();
        let buf = bufs.entry(session_key.to_string()).or_default();
        let overwrite = rec.overwrite_detected;
        buf.session_writes.push(rec);
        if buf.session_writes.len() > MAX_SESSION_WRITES {
            let drop_n = buf.session_writes.len() - MAX_SESSION_WRITES;
            buf.session_writes.drain(0..drop_n);
        }
        // An overwrite is itself a failure signal worth flushing immediately,
        // so the timeline is captured even if the LLM call later happens to
        // succeed.
        if overwrite {
            drop(bufs);
            self.flush(session_key, "session_overwrite", None, None);
        }
    }

    /// Flush all buffered evidence for a session + the failure payload to
    /// `logs/capture/{session_key}/{ts}_{signal}/`. Consumes the in-memory
    /// buffer for that session. Best-effort: any IO error is logged and
    /// swallowed (capture must never break the agent loop).
    pub fn flush(
        &self,
        session_key: &str,
        signal: &str,
        trace_id: Option<&str>,
        error_text: Option<&str>,
    ) {
        if !self.enabled {
            return;
        }
        let buffer = self
            .buffers
            .lock()
            .unwrap()
            .remove(session_key)
            .unwrap_or_default();

        let ts = chrono::Local::now()
            .format("%Y-%m-%d_%H-%M-%S")
            .to_string();
        let dir = self
            .workspace
            .join("logs")
            .join("capture")
            .join(sanitize(session_key))
            .join(format!("{}_{}", ts, sanitize(signal)));

        if let Err(e) = fs::create_dir_all(&dir) {
            warn!("[CaptureSink] failed to create capture dir: {}", e);
            return;
        }

        // 00.summary.json
        let summary = serde_json::json!({
            "ts": ts,
            "signal": signal,
            "session_key": session_key,
            "trace_id": trace_id,
            "tool_calls": buffer.tools.len(),
            "session_writes": buffer.session_writes.len(),
        });
        write_json(&dir.join("00.summary.json"), &summary);

        // 01.tools.json — full pre-truncation tool results
        if !buffer.tools.is_empty() {
            write_json(&dir.join("01.tools.json"), &buffer.tools);
        }

        // 02.session_writes.jsonl — write timeline (incl. overwrite flags)
        if !buffer.session_writes.is_empty() {
            write_jsonl(&dir.join("02.session_writes.jsonl"), &buffer.session_writes);
        }

        // 05.error.txt — full untruncated error text (the user-visible error
        // is short by construction; this captures the complete source string)
        if let Some(txt) = error_text {
            if let Err(e) = fs::write(dir.join("05.error.txt"), txt) {
                warn!("[CaptureSink] failed to write error.txt: {}", e);
            }
        }
    }
}

fn now_ts() -> String {
    chrono::Local::now().to_rfc3339()
}

/// Sanitize a string for use as a path segment (session_key contains ':' etc.).
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn write_json<T: Serialize>(path: &Path, value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => {
            if let Err(e) = fs::write(path, text) {
                warn!("[CaptureSink] failed to write {:?}: {}", path, e);
            }
        }
        Err(e) => warn!("[CaptureSink] failed to serialize {:?}: {}", path, e),
    }
}

fn write_jsonl<T: Serialize>(path: &Path, items: &[T]) {
    use std::io::Write;
    let mut f = match fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            warn!("[CaptureSink] failed to create {:?}: {}", path, e);
            return;
        }
    };
    for item in items {
        if let Ok(line) = serde_json::to_string(item) {
            let _ = writeln!(f, "{}", line);
        }
    }
}

#[cfg(test)]
impl CaptureSink {
    /// Test-only constructor bypassing the global OnceLock, so flush behavior
    /// can be exercised without polluting the process-global singleton.
    fn for_test(workspace: PathBuf) -> Self {
        Self {
            workspace,
            enabled: true,
            buffers: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize("web:b6e6d66b"), "web_b6e6d66b");
        assert_eq!(sanitize("ok-name.1"), "ok-name.1");
    }

    #[test]
    fn uninit_global_is_noop() {
        // Without init, global() is None — call sites must tolerate this.
        let _ = CaptureSink::global(); // must not panic
    }

    #[test]
    fn flush_writes_all_evidence_files() {
        let dir = std::env::temp_dir().join(format!(
            "nemesis_cap_flush_{}_{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let sink = CaptureSink::for_test(dir.clone());
        let sk = "web:abc123";

        // Simulate a bloated tool output (the suspected context-blowout trigger).
        sink.record_tool(
            sk,
            ToolCapture {
                tool_name: "read_file".to_string(),
                arguments: "{}".to_string(),
                result: "X".repeat(5000),
                success: true,
                duration_ms: 12,
                error: String::new(),
                llm_round: 1,
                ts: String::new(),
            },
        );
        // Simulate the suspected old-snapshot overwrite (before<after? no: 4<52).
        sink.record_session_write(
            sk,
            SessionWriteCapture {
                writer: "set_history".to_string(),
                op: "set_history".to_string(),
                before_len: Some(52),
                after_len: Some(4),
                first_role: None,
                last_role: None,
                messages_hash: "deadbeef".to_string(),
                overwrite_detected: false, // avoid auto-flush; tested separately
                ts: String::new(),
            },
        );
        sink.flush(
            sk,
            "llm_retry_exhausted",
            Some("trace-1"),
            Some("context_length_exceeded: this model maximum is 65536 tokens"),
        );

        let base = dir.join("logs").join("capture").join("web_abc123");
        let entries: Vec<_> = std::fs::read_dir(&base).unwrap().collect();
        assert_eq!(entries.len(), 1, "exactly one capture event dir expected");
        let edir = entries[0].as_ref().unwrap().path();
        let edir_s = edir.to_string_lossy().to_string();
        assert!(edir_s.contains("llm_retry_exhausted"), "dir: {}", edir_s);
        assert!(edir.join("00.summary.json").exists());
        assert!(edir.join("01.tools.json").exists(), "tool capture missing");
        assert!(
            edir.join("02.session_writes.jsonl").exists(),
            "session writes missing"
        );
        assert!(edir.join("05.error.txt").exists(), "error text missing");
        let err = std::fs::read_to_string(edir.join("05.error.txt")).unwrap();
        assert!(
            err.contains("context_length_exceeded"),
            "full error text must be preserved untruncated"
        );
        let tools = std::fs::read_to_string(edir.join("01.tools.json")).unwrap();
        assert!(tools.contains("read_file"), "tool name missing");
        assert!(tools.contains(&"X".repeat(100)), "full result must be captured");
        let summary = std::fs::read_to_string(edir.join("00.summary.json")).unwrap();
        assert!(summary.contains("trace-1"));
        assert!(summary.contains("\"tool_calls\": 1"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrite_auto_flushes() {
        let dir = std::env::temp_dir().join(format!(
            "nemesis_cap_ow_{}_{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let sink = CaptureSink::for_test(dir.clone());
        let sk = "web:ow";

        sink.record_session_write(
            sk,
            SessionWriteCapture {
                writer: "set_history".to_string(),
                op: "set_history".to_string(),
                before_len: Some(52),
                after_len: Some(4),
                first_role: None,
                last_role: None,
                messages_hash: "h".to_string(),
                overwrite_detected: true,
                ts: String::new(),
            },
        );

        let base = dir.join("logs").join("capture").join("web_ow");
        let entries: Vec<_> = std::fs::read_dir(&base).unwrap().collect();
        assert_eq!(entries.len(), 1, "overwrite should auto-flush immediately");
        let edir = entries[0].as_ref().unwrap().path();
        assert!(
            edir.to_string_lossy().contains("session_overwrite"),
            "dir: {}",
            edir.display()
        );
        assert!(edir.join("02.session_writes.jsonl").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
