//! Z1 (Phase4-d) session-fork tests.
//!
//! The SessionStore side is fully isolated (`new_with_storage(tempdir)`).
//! The chat_log / boundary-event side goes through the process-global path
//! manager (shared home in this lib binary — see
//! `tests/history_search_fts.rs` for why per-test homes are impossible
//! here), so these tests use nanos-unique keys AND `delete_chat_log` both
//! keys at the end (which also removes the boundary sidecar).

use super::*;
use crate::chat_log::{append_chat_log, delete_chat_log, read_boundary_events, read_chat_log};
use crate::session::StoredMessage;

fn msg(role: &str, content: &str) -> StoredMessage {
    StoredMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: "2026-08-24T00:00:00+08:00".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

/// system + (u,a) × 3, with a tool exchange inside turn 2.
fn three_turn_history() -> Vec<StoredMessage> {
    vec![
        msg("system", "You are a test."),
        msg("user", "turn one question"),
        msg("assistant", "turn one answer"),
        msg("user", "turn two question"),
        msg("assistant", "calling tool"),
        msg("tool", "tool output"),
        msg("assistant", "turn two answer"),
        msg("user", "turn three question"),
        msg("assistant", "turn three answer"),
    ]
}

fn unique_src() -> String {
    format!(
        "z1fork:src:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[test]
fn test_turn_count_and_cut_semantics() {
    let h = three_turn_history();
    assert_eq!(user_turn_count(&h), 3);
    // --at 1 keeps turn 1 complete: cut right before the 2nd user message.
    assert_eq!(turn_cut(&h, 1), 3);
    // --at 2 keeps turns 1-2 (incl. the tool exchange inside turn 2).
    assert_eq!(turn_cut(&h, 2), 7);
    // --at >= 3 keeps everything.
    assert_eq!(turn_cut(&h, 3), h.len());
    assert_eq!(turn_cut(&h, 99), h.len());
    // Empty / system-only histories have no turns.
    assert_eq!(user_turn_count(&[]), 0);
    assert_eq!(user_turn_count(&[msg("system", "s")]), 0);
}

#[test]
fn test_fork_default_full_history_and_source_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    store.get_or_create(&src);
    store.set_history(&src, three_turn_history());

    let info = fork_session(&store, &src, None, None).unwrap();
    assert_eq!(info.at_turn, 3);
    assert_eq!(info.new_key, format!("{}__fork", src));
    assert_eq!(info.kept_messages, 9);
    assert_eq!(info.dropped_messages, 0);
    // New session history == source history (byte-comparable via clone).
    assert_eq!(store.get_history(&info.new_key), store.get_history(&src));
    // SOURCE untouched (true branch, not rollback).
    assert_eq!(store.get_history(&src).len(), 9);
    delete_chat_log(&src);
    delete_chat_log(&info.new_key);
}

#[test]
fn test_fork_at_turn_boundary_cuts_and_drops_summary_incoherent() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    store.get_or_create(&src);
    let hist = three_turn_history();
    store.set_history(&src, hist.clone());
    // Summary claims to cover the first 8 messages — beyond the --at 1 cut
    // (3), so carrying it would reference content the fork drops.
    store.set_summary(&src, "summary of turns 1-3");
    store.set_summary_covers_up_to(&src, Some(8));

    let info = fork_session(&store, &src, None, Some(1)).unwrap();
    assert_eq!(info.kept_messages, 3);
    assert_eq!(info.dropped_messages, 6);
    assert!(!info.summary_kept);
    assert_eq!(store.get_summary(&info.new_key), "");
    assert_eq!(store.get_summary_covers_up_to(&info.new_key), None);
    // Kept prefix is exactly history[..3].
    let kept = store.get_history(&info.new_key);
    assert_eq!(kept.len(), 3);
    assert_eq!(kept[1].content, "turn one question");
    assert_eq!(kept[2].content, "turn one answer");
    // Source keeps its summary and full history.
    assert_eq!(store.get_summary(&src), "summary of turns 1-3");
    assert_eq!(store.get_history(&src).len(), 9);
    delete_chat_log(&src);
    delete_chat_log(&info.new_key);
}

#[test]
fn test_fork_summary_kept_when_covers_within_cut() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    store.get_or_create(&src);
    store.set_history(&src, three_turn_history());
    store.set_summary(&src, "summary of turn 1");
    store.set_summary_covers_up_to(&src, Some(3));

    let info = fork_session(&store, &src, None, Some(1)).unwrap();
    assert_eq!(info.kept_messages, 3);
    assert!(info.summary_kept, "covers(3) <= cut(3)");
    assert_eq!(store.get_summary(&info.new_key), "summary of turn 1");
    assert_eq!(store.get_summary_covers_up_to(&info.new_key), Some(3));

    // Legacy: covers None + non-empty summary + FULL cut → kept verbatim.
    let src2 = unique_src();
    store.get_or_create(&src2);
    store.set_history(&src2, three_turn_history());
    store.set_summary(&src2, "legacy summary");
    store.set_summary_covers_up_to(&src2, None);
    let info2 = fork_session(&store, &src2, None, None).unwrap();
    assert!(info2.summary_kept, "legacy full-history fork keeps summary");
    // Legacy + PARTIAL cut → dropped (coverage unknowable).
    let info3 = fork_session(&store, &src2, None, Some(2)).unwrap();
    assert!(!info3.summary_kept, "legacy partial cut drops summary");
    for k in [&src, &info.new_key, &src2, &info2.new_key, &info3.new_key] {
        delete_chat_log(k);
    }
}

