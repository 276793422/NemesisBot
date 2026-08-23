//! Z1 (Phase4-d): session forking — a TRUE branch, not a rollback.
//!
//! Fork = copy a session's history up to a selected TURN boundary under a
//! NEW session key. The original session is never touched (its later turns
//! keep flowing); the new session starts with the copied prefix and its own
//! ledger/boundary/chat-log files (keys ARE the ledger identity, so the new
//! key's U9 replay ledger and boundary sidecar start fresh at the fork
//! point by construction).
//!
//! Turn semantics: a "turn" is one user→…→assistant exchange. History index
//! 0 is the system prompt (not a turn). `--at N` keeps turns 1..N COMPLETE
//! (the cut lands right before the (N+1)-th user message); `N >= turn
//! count` keeps the whole history (fork at head); the default is the whole
//! history. Summary-cache coherence: `summary_covers_up_to` indexes the
//! same history; the summary is copied only when the cut does not drop
//! covered content (`cut >= covers`) — otherwise it is dropped, because the
//! summary text would reference messages the fork no longer has. Legacy
//! sessions (`covers == None` with a non-empty summary) get the same
//! conservative treatment on a partial cut: coverage is unknowable, so the
//! summary is dropped rather than kept incoherent.
//!
//! The copy spans BOTH stores that make a session usable:
//! - `SessionStore` (model context restore — `get_or_create_instance`
//!   rebuilds the provider-visible history from it, X1 projection applied
//!   deterministically at build time), and
//! - `chat_log` jsonl (Dashboard session browser / history reads).
//! Boundary events (`session_fork_out` / `session_fork_in`) land in the
//! U9 sidecar for both keys.

use crate::chat_log;
use crate::session::{SessionStore, StoredMessage};

/// Result of a successful fork.
#[derive(Debug, Clone)]
pub struct ForkInfo {
    pub source_key: String,
    pub new_key: String,
    /// The 1-based turn boundary actually used (clamped).
    pub at_turn: usize,
    /// Messages kept in the new session's store history.
    pub kept_messages: usize,
    /// Messages dropped from the copy (source keeps them — they are only
    /// excluded from the FORK).
    pub dropped_messages: usize,
    /// Whether the summary cache was carried over.
    pub summary_kept: bool,
    /// chat_log lines copied under the new key.
    pub chat_log_lines: usize,
}

/// Count COMPLETE user turns in a stored history (the system prompt at
/// index 0 and any trailing messages without a following user message are
/// not counted).
pub fn user_turn_count(messages: &[StoredMessage]) -> usize {
    messages.iter().filter(|m| m.role == "user").count()
}

/// Message index where the fork cut lands for `--at N`: right BEFORE the
/// (N+1)-th user message, i.e. turns 1..N complete. `n >= turn count` (or
/// `n == 0` with an empty turn set) keeps the whole history.
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
/// on-disk file — the latter covers a store constructed before the file
/// appeared, e.g. the running gateway).
fn unique_key(store: &SessionStore, source_key: &str, requested: Option<String>) -> String {
    let base = requested.unwrap_or_else(|| format!("{}__fork", source_key));
    if !store.contains(&base) && !store.file_exists(&base) {
        return base;
    }
    for n in 2.. {
        let cand = format!("{}_{}", base, n);
        if !store.contains(&cand) && !store.file_exists(&cand) {
            return cand;
        }
    }
    unreachable!("uniquifier exhausted u64-ish range");
}

/// Copy the source session's chat_log prefix (same user-turn counting) to
/// the new key, preserving each line's original fields verbatim
/// (timestamps included). Delegates to `chat_log::copy_chat_log_prefix`
/// (file-layout internals stay private to chat_log). Returns lines copied.
fn copy_chat_log_prefix(source_key: &str, new_key: &str, at_turn: usize) -> usize {
    chat_log::copy_chat_log_prefix(source_key, new_key, at_turn)
}

/// Fork `source_key` at a turn boundary into a new session key.
///
/// The source session is only READ (plus one boundary event appended); all
/// writes target the new key. Errors are user-facing (CLI prints them).
pub fn fork_session(
    store: &SessionStore,
    source_key: &str,
    requested_new_key: Option<String>,
    at_turn: Option<usize>,
) -> Result<ForkInfo, String> {
    let messages = store.get_history(source_key);
    if messages.is_empty() {
        return Err(format!(
            "source session {:?} 不存在或历史为空（先确认 session key，例如 agent:main:session:legacy）",
            source_key
        ));
    }
    let turns = user_turn_count(&messages);
    if turns == 0 {
        return Err(format!(
            "source session {:?} 没有任何完整 user 轮次，无可分支内容",
            source_key
        ));
    }
    let at = at_turn.unwrap_or(turns).min(turns).max(1);
    let cut = turn_cut(&messages, at);

    // Summary coherence (see module doc): keep only when the cut does not
    // drop covered content. `covers` indexes the full history including the
    // system prompt at 0 — same indexing as `messages`.
    let summary = store.get_summary(source_key);
    let covers = store.get_summary_covers_up_to(source_key);
    let (summary_kept, new_summary, new_covers) = if summary.is_empty() {
        (false, String::new(), None)
    } else {
        match covers {
            Some(c) if cut >= c => (true, summary.clone(), Some(c)),
            // Legacy None + partial cut: coverage unknowable → drop.
            None if cut == messages.len() => (true, summary.clone(), None),
            _ => (false, String::new(), None),
        }
    };

    let new_key = unique_key(store, source_key, requested_new_key);

    // Materialize the new session in the store and persist it.
    store.get_or_create(&new_key);
    store.set_history(&new_key, messages[..cut].to_vec());
    if summary_kept {
        store.set_summary(&new_key, &new_summary);
        store.set_summary_covers_up_to(&new_key, new_covers);
    } else {
        store.set_summary(&new_key, "");
        store.set_summary_covers_up_to(&new_key, None);
    }
    store.save(&new_key).map_err(|e| format!("写入新会话失败: {}", e))?;

    // Mirror the prefix into the chat log so the Dashboard session browser
    // and history reads see the forked conversation, not an empty key.
    let chat_log_lines = copy_chat_log_prefix(source_key, &new_key, at);

    // Boundary events on BOTH keys (U9 sidecar; new key = fresh ledger).
    chat_log::append_boundary_event(
        source_key,
        "session_fork_out",
        &format!(
            "forked to {} at turn {} (kept {} / dropped {} messages)",
            new_key,
            at,
            cut,
            messages.len() - cut
        ),
    );
    chat_log::append_boundary_event(
        &new_key,
        "session_fork_in",
        &format!(
            "forked from {} at turn {} (kept {} messages, summary_kept={})",
            source_key,
            at,
            cut,
            summary_kept
        ),
    );

    Ok(ForkInfo {
        kept_messages: cut,
        dropped_messages: messages.len() - cut,
        at_turn: at,
        source_key: source_key.to_string(),
        new_key,
        summary_kept,
        chat_log_lines,
    })
}

#[cfg(test)]
mod tests;
