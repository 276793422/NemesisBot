//! B26 敏感 flag 细化单测（写侧臂）：归约产出 flag 级 pattern +
//! 存量无 flag 行为逐字节回归。

use super::*;

// ---------- 写侧臂：flag 级细化 ----------

#[test]
fn b26_push_force_tail_flag_refines_pattern() {
    // 原始洞场景：批准 `git push origin main --force` → flag 级 pattern
    assert_eq!(
        reduce_command("git push origin main --force"),
        "git push --force *"
    );
}

#[test]
fn b26_push_force_in_prefix_same_pattern() {
    // flag 在保留前缀之后、其余参数之前：同一产出（flag 插入前缀尾）
    assert_eq!(
        reduce_command("git push --force origin main"),
        "git push --force *"
    );
}

#[test]
fn b26_reset_hard_refines_pattern() {
    assert_eq!(
        reduce_command("git reset --hard HEAD~1"),
        "git reset --hard *"
    );
}

#[test]
fn b26_commit_no_verify_refines_pattern() {
    assert_eq!(
        reduce_command("git commit --no-verify -m \"fix\""),
        "git commit --no-verify *"
    );
}

#[test]
fn b26_multiple_flags_in_order() {
    // 多 flag 按出现顺序追加（去重）
    assert_eq!(
        reduce_command("git push --force --no-verify origin main"),
        "git push --force --no-verify *"
    );
}

#[test]
fn b26_duplicate_flag_not_duplicated() {
    assert_eq!(
        reduce_command("git push --force origin --force main"),
        "git push --force *"
    );
}

#[test]
fn b26_short_command_with_flag_refines() {
    // `git push --force`（3 token > keep=2）：走截断+细化路径
    assert_eq!(reduce_command("git push --force"), "git push --force *");
}

#[test]
fn b26_exact_length_flag_within_prefix() {
    // ≤ arity（`npm run` keep=3，命令恰 3 token）：原样输出（无 star），
    // flag 天然显式在 pattern 中——不变量（flag 永远是 pattern 显式
    // 组成部分）由原样输出满足
    assert_eq!(reduce_command("npm run --force"), "npm run --force");
}

#[test]
fn b26_env_prefix_flag_still_refines() {
    assert_eq!(
        reduce_command("FOO=1 git push --force origin main"),
        "git push --force *"
    );
}

// ---------- 已知边界：引号值撞 flag 名 ----------

#[test]
fn b26_quoted_flag_value_treated_as_sensitive() {
    // `-m` 的值恰好是 `--force`：tokenize 剥引号后按 token 判定命中——
    // 宁可多细化（多弹一次卡，安全方向），不做引号态追踪（模块文档
    // 已知边界，此测试钉住行为防无意变更）
    assert_eq!(
        reduce_command("git commit -m \"--force\" x"),
        "git commit --force *"
    );
}

// ---------- `-f` 短标志明确暂缓 ----------

#[test]
fn b26_short_f_flag_not_sensitive() {
    // 初版只收长标志：`-f` 不触发细化（grep -f / ffmpeg -f 等
    // 非 force 语义误伤面大，宁窄勿宽）
    assert_eq!(reduce_command("git push -f origin main"), "git push *");
    assert_eq!(reduce_command("grep -f pattern.txt file"), "grep *");
}

// ---------- 存量行为回归（无敏感 flag → 逐字节一致） ----------

#[test]
fn b26_regression_no_flag_commands_unchanged() {
    assert_eq!(reduce_command("git push origin main"), "git push *");
    assert_eq!(reduce_command("cargo build --release"), "cargo build *");
    assert_eq!(reduce_command("cargo test -p x --lib"), "cargo test *");
    assert_eq!(reduce_command("git commit -m \"fix\""), "git commit *");
    assert_eq!(reduce_command("git status"), "git status");
    assert_eq!(reduce_command("npm run dev -- --port"), "npm run dev *");
    assert_eq!(reduce_command("rm -rf /tmp/build"), "rm *");
}

#[test]
fn b26_regression_release_flag_not_sensitive() {
    // `--release` 等普通参数不进敏感表（只收破坏力升级 flag）
    assert_eq!(
        reduce_command("cargo build --release --features foo"),
        "cargo build *"
    );
}

// ---------- sensitive_flags_in 单元 ----------

#[test]
fn b26_sensitive_flags_in_order_and_dedup() {
    let tokens: Vec<String> = ["git", "push", "--force", "x", "--no-verify", "--force"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(sensitive_flags_in(&tokens), vec!["--force", "--no-verify"]);
}

#[test]
fn b26_sensitive_flags_in_empty_and_clean() {
    assert!(sensitive_flags_in(&[]).is_empty());
    let clean: Vec<String> = ["cargo", "build", "--release"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(sensitive_flags_in(&clean).is_empty());
}
