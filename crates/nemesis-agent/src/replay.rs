//! Request-replay projection ledger + byte-exact verification (T8 / U9 ②).
//!
//! U9's acceptance: "session_log 可重放出与 request_log 逐字节一致的请求序列".
//! Before this module the only cross-check was [`check_request_log_consistency`]
//! (role-subsequence, no bodies). The gap: every LLM request's message list is a
//! PROJECTION of the persisted history — `build_messages_with_memory` folds the
//! summary cache, repairs tool pairs, and injects transient messages (the merged
//! `<system-reminder>` context digest, grace/degenerate/repetition nudges, the
//! voice-playback suffix) that are NEVER persisted. Replaying from the session
//! store alone cannot reproduce those bytes.
//!
//! This module closes the gap with a projection ledger: at request time the
//! agent loop records, per LLM round, every non-persisted injection (exact
//! content + final-vec position) plus the summary-cache state as-of that round.
//! Replaying = persisted history (session store) → the SAME deterministic fold
//! (`project_history_for_request`) → re-apply the recorded injections → compare
//! byte-exact against the request_logger's raw.json `body.messages`.
//!
//! Honest boundaries (documented, by design):
//! - **Trimmed history**: `SessionStore::set_history` trims to
//!   `MAX_STORED_MESSAGES`; if the messages a round needed were dropped,
//!   byte-exact replay is impossible → rebuild reports `Unavailable` (never
//!   fabricates).
//! - **Ledger absent** (sessions predating this feature, or boundary-logging
//!   exempt turns — cron/heartbeat, same gate as boundary events): replay
//!   degrades EXPLICITLY to the role-subsequence anchor.
//! - **Scope**: the MAIN request path only (the LlmRequest observer event that
//!   feeds `NN.AI.Request.raw.json`). Summarizer sub-requests and compaction
//!   calls have their own logging and are out of scope.
//! - steer messages are persisted into the instance history when claimed (see
//!   the claim site in `run_llm_loop`) — they are NOT transient, so they need
//!   no ledger entry.
//!
//! [`check_request_log_consistency`]: crate::request_logger::check_request_log_consistency

use chrono::Local;
use nemesis_path::default_path_manager;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::r#loop::LlmMessage;
use crate::session::SessionStore;
use crate::types::ConversationTurn;

/// Ledger source tag: the merged time/env + skills + instructions + memory
/// `<system-reminder>` injected by `build_messages_with_memory`.
pub const INJECTION_CONTEXT_DIGEST: &str = "context_digest";
/// Ledger source tag: the grace-round closing nudge (transient, appended).
pub const INJECTION_GRACE_NUDGE: &str = "grace_nudge";
/// Ledger source tag: the degenerate-answer nudge (transient, appended).
pub const INJECTION_DEGENERATE_NUDGE: &str = "degenerate_nudge";
/// Ledger source tag: the prose-repetition nudge (transient, appended).
pub const INJECTION_REPETITION_NUDGE: &str = "repetition_nudge";
/// Ledger source tag (voice): the playback-mode suffix appended to the last
/// user message's content — a mutation, not a standalone message.
pub const INJECTION_VOICE_APPEND: &str = "voice_append";
/// Ledger source tag (K1b/U14): messages appended by a registered LLM pre
/// hook (reminder / discipline prompts), between the built nudges and the
/// LlmRequest observer event.
pub const INJECTION_LLM_HOOK: &str = "llm_hook";

/// One non-persisted injection recorded for replay. `index` is the position in
/// the FINAL message vec sent to the provider (after all inserts/appends that
/// preceded it in production order).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionRecord {
    /// Position in the final outgoing message vec.
    pub index: usize,
    /// Role of the injected message (`user` for the context digest under the
    /// default snapshot role, `system`/`user` for nudges).
    pub role: String,
    /// Which injection mechanism produced it (see the `INJECTION_*` consts).
    pub source: String,
    /// Exact injected content (byte-identical to what the provider saw).
    pub content: String,
}

/// The voice-playback suffix mutation: `messages[index].content += suffix`.
/// Recorded separately because it edits a persisted-derived message rather
/// than adding one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceAppend {
    /// Position of the mutated (last user) message in the final vec.
    pub index: usize,
    /// The exact suffix appended.
    pub suffix: String,
}

