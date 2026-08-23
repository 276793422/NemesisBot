//! Tool-result spill storage (U4, dsh-alignment first batch).
//!
//! Results too large even for the pruned inline form (see `prune.rs` for the
//! smaller tier) are written whole to a session-scoped file under
//! `<home>/logs/spill/…`; the conversation keeps only a bounded preview plus
//! the file locator and a retrieval hint, so the model can `read_file` the
//! spill back with offset/limit or `grep` it when it actually needs the full
//! text. Mirrors dsh's spill seam (SpillStore + spill-policy) at a fraction
//! of the shape: no backend abstraction yet — local std-fs only, per the
//! goal's no-new-dependencies constraint.
//!
//! Path safety: session keys and call ids are model-influenced strings, so
//! they are sanitized to a conservative filename character set BEFORE they
//! become path segments (a `session_key` of `../../etc` must not escape the
//! spill root). Timestamps come from the caller.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Results at or above this many chars spill to disk instead of being kept
/// (pruned) inline. Must be >= `crate::prune::MAX_TOOL_RESULT_INLINE_CHARS`
/// so the two tiers compose: <=8192 inline, 8193..65535 pruned inline,
/// >=65536 spilled (and the spill REPLACES the inline text entirely with a
/// short preview + locator).
pub const SPILL_THRESHOLD_CHARS: usize = 65_536;

/// Preview budget (chars) kept in-conversation for a spilled result.
const SPILL_PREVIEW_CHARS: usize = 2000;

/// Conservative filename charset for path segments derived from
/// model-influenced ids: ASCII alphanumerics, `-`, `_`, and `.` only when it
/// follows an alphanumeric (so `..`, `./`, `a..` collapse). Everything else
/// collapses to `_`; a segment that sanitizes to empty (or to a dot-form)
/// becomes `_`.
fn sanitize_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        let ok = c.is_ascii_alphanumeric() || c == '-' || c == '_'
            || (c == '.' && out.ends_with(|p: char| p.is_ascii_alphanumeric()));
        out.push(if ok { c } else { '_' });
    }
    // Guard against `.`/`..` after sanitization and empty names.
    if out.is_empty() || out == "." || out == ".." {
        out.push('_');
    }
    // Cap length: a hostile very-long id should not blow path limits.
    out.chars().take(80).collect()
}

/// Where a spill file lands: `<spill_root>/<sanitized session>/<stamp>_<sanitized call_id>.txt`.
/// Exposed for tests.
fn spill_path(spill_root: &Path, session_key: &str, stamp: &str, call_id: &str) -> PathBuf {
    spill_root
        .join(sanitize_segment(session_key))
        .join(format!(
            "{}_{}.txt",
            sanitize_segment(stamp),
            sanitize_segment(call_id)
        ))
}

/// Outcome of trying to spill a result.
pub enum SpillOutcome {
    /// Result below threshold — caller keeps the (pruned) inline form.
    BelowThreshold,
    /// Spill failed (storage error). The caller keeps the ORIGINAL result —
    /// a spill failure must never turn a successful tool call into a lossy
    /// one (best-effort semantics, same stance as dsh's spill-policy).
    SpillFailed,
    /// Spilled; this is the replacement text for the conversation.
    Spilled(String),
}

/// Full-text spill with bounded preview + locator. `stamp` is the caller's
/// sortable timestamp suffix (e.g. `20260821_153000_123`).
pub fn spill_tool_result(
    result: &str,
    tool_name: &str,
    spill_root: &Path,
    session_key: &str,
    stamp: &str,
    call_id: &str,
) -> SpillOutcome {
    if result.chars().count() < SPILL_THRESHOLD_CHARS {
        return SpillOutcome::BelowThreshold;
    }
    let path = spill_path(spill_root, session_key, stamp, call_id);
    let dir = match path.parent() {
        Some(d) => d,
        None => return SpillOutcome::SpillFailed,
    };
    if std::fs::create_dir_all(dir).is_err() {
        return SpillOutcome::SpillFailed;
    }
    let mut file = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(_) => return SpillOutcome::SpillFailed,
    };
    if file.write_all(result.as_bytes()).is_err() {
        // Remove the partial file so the locator never points at a truncated
        // spill; the caller falls back to the pruned inline form.
        let _ = std::fs::remove_file(&path);
        return SpillOutcome::SpillFailed;
    }
    let preview: String = result.chars().take(SPILL_PREVIEW_CHARS).collect();
    let total = result.chars().count();
    SpillOutcome::Spilled(format!(
        "{preview}\n[输出过大（{} 字符）已完整保存到：{}。可用 read_file 工具按 offset/limit 分段读取该文件，或用 grep 工具在其中检索关键词。]\n（工具：{}）",
        total,
        path.display(),
        tool_name
    ))
}

/// U4 retention cleanup: delete spill files older than `retention_days`
/// (by file mtime), then remove session dirs that became empty (and the
/// spill root itself if it became empty). Returns the number of files
/// deleted. Failures on individual files/dirs WARN but never abort the sweep
/// — cleanup is housekeeping, not a correctness path. `retention_days == 0`
/// is the caller's "disabled" flag (returns 0 without touching the tree).
pub fn cleanup_expired(spill_root: &Path, retention_days: u64) -> usize {
    if retention_days == 0 {
        return 0;
    }
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(retention_days * 24 * 3600);
    let mut deleted = 0usize;

    let sessions = match std::fs::read_dir(spill_root) {
        Ok(rd) => rd,
        Err(_) => return 0, // no root / unreadable — nothing to clean
    };
    for entry in sessions.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let mut dir_empty = true;
            if let Ok(files) = std::fs::read_dir(&path) {
                for f in files.flatten() {
                    let fp = f.path();
                    if fp.is_dir() {
                        dir_empty = false; // unexpected nesting — leave it alone
                        continue;
                    }
                    let expired = f
                        .metadata()
                        .and_then(|m| m.modified())
                        .map(|mt| mt < cutoff)
                        .unwrap_or(false);
                    if expired {
                        match std::fs::remove_file(&fp) {
                            Ok(()) => deleted += 1,
                            Err(e) => {
                                tracing::warn!(
                                    "[Spill] retention cleanup failed to delete '{}': {}",
                                    fp.display(),
                                    e
                                );
                                dir_empty = false; // file survived — dir not empty
                            }
                        }
                    } else {
                        dir_empty = false; // fresh file kept
                    }
                }
            }
            if dir_empty {
                if let Err(e) = std::fs::remove_dir(&path) {
                    tracing::warn!(
                        "[Spill] retention cleanup failed to remove dir '{}': {}",
                        path.display(),
                        e
                    );
                }
            }
        } else if path.is_file() {
            // A stray file directly under the root (not expected from
            // spill_tool_result, but sweep it under the same rule).
            let expired = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|mt| mt < cutoff)
                .unwrap_or(false);
            if expired {
                if let Err(e) = std::fs::remove_file(&path) {
                    tracing::warn!(
                        "[Spill] retention cleanup failed to delete '{}': {}",
                        path.display(),
                        e
                    );
                } else {
                    deleted += 1;
                }
            }
        }
    }
    deleted
}

#[cfg(test)]
mod tests;
