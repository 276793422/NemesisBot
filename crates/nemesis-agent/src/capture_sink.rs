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
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tracing::warn;

/// Max tool-call records kept per session in memory (ringbuffer bound).
const MAX_TOOLS: usize = 50;
/// Max session-write records kept per session (ringbuffer bound).
const MAX_SESSION_WRITES: usize = 200;
/// Global byte budget across ALL sessions' in-memory buffers (~32 MiB).
///
/// The per-session rings above bound *count*, not *size* — a tool result can
/// be arbitrarily large (capture happens pre-truncation by design), so a
/// long-lived gateway accumulating buffers for many sessions that never hit a
/// failure signal would grow without bound. When the approximate total
/// exceeds this budget, whole idle sessions are evicted oldest-first (LRU:
/// every record refreshes the session's position) until back under budget.
/// The most recently recorded session is never evicted, so an active
/// session's evidence survives; a single session may still exceed the budget
/// on its own (bounded by MAX_TOOLS/MAX_SESSION_WRITES entries) — deliberate:
/// full pre-truncation evidence for the live session is the point of this
/// sink. Internal safety constant, deliberately NOT config-exposed (matches
/// MAX_TOOLS / MAX_SESSION_WRITES precedent).
const MAX_TOTAL_BUFFER_BYTES: usize = 32 * 1024 * 1024;

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

/// Approximate in-memory footprint of one buffered record (payload strings
/// only; struct/bookkeeping overhead ignored). Used exclusively for budget
/// accounting — precision is not required, consistency is.
fn tool_rec_bytes(t: &ToolCapture) -> usize {
    t.tool_name.len() + t.arguments.len() + t.result.len() + t.error.len()
}

fn write_rec_bytes(w: &SessionWriteCapture) -> usize {
    w.writer.len()
        + w.op.len()
        + w.messages_hash.len()
        + w.first_role.as_deref().map_or(0, str::len)
        + w.last_role.as_deref().map_or(0, str::len)
}

impl CaptureBuffer {
    fn approx_bytes(&self) -> usize {
        self.tools.iter().map(tool_rec_bytes).sum::<usize>()
            + self
                .session_writes
                .iter()
                .map(write_rec_bytes)
                .sum::<usize>()
    }
}

/// All sessions' buffers + LRU order + global byte accounting, packed under
/// one mutex (the three fields are only ever mutated together).
#[derive(Default)]
struct CaptureBuffers {
    map: HashMap<String, CaptureBuffer>,
    /// Live session keys, back = most recently recorded (LRU).
    order: VecDeque<String>,
    /// Sum of `approx_bytes()` over all live buffers.
    total_bytes: usize,
}

impl CaptureBuffers {
    /// Move `key` to the back of the LRU order (inserting it if this is its
    /// first record — the map entry may already have been created via
    /// `or_default` before this runs).
    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
    }

    /// Remove a session's buffer and release its accounting.
    fn remove_session(&mut self, key: &str) -> Option<CaptureBuffer> {
        let buf = self.map.remove(key)?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.total_bytes = self.total_bytes.saturating_sub(buf.approx_bytes());
        Some(buf)
    }

    /// Evict whole idle sessions oldest-first until back under budget. The
    /// most recently recorded session (back of `order`) is never evicted.
    fn enforce_budget(&mut self, max_bytes: usize) {
        if self.total_bytes <= max_bytes {
            return;
        }
        let mut evicted = 0usize;
        while self.total_bytes > max_bytes && self.order.len() > 1 {
            let oldest = match self.order.pop_front() {
                Some(k) => k,
                None => break,
            };
            if let Some(buf) = self.map.remove(&oldest) {
                evicted += 1;
                self.total_bytes = self.total_bytes.saturating_sub(buf.approx_bytes());
            }
        }
        if evicted > 0 {
            warn!(
                "[CaptureSink] buffer budget ({} bytes) exceeded: evicted {} idle session(s), {} bytes retained",
                max_bytes, evicted, self.total_bytes
            );
        }
    }
}