/// Summary-cache state AS OF the recorded request (the fold input). The
/// session file's FINAL summary may have advanced later in the same turn, so
/// replay must use this snapshot, not the final one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryAsOf {
    /// `covers_up_to` value the fold used (indexes the full history).
    pub covers_up_to: usize,
    /// Summary text the fold used.
    pub text: String,
}

/// What `build_messages_with_memory_annotated` reports alongside the messages:
/// everything a later replay needs that is not derivable from the final
/// session file.
#[derive(Debug, Clone, Default)]
pub struct BuildAnnotation {
    /// Position of the merged context-digest injection in the returned vec,
    /// `None` when no injection happened (no system prompt to protect).
    pub digest_index: Option<usize>,
    /// Length of the folded+repaired history view (the persisted-derived
    /// prefix; final vec = this + injections).
    pub history_len: usize,
    /// Summary cache state the fold used (`None` = no active cache).
    pub summary_as_of: Option<SummaryAsOf>,
}

/// One ledger row: the projection of one LLM round's request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestProjectionRecord {
    /// Trace ID of the owning turn (disambiguates equal round numbers across
    /// turns — `round` restarts at 1 every turn).
    pub trace_id: String,
    /// Session key.
    pub session_key: String,
    /// LLM round within the turn (1-based, `turns_used + 1`).
    pub round: usize,
    /// Wall-clock timestamp (diagnostics only — never part of the replay).
    pub ts: String,
    /// Total messages sent (`roles.len()`).
    pub messages_count: usize,
    /// Role of every message, in order (cheap diff diagnosis aid).
    pub roles: Vec<String>,
    /// Length of the persisted-derived (folded+repaired) prefix.
    pub history_len_at_build: usize,
    /// Every non-persisted injection, in final-vec positions.
    pub injections: Vec<InjectionRecord>,
    /// The voice-playback suffix mutation, when active.
    pub voice_append: Option<VoiceAppend>,
    /// Summary-cache state as-of this request (`None` = full verbatim history).
    pub summary_as_of: Option<SummaryAsOf>,
}

/// Resolve the replay-ledger sidecar path. Lives in the boundary-events dir
/// (`logs/boundary/`) with a `.replay.jsonl` suffix so it never collides with
/// the flat `<key>.jsonl` boundary-event file (which carries no bodies).
fn replay_ledger_path(session_key: &str) -> PathBuf {
    let safe_key = session_key.replace(':', "_");
    default_path_manager()
        .boundary_events_dir()
        .join(format!("{}.replay.jsonl", safe_key))
}

/// Append one projection record to the session's replay ledger (best-effort:
/// failures warn and never touch the request path).
pub fn append_projection_record(rec: &RequestProjectionRecord) {
    let path = replay_ledger_path(&rec.session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "[replay] ledger open failed {}: {}",
                path.display(),
                e
            );
            return;
        }
    };
    let mut entry = serde_json::to_value(rec).unwrap_or_else(|_| serde_json::json!({}));
    // Marker parallel to the boundary events' `role: "boundary"` convention.
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("kind".to_string(), serde_json::json!("request_projection"));
    }
    if let Ok(line) = serde_json::to_string(&entry) {
        let _ = writeln!(file, "{}", line);
    }
}

/// Load all projection records for a session, oldest first. Missing file →
/// empty vec (caller treats as `NoLedger`).
pub fn load_projection_records(session_key: &str) -> Vec<RequestProjectionRecord> {
    load_projection_records_from(&replay_ledger_path(session_key))
}

/// Core loader for an EXPLICIT ledger path. `load_projection_records` is the
/// session-key convenience wrapper; the explicit form serves callers whose
/// workspace differs from the global path manager (dashboard logs handler —
/// the ledger lives at `<workspace>/logs/boundary/{sanitized}.replay.jsonl`).
pub fn load_projection_records_from(path: &std::path::Path) -> Vec<RequestProjectionRecord> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(|l| l.ok())
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Rebuild
// ---------------------------------------------------------------------------

