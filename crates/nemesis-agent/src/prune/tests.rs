//! Tests for model-free tool-result pruning (U3).

use super::*;

#[test]
fn test_prune_under_threshold_passthrough() {
    let short = "x".repeat(100);
    assert!(prune_tool_result(&short, "grep", false).is_none());
    let exactly = "y".repeat(MAX_TOOL_RESULT_INLINE_CHARS);
    assert!(prune_tool_result(&exactly, "grep", false).is_none());
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
    let out = prune_tool_result(&s, "exec", false).expect("must prune");
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
    let out = prune_tool_result(&s, "read_file", false).expect("must prune");
    assert!(out.chars().count() <= MAX_TOOL_RESULT_INLINE_CHARS + 200);
    // First and last chars survive intact (valid string, no replacement chars
    // from a broken boundary).
    assert!(out.starts_with('中'));
    assert!(out.contains('全'));
}

#[test]
fn test_prune_marker_names_tool_and_gives_recovery_hint() {
    let s = "a".repeat(MAX_TOOL_RESULT_INLINE_CHARS + 1);
    let out = prune_tool_result(&s, "web_fetch", false).expect("must prune");
    assert!(out.contains("web_fetch"));
    assert!(out.contains("缩小范围重试"));
}

// ---------------------------------------------------------------------------
// B3 (devtool-upgrade 阶段 3): sub-agent hint switches on registry capability.
// 验收：有/无 spawn 两种文案断言。
// ---------------------------------------------------------------------------

/// Without the hint (no spawn tool in registry): marker text is byte-stable
/// with the pre-B3 form — no spawn wording anywhere.
#[test]
fn test_prune_without_hint_no_spawn_wording() {
    let s = "a".repeat(MAX_TOOL_RESULT_INLINE_CHARS + 1);
    let out = prune_tool_result(&s, "exec", false).expect("must prune");
    assert!(
        !out.contains("子代理"),
        "无 spawn 时文案不得出现子代理提示: {out}"
    );
    assert!(!out.contains(SUBAGENT_HINT_SUFFIX));
}

/// With the hint (spawn tool registered): the marker appends the sub-agent
/// guidance inside the marker brackets.
#[test]
fn test_prune_with_hint_appends_subagent_guidance() {
    let s = "a".repeat(MAX_TOOL_RESULT_INLINE_CHARS + 1);
    let out = prune_tool_result(&s, "exec", true).expect("must prune");
    assert!(out.contains(SUBAGENT_HINT_SUFFIX), "hint 附加文案缺失");
    assert!(out.contains("spawn 工具"));
    // Hint must sit INSIDE the marker brackets (before the closing `]`),
    // not dangling after the tail re-attachment.
    let marker_end = out.find("。]").expect("marker closing");
    let marker_start = out.find("[结果过长已截断").expect("marker opening");
    assert!(
        out[marker_start..marker_end].contains("子代理"),
        "hint 必须在 marker 括号内"
    );
    // Both forms keep the same head (prune geometry unchanged by the hint).
    let plain = prune_tool_result(&s, "exec", false).expect("must prune");
    let hint_len = SUBAGENT_HINT_SUFFIX.chars().count();
    assert_eq!(
        plain.chars().count(),
        out.chars().count() - hint_len,
        "带 hint 版本只多出 hint 长度"
    );
}
