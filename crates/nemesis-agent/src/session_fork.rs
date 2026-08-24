//! Z1 (Phase4-d): session forking — a TRUE branch, not a rollback.
//!
//! Fork = copy a session's history up to a selected TURN boundary under a
//! NEW session key. The original session is never touched (its later turns
//! keep flowing); the new session starts with the copied prefix and its own
//! ledger/boundary/chat-log files (keys ARE the ledger identity, so the new
//! key's U9 replay ledger and boundary sidecar start fresh at the fork
//! point by construction).
//!
//! Turn semantics: a "turn" is one user→…→assistant exchange over the
//! chat_log rows. `--at N` keeps turns 1..N COMPLETE (the cut lands right
//! before the (N+1)-th user row); `N >= turn count` keeps the whole log
//! (fork at head); the default is the whole log.
//!
//! ⚠ ROUND-3 FIX (2026-08-25 fork 第三轮): **the chat_log jsonl is the
//! single source of truth for turn semantics** — the fork dialog's turn
//! table counts jsonl rows, the Dashboard renders jsonl rows, so the fork
//! cut must be taken on the SAME rows the user picked by. Round 2
//! (earlier the same day) had inverted this: it counted and copied from
//! the SessionStore prefix on the belief that the store was the truth —
//! but the self-heal fix (later the same day) established the opposite
//! world: the store is a LOSSY, REBUILDABLE CACHE that compaction folds,
//! tool intermediates pollute, and the 7-day TTL deletes. Real production
//! case: the user picked "第 9 轮" by the clean jsonl (rows 0-17, the
//! "1i+1i=2i" turn) while the fork copied the store's coordinate system
//! (truncated to August content, starting mid-tool-intermediate, 9 user
//! turns ending elsewhere, the same reply duplicated) — a garbage fork
//! that "verified green" in round 2 because the dialog's preview and the
//! copy read the same defective store (circular validation).
//!
//! Consequences of the fix, all by construction:
//! - the new jsonl = the source jsonl's first-N-turn rows **verbatim**
//!   (timestamps / model badges / cron markers preserved — see
//!   `chat_log::write_chat_log_rows`);
//! - the new store = `session::projected_messages_from_rows` over the same
//!   cut rows — byte-identical to what a later self-heal rebuild of the
//!   fork's jsonl would produce, so the fork's model context can never
//!   silently shift the day its store json ages out of the TTL;
//! - the summary is NEVER carried (jsonl records no summary; a store
//!   summary text may reference folded/truncated content that isn't in
//!   the fork). `ForkInfo::summary_kept` stays in the API shape and is
//!   always `false`.
//!
//! Boundary events (`session_fork_out` / `session_fork_in`) land in the
//! U9 sidecar for both keys.

use crate::chat_log;
use crate::session::{self, SessionStore, StoredMessage};
use serde_json::Value;

/// Result of a successful fork.
#[derive(Debug, Clone)]
pub struct ForkInfo {
    pub source_key: String,
    pub new_key: String,
    /// The 1-based turn boundary actually used (clamped).
    pub at_turn: usize,
    /// chat_log rows kept in the new session (the fork dialog's cumulative
    /// "kept" for this turn — round 3: counted on jsonl rows).
    pub kept_messages: usize,
    /// chat_log rows excluded from the fork (source keeps them).
    pub dropped_messages: usize,
    /// Always `false` under the jsonl-truth rule (round 3): the fork store
    /// is rebuilt from jsonl rows, which carry no summary. Kept for API
    /// shape compatibility.
    pub summary_kept: bool,
    /// chat_log lines copied under the new key (verbatim).
    pub chat_log_lines: usize,
}

/// Count COMPLETE user turns over chat_log rows (round-3 truth source: the
/// same rows the fork dialog counts and the Dashboard renders).
pub fn row_user_turn_count(rows: &[Value]) -> usize {
    rows.iter()
        .filter(|v| v.get("role").and_then(|r| r.as_str()) == Some("user"))
        .count()
}

/// Row index where the fork cut lands for `--at N`: right BEFORE the
/// (N+1)-th user row, i.e. turns 1..N complete. `n >= turn count` (or
/// `n == 0` with an empty turn set) keeps the whole log.
fn row_turn_cut(rows: &[Value], n: usize) -> usize {
    let mut seen = 0usize;
    for (i, v) in rows.iter().enumerate() {
        if v.get("role").and_then(|r| r.as_str()) == Some("user") {
            seen += 1;
            if seen > n {
                return i;
            }
        }
    }
    rows.len()
}

/// Count COMPLETE user turns in a STORED history.
///
/// ⚠ SUPERSEDED (2026-08-25 fork 第三轮): the store is a lossy cache — turn
/// semantics must be counted on jsonl rows (`row_user_turn_count`), which
/// is what the fork dialog and the Dashboard do. Kept (not deleted) per
/// the code-change discipline.
#[allow(dead_code)]
pub fn user_turn_count(messages: &[StoredMessage]) -> usize {
    messages.iter().filter(|m| m.role == "user").count()
}

