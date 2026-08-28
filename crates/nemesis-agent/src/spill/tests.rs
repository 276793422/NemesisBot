//! Tests for tool-result spill (U4).

use super::*;

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_spill_test_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    dir
}

/// Path traversal: a hostile session_key / call_id containing `../` must not
/// escape the spill root — it sanitizes to inert segments.
#[test]
fn test_spill_rejects_path_traversal() {
    let root = temp_root("traversal");
    let out = spill_tool_result(
        &"x".repeat(SPILL_THRESHOLD_CHARS),
        "exec",
        &root,
        "../../../etc",
        "20260821_000000",
        "call_..\\..\\evil",
    );
    match out {
        SpillOutcome::Spilled(text) => {
            assert!(text.contains(&root.display().to_string()));
            // The written file must live UNDER the root (no `..` anywhere in
            // the resolved path relative to root).
            assert!(!text.contains(".."));
            // Verify on disk: find any file under root — it must resolve
            // inside root.
            let mut found = false;
            for entry in walk(&root) {
                assert!(entry.starts_with(&root), "escaped root: {entry:?}");
                found = true;
            }
            assert!(found, "spill file exists under root");
        }
        _ => panic!("expected spill"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Round-trip: the spilled file contains the FULL original text; the model
/// can read it back (offset/limit is read_file's concern — here we verify
/// the file is complete and the locator path is accurate).
#[test]
fn test_spill_roundtrip_readback() {
    let root = temp_root("roundtrip");
    // Distinctive head and tail so truncation would be detectable.
    let mut body = String::from("HEAD_MARKER_START\n");
    for i in 0..70_000 {
        body.push_str(&format!("line {:06}\n", i));
    }
    body.push_str("TAIL_MARKER_END\n");
    let out = spill_tool_result(&body, "exec", &root, "sess", "20260821_010101", "call_1");
    let text = match out {
        SpillOutcome::Spilled(t) => t,
        _ => panic!("expected spill"),
    };
    // Locator present with retrieval guidance.
    assert!(text.contains("read_file"));
    assert!(text.contains("grep"));
    // Extract the path from the locator line and read the file back.
    let path_line = text
        .lines()
        .find(|l| l.contains("已完整保存到"))
        .expect("locator line");
    let start = path_line.find("：").map(|i| i + "：".len()).unwrap_or(0);
    let end = path_line[start..].find("。").map(|i| start + i).unwrap_or(path_line.len());
    let path_str = &path_line[start..end];
    let spilled = std::fs::read_to_string(path_str).expect("spill file readable");
    assert!(spilled.starts_with("HEAD_MARKER_START"));
    assert!(spilled.ends_with("TAIL_MARKER_END\n"));
    assert_eq!(spilled.len(), body.len(), "full text preserved");
    let _ = std::fs::remove_dir_all(&root);
}

/// Below-threshold results do not spill.
#[test]
fn test_spill_below_threshold_passthrough() {
    let root = temp_root("below");
    let out = spill_tool_result(
        &"x".repeat(SPILL_THRESHOLD_CHARS - 1),
        "exec",
        &root,
        "s",
        "t",
        "c",
    );
    assert!(matches!(out, SpillOutcome::BelowThreshold));
    let _ = std::fs::remove_dir_all(&root);
}

/// Spill failure is non-destructive: an unwritable root returns SpillFailed
/// (caller keeps the original result).
#[test]
fn test_spill_failure_is_best_effort() {
    // A FILE where a directory is needed → create_dir_all fails.
    let root = temp_root("blocked");
    std::fs::create_dir_all(&root).unwrap();
    let file_blocker = root.join("sess");
    std::fs::write(&file_blocker, b"blocker").unwrap();
    let out = spill_tool_result(
        &"x".repeat(SPILL_THRESHOLD_CHARS + 10),
        "exec",
        &root,
        "sess",
        "t",
        "c",
    );
    assert!(matches!(out, SpillOutcome::SpillFailed));
    let _ = std::fs::remove_dir_all(&root);
}

/// Segment sanitization unit checks.
#[test]
fn test_sanitize_segment() {
    assert_eq!(sanitize_segment("normal-key_1.2"), "normal-key_1.2");
    assert_eq!(sanitize_segment("../etc"), "___etc");
    // `..` collapses to `__` — inert as a path segment (no traversal).
    assert_eq!(sanitize_segment(".."), "__");
    assert_eq!(sanitize_segment("."), "_");
    assert_eq!(sanitize_segment(""), "_");
    assert_eq!(sanitize_segment("a/b\\c:d"), "a_b_c_d");
    let long = "a".repeat(500);
    assert_eq!(sanitize_segment(&long).chars().count(), 80);
}

// ---------------------------------------------------------------------------
// U4 retention cleanup (startup scan + daily task call this)
// ---------------------------------------------------------------------------

/// Create a spill file and set its mtime to `age_secs` ago (std-only:
/// `File::set_modified`, no new dev-dependency).
fn make_spill_file(root: &Path, session: &str, name: &str, age_secs: u64, content: &str) {
    use std::io::Write;
    let dir = root.join(session);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    f.write_all(content.as_bytes()).unwrap();
    drop(f);
    let past =
        std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_modified(past).unwrap();
}

#[test]
fn test_cleanup_expired_deletes_old_and_keeps_fresh() {
    let root = temp_root("retention");
    make_spill_file(&root, "s1", "old.txt", 10 * 24 * 3600, "old"); // 10 days
    make_spill_file(&root, "s1", "fresh.txt", 1 * 24 * 3600, "fresh"); // 1 day
    make_spill_file(&root, "s2", "also_old.txt", 30 * 24 * 3600, "old2"); // 30 days

    let deleted = cleanup_expired(&root, 7);
    assert_eq!(deleted, 2, "two expired files deleted");
    assert!(!root.join("s1").join("old.txt").exists());
    assert!(root.join("s1").join("fresh.txt").exists(), "fresh file kept");
    assert!(!root.join("s2").exists(), "s2 became empty -> dir removed");
    assert!(root.join("s1").exists(), "s1 still has a file -> dir kept");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_cleanup_zero_days_disables() {
    let root = temp_root("retention0");
    make_spill_file(&root, "s1", "old.txt", 365 * 24 * 3600, "ancient");
    assert_eq!(cleanup_expired(&root, 0), 0, "0 = disabled, nothing touched");
    assert!(root.join("s1").join("old.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_cleanup_missing_root_is_noop() {
    let root = temp_root("retention_absent");
    assert_eq!(cleanup_expired(&root, 7), 0);
}

#[test]
fn test_cleanup_removes_root_when_fully_empty() {
    let root = temp_root("retention_all_old");
    make_spill_file(&root, "s1", "a.txt", 10 * 24 * 3600, "x");
    make_spill_file(&root, "s2", "b.txt", 10 * 24 * 3600, "y");
    assert_eq!(cleanup_expired(&root, 7), 2);
    assert!(
        !root.join("s1").exists() && !root.join("s2").exists(),
        "empty session dirs removed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Additional branch coverage (W3a)
// ---------------------------------------------------------------------------

/// `File::create` failure: a DIRECTORY pre-created exactly where the spill
/// FILE would land makes create_dir_all succeed but File::create fail →
/// SpillFailed (best-effort: caller keeps the original result).
#[test]
fn test_spill_file_create_blocked_by_directory_at_path() {
    let root = temp_root("blocked_create");
    // Spill file would be root/sess/<stamp>_<call>.txt — make that path a dir.
    std::fs::create_dir_all(root.join("sess").join("20260821_000000_call.txt")).unwrap();
    let out = spill_tool_result(
        &"x".repeat(SPILL_THRESHOLD_CHARS + 10),
        "exec",
        &root,
        "sess",
        "20260821_000000",
        "call",
    );
    assert!(matches!(out, SpillOutcome::SpillFailed));
    let _ = std::fs::remove_dir_all(&root);
}

/// Unexpected NESTED dir inside a session dir: `dir_empty = false` (line 141)
/// and the nested dir is left alone; flat expired files still swept.
#[test]
fn test_cleanup_nested_dir_left_alone() {
    let root = temp_root("retention_nested");
    std::fs::create_dir_all(root.join("s1").join("nested")).unwrap();
    make_spill_file(&root, "s1", "old.txt", 10 * 24 * 3600, "x");
    let deleted = cleanup_expired(&root, 7);
    assert_eq!(deleted, 1, "only the flat expired file deleted");
    assert!(
        root.join("s1").join("nested").exists(),
        "nested dir left alone"
    );
    assert!(root.join("s1").exists(), "dir not empty -> kept");
    let _ = std::fs::remove_dir_all(&root);
}

/// Expired file that CANNOT be deleted (read-only on Windows): the sweep warns
/// (line 152-153), keeps `dir_empty = false` (line 158), so the session dir
/// survives with the locked file inside. Cleanup never aborts.
#[cfg(windows)]
#[test]
fn test_cleanup_readonly_file_survives_and_keeps_dir() {
    let root = temp_root("retention_ro");
    make_spill_file(&root, "s_ro", "locked.txt", 10 * 24 * 3600, "locked");
    let p = root.join("s_ro").join("locked.txt");

    // 探针判定只读删除语义：部分文件系统（ReFS/Dev Drive）不强制，实测可删
    // ——未强制时跳过（该 warn/保留分支只在不强制只读的机器上可达）。
    let probe = root.join("ro_probe.txt");
    std::fs::write(&probe, b"p").unwrap();
    let mut perm = std::fs::metadata(&probe).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&probe, perm).unwrap();
    let enforced = std::fs::remove_file(&probe).is_err();
    if !enforced {
        eprintln!("skipping readonly-survives arm: filesystem does not enforce readonly deletes");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&p, perm).unwrap();

    let deleted = cleanup_expired(&root, 7);
    assert_eq!(deleted, 0, "read-only file could not be deleted");
    assert!(p.exists(), "locked file survived the sweep");
    assert!(root.join("s_ro").exists(), "dir kept (still not empty)");

    // Cleanup: clear readonly so remove_dir_all can remove the tree.
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(false);
    std::fs::set_permissions(&p, perm).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// Stray files DIRECTLY under the spill root (lines 175-193): expired ones are
/// swept by the same retention rule (and counted); fresh ones kept.
#[test]
fn test_cleanup_stray_files_under_root_swept_by_same_rule() {
    let root = temp_root("retention_stray");
    std::fs::create_dir_all(&root).unwrap();

    let old = root.join("stray_old.txt");
    std::fs::write(&old, "x").unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 24 * 3600);
    let f = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
    f.set_modified(past).unwrap();
    drop(f);

    let fresh = root.join("stray_fresh.txt");
    std::fs::write(&fresh, "y").unwrap();

    let deleted = cleanup_expired(&root, 7);
    assert_eq!(deleted, 1, "expired stray deleted, fresh kept");
    assert!(!old.exists(), "expired stray removed");
    assert!(fresh.exists(), "fresh stray kept");
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// G3: status() — Dashboard spill 卡的只读聚合
// ---------------------------------------------------------------------------

#[test]
fn test_status_missing_root_is_all_zero() {
    let root = temp_root("status_absent");
    let st = status(&root);
    assert_eq!(st.files, 0);
    assert_eq!(st.bytes, 0);
    assert!(st.oldest.is_none());
}

#[test]
fn test_status_counts_files_bytes_and_oldest() {
    let root = temp_root("status_tree");
    make_spill_file(&root, "s1", "newer.txt", 1 * 24 * 3600, "0123456789"); // 10 bytes
    make_spill_file(&root, "s2", "older.txt", 5 * 24 * 3600, "abc"); // 3 bytes

    let st = status(&root);
    assert_eq!(st.files, 2);
    assert_eq!(st.bytes, 13);
    // oldest 是 RFC3339 且早于 now - 2 天（5 天前那个）。
    let oldest = chrono::DateTime::parse_from_rfc3339(st.oldest.as_ref().unwrap()).unwrap();
    let cutoff = chrono::Local::now() - chrono::Duration::days(2);
    assert!(oldest < cutoff, "oldest should be the 5-day-old file");
    let _ = std::fs::remove_dir_all(&root);
}
