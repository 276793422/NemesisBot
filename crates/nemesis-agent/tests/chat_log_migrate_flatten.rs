//! `migrate_nested_session_logs` 嵌套目录平化迁移 — ISOLATED test binary。
//!
//! WHY a dedicated integration binary (same rationale as
//! `history_search_fts.rs`): `default_path_manager()` is a process-global
//! `OnceLock` singleton whose home is baked by the FIRST resolver in the
//! process. In the lib test binary the first call belongs to whichever
//! thread wins the startup race, so the home is the real `~/.nemesisbot` —
//! shared with every other test binary in the workspace. A crash between
//! move and cleanup (or two binaries running the suite concurrently, e.g.
//! parallel coverage measurement) leaves debris there that makes the
//! in-lib copy of this test fail on its own leftover ("目标已存在" skip arm
//! fires before the move assert). Exactly the fts flake family's root
//! cause, seen live on 2026-09-25.
//!
//! In THIS binary a one-time init guard sets `NEMESISBOT_HOME` to a fresh
//! per-process tempdir BEFORE the singleton's first resolution, so every
//! run starts from an empty home — no cross-run ghosts, no cross-binary
//! contention — without touching production code. Do NOT move this back
//! into `src/chat_log/cov_tests.rs`: it loses home isolation the moment a
//! sibling test resolves the singleton first.

use nemesis_agent::chat_log::migrate_nested_session_logs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serialize the family: they share the per-process home's session dirs.
static FAMILY_LOCK: Mutex<()> = Mutex::new(());

/// Per-process isolated home (created+set exactly once, before this binary
/// resolves any path). `resolve_home_dir()` joins `.nemesisbot` onto the env
/// value, so the actual home is `<tempdir>/.nemesisbot`.
static FAMILY_HOME: OnceLock<PathBuf> = OnceLock::new();

/// Acquire the family lock AND make sure the isolated home is live.
fn family_guard() -> MutexGuard<'static, ()> {
    let guard = FAMILY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = FAMILY_HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "nb_migrate_home_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: set_var while other threads may run is UB-adjacent in
        // general; here the FAMILY_LOCK guard blocks every other test thread
        // in this binary before any of them touch env or paths.
        unsafe { std::env::set_var("NEMESISBOT_HOME", &dir) };
        // Bake the singleton NOW (first resolution in this process) so all
        // path resolution lands under the tempdir.
        let _ = nemesis_path::default_path_manager();
        dir
    });
    debug_assert!(home.is_absolute());
    guard
}

/// 嵌套目录平化迁移：移动 / 目标已存在跳过 / 幂等（chat_log 962-1010 全链）。
#[test]
fn migrate_nested_session_logs_flattens_legacy_dirs() {
    let _g = family_guard();
    let pm = nemesis_path::default_path_manager();
    let root = pm.sessions_log_dir();
    std::fs::create_dir_all(&root).unwrap();

    // 嵌套 → 平化移动。
    let nested = root.join("covflat_d");
    std::fs::create_dir_all(&nested).unwrap();
    let f = nested.join("covflat_f.jsonl");
    std::fs::write(&f, "{\"role\":\"user\"}\n").unwrap();
    migrate_nested_session_logs();
    assert!(!f.exists(), "nested file must be moved");
    assert!(root.join("covflat_d_covflat_f.jsonl").exists());

    // 目标已存在 → 跳过不覆盖（嵌套源保留 = 用户现状优先）。
    let nested2 = root.join("covflat_s");
    std::fs::create_dir_all(&nested2).unwrap();
    let f2 = nested2.join("covg.jsonl");
    std::fs::write(&f2, "nested\n").unwrap();
    std::fs::write(root.join("covflat_s_covg.jsonl"), "existing\n").unwrap();
    migrate_nested_session_logs();
    assert!(f2.exists(), "skip keeps nested source");
    assert_eq!(
        std::fs::read_to_string(root.join("covflat_s_covg.jsonl")).unwrap(),
        "existing\n"
    );

    // 幂等：二次运行零操作不 panic。
    migrate_nested_session_logs();

    // 清理（临时 home 内，进程结束整体可弃）。
    let _ = std::fs::remove_dir_all(&nested);
    let _ = std::fs::remove_dir_all(&nested2);
    let _ = std::fs::remove_file(root.join("covflat_d_covflat_f.jsonl"));
    let _ = std::fs::remove_file(root.join("covflat_s_covg.jsonl"));
}
