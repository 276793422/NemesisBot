//! history_search recovery family — ISOLATED test binary (BUG-1 regression).
//!
//! WHY a second dedicated binary (besides `history_search_fts.rs`): the FTS
//! index connection lives in a process-global `static INDEX` and, once
//! opened, stays open for the process lifetime (by design — connection
//! reuse). Recovery-path tests need the conn ABSENT state (open fails →
//! retry → rebuild), which cannot be reached in a binary whose earlier
//! tests opened the index: with a conn present, injected on-disk corruption
//! is invisible (sqlite serves its page cache; the stale fd keeps working),
//! and the db file cannot even be deleted/replaced on Windows while the
//! handle is held. A fresh process = fresh `INDEX = None`.
//!
//! Isolation harness mirrors `history_search_fts.rs` (per-process
//! `NEMESISBOT_HOME` tempdir baked before the path singleton resolves; the
//! session jsonl fixtures are written DIRECTLY — bypassing `append_chat_log`,
//! which would open the index via the incremental hook and defeat the whole
//! premise).
//!
//! BUG-1 context (2026-11-25 remote-home forensics): `conn: None` used to be
//! cached permanently in the static state — one failed open (hot journal
//! leftover / disk failure) silently killed FTS indexing for the rest of the
//! process life (zero retry / zero warn / zero self-heal; a full day of
//! conversation went unindexed). Fixed semantics: retry open on every
//! missing-conn call; rebuild-by-delete on corruption (REBUILDABLE cache,
//! the jsonl files are the source of truth); throttled WARN while down;
//! mtime registrations cleared on recovery so the next reindex backfills
//! everything missed.

use nemesis_agent::history_search::{reindex_session_logs, search};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serialize the family (shared home dir + global FTS index state).
static FAMILY_LOCK: Mutex<()> = Mutex::new(());

/// Per-process isolated home (set exactly once, before first path access).
static FAMILY_HOME: OnceLock<PathBuf> = OnceLock::new();

fn family_guard() -> MutexGuard<'static, ()> {
    let guard = FAMILY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = FAMILY_HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "nb_hsrec_home_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: same env-test-race-lock-pattern as history_search_fts.rs —
        // the write happens once under FAMILY_LOCK while every sibling test
        // thread in this binary is blocked on the same lock.
        unsafe { std::env::set_var("NEMESISBOT_HOME", &dir) };
        let _ = nemesis_path::default_path_manager();
        dir
    });
    debug_assert!(home.is_absolute());
    guard
}

/// Write a session jsonl fixture DIRECTLY into `session_logs/` (two lines),
/// bypassing `append_chat_log` so the FTS index stays unopened. Returns the
/// session key; the on-disk stem is the sanitized form (':' → '_').
fn write_jsonl_fixture(prefix: &str, marker: &str) -> String {
    let key = format!(
        "test:hsrec:{}:{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dir = nemesis_path::default_path_manager().sessions_log_dir();
    std::fs::create_dir_all(&dir).expect("sessions_log_dir created");
    let stem = nemesis_utils::sanitize::sanitize_path_segment(&key);
    let path = dir.join(format!("{stem}.jsonl"));
    let line = |role: &str, content: &str| {
        serde_json::json!({"role": role, "content": content, "timestamp": ""}).to_string()
    };
    std::fs::write(
        &path,
        format!("{}\n{}\n", line("user", marker), line("assistant", "ack")),
    )
    .expect("fixture written");
    key
}

fn remove_fixture(key: &str) {
    let dir = nemesis_path::default_path_manager().sessions_log_dir();
    let stem = nemesis_utils::sanitize::sanitize_path_segment(key);
    let _ = std::fs::remove_file(dir.join(format!("{stem}.jsonl")));
}

fn db_path() -> PathBuf {
    nemesis_agent::history_search::index_db_path()
}

fn hits_for(needle: &str, stem: &str) -> Vec<nemesis_agent::history_search::HistoryHit> {
    search(needle, 10)
        .into_iter()
        .filter(|h| h.session_key == stem)
        .collect()
}

/// Corruption injected BEFORE the index is ever opened: garbage db header
/// (sqlite "file is not a database") + stale journal leftover — the 11-25
/// 00:13 crime scene. First reindex must: fail the open, run the recovery
/// (delete db/-journal/-wal/-shm, rebuild), rescan everything, and leave a
/// WORKING index. Search must answer afterwards.
#[test]
fn test_corrupt_db_rebuilt_on_first_open() {
    let _g = family_guard();
    let key = write_jsonl_fixture("corrupt", "corruptdbmarker rebuild phrase kq3");
    let stem = nemesis_utils::sanitize::sanitize_path_segment(&key);

    let db = db_path();
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent).expect("logs dir");
    }
    std::fs::write(&db, b"NOT_A_SQLITE_HEADER_AT_ALL____").expect("garbage db");
    let journal = PathBuf::from(format!("{}-journal", db.display()));
    std::fs::write(&journal, b"stale journal bytes").expect("stale journal");

    // First touch: open fails (NOTADB) → recovery deletes the four files →
    // fresh db → full rescan (nothing was registered yet).
    reindex_session_logs();

    assert!(db.exists(), "db rebuilt");
    // A live TRUNCATE-mode journal may exist as a 0-byte leftover from the
    // rescan's write txn — the stale BYTES must be gone either way.
    let jlen = std::fs::metadata(&journal).map(|m| m.len()).unwrap_or(0);
    assert_eq!(jlen, 0, "stale journal content must be gone, got {jlen}B");

    let hits = hits_for("corruptdbmarker", &stem);
    assert_eq!(
        hits.len(),
        1,
        "search must work after corruption recovery: {hits:?}"
    );

    remove_fixture(&key);
}

/// A HEALTHY db must never be touched by the recovery path (no delete, no
/// rebuild): consecutive reindexes keep the file intact and skip unchanged
/// files (rows == 0). Guards the fix's failure branch against over-firing.
#[test]
fn test_healthy_db_not_rebuilt() {
    let _g = family_guard();
    let key = write_jsonl_fixture("healthy", "healthydbmarker untouched phrase wp1");
    let stem = nemesis_utils::sanitize::sanitize_path_segment(&key);

    reindex_session_logs();
    let db = db_path();
    let before = std::fs::metadata(&db).expect("db exists").len();
    let rows = reindex_session_logs();
    assert_eq!(rows, 0, "unchanged files must be skipped, got {rows}");
    let after = std::fs::metadata(&db).expect("db exists").len();
    assert_eq!(before, after, "healthy db must not be rewritten");
    assert_eq!(hits_for("healthydbmarker", &stem).len(), 1);

    remove_fixture(&key);
}
