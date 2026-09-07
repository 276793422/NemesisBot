//! A1（2026-09-04）：edit_file 失败反馈 → 修复指令的单元 + execute 集成测试。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{build_not_found_hint, match_line_numbers, unified_diff};
use crate::context::RequestContext;
use crate::r#loop::Tool;
use crate::loop_tools::EditFileTool;

// ---------------------------------------------------------------------------
// 纯函数层
// ---------------------------------------------------------------------------

#[test]
fn hint_offers_crlf_advice() {
    let content = "alpha\r\nbeta\r\n";
    let hint = build_not_found_hint(content, "alpha\nbeta");
    assert!(hint.contains("CRLF"), "hint should mention CRLF: {hint}");
}

#[test]
fn hint_offers_trailing_whitespace_advice() {
    let content = "let x = 1;   \nlet y = 2;\n";
    let hint = build_not_found_hint(content, "let x = 1;\nlet y = 2;");
    assert!(
        hint.contains("trailing whitespace"),
        "hint should mention trailing whitespace: {hint}"
    );
}

#[test]
fn hint_offers_closest_candidates_with_line_numbers() {
    let content = "fn alpha() {\n    fn do_smt() {}\n}\n";
    let hint = build_not_found_hint(content, "fn do_sumt() {");
    assert!(
        hint.contains("line 2"),
        "should cite candidate line: {hint}"
    );
    assert!(
        hint.contains("do_smt"),
        "should show candidate text: {hint}"
    );
    assert!(
        hint.contains("edit distance"),
        "should show distance: {hint}"
    );
}

#[test]
fn hint_preview_head_tail_for_long_files() {
    let content: String = (1..=80).map(|i| format!("line {i}\n")).collect();
    let hint = build_not_found_hint(&content, "no such text anywhere");
    assert!(
        hint.contains("(35 more lines)"),
        "80 lines = 30 head + 35 omitted + 15 tail: {hint}"
    );
    assert!(
        hint.contains("   80 |"),
        "should show last line number: {hint}"
    );
    assert!(
        hint.contains("    1 |"),
        "should show first line number: {hint}"
    );
}

#[test]
fn match_line_numbers_finds_all_occurrences() {
    let content = "a\ndup\nb\nc\ndup\nf\ng\nh\ndup\n";
    assert_eq!(match_line_numbers(content, "dup"), vec![2, 5, 9]);
    assert!(match_line_numbers(content, "absent").is_empty());
}

// ---------------------------------------------------------------------------
// execute 集成层
// ---------------------------------------------------------------------------

fn tmp_file(name: &str, content: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!(
        "nb_edit_hint_{}_{n}_{}.txt",
        std::process::id(),
        name
    ));
    std::fs::write(&p, content).expect("write tmp file");
    p
}

async fn run_edit(path: &Path, old_text: &str, new_text: &str) -> Result<String, String> {
    let tool = EditFileTool::default();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": path.to_string_lossy(),
        "old_text": old_text,
        "new_text": new_text,
    });
    tool.execute(&args.to_string(), &ctx).await
}

#[tokio::test]
async fn execute_not_found_returns_repair_hint() {
    let p = tmp_file("notfound", "alpha\nbeta\n");
    let err = run_edit(&p, "alpha\ngamma\n", "x")
        .await
        .expect_err("must fail");
    assert!(err.contains("not found in"), "got: {err}");
    assert!(
        err.contains("File content preview"),
        "hint must embed file preview: {err}"
    );
    let _ = std::fs::remove_file(&p);
}

#[tokio::test]
async fn execute_multi_hit_lists_line_numbers() {
    let p = tmp_file("multi", "a\ndup\nb\nc\ndup\n");
    let err = run_edit(&p, "dup", "x").await.expect_err("must fail");
    assert!(err.contains("appears 2 times"), "got: {err}");
    assert!(err.contains("[2, 5]"), "should list hit lines: {err}");
    let _ = std::fs::remove_file(&p);
}

#[tokio::test]
async fn execute_success_includes_diff() {
    let p = tmp_file("success", "alpha\nbeta\n");
    let out = run_edit(&p, "alpha", "ALPHA").await.expect("must succeed");
    assert!(out.starts_with("File edited:"), "got: {out}");
    // A2：成功回 unified diff 代码块。
    assert!(out.contains("```diff"), "should embed diff fence: {out}");
    assert!(out.contains("-alpha"), "should show removed line: {out}");
    assert!(out.contains("+ALPHA"), "should show added line: {out}");
    let _ = std::fs::remove_file(&p);
}