/// Result of a rebuild attempt for one round.
#[derive(Debug)]
pub enum RebuildOutcome {
    /// Byte-exact sources available: the rebuilt message list for the round.
    Rebuilt(Vec<LlmMessage>),
    /// No ledger record for the round (pre-feature session or an exempted
    /// turn). Byte-exact replay impossible — fall back to the
    /// role-subsequence anchor (`check_request_log_consistency`).
    NoLedger { note: String },
    /// A ledger record exists but the persisted history it needs is gone
    /// (trim dropped it) — byte-exact replay impossible. Never fabricates.
    Unavailable { needed: usize, available: usize },
}

/// Same mapping `build_messages_with_memory` applies to history turns.
fn turn_to_msg(turn: &ConversationTurn) -> LlmMessage {
    LlmMessage {
        role: turn.role.clone(),
        content: turn.content.clone(),
        tool_calls: if turn.tool_calls.is_empty() {
            None
        } else {
            Some(turn.tool_calls.clone())
        },
        tool_call_id: turn.tool_call_id.clone(),
        reasoning_content: turn.reasoning_content.clone(),
    }
}

/// Rebuild the message list one recorded LLM round sent to the provider.
///
/// Inputs: the FINAL session-store history (the store is the durable
/// full-fidelity layer — chat_log is role+content only and cannot carry tool
/// calls) plus the projection ledger. When multiple turns produced the same
/// `round` number, the LAST record wins (most recent turn); pass a `trace_id`
/// variant through `load_projection_records` for exact disambiguation.
///
/// Projection order mirrors production exactly: fold (summary as-of) → repair
/// → truncate to the round's history view → insert/append injections at their
/// recorded final-vec positions → apply the voice suffix mutation.
pub fn rebuild_request_messages(
    store: &SessionStore,
    session_key: &str,
    round: usize,
) -> Result<RebuildOutcome, String> {
    rebuild_request_messages_in(store, &replay_ledger_path(session_key), round, None)
}

/// Core rebuild for an EXPLICIT ledger path (dashboard logs handler; see
/// `load_projection_records_from`). Semantics identical to
/// `rebuild_request_messages` — only the ledger source differs.
///
/// `trace_id` disambiguates equal `round` numbers across turns (round
/// restarts at 1 every turn): `Some(id)` selects that turn's record; `None`
/// keeps the last-record-wins behavior.
pub fn rebuild_request_messages_in(
    store: &SessionStore,
    ledger_path: &std::path::Path,
    round: usize,
    trace_id: Option<&str>,
) -> Result<RebuildOutcome, String> {
    let records = load_projection_records_from(ledger_path);
    let Some(rec) = records
        .iter()
        .rev()
        .find(|r| r.round == round && trace_id.is_none_or(|t| r.trace_id == t))
        .cloned()
    else {
        let ledger_name = ledger_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<ledger>");
        let trace_note = trace_id
            .map(|t| format!(" (trace_id `{t}`)"))
            .unwrap_or_default();
        return Ok(RebuildOutcome::NoLedger {
            note: format!(
                "no projection ledger record in `{}` for round {}{} \
                 (pre-feature session or boundary-logging-exempt turn)",
                ledger_name, round, trace_note
            ),
        });
    };

    // The ledger record carries the RAW (unsanitized) session key — the
    // explicit-ledger path intentionally has no separate key parameter.
    let session_key = rec.session_key.clone();
    store.get_or_create(&session_key);
    let history: Vec<ConversationTurn> = store
        .get_history(&session_key)
        .into_iter()
        .map(|m| m.into())
        .collect();

    let folded = crate::r#loop::project_history_for_request(
        &history,
        rec.summary_as_of
            .as_ref()
            .map(|s| (s.text.as_str(), s.covers_up_to)),
    );
    if folded.len() < rec.history_len_at_build {
        // The store's history (after fold) is shorter than what the round
        // saw: trim_to_limit (or a store reset) dropped the needed prefix.
        return Ok(RebuildOutcome::Unavailable {
            needed: rec.history_len_at_build,
            available: folded.len(),
        });
    }

    let mut view: Vec<LlmMessage> = folded[..rec.history_len_at_build]
        .iter()
        .map(turn_to_msg)
        .collect();

    // Re-apply injections at their recorded final-vec positions. Mid-vec
    // entries (context digest) insert; tail nudges were pushed at what was
    // then the vec end — sorted order reproduces the same final layout.
    let mut injections = rec.injections.clone();
    injections.sort_by_key(|i| i.index);
    for inj in injections {
        let msg = LlmMessage {
            role: inj.role,
            content: inj.content,
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        };
        let idx = inj.index.min(view.len());
        if idx >= view.len() {
            view.push(msg);
        } else {
            view.insert(idx, msg);
        }
    }

    // Voice suffix mutates a persisted-derived message at its final position.
    if let Some(va) = rec.voice_append
        && let Some(m) = view.get_mut(va.index) {
            m.content.push_str(&va.suffix);
        }

    Ok(RebuildOutcome::Rebuilt(view))
}

