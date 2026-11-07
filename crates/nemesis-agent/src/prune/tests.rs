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

// ---------------------------------------------------------------------------
// B30（2026-11-06 三方对比整改）：cap 参数化形态 `prune_tool_result_with_cap`
// ——`agents.tool_result_cap` 的 gate 入口（拍板③：缺省 8000、0=关）。
// ---------------------------------------------------------------------------

/// `0 = 关`：任何大小都原样放行（跨工具兜底封顶关闭；一键回旧行为）。
#[test]
fn b30_cap_zero_disables_pruning_entirely() {
    let huge = "a".repeat(1_000_000);
    assert!(
        prune_tool_result_with_cap(&huge, "exec", false, 0).is_none(),
        "cap=0 必须完全关闭（含百万字符巨量结果）"
    );
    let huge_hint = "a".repeat(1_000_000);
    assert!(
        prune_tool_result_with_cap(&huge_hint, "exec", true, 0).is_none(),
        "cap=0 关闭不受 hint_subagent 影响"
    );
}

/// `>cap` 才裁：≤cap 原样放行；>cap 裁剪且有诚实注记，输出有界。
#[test]
fn b30_cap_threshold_and_bounded_output() {
    let at_cap = "x".repeat(8000);
    assert!(prune_tool_result_with_cap(&at_cap, "grep", false, 8000).is_none());
    let one_over = "x".repeat(8001);
    let out = prune_tool_result_with_cap(&one_over, "grep", false, 8000).expect("must prune");
    assert!(out.contains("结果过长已截断"));
    assert!(out.contains("grep"));
    // 有界：头尾 3600+3600 + marker（缺省 cap 下与 legacy 预算同源）。
    assert!(out.chars().count() <= 8000 + 200);
}

/// 小 cap：按预算收缩头尾，输出仍 ≤ cap（marker 预算内）。
#[test]
fn b30_small_cap_shrinks_head_tail_budget() {
    let s = "y".repeat(50_000);
    let cap = 1000;
    let out = prune_tool_result_with_cap(&s, "exec", false, cap).expect("must prune");
    let n = out.chars().count();
    assert!(
        n <= cap + 200,
        "小 cap 输出必须近界（{n} vs cap {cap}；+200=marker 文案余量）"
    );
    // 头尾都还在（不是只剩 marker）。
    assert!(n > 400, "cap=1000 时头尾各 ~370 字符必须保留");
}

/// 极小 cap（注记都装不下）：诚实性高于字节预算——注记仍在场，不 panic
/// 不空串（`omitted` 不下溢：total > cap ≥ head+tail 恒成立）。
#[test]
fn b30_tiny_cap_keeps_marker_honest_over_budget() {
    let s = "z".repeat(10_000);
    let out = prune_tool_result_with_cap(&s, "exec", false, 50).expect("must prune");
    assert!(out.contains("结果过长已截断"), "注记优先于字节预算");
    assert!(out.contains("exec"));
    // head=0/tail=0 形态：输出就是注记本身，无 panic。
    assert!(out.chars().count() < 300);
}

/// CJK：chars 口径（非字节），多字节不劈半不 panic（str-slice 事故类）。
#[test]
fn b30_cap_counts_chars_not_bytes_cjk_safe() {
    // 8000 个中文 = 24000 字节；按字节算会漏裁，按 chars 算必须裁。
    let s = "汉".repeat(8000);
    assert!(prune_tool_result_with_cap(&s, "read_file", false, 8000).is_none());
    let s_over = "汉".repeat(8001);
    let out = prune_tool_result_with_cap(&s_over, "read_file", false, 8000).expect("must prune");
    assert!(out.starts_with('汉'), "头字符完整（无边界劈半）");
    assert!(out.contains("结果过长已截断"));
}

/// X1 同构判定（[`cap_prune_matches_recompute`]）：同构 → gate 不记投影
/// （纯重算照常）；不同构 → 必须记（否则重算 None/异形回灌不一致文本）。
#[test]
fn b30_cap_prune_matches_recompute_predicate() {
    // gate 未裁：cap=0 或 ≤cap → 同构（无裁剪发生）。
    assert!(cap_prune_matches_recompute(0, 1_000_000), "cap=0 未裁");
    assert!(cap_prune_matches_recompute(8000, 8000), "恰好 ≤cap 未裁");
    // 缺省 cap 深超限：头尾未收缩且重算也裁 → 同构（投影不记）。
    assert!(
        cap_prune_matches_recompute(8000, 40_000),
        ">8192 深超限与 legacy 同形"
    );
    // 缺省 cap 灰区（8001..=8192）：gate 裁、重算 None → 不同构（必记）。
    assert!(
        !cap_prune_matches_recompute(8000, 8100),
        "灰区必须记投影（UNDO 泄漏防线）"
    );
    // 预算收缩（小 cap）：头尾异形 → 不同构（必记）。
    assert!(
        !cap_prune_matches_recompute(4000, 40_000),
        "小 cap 头尾异形"
    );
    // 大 cap（>8192）深超限：gate 裁 head/tail 3600，重算同形 → 同构。
    assert!(
        cap_prune_matches_recompute(20_000, 40_000),
        "大 cap 头尾封顶 3600 与重算同形"
    );
    // 大 cap 但结果落在 (8192, cap]：gate 未裁（≤cap），重算也未裁 → 同构。
    assert!(
        cap_prune_matches_recompute(20_000, 10_000),
        "gate 未裁即同构"
    );
}

/// 缺省 cap（8000）与 legacy 缺省形态的关系：
/// - >8192（两者都裁）→ 输出**逐字节同形**（head/tail 几何同源）；
/// - 8001..=8192（legacy 放行、cap 裁）→ 本键的预期行为变化
///   （cap 8000 < legacy 内联阈 8192，正是立项目的）。
#[test]
fn b30_default_cap_byte_identical_to_legacy_for_deeply_oversized() {
    let s = "a".repeat(50_000);
    let legacy = prune_tool_result(&s, "exec", false).expect("legacy must prune");
    let capped = prune_tool_result_with_cap(&s, "exec", false, 8000).expect("capped must prune");
    assert_eq!(legacy, capped, "cap=8000 对 >8192 结果与缺省形态逐字节同形");
    // hint 形态同断言（SUBAGENT_HINT_SUFFIX 41 chars，不挤动 3600 头尾）。
    let legacy_h = prune_tool_result(&s, "exec", true).expect("legacy hint must prune");
    let capped_h =
        prune_tool_result_with_cap(&s, "exec", true, 8000).expect("capped hint must prune");
    assert_eq!(legacy_h, capped_h, "hint 形态同形");
    // 灰区：legacy 放行、cap 裁（行为变化本体，钉死防回退）。
    let gray = "b".repeat(8100);
    assert!(
        prune_tool_result(&gray, "exec", false).is_none(),
        "legacy ≤8192 放行"
    );
    assert!(
        prune_tool_result_with_cap(&gray, "exec", false, 8000).is_some(),
        "cap=8000 对 8001..=8192 灰区必须裁（预期行为变化）"
    );
}
