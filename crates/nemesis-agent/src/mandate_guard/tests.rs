//! L13-b mandate 执行面守卫单测（纯裁决矩阵——授权面/时限/预算/记录）。

use super::*;
use nemesis_types::cluster::MandateEnforcement;

/// 造守卫（enforcement 字段可覆写）。
fn guard_with(
    allowlist: Option<Vec<String>>,
    max_tokens: u64,
    max_tool_calls: u64,
) -> MandateGuard {
    MandateGuard::new(
        MandateEnforcement {
            task_id: "t-1".into(),
            tool_allowlist: allowlist,
            expires_at: 1000,
            max_tokens,
            max_tool_calls,
        },
        500,
    )
}

#[test]
fn allowlist_unlimited_allows_any_tool() {
    let g = guard_with(None, 0, 0);
    assert!(g.check_tool_call("exec", 100).is_ok());
    assert!(g.check_tool_call("write_file", 100).is_ok());
}

#[test]
fn allowlist_explicit_denies_outside() {
    let g = guard_with(Some(vec!["exec".into(), "read_file".into()]), 0, 0);
    assert!(g.check_tool_call("exec", 100).is_ok());
    let err = g.check_tool_call("shell", 100).unwrap_err();
    assert!(err.contains("不在 mandate 授权面"), "got: {err}");
    assert!(err.contains("shell"));
}

#[test]
fn allowlist_empty_is_zero_authorization() {
    // 空数组 = 零授权（与 None=不限 语义相反）
    let g = guard_with(Some(vec![]), 0, 0);
    assert!(g.check_tool_call("exec", 100).is_err());
}

#[test]
fn allowlist_denial_does_not_consume_call_budget() {
    // 工具面拒绝不递增计数（没执行不花预算）
    let g = guard_with(Some(vec!["exec".into()]), 0, 5);
    assert!(g.check_tool_call("shell", 100).is_err());
    assert_eq!(g.tool_calls.load(Ordering::SeqCst), 0);
    assert!(g.check_tool_call("exec", 100).is_ok());
    assert_eq!(g.tool_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn expiry_denies_calls_and_round_top() {
    let g = guard_with(None, 0, 0);
    assert!(g.check_tool_call("exec", 999).is_ok());
    let err = g.check_tool_call("exec", 1000).unwrap_err();
    assert!(err.contains("时间窗已过期"), "got: {err}");
    // 轮顶检同判
    assert!(g.check_expiry(999).is_ok());
    assert!(g.check_expiry(1000).is_err());
    // 过期拒绝不占调用预算槽（本守卫 max_tool_calls=0 = 不限不计数）
    assert_eq!(g.tool_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn tool_call_budget_cap_and_storm_guard() {
    let g = guard_with(None, 0, 2);
    assert!(g.check_tool_call("exec", 100).is_ok());
    assert!(g.check_tool_call("exec", 100).is_ok());
    // 第 3 次 = 超限拒；计数继续递增（占位防重试风暴——每次尝试都被拒）
    let err = g.check_tool_call("exec", 100).unwrap_err();
    assert!(err.contains("次数超限"), "got: {err}");
    assert_eq!(g.tool_calls.load(Ordering::SeqCst), 3);
    assert!(g.check_tool_call("exec", 100).is_err());
    // 轮顶检同判：已达上限
    assert!(g.check_tool_call_budget().is_err());
}

#[test]
fn tool_call_budget_zero_is_unlimited() {
    let g = guard_with(None, 0, 0);
    for _ in 0..100 {
        assert!(g.check_tool_call("exec", 100).is_ok());
    }
    assert!(g.check_tool_call_budget().is_ok());
}

#[test]
fn token_budget_delta_semantics() {
    let g = guard_with(None, 1000, 0);
    // baseline=500；used=1200 → delta=700 < 1000 未超
    assert!(g.check_token_budget(1200).is_ok());
    // used=1500 → delta=1000 >= 1000 超限
    let err = g.check_token_budget(1500).unwrap_err();
    assert!(err.contains("token 预算耗尽"), "got: {err}");
    assert!(err.contains("1000/1000"));
}

#[test]
fn token_budget_zero_is_unlimited() {
    let g = guard_with(None, 0, 0);
    assert!(g.check_token_budget(u64::MAX).is_ok());
}

#[test]
fn token_budget_underflow_baseline_guarded() {
    // used < baseline（retention 清扫削行）→ saturating → delta=0
    let g = guard_with(None, 100, 0);
    assert!(g.check_token_budget(100).is_ok());
    assert!(g.check_token_budget(50).is_ok());
}

#[test]
fn violation_report_format_and_cap() {
    let g = guard_with(Some(vec!["exec".into()]), 0, 0);
    assert!(g.violation_report().is_none());
    for _ in 0..(MAX_RECORDED_VIOLATIONS + 5) {
        let _ = g.check_tool_call("shell", 100);
    }
    let report = g.violation_report().unwrap();
    // 上限截断注记本身占一条（32 条记录 + 1 条注记 = 报告头 33）
    assert!(report.contains(&format!(
        "{MANDATE_VIOLATION_MARKER}（{} 条）",
        MAX_RECORDED_VIOLATIONS + 1
    )));
    assert!(report.contains("后续违规记录因上限截断"));
    assert!(report.contains("1. 工具 `shell` 不在 mandate 授权面"));
}

#[test]
fn terminal_notice_starts_with_marker() {
    let n = terminal_notice("token 预算耗尽（1000/1000）");
    assert!(n.starts_with(MANDATE_VIOLATION_MARKER));
    assert!(n.contains("已完成的工作已保存"));
}

#[test]
fn now_unix_monotone_sane() {
    // 2026-11-03 之后的合理量级（不接受 0 / 负数归零形态）
    assert!(now_unix() > 1_700_000_000);
}
