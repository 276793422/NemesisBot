//! Degraded-path test for `history_search::search` — ISOLATED binary.
//!
//! WHY a dedicated binary (same rationale as `history_search_fts.rs`, X3):
//! `search()` first tries the FTS index; on ANY DB failure it must degrade to
//! `search_linear` (a direct grep-style scan of `session_logs/*.jsonl`).
//! The in-lib tests never exercised that fallback — the lib test process has
//! a live shared index, and the FTS-family binary above always opens the DB
//! successfully. To force `open_conn` to fail, this process must make the DB
//! path un-openable BEFORE the `INDEX` singleton is first resolved — hence
//! its own binary (process-global `OnceLock`s start fresh here).
//!
//! Mechanism: `<home>/logs/history_index.db` is created as a DIRECTORY —
//! SQLite cannot open a directory as a database, so `Connection::open`
//! fails, `with_conn` yields `None`, `search_fts` yields `None`, and
//! `search` must fall through to the linear scan and still answer.

use nemesis_agent::chat_log::append_chat_log;
use nemesis_agent::history_search::{index_db_path, search};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

static FAMILY_LOCK: Mutex<()> = Mutex::new(());
static FAMILY_HOME: OnceLock<PathBuf> = OnceLock::new();

fn family_guard() -> MutexGuard<'static, ()> {
    let guard = FAMILY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    FAMILY_HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "nb_fts_degraded_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: set_var happens once, under FAMILY_LOCK, before any other
        // test thread in THIS binary can resolve a path (env-test-race-lock
        // pattern).
        unsafe { std::env::set_var("NEMESISBOT_HOME", &dir) };
        // Bake the singleton to the tempdir BEFORE sabotaging the DB path.
        let _ = nemesis_path::default_path_manager();
        // Sabotage: make the index DB path a directory → open always fails.
        let db = index_db_path();
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        std::fs::create_dir(&db).expect("pre-create history_index.db as a dir");
        dir
    });
    guard
}

#[test]
fn search_degrades_to_linear_when_db_unopenable() {
    let _g = family_guard();
    let key = format!(
        "test:degraded:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    append_chat_log(&key, "user", "degradedpathmarker unique phrase qq9");
    append_chat_log(&key, "assistant", "linear scan should still see this reply");

    // With the DB un-openable this MUST come from search_linear.
    let hits = search("degradedpathmarker", 10);
    assert!(
        !hits.is_empty(),
        "search must degrade to linear scan and still answer"
    );
    assert_eq!(
        hits[0].session_key, key.replace(':', "_"),
        "hit must come from the session we just wrote"
    );
    assert_eq!(hits[0].role, "user");
    assert!(
        hits[0].snippet.contains("degradedpathmarker"),
        "snippet carries the matched content: {}",
        hits[0].snippet
    );

    // The assistant line is findable too (scan covers all lines, not just user).
    let hits2 = search("linear scan should still see", 10);
    assert!(
        hits2.iter().any(|h| h.role == "assistant" && h.session_key == key.replace(':', "_")),
        "assistant turn reachable via degraded path"
    );

    // No-hit query stays empty (not an error, not a false positive).
    assert!(search("zebraunicornmissing", 10).is_empty());

    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[test]
fn degraded_db_never_breaks_empty_query_short_circuit() {
    let _g = family_guard();
    // Empty/whitespace query short-circuits before any DB touch — must hold
    // in the degraded state as well.
    assert!(search("", 10).is_empty());
    assert!(search("   ", 10).is_empty());
}
