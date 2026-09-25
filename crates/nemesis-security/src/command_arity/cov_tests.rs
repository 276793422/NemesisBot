// command_arity.rs 覆盖率补充测试（is_env_assignment 的 `=x` 形态
// eq==0 早退臂 122，经 reduce_command 驱动）。

use super::*;

/// 头部 `=x`（eq==0）不是合法环境变量前缀 → 不被跳过（122）；
/// 对照 `FOO=bar` 形态被正常归约。
#[test]
fn env_assignment_edge_forms() {
    // "=x" 以 = 开头 → 非环境前缀 → 保留在输出里。
    let reduced = reduce_command("=x dir");
    assert!(reduced.starts_with("=x"), "非法前缀不得被跳过: {reduced}");
    assert!(
        reduced.contains("*") || reduced.contains("dir"),
        "{reduced}"
    );

    // 正常 VAR=value 前缀 → 被跳过。
    let reduced = reduce_command("FOO=bar dir");
    assert!(!reduced.contains("FOO=bar"), "环境前缀应被归约: {reduced}");

    // 纯 "=x"（无可执行）：不构成环境前缀也不匹配 arity 表 → 原样返回
    // （rest 非空、keep=1、rest.len()==1 走 ≤1 早退臂）。
    assert_eq!(reduce_command("=x"), "=x");
}

/// tokenize 基本形态（引号合并）行为锁定。
#[test]
fn tokenize_merges_quoted_tokens() {
    assert_eq!(
        tokenize_command("git commit -m \"two words\""),
        vec![
            "git".to_string(),
            "commit".to_string(),
            "-m".to_string(),
            "two words".to_string()
        ]
    );
}
