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
//! SINGLE-TEST DISCIPLINE (2026-11-27 实证，两轮误诊后的真根因)：本 binary
//! 必须只有一个测试。两个测试并存时 libtest 并行启动、FAMILY_LOCK 抢锁
//! 顺序非确定——healthy 先抢锁会把进程级 `INDEX.conn` 打开，corrupt 随后
//! 运行时 `ensure_conn` 首行 `conn.is_some()` 直接短路，恢复阶梯完全不可
//! 达：夹具 journal 永不删除 → 断言红（≈30-40% 红率，随抢锁顺序浮动）。
//! solo 过滤跑恒绿（进程里只有 corrupt，conn 恒 None）——当初据此误诊为
//! AV 删除挂起幻影（两层假说：毫秒级幻影窗 → 删除真失败），全部被插桩
//! 证伪：红轮里 recover_and_open 从未被进入。合并成单测试后恢复链与
//! 「不误伤」验证天然有序，进程态前提恒成立。
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

/// Serialize within this binary (single test — kept for form; shared home +
/// global FTS index state).
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

/// Corruption recovery + stability, in one test (see module doc for why the
/// binary must host a single test).
///
/// Phase 1 — corruption injected BEFORE the index is ever opened: garbage db
/// header (sqlite "file is not a database") + stale journal leftover — the
/// 11-25 00:13 crime scene. First reindex must: fail the open, run the
/// recovery (delete db/-journal/-wal/-shm, rebuild), rescan everything, and
/// leave a WORKING index.
///
/// Phase 2 — a HEALTHY db must never be touched by the recovery path (no
/// delete, no rebuild): consecutive reindexes keep the file intact and skip
/// unchanged files (rows == 0). Guards the fix's failure branch against
/// over-firing.
#[test]
fn test_corrupt_recovery_then_healthy_stability() {
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
    // rescan's write txn — the stale BYTES must be gone either way. Bounded
    // poll gives any transient handle a settle window without loosening the
    // semantics (still 19B at deadline = genuine failure; the diagnostics in
    // the message discriminate db-rebuilt vs db-still-garbage).
    let mut jlen = std::fs::metadata(&journal).map(|m| m.len()).unwrap_or(0);
    for _ in 0..40 {
        if jlen == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        jlen = std::fs::metadata(&journal).map(|m| m.len()).unwrap_or(0);
    }
    if jlen != 0 {
        let dbhead = std::fs::read(&db)
            .map(|b| String::from_utf8_lossy(&b[..b.len().min(16)]).into_owned())
            .unwrap_or_else(|e| format!("<unreadable: {e}>"));
        let jcontent = std::fs::read(&journal)
            .map(|b| String::from_utf8_lossy(&b[..b.len().min(19)]).into_owned())
            .unwrap_or_else(|e| format!("<unreadable: {e}>"));
        panic!(
            "stale journal content must be gone, got {jlen}B; db={dbhead:?}; journal={jcontent:?} \
             (db NOT-A-SQLITE here = recovery never ran, see module doc)"
        );
    }

    let hits = hits_for("corruptdbmarker", &stem);
    assert_eq!(
        hits.len(),
        1,
        "search must work after corruption recovery: {hits:?}"
    );

    // Phase 2: healthy stability — recovery must not over-fire. New fixture
    // gets indexed by the next reindex (incremental backfill over the fresh
    // recovery); the reindex after THAT must be a pure no-op skip.
    let key2 = write_jsonl_fixture("healthy", "healthydbmarker untouched phrase wp1");
    let stem2 = nemesis_utils::sanitize::sanitize_path_segment(&key2);
    let rows_indexed = reindex_session_logs();
    assert!(
        rows_indexed >= 1,
        "new healthy fixture must be indexed, got {rows_indexed}"
    );
    let before = std::fs::metadata(&db).expect("db exists").len();
    let rows = reindex_session_logs();
    assert_eq!(rows, 0, "unchanged files must be skipped, got {rows}");
    let after = std::fs::metadata(&db).expect("db exists").len();
    assert_eq!(before, after, "healthy db must not be rewritten");
    assert_eq!(hits_for("healthydbmarker", &stem2).len(), 1);

    remove_fixture(&key);
    remove_fixture(&key2);
}
