//! FTS history-search family — ISOLATED test binary (/ debt).
//!
//! WHY a dedicated integration binary instead of `src/history_search/tests.rs`:
//! `default_path_manager()` is a process-global `OnceLock` singleton whose
//! home is baked by the FIRST resolver in the process. In the lib test binary
//! (~1400 sibling tests) that first call belongs to whichever thread wins the
//! startup race, so the home is `~/.nemesisbot` — shared with every other
//! test binary in the workspace (`agent_*.jsonl` / `session1.jsonl`
//! leftovers and a stray `history_index.db-journal` accumulate there). That
//! sharing is the fts_chinese flake family's root cause: ghost rows from
//! crashed prior runs can satisfy unfiltered count assertions
//! (`test_index_append_incremental`'s `len() == 1`), and any concurrent
//! writer to the same SQLite db contends with reindex.
//!
//! In THIS binary the only tests are this family, so a one-time init guard
//! can set `NEMESISBOT_HOME` to a fresh per-process tempdir BEFORE the
//! singleton's first resolution (env-test-race-lock-pattern adapted: the env
//! write happens once, under the family lock, ahead of any path access).
//! Every run starts from an empty home — no cross-run ghosts, no
//! cross-binary contention — without touching production code. The
//! assertions are byte-identical to the ones that used to live in the lib
//! tests; only the isolation harness is new.
//!
//! NOTE for maintainers: do NOT move these back into the lib tests file —
//! they lose home isolation the moment a sibling test resolves the singleton
//! first. Pure helpers (`cjk_bigrams` / `render_hits` / empty-query
//! short-circuit) remain unit-tested in `src/history_search/tests.rs`.

use nemesis_agent::chat_log::{
    append_chat_log, clear_chat_log, delete_chat_log, truncate_chat_log_rows,
};
use nemesis_agent::history_search::{append_guard, reindex_session_logs, search};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serialize the family: they share the per-process home's session dir and
/// the global FTS index state, so interleaving reindex/search across threads
/// would race (same contract the old in-lib IDX_LOCK had).
static FAMILY_LOCK: Mutex<()> = Mutex::new(());

/// Per-process isolated home (created+set exactly once, before this binary
/// resolves any path). `resolve_home_dir()` joins `.nemesisbot` onto the env
/// value, so the actual home is `<tempdir>/.nemesisbot`.
static FAMILY_HOME: OnceLock<PathBuf> = OnceLock::new();

/// Acquire the family lock AND make sure the isolated home is live.
/// Call at the top of every test; keep the guard alive for the whole body.
fn family_guard() -> MutexGuard<'static, ()> {
    let guard = FAMILY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = FAMILY_HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "nb_fts_home_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // env mutation happens exactly once in this binary's lifetime, while
        // every other family test is blocked on FAMILY_LOCK — no env race.
        // SAFETY: set_var while other threads may run is UB-adjacent in
        // general; here the FAMILY_LOCK guard (held by the caller) blocks
        // every other test thread in this binary before any of them touch
        // env or paths.
        unsafe { std::env::set_var("NEMESISBOT_HOME", &dir) };
        // Bake the singleton NOW (first resolution in this process) so all
        // chat_log / history_search writes land under the tempdir.
        let _ = nemesis_path::default_path_manager();
        dir
    });
    debug_assert!(home.is_absolute());
    guard
}

