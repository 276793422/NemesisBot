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

#[cfg(test)]
mod tests;