// ---------------------------------------------------------------------------
// Byte-exact verification
// ---------------------------------------------------------------------------

/// First-difference diagnosis between a rebuild and the recorded request.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayDiff {
    /// Index of the first differing message (or the shorter side's length for
    /// count mismatches).
    pub index: usize,
    /// Diff class: "count" / "role" / "content" / "tool_calls" / "field".
    pub kind: String,
    /// Human-oriented detail (includes the byte offset for content diffs,
    /// char-boundary safe).
    pub detail: String,
}

/// Byte-exact comparison of a rebuilt message list against the recorded
/// request messages (`NN.AI.Request.raw.json` → `body.messages`, which are
/// `serde_json::to_value(LlmMessage)` — the same serialization both sides).
/// Returns the FIRST difference with diagnosis.
pub fn verify_request_replay(
    rebuilt: &[LlmMessage],
    recorded: &[serde_json::Value],
) -> Result<(), ReplayDiff> {
    if rebuilt.len() != recorded.len() {
        return Err(ReplayDiff {
            index: rebuilt.len().min(recorded.len()),
            kind: "count".to_string(),
            detail: format!(
                "rebuilt has {} messages, recorded has {}",
                rebuilt.len(),
                recorded.len()
            ),
        });
    }
    for (i, msg) in rebuilt.iter().enumerate() {
        let rv = serde_json::to_value(msg)
            .map_err(|e| ReplayDiff {
                index: i,
                kind: "field".to_string(),
                detail: format!("rebuilt message failed to serialize: {}", e),
            })?;
        let rec = &recorded[i];
        if rv == *rec {
            continue;
        }
        // First difference: classify.
        let role_r = rv.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let role_c = rec.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role_r != role_c {
            return Err(ReplayDiff {
                index: i,
                kind: "role".to_string(),
                detail: format!("rebuilt role `{}` vs recorded `{}`", role_r, role_c),
            });
        }
        let content_r = rv.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let content_c = rec.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if content_r != content_c {
            let offset = first_diff_offset(content_r, content_c);
            return Err(ReplayDiff {
                index: i,
                kind: "content".to_string(),
                detail: format!(
                    "content differs at byte ~{}; rebuilt `{}` vs recorded `{}`",
                    offset,
                    preview(content_r, offset),
                    preview(content_c, offset)
                ),
            });
        }
        if rv.get("tool_calls") != rec.get("tool_calls") {
            return Err(ReplayDiff {
                index: i,
                kind: "tool_calls".to_string(),
                detail: format!(
                    "tool_calls differ: rebuilt {:?} vs recorded {:?}",
                    rv.get("tool_calls"), rec.get("tool_calls")
                ),
            });
        }
        return Err(ReplayDiff {
            index: i,
            kind: "field".to_string(),
            detail: format!(
                "message fields differ beyond role/content/tool_calls: rebuilt `{}` vs recorded `{}`",
                rv, rec
            ),
        });
    }
    Ok(())
}

/// Byte offset of the first difference between two strings (char-boundary
/// floored so the reported offset always lands on a char start).
fn first_diff_offset(a: &str, b: &str) -> usize {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let mut i = 0usize;
    while i < ab.len() && i < bb.len() && ab[i] == bb[i] {
        i += 1;
    }
    floor_char_boundary_str(a, i)
}

