//! yaml_depth 单测：度量件与深度闸（BUG-20261110-044）。

use super::*;

#[test]
fn depth_counts_flow_brackets() {
    assert_eq!(yaml_nesting_depth("[[[[]]]]"), 4);
    assert_eq!(yaml_nesting_depth("{a: [b, {c: 1}]}"), 3);
    assert_eq!(yaml_nesting_depth("plain: text"), 0);
}

#[test]
fn depth_counts_block_indent() {
    // 缩进宽度 / 2 计深：最深行 8 空格 → 深度 4。
    let fm = "root:\n  a:\n    b:\n      c:\n        d:\n";
    assert_eq!(yaml_nesting_depth(fm), 4);
}

#[test]
fn depth_ignores_brackets_in_same_line_closed_quotes() {
    // 同行闭合引号内是字面量：serde 不会为字符串内容递归。
    let fm = format!("description: \"{}\"", "[".repeat(100));
    assert_eq!(yaml_nesting_depth(&fm), 0);
}

#[test]
fn depth_counts_flow_after_plain_scalar_apostrophe_line() {
    // BUG-20261110-044 回归锁（度量件直测）：plain scalar 内的撇号曾把
    // 幻影单引号跨行永久开启，下一行 flow 括号全被跳过计深（报 0）→
    // 守卫被绕过 → serde_yaml 栈溢出 abort。修复后必须计入。
    let fm = format!("desc: it's\ntests: {}", "[".repeat(100));
    assert!(
        yaml_nesting_depth(&fm) >= 100,
        "plain scalar 撇号后的下一行 flow 括号必须计深"
    );
}

#[test]
fn depth_multiline_quoted_content_counts_as_outer() {
    // 逐行重置的保守方向：跨行引号标量的内容行按外层计深（只会多计，
    // 上界估计器语义；真实深度 0 也报 ≥4 可接受——绝不漏计真实结构）。
    let fm = "desc: \"line1\n[[[[";
    assert!(yaml_nesting_depth(fm) >= 4);
}

#[test]
fn check_gate_rejects_over_max_and_allows_normal() {
    let bomb = "[".repeat(YAML_MAX_NESTING_DEPTH + 1);
    let err = check_yaml_nesting_depth(&bomb).unwrap_err();
    assert!(err.contains("nesting depth"), "{err}");
    check_yaml_nesting_depth("name: d\ntests:\n  - name: t\n").unwrap();
}

#[test]
fn check_gate_catches_bomb_hidden_behind_apostrophe() {
    // 端到端攻击形态：撇号行 + 深度超限 flow 炸弹 → 闸必须拒。
    let fm = format!(
        "desc: it's\ntests: {}",
        "[".repeat(YAML_MAX_NESTING_DEPTH + 1)
    );
    assert!(check_yaml_nesting_depth(&fm).is_err());
}
