//! B26 敏感 flag 单测（读侧臂 + 全链）：flag 命令不命中无 flag
//! pattern（原始洞修复）、flag pattern 正常放行、复合段独立判定、
//! 存量无 flag 行为回归。

use super::*;

// ---------- 读侧臂：原始洞修复 ----------

#[test]
fn b26_readsides_reject_flag_command_on_coarse_pattern() {
    // 原始洞：批准过不含 flag 的 `git push origin main`（存量粗 pattern
    // `git push *`），后续携带 --force 的命令不再被放行
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    assert!(
        !m("git push *", "git push origin main --force"),
        "存量粗 pattern 不得放行 flag 命令（读侧臂）"
    );
    assert!(!m("git push *", "git push --force origin main"));
    assert!(!m("git push *", "git push origin main --no-verify"));
    assert!(!m("git reset *", "git reset --hard HEAD~1"));
}

#[test]
fn b26_readsides_accept_flag_command_on_flag_pattern() {
    // flag pattern：写侧细化的产物正常放行同 flag 命令
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    assert!(m("git push --force *", "git push --force origin main"));
    assert!(m("git push --force *", "git push --force"), "bare prefix");
    assert!(m(
        "git push --force --no-verify *",
        "git push --force --no-verify x"
    ));
    // flag 顺序无关（命令里 flag 位置不同但都含该 flag，前缀形态一致）
    assert!(m("git push --force *", "git push --force"));
}

#[test]
fn b26_readsides_flag_position_not_normalized() {
    // flag 位置不归一：写侧归一把 flag 固定在 pattern 前缀尾（`git push
    // --force *`），命令里 flag 在别处（尾部）→ token 前缀位置不等 →
    // 不命中重新弹卡。保守方向（只可能多弹卡不可能漏放），刻意不做成
    // 「flag 集合相等」语义——钉住防未来误改松。
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    assert!(!m("git push --force *", "git push other-branch --force"));
}

#[test]
fn b26_readsides_reject_nonflag_on_flag_pattern() {
    // flag pattern 不罩无 flag 命令（token 前缀第三位不等）
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    assert!(!m("git push --force *", "git push origin main"));
    assert!(!m("git reset --hard *", "git reset HEAD~1"));
}

#[test]
fn b26_readsides_flag_in_pattern_but_command_has_extra_flag() {
    // pattern 含 --force、命令多带 --no-verify：命令含 pattern 没有的
    // flag → 不命中（重新审批）
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    assert!(!m(
        "git push --force *",
        "git push --force origin main --no-verify"
    ));
}

// ---------- 全链：pattern_for 产出 + matches_rule 消费 ----------

#[test]
fn b26_pattern_for_writeside_refinement() {
    assert_eq!(
        pattern_for("process_exec", "git push origin main --force"),
        "git push --force *"
    );
    assert_eq!(
        pattern_for("process_spawn", "git reset --hard HEAD~1"),
        "git reset --hard *"
    );
    // 无 flag 存量行为不变
    assert_eq!(
        pattern_for("process_exec", "git push origin main"),
        "git push *"
    );
}

#[test]
fn b26_end_to_end_flag_memory_roundtrip() {
    // 全链：批准（写侧产出）→ 后续同 flag 命令自动放行（读侧命中）
    let target = "git push origin main --force";
    let pattern = pattern_for("process_exec", target);
    assert_eq!(pattern, "git push --force *");
    assert!(matches_rule(
        "process_exec",
        &pattern,
        "process_exec",
        "git push --force other-branch"
    ));
    // 非 flag push 不被 flag 规则罩住
    assert!(!matches_rule(
        "process_exec",
        &pattern,
        "process_exec",
        "git push origin main"
    ));
}

// ---------- 复合命令：全段命中才放，每段独立过 flag 判定 ----------

#[test]
fn b26_readsides_compound_segment_flag_independent() {
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    // 全段前缀都命中的复合命令，任一段含 pattern 没有的 flag → 该段被
    // flag 臂拒 → 整体不放行（flag 臂在复合段独立生效）
    assert!(!m("git push *", "git push a && git push --force b"));
    // 全段都命中 flag pattern（前缀 + flag 显式一致）→ 放行
    assert!(m(
        "git push --force *",
        "git push --force a && git push --force b"
    ));
}

// ---------- 存量行为回归（无敏感 flag → 与现状一致） ----------

#[test]
fn b26_readsides_regression_nonflag_unchanged() {
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    assert!(m("cargo test *", "cargo test --release"));
    assert!(m("cargo test *", "cargo test"));
    assert!(!m("cargo test *", "cargo publish"));
    assert!(m("git push *", "git push origin main"));
    // env 前缀照常剥（闭环-13 语义不受 B26 影响）
    assert!(m("cargo test *", "FOO=1 cargo test --release"));
}

#[test]
fn b26_readsides_exact_pattern_vector_unchanged() {
    // 精确向量路径（无 ` *` 后缀手写 pattern）：向量相等天然要求 flag
    // 显式一致，B26 不改此路径
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    assert!(m("git push --force", "git push --force"));
    assert!(!m("git push --force", "git push"));
}
