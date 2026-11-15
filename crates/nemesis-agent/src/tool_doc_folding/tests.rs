//! pure-helper tests: cosine, one-line summary extraction,
//! and the fold decision/rendering (determinism, tie-breaks, passthrough).

use super::*;

fn def(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: crate::types::ToolFunctionDef {
            name: name.to_string(),
            description: description.to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }
}

#[test]
fn test_cosine_basics() {
    // Identical direction → 1.0 (modulo f32 rounding).
    let a = vec![1.0, 0.0, 0.0];
    assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
    // Orthogonal → 0.0.
    assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    // Opposite → -1.0.
    assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    // Zero-norm / empty → 0.0, never NaN.
    assert_eq!(cosine(&[], &[]), 0.0);
    assert_eq!(cosine(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
    // Length mismatch: compare the overlapping prefix deterministically.
    assert!(cosine(&[1.0], &[1.0, 5.0]) >= 0.99);
}

#[test]
fn test_one_line_summary_first_sentence() {
    // ASCII: cut at ". " — the second sentence is dropped.
    assert_eq!(
        one_line_summary(
            "Search the web for current information. Returns titles, URLs, and snippets."
        ),
        "Search the web for current information."
    );
    // "e.g." must NOT cut (period not followed by whitespace).
    assert_eq!(
        one_line_summary("Run a command, e.g. ls -la. Then returns output."),
        "Run a command, e.g. ls -la."
    );
    // CJK: cut at 。 (terminator kept, mirroring sentence text).
    assert_eq!(
        one_line_summary("对代码做只读语义查询。由真实语言服务器驱动，比 grep 精确。"),
        "对代码做只读语义查询。"
    );
    // Newline / semicolon terminate.
    assert_eq!(one_line_summary("line one\nline two"), "line one");
    assert_eq!(
        one_line_summary("half a thought; rest dropped"),
        "half a thought"
    );
    // Single-sentence short descriptions pass through BYTE-identical.
    let short = "Read the contents of a file";
    assert_eq!(one_line_summary(short), short);
    // Leading/trailing whitespace trimmed.
    assert_eq!(one_line_summary("  padded  "), "padded");
    // Empty stays empty.
    assert_eq!(one_line_summary("   "), "");
}

#[test]
fn test_one_line_summary_cap_is_char_boundary_safe() {
    // A first sentence longer than the cap cuts on a char boundary — CJK
    // text must not panic mid-codepoint (str-slice-multibyte-panic lesson).
    let long = "很".repeat(500);
    let s = one_line_summary(&long);
    assert_eq!(s.chars().count(), super::SUMMARY_CHAR_CAP + 1);
    assert!(s.ends_with('…'));
    // Mixed-width text also lands on a boundary.
    let mixed = format!("{} tail", "字".repeat(300));
    let s2 = one_line_summary(&mixed);
    assert!(s2.ends_with('…') || s2.ends_with("tail"));
}

#[test]
fn test_fold_top_n_expansion_and_shape() {
    let defs = vec![
        def("aaa", "First sentence dropped here. Second one too."),
        def("bbb", "Weather lookup tool. Returns the forecast."),
        def("ccc", "Web search tool. Returns snippets."),
        def("ddd", "File reader tool. Reads files."),
    ];
    let mut sims = HashMap::new();
    sims.insert("bbb".to_string(), 0.9);
    sims.insert("ccc".to_string(), 0.8);

    let folded = fold_tool_defs(defs.clone(), &sims, 1);

    // Order and count preserved; names/schemas untouched.
    assert_eq!(folded.len(), 4);
    let names: Vec<&str> = folded.iter().map(|d| d.function.name.as_str()).collect();
    assert_eq!(names, vec!["aaa", "bbb", "ccc", "ddd"]);
    assert_eq!(folded[0].function.parameters, defs[0].function.parameters);

    // Top-1 (bbb, 0.9) keeps FULL description bytes.
    assert_eq!(
        folded[1].function.description,
        "Weather lookup tool. Returns the forecast."
    );
    // Everyone else folds to the first sentence.
    assert_eq!(
        folded[0].function.description,
        "First sentence dropped here."
    );
    assert_eq!(folded[2].function.description, "Web search tool.");
    assert_eq!(folded[3].function.description, "File reader tool.");
}

#[test]
fn test_fold_deterministic_and_tie_breaks_by_name() {
    let defs = vec![
        def("zeta", "Z tool. Folded."),
        def("alpha", "A tool. Folded."),
        def("mid", "M tool. Folded."),
    ];
    // All ties (no scores at all → 0.0): expansion goes to the lowest names
    // alphabetically — deterministic, never map-iteration order.
    let sims = HashMap::new();
    let f1 = fold_tool_defs(defs.clone(), &sims, 1);
    let f2 = fold_tool_defs(defs.clone(), &sims, 1);
    assert_eq!(f1[0].function.description, "Z tool."); // zeta folded
    assert_eq!(f1[1].function.description, "A tool. Folded."); // alpha expanded
    assert_eq!(f1[2].function.description, "M tool."); // mid folded
    // Same inputs → identical output bytes, twice.
    let b1 = serde_json::to_string(&f1).unwrap();
    let b2 = serde_json::to_string(&f2).unwrap();
    assert_eq!(b1, b2);
}

#[test]
fn test_fold_passthrough_when_top_n_covers_all() {
    let defs = vec![def("a", "A tool."), def("b", "B tool.")];
    let mut sims = HashMap::new();
    sims.insert("a".to_string(), 0.1);
    // top_n >= len → byte-identical passthrough (both descriptions full).
    let out = fold_tool_defs(defs.clone(), &sims, 2);
    assert_eq!(
        serde_json::to_string(&out).unwrap(),
        serde_json::to_string(&defs).unwrap()
    );
    // Empty set likewise.
    assert!(fold_tool_defs(vec![], &sims, 0).is_empty());
}

/// one_line_summary terminator branches:
/// - CJK '。' keeps the punctuation in the cut;
/// - '\n' drops the terminator;
/// - '.' followed by an uppercase word ends the sentence (ASCII rule).
#[test]
fn test_one_line_summary_cjk_and_newline_cuts() {
    assert_eq!(one_line_summary("第一句。第二句"), "第一句。");
    assert_eq!(one_line_summary("line1\nline2"), "line1");
    assert_eq!(one_line_summary("Ends here. Then more"), "Ends here.");
    // No terminator → whole string (already covered elsewhere, pin anyway).
    assert_eq!(one_line_summary("just words"), "just words");
}

// BUG-20261116-015：守卫此前只挡 `na <= 0.0`——NaN 比较恒 false、inf
// 通过比较，两者都穿透余弦 → NaN 分数 → fold_tool_defs 排序
// `partial_cmp().unwrap_or(Equal)` 违反全序（driftsort 可 panic）。
// f32 累加与共享版（f64）不同：有限大元素平方即可溢出 inf，非有限
// 形态两条路都可达。非有限范数 = 不可比较，待遇 0.0（解析域自守）。
#[test]
fn test_cosine_non_finite_norms_yield_zero() {
    // NaN 元素：`NaN <= 0.0` 为 false，旧码穿透返回 NaN。
    assert_eq!(cosine(&[f32::NAN], &[1.0]), 0.0);
    assert_eq!(cosine(&[1.0], &[f32::NAN]), 0.0);
    // inf 元素：na = inf → inf/(inf*1) = NaN（旧码穿透）。
    assert_eq!(cosine(&[f32::INFINITY], &[1.0]), 0.0);
    assert_eq!(cosine(&[1.0], &[f32::INFINITY]), 0.0);
    // 有限大元素 f32 平方溢出（1e20² = 1e40 > f32::MAX）：na = inf → NaN。
    assert_eq!(cosine(&[1e20f32], &[1e20f32]), 0.0);
    // 有限正常值行为不变。
    let s = cosine(&[3.0, 4.0], &[3.0, 4.0]);
    assert!((s - 1.0).abs() < 1e-6, "正常向量不受守卫影响: {s}");
}
