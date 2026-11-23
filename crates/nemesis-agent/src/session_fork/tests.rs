//! session-fork tests — (2026-08-25 fork):
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
        image_refs: Vec::new(),
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

/// ⚠ 并行唯一性不能只靠时钟：Windows SystemTime 粒度粗（~15.6ms tick），
/// 并行测试线程同 tick 会取到**完全相同**的 nanos → 同 key 交错
/// append/delete → jsonl 行数丢失/重复（曾致本家族随机红）。追加进程内
/// 原子计数器（真唯一）+ pid（防跨进程二进制并行复用）。
fn unique_src() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "z1fork:src:{}:{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        seq
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

/// REGRESSION (the production incident this fix is for): the
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

    // Boundary events on both keys (sidecar).
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

    // guard: a previous fork's jsonl outlives its store json (7-day
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

/// BUG-20261106-038: a META-ONLY leftover at the auto candidate key
/// (`{src}__fork`) must also pin the uniquifier — producer: trash half-
/// failure / hard-delete single-file retry exhaustion leaves the meta
/// sidecar behind while jsonl + store json are gone. Upsert-style meta
/// writes (write_session_meta / write_session_parent / …) preserve stale
/// fields, so silent reuse would inherit the dead session's
/// allow_all/branch_summary/title into the fresh fork.
#[test]
fn test_fork_unique_key_skips_meta_only_stale_candidate() {
    let src = unique_src();
    seed_clean_log(&src);

    let stale = format!("{}__fork", src);
    crate::chat_log::write_session_meta(&stale, "已删分叉的陈旧标题");
    assert!(!crate::chat_log::chat_log_exists(&stale));
    assert!(crate::chat_log::session_occupied(&stale));

    // In-memory store: store-side checks all miss; jsonl absent at the
    // candidate; only the meta sidecar occupies it.
    let store = SessionStore::new_in_memory();
    let info = fork_session(&store, &src, None, None).unwrap();
    assert_eq!(
        info.new_key,
        format!("{}__fork_2", src),
        "meta-only leftover must pin the key"
    );

    delete_chat_log(&src);
    delete_chat_log(&info.new_key);
    delete_chat_log(&stale);
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

/// at_turn=0 诚实拒绝（CLI --at 0 / WSAPI at_turn 0）。
/// 此前 `.max(1)` 静默 clamp 成 fork 1 轮——与 interactive_pick 的
/// `Ok(n) if n >= 1` 拒 0 臂行为分裂，静默纠偏违反 loud 纪律。
#[test]
fn test_fork_rejects_zero_turn_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);
    let err = fork_session(&store, &src, None, Some(0)).unwrap_err();
    assert!(err.contains("无效轮次"), "unexpected error: {err}");
    // 拒绝路径零副作用：源会话 jsonl 未被触碰（fork 源只读契约）。
    let (rows, _, _, _) = read_chat_log(&src, usize::MAX, None);
    assert_eq!(rows.len(), 6, "源会话必须原样保留");
    delete_chat_log(&src);
}

/// 横向同族：--new-key 空/空白值诚实拒绝（此前会经
/// unique_key 产出空名会话文件垃圾残留）。
#[test]
fn test_fork_rejects_empty_new_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);
    for bad in ["", "   "] {
        let err = fork_session(&store, &src, Some(bad.to_string()), None).unwrap_err();
        assert!(err.contains("new_key"), "unexpected error: {err}");
    }
    delete_chat_log(&src);
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

// ---: fork 成功路径写血缘 sidecar meta ---

/// fork_session 在新 key 上落 `parent` + `forked_at_turn`（血缘）；随后
/// fork endpoint 写 title（api_handlers 的 rename 臂语义）时 upsert 保留
/// 血缘 —— 的验收契约「fork 后 list 返回 parent 字段」的数据侧前提。
#[test]
fn test_fork_writes_parent_lineage_meta() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);

    let info = fork_session(&store, &src, None, Some(2)).unwrap();
    assert_eq!(info.at_turn, 2);

    let full =
        crate::chat_log::read_session_meta_full(&info.new_key).expect("fork 必须落血缘 meta");
    assert_eq!(full.parent.as_deref(), Some(src.as_str()));
    assert_eq!(full.forked_at_turn, Some(2));

    // endpoint 随后写 title —— upsert 不抹血缘。
    crate::chat_log::write_session_meta(&info.new_key, "forked title");
    let after = crate::chat_log::read_session_meta_full(&info.new_key).unwrap();
    assert_eq!(after.title.as_deref(), Some("forked title"));
    assert_eq!(after.parent.as_deref(), Some(src.as_str()));
    assert_eq!(after.forked_at_turn, Some(2));

    // 源会话不带血缘 meta（血缘只属于新分支）。
    assert!(crate::chat_log::read_session_meta_full(&src).is_none());

    delete_chat_log(&info.new_key);
    delete_chat_log(&src);
}