#[test]
fn test_fork_new_key_uniqueness() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    store.get_or_create(&src);
    store.set_history(&src, three_turn_history());

    let i1 = fork_session(&store, &src, None, None).unwrap();
    let i2 = fork_session(&store, &src, None, None).unwrap();
    let i3 = fork_session(&store, &src, Some(i1.new_key.clone()), None).unwrap();
    assert_eq!(i1.new_key, format!("{}__fork", src));
    assert_eq!(i2.new_key, format!("{}__fork_2", src));
    // An explicit --new-key that collides gets suffixed past ALL taken
    // names (both __fork and __fork_2 exist by now), never overwritten.
    assert_eq!(i3.new_key, format!("{}__fork_3", src));
    assert_ne!(i1.new_key, i3.new_key);
    // All three forks carry the full history.
    for k in [&i1.new_key, &i2.new_key, &i3.new_key] {
        assert_eq!(store.get_history(k).len(), 9);
    }
    for k in [&src, &i1.new_key, &i2.new_key, &i3.new_key] {
        delete_chat_log(k);
    }
}

#[test]
fn test_fork_errors_on_empty_or_turnless_source() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    // Absent source.
    let missing = unique_src();
    assert!(fork_session(&store, &missing, None, None).is_err());
    // Present but no user turns (e.g. only system + assistant).
    let src = unique_src();
    store.get_or_create(&src);
    store.set_history(&src, vec![msg("system", "s"), msg("assistant", "a")]);
    assert!(fork_session(&store, &src, None, None).is_err());
}

#[test]
fn test_fork_copies_chat_log_prefix_verbatim_plus_boundary_events() {
    let src = unique_src();
    // 3 turns in the chat log (user/assistant pairs).
    append_chat_log(&src, "user", "chat turn one q");
    append_chat_log(&src, "assistant", "chat turn one a");
    append_chat_log(&src, "user", "chat turn two q");
    append_chat_log(&src, "assistant", "chat turn two a");
    append_chat_log(&src, "user", "chat turn three q");
    append_chat_log(&src, "assistant", "chat turn three a");

    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    store.get_or_create(&src);
    store.set_history(&src, three_turn_history());

    let info = fork_session(&store, &src, None, Some(2)).unwrap();
    // Chat log prefix: turns 1-2 = 4 lines, timestamps VERBATIM (equal to
    // the source's first four entries).
    let (copied, t, _m, _o) = read_chat_log(&info.new_key, 50, None);
    assert_eq!(t, 4);
    let (orig, _ot, _om, _oo) = read_chat_log(&src, 50, None);
    for (c, o) in copied.iter().zip(orig.iter()) {
        assert_eq!(c, o, "copied chat-log lines must be verbatim");
    }
    // Boundary events recorded on both keys (U9 sidecar).
    let out_ev = read_boundary_events(&src);
    assert!(out_ev
        .iter()
        .any(|v| v.get("event").and_then(|e| e.as_str()) == Some("session_fork_out")));
    let in_ev = read_boundary_events(&info.new_key);
    assert!(in_ev
        .iter()
        .any(|v| v.get("event").and_then(|e| e.as_str()) == Some("session_fork_in")));

    delete_chat_log(&src);
    delete_chat_log(&info.new_key);
}

#[test]
fn test_session_store_disk_fallback_on_miss() {
    // Store A materializes + saves a session file AFTER store B was
    // constructed (the live-fork scenario: CLI writes, gateway loads).
    let dir = tempfile::tempdir().unwrap();
    let store_b = SessionStore::new_with_storage(dir.path()); // constructed first
    let store_a = SessionStore::new_with_storage(dir.path());
    let key = format!(
        "z1fork:disk:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    store_a.get_or_create(&key);
    store_a.set_history(&key, three_turn_history());
    store_a.save(&key).unwrap();

    // B never saw the file at construction; the miss must now fall back to
    // disk instead of materializing an empty session (which a later save
    // would use to clobber the fork).
    assert!(!store_b.contains(&key));
    let loaded = store_b.get_or_create(&key);
    assert_eq!(loaded.messages.len(), 9);
    assert_eq!(store_b.get_history(&key).len(), 9);

    // Corrupt file for an unseen key → same empty-session behavior as
    // before (no panic, no error propagation). Key has no `:`/`/`/`\` so
    // the on-disk filename is the key verbatim (no sanitizer dependency).
    let bad_key = format!(
        "z1forkdiskbad_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let bad = dir.path().join(format!("{}.json", bad_key));
    std::fs::write(&bad, "{not json").unwrap();
    let empty = store_b.get_or_create(&bad_key);
    assert!(empty.messages.is_empty());
}
