//! Tests for model-free tool-result pruning (U3).

use super::*;

#[test]
fn test_prune_under_threshold_passthrough() {
    let short = "x".repeat(100);
    assert!(prune_tool_result(&short, "grep").is_none());
    let exactly = "y".repeat(MAX_TOOL_RESULT_INLINE_CHARS);
    assert!(prune_tool_result(&exactly, "grep").is_none());
}

#[test]
fn test_prune_over_threshold_head_tail_marker() {
    // ASCII base: head and tail content are recognizable.
    let mut s = String::new();
    for i in 0..6000 {
        s.push_str(&format!("H{:05}\n", i)); // head region
    }
    for i in 0..20000 {
        s.push_str(&format!("M{:05}\n", i)); // middle region (elided)
    }
    for i in 0..6000 {
        s.push_str(&format!("T{:05}\n", i)); // tail region
    }
    let out = prune_tool_result(&s, "exec").expect("must prune");
    assert!(out.starts_with("H00000"));
    assert!(out.ends_with("T05999\n") || out.ends_with("T05999"));
    assert!(out.contains("exec"));
    assert!(out.contains("中间省略"));
    // Total pruned output stays within the inline budget.
    assert!(out.chars().count() <= MAX_TOOL_RESULT_INLINE_CHARS + 200);
}

#[test]
fn test_prune_multibyte_no_panic() {
    // Chinese chars are 3 bytes each in UTF-8; any naive byte-slice would
    // panic. 8192 chars of 中文, each repeated to be safely over threshold.
    let s = "中文内容测试边界安全".repeat(1200);
    let out = prune_tool_result(&s, "read_file").expect("must prune");
    assert!(out.chars().count() <= MAX_TOOL_RESULT_INLINE_CHARS + 200);
    // First and last chars survive intact (valid string, no replacement chars
    // from a broken boundary).
    assert!(out.starts_with('中'));
    assert!(out.contains('全'));
}

#[test]
fn test_prune_marker_names_tool_and_gives_recovery_hint() {
    let s = "a".repeat(MAX_TOOL_RESULT_INLINE_CHARS + 1);
    let out = prune_tool_result(&s, "web_fetch").expect("must prune");
    assert!(out.contains("web_fetch"));
    assert!(out.contains("缩小范围重试"));
}