// ---：fork 继承项目归属 ---

/// 源会话带项目绑定时 fork：新 key 的 sidecar meta与 store 缓存
/// 镜像都带同一绑定，且随 store.save 落盘（六矩阵⑥ 的数据侧前提——路由
/// owner_of miss 时回读 sidecar 兜底）。源无绑定 = fork 后仍无绑定。
#[test]
fn test_fork_inherits_project_binding() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);
    crate::chat_log::write_session_project(&src, "p-deadbeef", "/tmp/proj_x");

    let info = fork_session(&store, &src, None, Some(1)).unwrap();

    // sidecar meta双字段就位，title/血缘共存。
    let meta = crate::chat_log::read_session_meta_full(&info.new_key).expect("fork 落 meta");
    assert_eq!(meta.project_id.as_deref(), Some("p-deadbeef"));
    assert_eq!(meta.project_path.as_deref(), Some("/tmp/proj_x"));
    assert_eq!(meta.parent.as_deref(), Some(src.as_str()), "血缘共存");

    // store 缓存镜像 + 落盘（重开同目录 store 验证持久化）。
    assert_eq!(
        store.get_project(&info.new_key),
        (
            Some("p-deadbeef".to_string()),
            Some("/tmp/proj_x".to_string())
        )
    );
    let reopened = SessionStore::new_with_storage(dir.path());
    assert_eq!(
        reopened.get_project(&info.new_key),
        (
            Some("p-deadbeef".to_string()),
            Some("/tmp/proj_x".to_string())
        ),
        "绑定随 save 落盘"
    );

    // 无绑定源：fork 后仍无绑定（meta 只带 血缘，project 双字段缺席）。
    let src2 = unique_src();
    seed_clean_log(&src2);
    let info2 = fork_session(&store, &src2, None, None).unwrap();
    let meta2 = crate::chat_log::read_session_meta_full(&info2.new_key).expect("fork 落血缘 meta");
    assert!(meta2.project_id.is_none(), "无绑定源 fork 不造绑定");
    assert!(meta2.project_path.is_none());
    assert_eq!(store.get_project(&info2.new_key), (None, None));

    delete_chat_log(&info.new_key);
    delete_chat_log(&src);
    delete_chat_log(&info2.new_key);
    delete_chat_log(&src2);
}

/// 回归锁：steer 注入行（`"steer": true`）是轮中间的
/// user 形态行——不计轮、不作 fork 切点；无标记行（含 `[steer] ` 内容
/// 前缀的旧行——内容前缀不是契约，存储字段才是）按旧行为照常计轮。
#[test]
fn test_row_turn_count_and_cut_skip_steer_rows() {
    let steer_row = serde_json::json!({
        "role": "user",
        "content": "[steer] 改买梨",
        "timestamp": "2026-10-25T00:00:00+08:00",
        "steer": true
    });
    let rows = vec![
        row("user", "q1"),
        row("assistant", "a1"),
        steer_row,
        row("assistant", "a1'"),
        row("user", "q2"),
        row("assistant", "a2"),
    ];
    assert_eq!(row_user_turn_count(&rows), 2, "steer 行不计轮");
    // --at 1 keeps turn 1 complete：切点跳过 steer 行，落在 q2（idx 4）上
    // ——旧行为切在 steer 行（idx 2）会分叉出半轮会话。
    assert_eq!(row_turn_cut(&rows, 1), 4);
    assert_eq!(row_turn_cut(&rows, 2), rows.len());
    // 缺 steer 字段的行 = 旧行为（轮边界，修复前写入的行按旧行为退化）。
    assert_eq!(row_user_turn_count(&[row("user", "legacy")]), 1);
}