// ---------------------------------------------------------------------------
// A3：replace_all
// ---------------------------------------------------------------------------

async fn run_edit_replace_all(
    path: &Path,
    old_text: &str,
    new_text: &str,
) -> Result<String, String> {
    let tool = EditFileTool::default();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    let args = serde_json::json!({
        "path": path.to_string_lossy(),
        "old_text": old_text,
        "new_text": new_text,
        "replace_all": true,
    });
    tool.execute(&args.to_string(), &ctx).await
}

#[tokio::test]
async fn execute_replace_all_replaces_every_occurrence() {
    let p = tmp_file("replaceall", "dup\nx\ndup\ny\ndup\n");
    let out = run_edit_replace_all(&p, "dup", "DUP")
        .await
        .expect("must succeed");
    assert!(
        out.contains("3 occurrences replaced"),
        "should report count: {out}"
    );
    assert!(out.contains("```diff"), "should embed diff fence: {out}");
    // 三处替换全部落盘。
    let on_disk = std::fs::read_to_string(&p).unwrap();
    assert_eq!(on_disk, "DUP\nx\nDUP\ny\nDUP\n");
    // diff 三处替换都可见。
    assert_eq!(
        out.lines().filter(|l| l.starts_with("+DUP")).count(),
        3,
        "diff shows all three additions: {out}"
    );
    let _ = std::fs::remove_file(&p);
}

#[tokio::test]
async fn execute_replace_all_identical_text_reports_count_without_diff() {
    let p = tmp_file("replaceall_same", "dup\nx\ndup\n");
    let out = run_edit_replace_all(&p, "dup", "dup")
        .await
        .expect("must succeed");
    assert!(
        out.contains("2 occurrences replaced"),
        "should report count: {out}"
    );
    assert!(
        !out.contains("```diff"),
        "no-op replacement has no diff: {out}"
    );
    let _ = std::fs::remove_file(&p);
}

// ---------------------------------------------------------------------------
// A2：unified_diff 纯函数层
// ---------------------------------------------------------------------------

#[test]
fn unified_diff_shows_replacement_with_context() {
    let diff = unified_diff("f.txt", "a\nb\nc\nd\ne", "a\nb\nX\nd\ne");
    assert!(diff.contains("--- a/f.txt"), "file header: {diff}");
    assert!(diff.contains("+++ b/f.txt"), "file header: {diff}");
    assert!(diff.contains("@@ -"), "hunk header: {diff}");
    assert!(diff.contains("-c"), "removed line: {diff}");
    assert!(diff.contains("+X"), "added line: {diff}");
    assert!(diff.contains("\n b\n"), "leading context: {diff}");
    assert!(diff.contains("\n d\n"), "trailing context: {diff}");
}

#[test]
fn unified_diff_pure_addition() {
    let diff = unified_diff("f.txt", "a\nb", "a\nb\nc");
    assert!(diff.contains("+c"), "{diff}");
    assert!(
        !diff.lines().skip(3).any(|l| l.starts_with('-')),
        "no removal lines after ---/+++/@@ headers: {diff}"
    );
}

#[test]
fn unified_diff_pure_deletion() {
    let diff = unified_diff("f.txt", "a\nb\nc", "a\nc");
    assert!(diff.contains("-b"), "{diff}");
    assert!(
        !diff.lines().skip(3).any(|l| l.starts_with('+')),
        "no addition lines after ---/+++/@@ headers: {diff}"
    );
}

#[test]
fn unified_diff_identical_returns_empty() {
    assert_eq!(unified_diff("f.txt", "same\ntext", "same\ntext"), "");
}

#[test]
fn unified_diff_long_output_truncated() {
    let old: String = (1..=500).map(|i| format!("line {i}\n")).collect();
    let new: String = (1..=500).map(|i| format!("LINE {i}\n")).collect();
    let diff = unified_diff("f.txt", &old, &new);
    assert!(
        diff.contains("truncated"),
        "long diff must truncate: {diff}"
    );
    assert!(
        diff.len() < 4096 + 200,
        "summary must be small: {}",
        diff.len()
    );
}
