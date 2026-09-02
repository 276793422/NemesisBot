//! Z1 (Phase4-d) session-fork tests — ROUND 3 (2026-08-25 fork 第三轮):
//! jsonl is the single source of truth for turn semantics. The fork cut is
//! taken on jsonl rows, the new jsonl is a VERBATIM copy, and the new store
//! is derived from the same rows (shared mapping with the self-heal
//! rebuild). The headline regression
//! (`test_fork_round3_regression_divergent_polluted_store`) reproduces the
//! production incident: a store truncated by compaction, polluted with tool
//! intermediates and holding a failed-LLM summary, next to a clean jsonl —
//! the fork must still deliver the clean jsonl prefix the user picked by.
//!
//! The SessionStore side is fully isolated (`new_with_storage(tempdir)`).
//! The chat_log / boundary-event side goes through the process-global path
//! manager (shared home in this lib binary — see
//! `tests/history_search_fts.rs` for why per-test homes are impossible
//! here), so these tests use nanos-unique keys AND `delete_chat_log` on
//! every key at the end (which also removes the boundary sidecar + meta).

use super::*;
use crate::chat_log::{
    append_chat_log, append_chat_log_with_model, delete_chat_log, read_boundary_events,
    read_chat_log,
};
use crate::session::StoredMessage;
use serde_json::Value;

fn msg(role: &str, content: &str) -> StoredMessage {
    StoredMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: "2026-08-05T00:00:00+08:00".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

/// Build a jsonl-shaped row Value (same shape `read_chat_log` returns).
fn row(role: &str, content: &str) -> Value {
    serde_json::json!({ "role": role, "content": content, "timestamp": "2026-08-24T00:00:00+08:00" })
}

/// Seed a clean 3-turn jsonl (the TRUTH side): (u,a) × 3 = 6 rows.
fn seed_clean_log(key: &str) {
    append_chat_log(key, "user", "turn one question");
    append_chat_log(key, "assistant", "turn one answer");
    append_chat_log(key, "user", "turn two question");
    append_chat_log(key, "assistant", "turn two answer");
    append_chat_log(key, "user", "turn three question");
    append_chat_log(key, "assistant", "turn three answer");
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
fn test_row_turn_count_and_cut_semantics() {
    let rows = vec![
        row("user", "q1"),
        row("assistant", "a1"),
        row("user", "q2"),
        row("tool", "tool output"), // only user rows start/count turns
        row("assistant", "a2"),
        row("user", "q3"),
        row("assistant", "a3"),
    ];
    assert_eq!(row_user_turn_count(&rows), 3);
    // --at 1 keeps turn 1 complete: cut right before the 2nd user row.
    assert_eq!(row_turn_cut(&rows, 1), 2);
    // --at 2 keeps turns 1-2 (incl. the tool row inside turn 2).
    assert_eq!(row_turn_cut(&rows, 2), 5);
    // --at >= 3 keeps everything.
    assert_eq!(row_turn_cut(&rows, 3), rows.len());
    assert_eq!(row_turn_cut(&rows, 99), rows.len());
    // Empty / no-user-row logs have no turns.
    assert_eq!(row_user_turn_count(&[]), 0);
    assert_eq!(row_user_turn_count(&[row("assistant", "a")]), 0);
}

#[test]
fn test_fork_default_full_history_and_source_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);

    let info = fork_session(&store, &src, None, None).unwrap();
    assert_eq!(info.at_turn, 3);
    assert_eq!(info.new_key, format!("{}__fork", src));
    assert_eq!(info.kept_messages, 6);
    assert_eq!(info.dropped_messages, 0);
    assert_eq!(info.chat_log_lines, 6);

    // New jsonl == source jsonl rows, verbatim (content AND timestamp).
    let (src_rows, _, _, _) = read_chat_log(&src, 50, None);
    let (fork_rows, _, _, _) = read_chat_log(&info.new_key, 50, None);
    assert_eq!(fork_rows.len(), src_rows.len());
    for (a, b) in fork_rows.iter().zip(src_rows.iter()) {
        assert_eq!(a["content"], b["content"]);
        assert_eq!(
            a["timestamp"], b["timestamp"],
            "fork must not re-stamp rows"
        );
    }

    // New store = mirror of the same rows (no system/tool rows, real
    // timestamps — the shared self-heal mapping).
    let hist = store.get_history(&info.new_key);
    assert_eq!(hist.len(), 6);
    assert!(
        hist.iter()
            .all(|m| m.role == "user" || m.role == "assistant")
    );
    assert_eq!(hist.last().unwrap().content, "turn three answer");

    // SOURCE untouched (true branch, not rollback): both stores keep
    // everything.
    let (src_after, _, _, _) = read_chat_log(&src, 50, None);
    assert_eq!(src_after.len(), 6);
    delete_chat_log(&src);
    delete_chat_log(&info.new_key);
}