/// Message index where the fork cut lands for `--at N` over a STORED
/// history.
///
/// ⚠ SUPERSEDED (2026-08-25 fork 第三轮) by `row_turn_cut` — see
/// `user_turn_count`. Kept (not deleted) per the code-change discipline.
#[allow(dead_code)]
fn turn_cut(messages: &[StoredMessage], n: usize) -> usize {
    let mut seen = 0usize;
    for (i, m) in messages.iter().enumerate() {
        if m.role == "user" {
            seen += 1;
            if seen > n {
                return i;
            }
        }
    }
    messages.len()
}

/// Choose a fresh key: the given candidate, or `{source}__fork`; suffixed
/// `_2`, `_3`, … while the store already knows the key (in-memory cache OR
/// on-disk file) **or the jsonl exists** (round 3: a previous fork's jsonl
/// survives its store json's 7-day TTL — reusing that key would APPEND the
/// new prefix onto the old fork's log, duplicating it).
fn unique_key(store: &SessionStore, source_key: &str, requested: Option<String>) -> String {
    let taken = |k: &str| {
        store.contains(k) || store.file_exists(k) || chat_log::chat_log_exists(k)
    };
    let base = requested.unwrap_or_else(|| format!("{}__fork", source_key));
    if !taken(&base) {
        return base;
    }
    for n in 2.. {
        let cand = format!("{}_{}", base, n);
        if !taken(&cand) {
            return cand;
        }
    }
    unreachable!("uniquifier exhausted u64-ish range");
}

/// Fork `source_key` at a turn boundary into a new session key.
///
/// The source session is only READ (plus one boundary event appended); all
/// writes target the new key. Errors are user-facing (CLI prints them).
///
/// Round-3 contract: the cut is taken on the chat_log rows (the truth the
/// user picks by); the new jsonl is a verbatim copy of the cut prefix; the
/// new store is derived from those same rows. See the module doc for why.
pub fn fork_session(
    store: &SessionStore,
    source_key: &str,
    requested_new_key: Option<String>,
    at_turn: Option<usize>,
) -> Result<ForkInfo, String> {
    // Whole-log read: fork is a one-shot admin op, not a hot path.
    let (rows, total, _, _) = chat_log::read_chat_log(source_key, usize::MAX, None);
    if rows.is_empty() {
        return Err(format!(
            "source session {:?} 不存在或聊天记录（jsonl）为空（先确认 session key，例如 agent:main:session:legacy）",
            source_key
        ));
    }
    let turns = row_user_turn_count(&rows);
    if turns == 0 {
        return Err(format!(
            "source session {:?} 没有任何完整 user 轮次，无可分支内容",
            source_key
        ));
    }
    let at = at_turn.unwrap_or(turns).min(turns).max(1);
    let cut = row_turn_cut(&rows, at);

    let new_key = unique_key(store, source_key, requested_new_key);

    // 1) jsonl side: verbatim copy of the picked prefix. MUST happen before
    // the store side: `get_or_create` below would otherwise consult its
    // self-heal layer against a not-yet-existing file (harmless either way,
    // but jsonl-first keeps the write order the same as the live append
    // path — durable truth before derived cache).
    let chat_log_lines = chat_log::write_chat_log_rows(&new_key, &rows[..cut]);

    // 2) store side: mirror the self-heal construction over the SAME rows
    // (single shared mapping — session::projected_messages_from_rows).
    // `set_history` applies the store's own MAX_STORED_MESSAGES trim, so an
    // over-long prefix ages exactly like any long session. Summary is
    // never carried (round-3 rule, see module doc).
    let messages = session::projected_messages_from_rows(&rows[..cut]);
    store.get_or_create(&new_key);
    store.set_history(&new_key, messages);
    store.set_summary(&new_key, "");
    store.set_summary_covers_up_to(&new_key, None);
    store.save(&new_key).map_err(|e| format!("写入新会话失败: {}", e))?;

    // Boundary events on BOTH keys (U9 sidecar; new key = fresh ledger).
    chat_log::append_boundary_event(
        source_key,
        "session_fork_out",
        &format!(
            "forked to {} at turn {} (kept {} / dropped {} log rows)",
            new_key, at, cut, total - cut
        ),
    );
    chat_log::append_boundary_event(
        &new_key,
        "session_fork_in",
        &format!(
            "forked from {} at turn {} (kept {} log rows, summary_kept=false)",
            source_key, at, cut
        ),
    );

    Ok(ForkInfo {
        kept_messages: cut,
        dropped_messages: total - cut,
        at_turn: at,
        source_key: source_key.to_string(),
        new_key,
        summary_kept: false,
        chat_log_lines,
    })
}

#[cfg(test)]
mod tests;
