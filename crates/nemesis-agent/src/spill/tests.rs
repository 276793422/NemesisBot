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