/// `str::floor_char_boundary` is still unstable — local copy (same approach
/// as nemesis-utils; kept here so the module is self-contained).
fn floor_char_boundary_str(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Short context preview around a diff offset (for diagnosis strings).
fn preview(s: &str, offset: usize) -> String {
    let ctx_start = offset.saturating_sub(24);
    let ctx_start = floor_char_boundary_str(s, ctx_start);
    let ctx_end = (offset + 24).min(s.len());
    let ctx_end = floor_char_boundary_str(s, ctx_end).max(ctx_start);
    format!("…{}…", &s[ctx_start..ctx_end])
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

// ---------------------------------------------------------------------------
// Orchestrator (rebuild + verify, with explicit degradation)
// ---------------------------------------------------------------------------

/// Verdict of a session-round replay verification.
#[derive(Debug)]
pub enum ReplayCheck {
    /// Rebuild succeeded and matched the recording byte-for-byte.
    ByteExact,
    /// No ledger for the round: degraded EXPLICITLY to the role-subsequence
    /// anchor. `verdict` carries that anchor's result (Ok or the mismatch).
    DegradedSubsequence {
        note: String,
        verdict: Result<(), String>,
    },
    /// Ledger exists but the persisted history needed was trimmed away.
    Unavailable { needed: usize, available: usize },
}

/// Verify one recorded round end-to-end: rebuild from the session store +
/// ledger, then byte-compare against the recorded request messages. Audit
/// semantics — never called on the production request path.
pub fn verify_session_round(
    store: &SessionStore,
    session_key: &str,
    round: usize,
    recorded: &[serde_json::Value],
) -> Result<ReplayCheck, ReplayDiff> {
    verify_session_round_in(
        store,
        &replay_ledger_path(session_key),
        session_key,
        round,
        None,
        recorded,
    )
}

/// Core verify for an EXPLICIT ledger path (dashboard logs handler).
/// `session_key` is the RAW store key used only by the NoLedger degraded
/// anchor (callers resolve it from the ledger record when one exists).
/// `trace_id` disambiguates equal `round` numbers across turns — see
/// [`rebuild_request_messages_in`].
pub fn verify_session_round_in(
    store: &SessionStore,
    ledger_path: &std::path::Path,
    session_key: &str,
    round: usize,
    trace_id: Option<&str>,
    recorded: &[serde_json::Value],
) -> Result<ReplayCheck, ReplayDiff> {
    match rebuild_request_messages_in(store, ledger_path, round, trace_id) {
        Ok(RebuildOutcome::Rebuilt(rebuilt)) => {
            verify_request_replay(&rebuilt, recorded).map(|_| ReplayCheck::ByteExact)
        }
        Ok(RebuildOutcome::NoLedger { note }) => {
            // Explicit degradation: the lightweight anchor still runs so the
            // caller gets SOME verdict, clearly labeled as not byte-exact.
            let stored = store.get_history(session_key);
            let rebuilt_roles: Vec<&str> = stored.iter().map(|m| m.role.as_str()).collect();
            let recorded_roles: Vec<&str> = recorded
                .iter()
                .filter_map(|v| v.get("role").and_then(|r| r.as_str()))
                .collect();
            Ok(ReplayCheck::DegradedSubsequence {
                note,
                verdict: crate::request_logger::check_request_log_consistency(
                    &recorded_roles,
                    &rebuilt_roles,
                ),
            })
        }
        Ok(RebuildOutcome::Unavailable { needed, available }) => Ok(ReplayCheck::Unavailable {
            needed,
            available,
        }),
        Err(e) => Err(ReplayDiff {
            index: 0,
            kind: "rebuild".to_string(),
            detail: e,
        }),
    }
}

/// Read one `NN.AI.Request.raw.json` envelope: returns `(round, body.messages)`.
/// `None` when the file is missing, unparsable, or has no messages array.
pub fn read_raw_request_messages(envelope_path: &std::path::Path) -> Option<(usize, Vec<serde_json::Value>)> {
    let content = fs::read_to_string(envelope_path).ok()?;
    let envelope: serde_json::Value = serde_json::from_str(&content).ok()?;
    let round = envelope.get("round").and_then(|v| v.as_u64())? as usize;
    let messages = envelope
        .get("body")
        .and_then(|b| b.get("messages"))
        .and_then(|m| m.as_array())?
        .clone();
    Some((round, messages))
}

/// Convenience for tests/diagnostics: the timestamp the ledger would stamp.
pub fn now_rfc3339() -> String {
    Local::now().to_rfc3339()
}

#[cfg(test)]
mod tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
