//! Chat log module — append-only JSONL log for user-facing chat history.
//!
//! Session files (`sessions/`) serve LLM context recovery (summarization,
//! truncation). This module provides a separate, append-only log that never
//! gets truncated, ensuring the user-facing chat history is always complete.

use chrono::Local;
use nemesis_path::default_path_manager;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Append a chat message to the JSONL log file.
pub fn append_chat_log(session_key: &str, role: &str, content: &str) {
    append_chat_log_full(session_key, role, content, None, None, None);
}

/// Append a chat message with an optional model badge (`provider/name`).
///
/// When `model` is `Some`, an extra `"model"` field is written so the
/// Dashboard can render a "供应商·模型名" badge on the assistant message after
/// a history reload. `None` (user rows, legacy callers) omits the field — old
/// jsonl entries without it parse fine (read side treats missing = no badge).
pub fn append_chat_log_with_model(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
) {
    append_chat_log_full(session_key, role, content, model, None, None);
}

/// Full append: optional model badge AND optional cron origin marker.
///
/// `cron_job_id` / `cron_job_name`: when `Some`, marks this entry as
/// originating from a scheduled (cron) task, so the Dashboard can label it
/// (🕒) and filter "只看定时任务" in the session browser. `None` (the common
/// case) omits the fields — old jsonl entries without them parse fine.
pub fn append_chat_log_full(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    cron_job_id: Option<&str>,
    cron_job_name: Option<&str>,
) {
    let path = log_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("[chat_log] Failed to open {}: {}", path.display(), e);
            return;
        }
    };
    let mut entry = serde_json::json!({
        "role": role,
        "content": content,
        "timestamp": Local::now().to_rfc3339(),
    });
    if let Some(m) = model {
        entry["model"] = serde_json::Value::String(m.to_string());
    }
    if let Some(id) = cron_job_id {
        entry["cron_job_id"] = serde_json::Value::String(id.to_string());
    }
    if let Some(name) = cron_job_name {
        entry["cron_job_name"] = serde_json::Value::String(name.to_string());
    }
    if let Err(e) = writeln!(file, "{}", entry) {
        tracing::warn!("[chat_log] Failed to write to {}: {}", path.display(), e);
        return;
    }
    // U20 (sixth batch): lazy FTS index hook — best-effort, failures inside
    // are swallowed (the next full reindex repairs). Timestamp mirrors the
    // entry written above.
    let ts = entry.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
    crate::history_search::index_append(session_key, role, content, ts);
}

/// Read chat log with pagination.
///
/// Returns `(page, total_count, has_more, oldest_index)`. `before_index` is the
/// exclusive upper bound — "give me items before this index". `None` means the
/// newest batch. Messages are returned in chronological order (oldest first).
///
/// Uses two-pass approach: first counts lines, then only deserializes the needed
/// range. Avoids loading the entire file into memory.
pub fn read_chat_log(
    session_key: &str,
    limit: usize,
    before_index: Option<usize>,
) -> (Vec<Value>, usize, bool, usize) {
    let path = log_path(session_key);
    // is_file 而非 exists：Linux 上 File::open 对目录成功（O_RDONLY 合法），
    // 而下面 BufReader::lines() 的 count/filter_map 在 read Err（目录 fd 每次
    // read 都 EISDIR，Lines 迭代器不熔断）上会无限自旋。目录占位按缺失处理
    // （Windows 上 open 目录本就失败走同臂）。（2026-09-01 Linux 首跑暴露）
    if !path.is_file() {
        return (Vec::new(), 0, false, 0);
    }

    // Pass 1: Count lines (no deserialization).
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), 0, false, 0),
    };
    let total = std::io::BufReader::new(file).lines().count();
    if total == 0 {
        return (Vec::new(), 0, false, 0);
    }

    let end = before_index.map(|bi| bi.min(total)).unwrap_or(total);
    let start = end.saturating_sub(limit);

    // Pass 2: Read only lines in [start, end), skip the rest.
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), 0, false, 0),
    };
    let page: Vec<Value> = std::io::BufReader::new(file)
        .lines()
        .skip(start)
        .take(end - start)
        .filter_map(|l| l.ok())
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        // ROUND-5 FIX: boundary events moved to a sidecar file (see
        // `boundary_path`), so the message jsonl can no longer contain them.
        // The filter stays as a one-line guard for dev machines that ran a
        // batch-3 build with interleaved boundaries — cheap and harmless.
        .filter(|v| v.get("role").and_then(|r| r.as_str()) != Some("boundary"))
        .collect();

    (page, total, start > 0, start)
}