/// 回归锁：fork 的 jsonl 落盘失败必须诚实 Err + 原子
/// 回退。旧代码把 open 失败（返回 0）静默吞掉——fork 照常建 store 落
/// meta 写 boundary 并返回成功，产出「store 有历史、jsonl 真相源缺席」
/// 的半残 fork（self-heal 一跑历史清零）。注入走 chat_log 的 key 定向
/// 测试钩子——目录占位会被 unique_key 的 taken 检查避让，注入不到 fork
/// 的写入位点。
#[test]
fn test_fork_jsonl_write_failure_fails_loudly_and_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);
    let target_key = format!("{}__fork", src);
    crate::chat_log::arm_test_fail_open(&target_key);

    let err = fork_session(&store, &src, Some(target_key.clone()), None)
        .expect_err("jsonl 落盘失败必须 Err，不得静默成功");
    assert!(err.contains("0/6"), "错误信息带行数现场: {err}");

    // 原子回退：jsonl 真相源不存在、store 无半残会话、boundary 无痕。
    assert!(
        !crate::chat_log::chat_log_exists(&target_key),
        "半残 jsonl 必须被回收"
    );
    assert!(!store.contains(&target_key), "store 不得有半残会话");
    assert!(
        read_boundary_events(&target_key).is_empty(),
        "boundary 不得有 fork 痕"
    );

    // 源会话原封不动（6 行全在——fork 全程只读源）。
    let (rows, total, _, _) = read_chat_log(&src, 100_000, None);
    assert_eq!(total, 6);
    assert_eq!(rows.len(), 6);

    delete_chat_log(&src);
    delete_chat_log(&target_key);
}

/// 回归锁（BUG-20261118-092）：fork 的 step-2 store.save 落盘失败必须
/// 原子回退。旧代码本臂裸 Err 不回退——此刻 jsonl 已提交（step-1 排他
/// 创建成功）、in-memory store 条目已建、项目绑定 meta 已落盘（继承块
/// 在 save 之前），boundary 与血缘 meta 尚未写——产出「被告知失败但
/// 实际存在」的幽灵 fork 会话（sessions.list 可见、无血缘无 boundary
/// 审计），重试被 unique_key 避让成 `_2` 第二份。注入走 SessionStore 的
/// key 定向测试钩子（`arm_test_fail_save`），与 205 的 arm_test_fail_open
/// 同款设计。本测还特意让源会话绑项目——回退必须连继承写出的新 key
/// 项目 meta 一起清。
#[test]
fn test_fork_store_save_failure_fails_loudly_and_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = unique_src();
    seed_clean_log(&src);
    let target_key = format!("{}__fork", src);
    // 项目绑定注入：源绑项目 → fork 继承块会写新 key 的项目 meta sidecar，
    // 且该写发生在 save 之前——save 失败臂的回退必须连它一起清。
    crate::chat_log::write_session_project(&src, "p1", "C:/proj");
    // 连武装两次（消费序实证，backtrace 探针钉死）：第一个消费点是
    // `get_or_create(&new_key)` 的自愈重建内部持久化——step-1 已把 jsonl
    // 落盘，get_or_create 走 rebuild_from_chat_log 后立即 self.save（失败
    // warn 续走 = BUG-20261028-95 设计行为，不触发 fork Err）；第二个才是
    // fork step-2 的 save。连武装两次恰好忠实模拟真实磁盘满的失败级联：
    // 内部持久化失败（warn）→ step-2 失败 → 本 BUG 的回退臂。
    crate::session::arm_test_fail_save(&target_key);
    crate::session::arm_test_fail_save(&target_key);

    let err = fork_session(&store, &src, Some(target_key.clone()), None)
        .expect_err("store.save 失败必须 Err，不得静默成功");
    assert!(err.contains("完整回退"), "错误信息带回退声明: {err}");

    // 原子回退全件套：jsonl 不存在、store 无半残会话、boundary 无痕、
    // 新 key 的项目绑定 meta 不存在（继承块产物一并回收）。
    assert!(
        !crate::chat_log::chat_log_exists(&target_key),
        "半残 jsonl 必须被回收"
    );
    assert!(!store.contains(&target_key), "store 不得有半残会话");
    assert!(
        read_boundary_events(&target_key).is_empty(),
        "boundary 不得有 fork 痕"
    );
    assert!(
        crate::chat_log::read_session_meta_full(&target_key).is_none(),
        "新 key 项目绑定 meta（继承块产物）必须被回收"
    );

    // 源会话原封不动（6 行全在 + 项目绑定仍在——fork 全程只读源）。
    let (rows, total, _, _) = read_chat_log(&src, 100_000, None);
    assert_eq!(total, 6);
    assert_eq!(rows.len(), 6);
    let src_meta =
        crate::chat_log::read_session_meta_full(&src).expect("源会话项目绑定不受回退影响");
    assert_eq!(src_meta.project_id.as_deref(), Some("p1"));

    delete_chat_log(&src);
    delete_chat_log(&target_key);
}