#[test]
fn test_fork_at_turn_boundary_verbatim_copy_and_store_mirror() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);
    // Source store ALSO carries a coherent summary — the fork must NOT
    // carry it (round-3 rule: jsonl carries no summary; a store summary may
    // reference folded content that isn't in the fork).
    store.get_or_create(&src);
    store.set_history(&src, (0..3).map(|_| msg("user", "store junk")).collect());
    store.set_summary(&src, "summary covering everything");
    store.set_summary_covers_up_to(&src, Some(4));

    let info = fork_session(&store, &src, None, Some(2)).unwrap();
    assert_eq!(info.kept_messages, 4); // u1 a1 u2 a2
    assert_eq!(info.dropped_messages, 2); // u3 a3
    assert!(!info.summary_kept);
    assert_eq!(store.get_summary(&info.new_key), "");
    assert_eq!(store.get_summary_covers_up_to(&info.new_key), None);

    // jsonl prefix copied VERBATIM — including a model badge on one row
    // (append one to prove extra fields survive the copy).
    delete_chat_log(&info.new_key); // redo with a badge in the source
    let src2 = unique_src();
    append_chat_log(&src2, "user", "badge question");
    append_chat_log_with_model(&src2, "assistant", "badge answer", Some("zhipu/glm-4.7"));
    append_chat_log(&src2, "user", "second question");
    append_chat_log(&src2, "assistant", "second answer");
    let info2 = fork_session(&store, &src2, None, Some(1)).unwrap();
    let (fork_rows, _, _, _) = read_chat_log(&info2.new_key, 50, None);
    assert_eq!(fork_rows.len(), 2);
    assert_eq!(fork_rows[0]["content"], "badge question");
    assert_eq!(fork_rows[1]["content"], "badge answer");
    assert_eq!(
        fork_rows[1]["model"], "zhipu/glm-4.7",
        "verbatim copy must preserve extra fields (model badge)"
    );
    // Store side mirrors the same rows; ends on the picked turn's reply.
    let hist = store.get_history(&info2.new_key);
    assert_eq!(hist.len(), 2);
    assert_eq!(hist.last().unwrap().content, "badge answer");
    // Source keeps its summary and junk history untouched.
    assert_eq!(store.get_summary(&src), "summary covering everything");
    for k in [&src, &info.new_key, &src2, &info2.new_key] {
        delete_chat_log(k);
    }
}