/// Read ONLY the boundary (audit) events of a session — replay/audit tooling
/// reads through here. Round-5 fix: these live in the sidecar file
/// (`logs/boundary/<safe_key>.jsonl`), not the message jsonl.
pub fn read_boundary_events(session_key: &str) -> Vec<Value> {
    let path = boundary_path(session_key);
    if !path.exists() {
        return Vec::new();
    }
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .collect()
}

/// Resolve the JSONL file path for a session key.
fn log_path(session_key: &str) -> PathBuf {
    let safe_key = session_key.replace(':', "_");
    default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key))
}

/// Does this session have a chat_log jsonl on disk? (2026-08-25 fork 第三轮)
/// `session_fork::unique_key` consults this so a fork never APPENDS onto a
/// previous fork's surviving jsonl after its store json aged out of the
/// 7-day TTL (append would duplicate the whole prefix).
pub fn chat_log_exists(session_key: &str) -> bool {
    log_path(session_key).exists()
}

/// Write pre-read chat_log rows under `new_key`, VERBATIM (2026-08-25 fork
/// 第三轮). Each row is a complete jsonl `Value` — original timestamps,
/// model badges, cron markers, everything preserved byte-faithfully. This is
/// the fork's jsonl side: the fork dialog counts turns ON these rows, so the
/// copy must be exactly the rows the user picked, not a re-derived
/// projection. Returns the number of lines written.
pub fn write_chat_log_rows(new_key: &str, rows: &[Value]) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut written = 0usize;
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&target) {
        for v in rows {
            let Ok(line) = serde_json::to_string(v) else { continue };
            if writeln!(f, "{}", line).is_ok() {
                written += 1;
            }
        }
    }
    written
}

/// Z1 (Phase4-d): copy the first `at_turn` COMPLETE user turns of
/// `source_key`'s chat log to `new_key`, lines VERBATIM (original
/// timestamps and extra fields preserved — a fork must not re-stamp
/// history). Uses the same user-turn counting as the SessionStore-side
/// fork cut, so the Dashboard log and the model-context store stay aligned.
/// Returns the number of lines copied. The lazy FTS full-index picks the
/// new file up on first search; no per-line index_append needed here.
///
/// ⚠ SUPERSEDED TWICE — do not re-enable:
/// - Round 2 (2026-08-25 上午, fork 内容错位第一修): disabled in favor of
///   `write_chat_log_from_store` on the then-belief that SessionStore was
///   the single source of truth for turn semantics (see that note below).
/// - Round 3 (2026-08-25 深夜): the self-heal fix made jsonl the single
///   source of truth (store = rebuildable cache that compaction folds and
///   TTL deletes); the round-2 assumption was backwards, and forking off
///   the store produced garbage on real production sessions (store held a
///   truncated, tool-intermediate-polluted history while the user picked a
///   turn by the clean jsonl the UI renders). `fork_session` now reads the
///   rows itself and writes them via `write_chat_log_rows`.
///   Kept (not deleted) per the code-change discipline.
#[allow(dead_code)]
pub fn copy_chat_log_prefix(source_key: &str, new_key: &str, at_turn: usize) -> usize {
    // Whole-log read: fork is a one-shot admin op, not a hot path.
    let (all, _total, _more, _oldest) = read_chat_log(source_key, usize::MAX, None);
    let mut turns = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for v in all {
        if v.get("role").and_then(|r| r.as_str()) == Some("user") {
            turns += 1;
            if turns > at_turn {
                break;
            }
        }
        if let Ok(line) = serde_json::to_string(&v) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&target) {
        for l in &lines {
            let _ = writeln!(f, "{}", l);
        }
    }
    lines.len()
}