// ---------------------------------------------------------------------------
// BUG-2 修复（2026-11-23）：前端行号 → at_turn 换算后端化。
// ChatPanel 三点菜单「从此分叉」此前在前端「已加载窗口」内数 user 轮得出
// 窗口相对值传给 at_turn（70 轮会话被切到第 6 轮的实录根因）。修复 =
// 前端只传 jsonl 绝对行号，换算走 message_index_to_turn（本文件上方）。
// ---------------------------------------------------------------------------

/// steer 注入行（轮中间的 user 形态行——不是轮首，BUG-20261025-19）。
fn steer_row(content: &str) -> Value {
    serde_json::json!({
        "role": "user",
        "content": content,
        "steer": true,
        "timestamp": "2026-08-24T00:00:00+08:00"
    })
}

#[test]
fn test_message_index_to_turn_semantics() {
    // 3-turn log with tool + steer rows interleaved:
    // 0:u1(boundary) 1:a1 2:u2(boundary) 3:a2 4:steer 5:a2' 6:tool 7:u3(boundary) 8:a3
    let rows = vec![
        row("user", "q1"),
        row("assistant", "a1"),
        row("user", "q2"),
        row("assistant", "a2"),
        steer_row("[steer] 插话"),
        row("assistant", "a2-b"),
        row("tool", "tool output"),
        row("user", "q3"),
        row("assistant", "a3"),
    ];
    // 首轮回复 → 轮 1。
    assert_eq!(message_index_to_turn(&rows, 1), Some(1));
    // 点击轮 2 的两条回复 → 轮 2（steer/tool 夹杂不换轮）。
    assert_eq!(message_index_to_turn(&rows, 3), Some(2));
    assert_eq!(
        message_index_to_turn(&rows, 4),
        Some(2),
        "steer 行归属所在轮"
    );
    assert_eq!(message_index_to_turn(&rows, 5), Some(2));
    assert_eq!(
        message_index_to_turn(&rows, 6),
        Some(2),
        "tool 行归属所在轮"
    );
    // 点击 user 提问行本身 = 其所在轮（保留该轮完整含回复）。
    assert_eq!(message_index_to_turn(&rows, 2), Some(2));
    // 末轮回复 → 轮 3。
    assert_eq!(message_index_to_turn(&rows, 8), Some(3));
    // 越界 clamp 到末行 → 末轮。
    assert_eq!(message_index_to_turn(&rows, 999), Some(3));
    // 空日志 / 首个边界之前的行（无轮首可回溯）= None（调用方诚实报错）。
    assert_eq!(message_index_to_turn(&[], 0), None);
    assert_eq!(
        message_index_to_turn(&[row("tool", "t"), row("assistant", "a")], 0),
        None
    );
    assert_eq!(
        message_index_to_turn(&[row("tool", "t"), row("assistant", "a")], 1),
        None
    );
}