/// ROUND-3 REGRESSION (the production incident this fix is for): the
/// source's SessionStore is compaction-truncated (starts mid-August, the
/// July turns are gone), polluted with a non-empty tool intermediate, a
/// tool row and an empty assistant row, ends on a DIFFERENT 9th turn, and
/// holds a summary that is actually a failed summarizer reply. The jsonl
/// is the clean 10-turn truth the user picked "第 9 轮" by. The fork must
/// deliver rows 0..18 of the JSONL — nothing from the store's coordinate
/// system may leak.
#[test]
fn test_fork_round3_regression_divergent_polluted_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();

    // jsonl TRUTH: 10 clean turns; turn 9 is the "1i+1i" turn.
    append_chat_log(&src, "user", "在么");
    append_chat_log(&src, "assistant", "在的");
    for i in 2..=10 {
        append_chat_log(&src, "user", &format!("truth question {i}"));
        append_chat_log(&src, "assistant", &format!("truth answer {i}"));
    }
    // Spot-check the truth layout: rows 16/17 are turn 9's pair.
    let (truth, _, _, _) = read_chat_log(&src, 50, None);
    assert_eq!(truth[16]["content"], "truth question 9");
    assert_eq!(truth[17]["content"], "truth answer 9");

    // STORE GARBAGE (production shape): truncated to August, tool
    // intermediates, duplicated reply, failed-LLM summary.
    let mut junk: Vec<StoredMessage> = vec![msg("system", "You are Nemesis.")];
    // 9 user turns of OTHER (August) content, "已记下" being the 9th.
    for i in 1..=8 {
        junk.push(msg("user", &format!("august question {i}")));
        junk.push(msg("assistant", &format!("august answer {i}")));
    }
    junk.push(msg("user", "august question 9"));
    // non-empty tool intermediate (碎碎念), tool row, empty assistant,
    // then the final reply duplicated twice.
    junk.push(msg("assistant", "让我看看工具结果……"));
    junk.push(msg("tool", "TOOL_JUNK output ...8KB..."));
    junk.push(msg("assistant", ""));
    junk.push(msg("assistant", "已记下"));
    junk.push(msg("assistant", "已记下"));
    store.get_or_create(&src);
    store.set_history(&src, junk);
    store.set_summary(
        &src,
        "It looks like the two conversation summaries weren't included. Could you paste them here?",
    );
    store.set_summary_covers_up_to(&src, Some(268));

    let info = fork_session(&store, &src, None, Some(9)).unwrap();
    assert_eq!(info.at_turn, 9);
    assert_eq!(info.kept_messages, 18); // 9 turns × 2 truth rows
    assert_eq!(info.dropped_messages, 2);
    assert_eq!(info.chat_log_lines, 18);

    let (fork_rows, total, _, _) = read_chat_log(&info.new_key, 50, None);
    assert_eq!(total, 18);
    // The fork starts at the jsonl's turn 1 and ends on the jsonl's turn 9
    // reply — the exact rows the user picked by in the dialog.
    assert_eq!(fork_rows.first().unwrap()["content"], "在么");
    assert_eq!(fork_rows.last().unwrap()["content"], "truth answer 9");
    // NOTHING from the store's coordinate system leaks in: no August
    // content, no tool junk, no tool-intermediate 碎碎念, no duplicates.
    for v in &fork_rows {
        let c = v["content"].as_str().unwrap_or("");
        assert!(!c.contains("august"), "store content leaked: {c}");
        assert!(!c.contains("TOOL_JUNK"), "tool row leaked: {c}");
        assert!(!c.contains("已记下"), "store 9th turn leaked: {c}");
        assert!(!c.contains("工具结果"), "tool intermediate leaked: {c}");
    }
    // Verbatim: every fork row equals its truth source row (ts included).
    for (a, b) in fork_rows.iter().zip(truth[..18].iter()) {
        assert_eq!(a["content"], b["content"]);
        assert_eq!(a["timestamp"], b["timestamp"]);
    }

    // Store side: mirrored from the same 18 rows — no tool rows, no
    // system, no summary, timestamps preserved.
    let hist = store.get_history(&info.new_key);
    assert_eq!(hist.len(), 18);
    assert_eq!(hist[0].content, "在么");
    assert_eq!(hist.last().unwrap().content, "truth answer 9");
    assert!(
        hist.iter()
            .all(|m| m.role == "user" || m.role == "assistant")
    );
    assert_eq!(hist[0].timestamp, truth[0]["timestamp"].as_str().unwrap());
    assert_eq!(store.get_summary(&info.new_key), "");

    // Source untouched: jsonl still 20 rows, store still junk + summary.
    let (src_rows, src_total, _, _) = read_chat_log(&src, 50, None);
    assert_eq!(src_total, 20);
    assert_eq!(src_rows.len(), 20);
    assert_eq!(store.get_history(&src).len(), 23);
    assert!(store.get_summary(&src).contains("paste them here"));

    // Boundary events on both keys (U9 sidecar).
    let out_ev = read_boundary_events(&src);
    assert!(
        out_ev
            .iter()
            .any(|v| v.get("event").and_then(|e| e.as_str()) == Some("session_fork_out"))
    );
    let in_ev = read_boundary_events(&info.new_key);
    assert!(
        in_ev
            .iter()
            .any(|v| v.get("event").and_then(|e| e.as_str()) == Some("session_fork_in"))
    );

    delete_chat_log(&src);
    delete_chat_log(&info.new_key);
}

