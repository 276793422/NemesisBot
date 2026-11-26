//! history_search outage family — ISOLATED test binary (BUG-1 regression).
//!
//! Separate binary from `history_search_recovery.rs` because the FTS index
//! connection lives in a process-global `static INDEX` whose state can only
//! move None → Some within one process. This file's scenario needs the conn
//! ABSENT state (directory-blocked open → retry each call → recover after
//! the blocker is removed); the corruption-rebuild scenario needs it too —
//! they cannot coexist in one process. Harness mirrors
//! `history_search_recovery.rs` (per-process `NEMESISBOT_HOME` tempdir;
//! session jsonl fixtures written DIRECTLY, bypassing `append_chat_log`
//! which would open the index via the incremental hook).
//!
//! BUG-1 context: `conn: None` used to be cached permanently — one failed
//! open silently killed FTS indexing for the rest of the process life.
//! Fixed semantics: retry open on every missing-conn call; while down,
//! search degrades to the linear scan (goal §八) and incremental appends
//! yield honestly; after the blocker is gone the next call recovers and the
//! full rescan backfills everything missed.

use nemesis_agent::chat_log::append_chat_log;
use nemesis_agent::history_search::{append_guard, index_append, reindex_session_logs, search};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

static FAMILY_LOCK: Mutex<()> = Mutex::new(());
static FAMILY_HOME: OnceLock<PathBuf> = OnceLock::new();

fn family_guard() -> MutexGuard<'static, ()> {
    let guard = FAMILY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = FAMILY_HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "nb_hsout_home_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: once, under FAMILY_LOCK, before any sibling test thread in
        // this binary touches env or paths (same pattern as the other fts
        // binaries).
        unsafe { std::env::set_var("NEMESISBOT_HOME", &dir) };
        let _ = nemesis_path::default_path_manager();
        dir
    });
    debug_assert!(home.is_absolute());
    guard
}

fn write_jsonl_fixture(prefix: &str, marker: &str) -> String {
    let key = format!(
        "test:hsout:{}:{}",
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

fn hits_for(needle: &str, stem: &str) -> Vec<nemesis_agent::history_search::HistoryHit> {
    search(needle, 10)
        .into_iter()
        .filter(|h| h.session_key == stem)
        .collect()
}

/// Persistent open failure (db path occupied by a DIRECTORY — open fails,
/// recovery's remove_file cannot delete a directory) → every call retries
/// instead of caching the failure; search degrades to the linear scan and
/// answers from the jsonl source; incremental appends yield honestly without
/// panicking; once the blocker is removed the very next call recovers and
/// the full rescan backfills everything missed during the outage.
#[test]
fn test_open_failure_not_permanent_and_backfilled_after_recovery() {
    let _g = family_guard();
    let key = write_jsonl_fixture("stuck", "stuckopenmarker retry phrase zr9");
    let stem = nemesis_utils::sanitize::sanitize_path_segment(&key);

    // Blocker: occupy the db path with a directory (no handle conflicts —
    // the index was never opened in this process).
    let db = nemesis_agent::history_search::index_db_path();
    std::fs::create_dir(&db).expect("placeholder dir");

    // 1. reindex during outage: honest yield, no panic, nothing indexed.
    let rows = reindex_session_logs();
    assert_eq!(rows, 0, "outage must yield, got {rows} rows");

    // 2. search degrades to linear scan and still answers from jsonl.
    let hits = hits_for("stuckopenmarker", &stem);
    assert_eq!(
        hits.len(),
        1,
        "linear fallback must answer while FTS is down: {hits:?}"
    );

    // 3. appends during outage: append_chat_log writes the jsonl SOURCE
    // row (the index hook yields honestly); the bare index_append forms —
    // both locked and unlocked — yield without panicking (they are index
    // hooks only; with no source row there is nothing to backfill later,
    // which is the contract: jsonl is the source of truth).
    append_chat_log(&key, "assistant", "stuckappendmarker while db down");
    if let Some(mut g) = append_guard() {
        g.index_append(&key, "assistant", "no-source-row yield-only", "");
    }
    index_append(&key, "assistant", "no-source-row yield-only two", "");

    // 4. remove the blocker → the NEXT call must recover (no permanent
    // failure state) and backfill outage-time rows that DO have source
    // lines via the full rescan.
    std::fs::remove_dir(&db).expect("placeholder removed");
    reindex_session_logs();
    for (needle, label) in [
        ("stuckopenmarker", "pre-downtime row"),
        ("stuckappendmarker", "source row appended during downtime"),
    ] {
        let hits = hits_for(needle, &stem);
        assert_eq!(
            hits.len(),
            1,
            "{label} must be backfilled after recovery: {hits:?}"
        );
    }

    remove_fixture(&key);
}