pub struct CaptureSink {
    workspace: PathBuf,
    enabled: bool,
    max_buffer_bytes: usize,
    buffers: Mutex<CaptureBuffers>,
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
            max_buffer_bytes: MAX_TOTAL_BUFFER_BYTES,
            buffers: Mutex::new(CaptureBuffers::default()),
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
        let rec_bytes = tool_rec_bytes(&tool);
        let mut bufs = self.buffers.lock().unwrap();
        let dropped_bytes = {
            let buf = bufs.map.entry(session_key.to_string()).or_default();
            buf.tools.push(tool);
            let mut dropped = 0usize;
            if buf.tools.len() > MAX_TOOLS {
                let drop_n = buf.tools.len() - MAX_TOOLS;
                dropped = buf.tools.drain(0..drop_n).map(|d| tool_rec_bytes(&d)).sum();
            }
            dropped
        };
        bufs.total_bytes = bufs
            .total_bytes
            .saturating_add(rec_bytes)
            .saturating_sub(dropped_bytes);
        bufs.touch(session_key);
        bufs.enforce_budget(self.max_buffer_bytes);
    }

    /// Record a session-store write event (see [`SessionWriteCapture`]).
    pub fn record_session_write(&self, session_key: &str, mut rec: SessionWriteCapture) {
        if !self.enabled {
            return;
        }
        if rec.ts.is_empty() {
            rec.ts = now_ts();
        }
        let rec_bytes = write_rec_bytes(&rec);
        let overwrite;
        let dropped_bytes;
        {
            let mut bufs = self.buffers.lock().unwrap();
            let buf = bufs.map.entry(session_key.to_string()).or_default();
            overwrite = rec.overwrite_detected;
            buf.session_writes.push(rec);
            let mut dropped = 0usize;
            if buf.session_writes.len() > MAX_SESSION_WRITES {
                let drop_n = buf.session_writes.len() - MAX_SESSION_WRITES;
                dropped = buf
                    .session_writes
                    .drain(0..drop_n)
                    .map(|d| write_rec_bytes(&d))
                    .sum();
            }
            dropped_bytes = dropped;
            bufs.total_bytes = bufs
                .total_bytes
                .saturating_add(rec_bytes)
                .saturating_sub(dropped_bytes);
            bufs.touch(session_key);
            bufs.enforce_budget(self.max_buffer_bytes);
        }
        // An overwrite is itself a failure signal worth flushing immediately,
        // so the timeline is captured even if the LLM call later happens to
        // succeed.
        if overwrite {
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
            .remove_session(session_key)
            .unwrap_or_default();

        let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let dir = capture_session_dir(&self.workspace, session_key).join(format!(
            "{}_{}",
            ts,
            sanitize(signal)
        ));

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
        if let Some(txt) = error_text
            && let Err(e) = fs::write(dir.join("05.error.txt"), txt)
        {
            warn!("[CaptureSink] failed to write error.txt: {}", e);
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

/// 会话 capture 目录的单一真相源（flush 落盘与删除级联共用同款 sanitize）。
///
/// 删除链路（`chat_log::delete_chat_log` 硬删 / `chat_trash::trash_chat_log`
/// 软删）必须经本函数寻址——capture 目录名用的是本模块私有 sanitize（保
/// 留点号、无 80 截断），与 `sanitize_path_segment`（jsonl/ledger/store
/// 文件名）在非 ASCII/超长键下**不同**，按 stem 反推 capture 目录名会错删
/// /漏删。
pub fn capture_session_dir(workspace: &std::path::Path, session_key: &str) -> std::path::PathBuf {
    capture_dir_at(&workspace.join("logs").join("capture"), session_key)
}

/// [`capture_session_dir`] 的 capture 根直输形态（`logs/capture` 目录本身）：
/// trash 注入形态（`TrashLocations::with_dirs`）按根注入，不持 workspace；
/// 与 [`remove_capture_at`] 对称供测试按同源 sanitize 预置目录。
pub fn capture_dir_at(capture_root: &std::path::Path, session_key: &str) -> std::path::PathBuf {
    capture_root.join(sanitize(session_key))
}

/// [`remove_session_capture`] 的删除核心：整目录韧性删除；缺席 = 幂等成功。
fn remove_capture_dir(dir: &std::path::Path) -> bool {
    if !dir.exists() {
        return true;
    }
    if nemesis_utils::fsutil::remove_dir_all_resilient(dir) {
        true
    } else {
        warn!(
            "[CaptureSink] 会话 capture 目录删除重试耗尽（残留交 capture TTL 清扫）: {}",
            dir.display()
        );
        false
    }
}

/// 会话删除级联：摘除该会话的 capture 目录（含全部证据条目）。
///
/// capture 是会话内容的**副本**（工具入参/回执全文），隐私清理语义下删除
/// 会话必须同步清——软删同样删（诊断证据不承诺恢复：回收站恢复只覆盖
/// chat/boundary/meta/todo 四件套，见 trash_chat_log 注释）。best-effort
/// 韧性删除（Windows 瞬时句柄重试）；目录缺席 = 幂等成功。
pub fn remove_session_capture(workspace: &std::path::Path, session_key: &str) -> bool {
    remove_capture_dir(&capture_session_dir(workspace, session_key))
}

/// [`remove_session_capture`] 的 capture 根直输形态（trash 注入链路用）。
pub fn remove_capture_at(capture_root: &std::path::Path, session_key: &str) -> bool {
    remove_capture_dir(&capture_dir_at(capture_root, session_key))
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
        Self::for_test_with_budget(workspace, MAX_TOTAL_BUFFER_BYTES)
    }

    /// Test-only constructor with a custom byte budget, so LRU eviction can
    /// be exercised without allocating ~32 MiB per test.
    fn for_test_with_budget(workspace: PathBuf, max_buffer_bytes: usize) -> Self {
        Self {
            workspace,
            enabled: true,
            max_buffer_bytes,
            buffers: Mutex::new(CaptureBuffers::default()),
        }
    }
}

#[cfg(test)]
mod tests;

// 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