/// FIX (2026-08-25): single source of truth for "which rows become visible
/// chat rows" — used by the self-heal rebuild / fork store mapping
/// (`session::projected_messages_from_rows`) AND the turns endpoint's
/// `end_preview` computation (api_handlers). The call sites must never
/// drift apart, or the fork dialog's "分叉末条" preview would disagree with
/// what the fork actually ends on — the exact class of bug this round
/// fixed. Tool/system rows never project (chat_log is the UI bubble
/// source); an `assistant` row with empty/whitespace content is a pure
/// tool_calls intermediate, not a displayable reply.
pub fn is_projected_chat_row(role: &str, content: &str) -> bool {
    match role {
        "user" => true,
        "assistant" => !content.trim().is_empty(),
        _ => false,
    }
}

/// FIX (2026-08-25 分叉内容错位, round 2): generate `new_key`'s chat_log by
/// PROJECTING a SessionStore history prefix — the replacement for
/// `copy_chat_log_prefix` (see its ⚠ SUPERSEDED note for the divergence
/// bug this fixes).
///
/// ⚠ SUPERSEDED (2026-08-25 round 3, fork 第三轮): this function's premise —
/// "SessionStore is the single source of truth for turn semantics" — was
/// inverted by the self-heal fix later the same day: jsonl is the truth,
/// the store is a lossy, compaction-folded, TTL-deleted cache. Projecting
/// the fork's chat_log FROM the store made the fork inherit every store
/// defect (truncated history, tool-intermediate pollution, folded turns),
/// producing garbage forks on real production sessions. `fork_session` now
/// copies the jsonl rows verbatim (`write_chat_log_rows`) and derives the
/// store FROM those rows (`session::projected_messages_from_rows`) — the
/// store→jsonl direction is dead. Kept (not deleted) per the code-change
/// discipline.
///
/// Projection semantics (historical, for when this was live):
/// - only `user` / `assistant` rows are written (chat_log is the UI bubble
///   source; tool/system rows were never logged there);
/// - `assistant` rows with empty/whitespace content are skipped — those are
///   pure tool_calls intermediate messages; the final per-turn reply has
///   content. (A non-empty intermediate reply is real model output and is
///   kept — honest content beats byte-parity with a source log that may
///   itself be stale.)
/// - timestamps come from the stored messages (a fork must not re-stamp
///   history);
/// - model badge / cron markers are NOT carried over (the store does not
///   record them; a missing badge degrades to "no badge" on the read side,
///   which parses fine — an acceptable display-only cost for guaranteed
///   store↔log alignment).
///
/// By construction the new session's two stores agree: the Dashboard's
/// last displayed message == the last user/assistant message of
/// `messages`. The lazy FTS full-index picks the new file up on first
/// search; no per-line index_append needed here (same as the old copy).
/// Returns the number of lines written.
#[allow(dead_code)]
pub fn write_chat_log_from_store(
    new_key: &str,
    messages: &[crate::session::StoredMessage],
) -> usize {
    let mut lines: Vec<String> = Vec::new();
    for m in messages {
        if !is_projected_chat_row(&m.role, &m.content) {
            continue;
        }
        let entry = serde_json::json!({
            "role": m.role,
            "content": m.content,
            "timestamp": m.timestamp,
        });
        if let Ok(line) = serde_json::to_string(&entry) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut written = 0usize;
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&target) {
        for l in &lines {
            if writeln!(f, "{}", l).is_ok() {
                written += 1;
            }
        }
    }
    written
}