/// 换算↔切点互逆性质（BUG-2 的核心回归锁）：对**任意** assistant 行，
/// 以其换算出的 at_turn 走 row_turn_cut 切点必须**保留点击行本身**（切点
/// 严格在点击行之后）——「从此分叉」点击哪条回复，新会话就必须以那条
/// 回复收尾。生产实录（70 轮会话切到第 6 轮）正是破坏了这条性质。
#[test]
fn test_message_index_to_turn_is_inverse_of_row_turn_cut_for_replies() {
    // 6 turns，每轮 (u, a)；第 2/4 轮多一条 steer + tool 中间行。
    let mut rows = Vec::new();
    for i in 1..=6 {
        rows.push(row("user", &format!("q{i}")));
        if i == 2 || i == 4 {
            rows.push(row("tool", &format!("intermediate {i}")));
            rows.push(steer_row(&format!("[steer] 插话 {i}")));
        }
        rows.push(row("assistant", &format!("a{i}")));
    }
    let last = rows.len() - 1;
    for (i, r) in rows.iter().enumerate() {
        if r.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            let turn = message_index_to_turn(&rows, i)
                .unwrap_or_else(|| panic!("reply row {i} must map to a turn"));
            let cut = row_turn_cut(&rows, turn);
            assert!(
                cut > i,
                "cut at turn {turn} (row {cut}) must retain the clicked reply (row {i})"
            );
        }
    }
    // 末轮回复 → 全量保留（cut == rows.len()）。
    assert_eq!(message_index_to_turn(&rows, last), Some(6));
    assert_eq!(row_turn_cut(&rows, 6), rows.len());
}

/// 生产实例形态（11-23 部署会话）：67 轮、轮 67 回复在 row 150、轮首
/// boundary 在 row 149 —— 前端旧窗口计数得 6，修复后必须得 67。
/// 构造：前 66 轮各 (u, tool, a) × 3 行 = 198 行，轮 67 = (u, a)。
#[test]
fn test_message_index_to_turn_production_incident_shape() {
    let mut rows = Vec::new();
    for i in 1..=66 {
        rows.push(row("user", &format!("q{i}")));
        rows.push(row("tool", &format!("tool {i}")));
        rows.push(row("assistant", &format!("a{i}")));
    }
    // 0-based: 轮 67 的 u = row 198, a = row 199。同时构造一个
    // 「中间轮」锚点：轮 67 的 u 在 198 —— 轮 67 回复换算 = 67。
    let q67_idx = rows.len(); // 198 = q67 的 index；回复 index = 199
    rows.push(row("user", "q67"));
    rows.push(row("assistant", "a67"));
    assert_eq!(message_index_to_turn(&rows, q67_idx + 1), Some(67));
    // 切点：at_turn 67 → 下一边界之前 = 全量保留（67 是最后一轮）。
    assert_eq!(row_turn_cut(&rows, 67), rows.len());
    // 前端旧窗口相对值 6 若再被送进来 → 只保留前 6 轮（旧行为的破坏面
    // 对照：正确值 67 与错误值 6 的 kept 差 = 61 轮）。
    assert_eq!(row_turn_cut(&rows, 6), 18);
    assert!(row_turn_cut(&rows, 6) < q67_idx, "旧行为会截掉点击轮");
}

/// fork 链的 B2 内容锚消费（单一真相源 crate::chat_log::resolve_anchor_index
/// ——rewind 族同款）：命中重定位 / 零命中 fail-closed。
#[test]
fn test_fork_anchor_relocation_consumes_chat_log_truth_source() {
    let rows = vec![
        row("user", "q1"),
        row("assistant", "a1"),
        row("user", "重复问题"),
        row("assistant", "same reply"),
        row("user", "重复问题"),
        row("assistant", "same reply"),
    ];
    // 快路径：index 处内容已相等 → 直通。
    assert_eq!(
        crate::chat_log::resolve_anchor_index(&rows, 1, Some("a1")).unwrap(),
        1
    );
    // 漂移路径：index 3 处不是 "重复问题"，重定位到 row 2。
    assert_eq!(
        crate::chat_log::resolve_anchor_index(&rows, 3, Some("重复问题")).unwrap(),
        2
    );
    // 多行命中取距 index 最近（row 4 vs row 2，index 3 → row 2）。
    // 零命中 = fail-closed Err（绝不按漂移 index 盲切）。
    let err = crate::chat_log::resolve_anchor_index(&rows, 1, Some("ghost"))
        .expect_err("zero-hit anchor must fail closed");
    assert!(err.contains("锚内容定位失败"), "{err}");
    // None/空锚 = 旧行为直通（ForkSessionModal 的 at_turn 形态不受影响）。
    assert_eq!(
        crate::chat_log::resolve_anchor_index(&rows, 1, None).unwrap(),
        1
    );
    assert_eq!(
        crate::chat_log::resolve_anchor_index(&rows, 1, Some("   ")).unwrap(),
        1
    );
}