/// Unique session key per call (nanos suffix). Under the per-process home no
/// prior-run leftovers can exist, so no family purge is needed anymore.
fn fresh_session(prefix: &str) -> String {
    format!(
        "test:hs:{}:{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// regression (found by the strict-completion final regression):
/// deleting a session's chat log must purge its FTS rows on the next
/// reindex — pre-fix, deleted files' rows lingered forever (ghost hits
/// accumulated across runs until they pushed live sessions out of the
/// search window; in production a deleted session stayed searchable).
#[test]
fn test_reindex_purges_deleted_session_rows() {
    let _g = family_guard();
    let k = fresh_session("ghost");
    append_chat_log(&k, "user", "ghostsessionmarker unique phrase zq7");
    reindex_session_logs();
    assert!(
        !search("ghostsessionmarker", 10).is_empty(),
        "must hit before delete"
    );

    delete_chat_log(&k);
    reindex_session_logs();
    let hits = search("ghostsessionmarker", 10);
    assert!(
        hits.iter().all(|h| h.session_key != k.replace(':', "_")),
        "deleted session's rows must be purged, got {:?}",
        hits.iter().map(|h| &h.session_key).collect::<Vec<_>>()
    );
}

#[test]
fn test_fts_chinese_cross_session() {
    let _g = family_guard();
    let k1 = fresh_session("zh1");
    let k2 = fresh_session("zh2");
    append_chat_log(&k1, "user", "请帮我部署文档系统到测试环境");
    append_chat_log(&k2, "user", "今天天气不错");
    append_chat_log(&k2, "assistant", "是的，适合出门散步");

    // Lazy full index then search a SHORT Chinese phrase that only the
    // bigram column can match (unicode61 would treat 部署文档... as one
    // token).
    reindex_session_logs();
    let hits = search("部署 文档", 20);
    assert!(
        hits.iter().any(|h| h.session_key == k1.replace(':', "_")),
        "must hit the deployment session: {:?}",
        hits.iter().map(|h| &h.session_key).collect::<Vec<_>>()
    );
    assert!(!hits.iter().any(|h| h.session_key == k2.replace(':', "_")));

    delete_chat_log(&k1);
    delete_chat_log(&k2);
}

#[test]
fn test_fts_english_and_snippet() {
    let _g = family_guard();
    let k = fresh_session("en");
    append_chat_log(&k, "user", "the quick brown fox jumps over the lazy dog");
    append_chat_log(&k, "assistant", "a plain reply about something else");
    reindex_session_logs();

    let hits = search("brown fox", 10);
    assert!(!hits.is_empty(), "english phrase hits");
    let h = &hits[0];
    assert_eq!(h.role, "user");
    // Same ghost-row lesson as the incremental test: pin the session so a
    // stale row from a deleted prior-run file can never satisfy the assert.
    assert_eq!(h.session_key, k.replace(':', "_"));
    assert!(h.snippet.contains("brown"), "snippet: {}", h.snippet);

    // No-hit query returns empty (not an error).
    assert!(search("zebraunicorn", 10).is_empty());

    delete_chat_log(&k);
}

#[test]
fn test_reindex_idempotent() {
    let _g = family_guard();
    let k = fresh_session("idem");
    let stem = k.replace(':', "_");
    append_chat_log(&k, "user", "idempotency probe content xyzzy");
    // First index.
    reindex_session_logs();
    // A second full pass must not duplicate this session's rows (the
    // DELETE-before-INSERT per-file contract), even if a concurrent test
    // touched OTHER files (the global "changed" count is racy across tests;
    // per-stem row count is the real correctness invariant).
    reindex_session_logs();
    let hits: Vec<_> = search("xyzzy", 50)
        .into_iter()
        .filter(|h| h.session_key == stem)
        .collect();
    assert_eq!(hits.len(), 1, "no duplicates for this session: {hits:?}");

    delete_chat_log(&k);
}

#[test]
fn test_index_append_incremental() {
    let _g = family_guard();
    let k = fresh_session("incr");
    // Full-index first (marks the file known).
    append_chat_log(&k, "user", "before marker plugh");
    reindex_session_logs();
    // Append AFTER indexing — index_append should pick it up without a
    // full reindex.
    append_chat_log(&k, "assistant", "after marker wabbajack");
    let hits = search("wabbajack", 10);
    assert_eq!(
        hits.len(),
        1,
        "appended row indexed incrementally: {hits:?}"
    );
    assert_eq!(hits[0].role, "assistant");
    // Key assertion: without it a STALE row from a deleted prior-run file
    // could satisfy this test (which is exactly how the raw-key/stem lookup
    // mismatch in index_append stayed masked — ghost rows answered the
    // query while the real append was never indexed).
    assert_eq!(hits[0].session_key, k.replace(':', "_"));

    delete_chat_log(&k);
}

/// The chat_log append path holds the index mutex across file-write +
/// incremental insert. Regression lock: a reindex racing that window must
/// BLOCK (not observe the "written but not yet indexed" state and then let
/// index_append re-insert the same line at a phantom MAX(seq)+1).
/// Deterministic — the shared mutex, not timing, forces the ordering; the
/// only timed part waits for the spawned thread to reach reindex and is
/// allowed to overshoot (a weak "blocked" assert can only over-pass, the
/// "completes after drop" join is the hard half).
#[test]
fn test_append_guard_excludes_reindex() {
    let _g = family_guard();
    let k = fresh_session("race");
    append_chat_log(&k, "user", "race guard probe quux");
    reindex_session_logs();

    let mut guard = append_guard().expect("index lock healthy");
    guard.index_append(&k, "assistant", "locked append frobnicate", "");

    let (tx, rx) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || {
        tx.send(()).ok(); // reindex entered
        reindex_session_logs();
        tx.send(()).ok(); // reindex finished
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("reindex thread started");
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "reindex must block while the append guard is held"
    );
    drop(guard);
    h.join().expect("reindex thread joins");
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("reindex completes after guard drop");

    // The locked variant indexed the row exactly once (and the racing
    // reindex — which re-read the unchanged file — must not have doubled it).
    let stem = k.replace(':', "_");
    let hits: Vec<_> = search("frobnicate", 50)
        .into_iter()
        .filter(|h| h.session_key == stem)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "locked append indexed exactly once: {hits:?}"
    );

    delete_chat_log(&k);
}

/// After a truncation the incremental hook's MAX(seq)+1 baseline is stale
/// (DB rows no longer match file line numbers). The hook must skip until
/// the next full reindex — pre-fix it inserted the appended row at a
/// phantom seq (pointing at no file line) and the truncated-away content
/// kept answering queries until the session's next append healed it.
#[test]
fn test_truncate_invalidates_incremental_baseline() {
    let _g = family_guard();
    let k = fresh_session("trunc");
    let stem = k.replace(':', "_");
    append_chat_log(&k, "user", "ancient line doomed qvex");
    append_chat_log(&k, "assistant", "second line slorvex also doomed");
    reindex_session_logs();

    // Keep only the first row (rewind-style truncation).
    let path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{stem}.jsonl"));
    let first = {
        let f = std::fs::File::open(&path).unwrap();
        std::io::BufReader::new(f).lines().next().unwrap().unwrap()
    };
    let kept: Vec<serde_json::Value> = vec![serde_json::from_str(&first).unwrap()];
    let written = truncate_chat_log_rows(&k, &kept);
    assert_eq!(written, 1);

    // Append after truncation: the hook must NOT touch the DB (baseline
    // invalidated). Pre-fix this produced a phantom row searchable before
    // any reindex.
    append_chat_log(&k, "user", "fresh tail after truncate zorph");
    let hits = search("zorph", 10);
    assert!(
        hits.is_empty(),
        "post-truncate append must wait for full reindex, got {hits:?}"
    );

    // Full rebuild: searchable with the CORRECT seq (file line 1), and the
    // truncated-away content is gone.
    reindex_session_logs();
    let hits: Vec<_> = search("zorph", 50)
        .into_iter()
        .filter(|h| h.session_key == stem)
        .collect();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].seq, 1, "seq must match the file line index");
    assert!(
        search("slorvex", 50).iter().all(|h| h.session_key != stem),
        "truncated-away content must be purged"
    );

    delete_chat_log(&k);
}