#[test]
fn test_fork_new_key_uniqueness() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);

    let i1 = fork_session(&store, &src, None, None).unwrap();
    let i2 = fork_session(&store, &src, None, None).unwrap();
    let i3 = fork_session(&store, &src, Some(i1.new_key.clone()), None).unwrap();
    assert_eq!(i1.new_key, format!("{}__fork", src));
    assert_eq!(i2.new_key, format!("{}__fork_2", src));
    // An explicit --new-key that collides gets suffixed past ALL taken
    // names (both __fork and __fork_2 exist by now), never overwritten.
    assert_eq!(i3.new_key, format!("{}__fork_3", src));
    assert_ne!(i1.new_key, i3.new_key);
    // All three forks carry the full log.
    for k in [&i1.new_key, &i2.new_key, &i3.new_key] {
        let (rows, total, _, _) = read_chat_log(k, 50, None);
        assert_eq!(total, 6);
        assert_eq!(rows.len(), 6);
    }

    // ROUND-3 guard: a previous fork's jsonl outlives its store json (7-day
    // TTL deletes only the store side). The uniquifier must consult the
    // jsonl too, or the next fork would APPEND onto the surviving log and
    // duplicate the whole prefix.
    std::fs::remove_file(
        dir.path()
            .join(format!("{}.json", i1.new_key.replace(':', "_"))),
    )
    .ok(); // simulate the TTL having deleted the store json
    let mem_only = SessionStore::new_in_memory(); // store-side checks all miss
    let i4 = fork_session(&mem_only, &src, None, None).unwrap();
    assert_eq!(
        i4.new_key,
        format!("{}__fork_4", src),
        "jsonl existence must pin the key"
    );
    let (rows4, total4, _, _) = read_chat_log(&i4.new_key, 50, None);
    assert_eq!(
        total4, 6,
        "fresh log, not an append onto i1's surviving jsonl"
    );
    assert_eq!(rows4.first().unwrap()["content"], "turn one question");

    for k in [&src, &i1.new_key, &i2.new_key, &i3.new_key, &i4.new_key] {
        delete_chat_log(k);
    }
}

#[test]
fn test_fork_errors_on_empty_or_turnless_source() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    // Absent source (no jsonl anywhere).
    let missing = unique_src();
    assert!(fork_session(&store, &missing, None, None).is_err());

    // jsonl exists but has NO user rows (forkable truth is empty) — even
    // though the STORE has a full history (round 3: store can't rescue it).
    let src = unique_src();
    append_chat_log(&src, "assistant", "only an assistant row");
    store.get_or_create(&src);
    store.set_history(&src, three_turn_store_history());
    let err = fork_session(&store, &src, None, None).unwrap_err();
    assert!(err.contains("user 轮次"), "unexpected error: {err}");
    delete_chat_log(&src);

    // Store-only session (jsonl missing entirely — e.g. pre-jsonl relic):
    // clear, user-facing error; must NOT silently fork from the store.
    let src2 = unique_src();
    store.get_or_create(&src2);
    store.set_history(&src2, three_turn_store_history());
    let err2 = fork_session(&store, &src2, None, None).unwrap_err();
    assert!(err2.contains("jsonl"), "unexpected error: {err2}");
}

/// system + (u,a) × 3, with a tool exchange inside turn 2 (STORE shape).
fn three_turn_store_history() -> Vec<StoredMessage> {
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
    store_a.set_history(&key, three_turn_store_history());
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

/// The store-based (SUPERSEDED, kept-per-discipline) helpers still behave:
/// `user_turn_count` counts user rows; `turn_cut` lands right before the
/// (N+1)-th user message and past-the-end N keeps the whole history.
#[test]
fn superseded_store_helpers_count_and_cut_correctly() {
    let msgs = vec![
        msg("system", "sys"),
        msg("user", "q1"),
        msg("assistant", "a1"),
        msg("user", "q2"),
        msg("assistant", "a2"),
    ];
    assert_eq!(user_turn_count(&msgs), 2);
    assert_eq!(turn_cut(&msgs, 1), 3, "cut lands before the 2nd user msg");
    assert_eq!(turn_cut(&msgs, 2), 5, "N == turn count keeps everything");
    assert_eq!(turn_cut(&msgs, 99), 5, "N past the end keeps everything");
    // Empty set edge cases.
    assert_eq!(user_turn_count(&[]), 0);
    assert_eq!(turn_cut(&[], 0), 0);
}