/// Delete a session's chat log file (JSONL). Used by session management
/// (delete conversation) to clear the user-facing history. Also deletes the
/// boundary-events sidecar (so a re-created session doesn't inherit stale
/// audit rows) and the title meta sidecar (2026-08-25: it used to survive as
/// an orphan — invisible to `sessions.list` which scans jsonl only, but dead
/// bytes on disk; `clear` deliberately KEEPS the meta because the
/// conversation stays alive with its title). No-op if absent.
pub fn delete_chat_log(session_key: &str) {
    let path = log_path(session_key);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("[chat_log] Failed to delete {}: {}", path.display(), e);
        }
    let bpath = boundary_path(session_key);
    if let Err(e) = std::fs::remove_file(&bpath)
        && e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("[chat_log] Failed to delete {}: {}", bpath.display(), e);
        }
    let mpath = meta_path(session_key);
    if let Err(e) = std::fs::remove_file(&mpath)
        && e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("[chat_log] Failed to delete {}: {}", mpath.display(), e);
        }
}

/// Clear (truncate) a session's chat log, keeping the file. Used by session
/// management "clear" — empties history but the session id stays usable.
/// Also truncates the boundary-events sidecar (same lifecycle).
pub fn clear_chat_log(session_key: &str) {
    let path = log_path(session_key);
    if let Err(e) = fs::write(&path, "") {
        tracing::warn!("[chat_log] Failed to clear {}: {}", path.display(), e);
    }
    let bpath = boundary_path(session_key);
    if bpath.exists()
        && let Err(e) = fs::write(&bpath, "") {
            tracing::warn!("[chat_log] Failed to clear {}: {}", bpath.display(), e);
        }
}

/// Path for the sidecar title meta file (`{safe_key}.meta.json`, next to the
/// `.jsonl`). Stores a user-editable conversation title for multi-session
/// management without touching the lazy-created SessionStore.
fn meta_path(session_key: &str) -> PathBuf {
    let safe_key = session_key.replace(':', "_");
    default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.meta.json", safe_key))
}

/// Write the conversation title to the sidecar meta file.
pub fn write_session_meta(session_key: &str, title: &str) {
    let path = meta_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&path, serde_json::json!({ "title": title }).to_string()) {
        tracing::warn!("[chat_log] failed to write meta {}: {}", path.display(), e);
    }
}

/// Read the conversation title from the sidecar meta file, if present.
pub fn read_session_meta(session_key: &str) -> Option<String> {
    let path = meta_path(session_key);
    let data = fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&data).ok()?;
    v.get("title")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// I3 (U9): boundary events (round-5 fix: sidecar file)
// ---------------------------------------------------------------------------

/// Resolve the boundary-events SIDECAR path (`logs/boundary/<safe_key>.jsonl`).
///
/// Round-5 review fix: boundary events used to be interleaved into the
/// message jsonl — which skewed `read_chat_log` pagination counts (total
/// counted boundary lines, pages underfilled or came back empty with
/// has_more=true), made every NEW session's first line a `turn_start` row
/// (blank title/preview in the Dashboard session list, which reads
/// `lines[0]["content"]`), and rendered empty bubbles in raw readers
/// (`logs.rs` session_detail / scan_session_logs bypass read_chat_log).
/// A separate file fixes all of them at the storage layer. Deliberately NOT
/// inside `session_logs/`: that dir is scanned for `*.jsonl` as sessions —
/// a sidecar there would appear as a phantom session.
fn boundary_path(session_key: &str) -> PathBuf {
    let safe_key = session_key.replace(':', "_");
    default_path_manager()
        .boundary_events_dir()
        .join(format!("{}.jsonl", safe_key))
}

/// Append a turn/step boundary event to the session's boundary sidecar.
/// Lightweight durable markers for replay/audit: `turn_start` / `turn_end`
/// (with a reason) / `llm_request` (model + token estimate) /
/// `steer_injected`. Never contains message bodies.
pub fn append_boundary_event(session_key: &str, kind: &str, detail: &str) {
    let path = boundary_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("[chat_log] boundary event open failed {}: {}", path.display(), e);
            return;
        }
    };
    let entry = serde_json::json!({
        "role": "boundary",
        "event": kind,
        "detail": detail,
        "timestamp": Local::now().to_rfc3339(),
    });
    if let Ok(line) = serde_json::to_string(&entry) {
        let _ = writeln!(file, "{}", line);
    }
}

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