/// Same contract as truncation, through `clear_chat_log` (separate write
/// path): after a clear the hook must skip until the next full reindex.
#[test]
fn test_clear_invalidates_incremental_baseline() {
    let _g = family_guard();
    let k = fresh_session("clr");
    append_chat_log(&k, "user", "pre-clear content doomleaf");
    reindex_session_logs();

    clear_chat_log(&k).expect("clear succeeds");
    append_chat_log(&k, "user", "post-clear first row mendicorn");
    let hits = search("mendicorn", 10);
    assert!(
        hits.is_empty(),
        "post-clear append must wait for full reindex, got {hits:?}"
    );

    reindex_session_logs();
    let stem = k.replace(':', "_");
    let hits: Vec<_> = search("mendicorn", 50)
        .into_iter()
        .filter(|h| h.session_key == stem)
        .collect();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].seq, 0);

    delete_chat_log(&k);
}

/// Deleting a session then recreating the SAME key: the old DB rows must
/// not serve as the incremental hook's baseline (pre-fix the recreated
/// session's first append landed at the stale MAX(seq)+1 — a phantom seq
/// that no file line matches).
#[test]
fn test_delete_invalidates_recreated_session_baseline() {
    let _g = family_guard();
    let k = fresh_session("recreate");
    append_chat_log(&k, "user", "first incarnation blenko");
    reindex_session_logs();
    delete_chat_log(&k);

    append_chat_log(&k, "user", "second incarnation cruft");
    let hits = search("cruft", 10);
    assert!(
        hits.is_empty(),
        "post-delete append must wait for full reindex, got {hits:?}"
    );

    reindex_session_logs();
    let stem = k.replace(':', "_");
    let hits: Vec<_> = search("cruft", 50)
        .into_iter()
        .filter(|h| h.session_key == stem)
        .collect();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].seq, 0);

    delete_chat_log(&k);
}
