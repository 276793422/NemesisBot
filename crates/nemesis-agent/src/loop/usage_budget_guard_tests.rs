//! #30 全局用量配额纯裁决测试（W1）：budget_breach 四维矩阵 + 边界 +
//! 窗口起点换算。接线器官（data_store 依赖）归真链路验证（集成面）。

use super::usage_budget_guard::{UsageWindowAgg, budget_breach, unix_secs_at_local_midnight};
use chrono::{Datelike, Timelike};
use nemesis_config::UsageBudgetSettings;

fn settings(
    daily_token: u64,
    monthly_token: u64,
    daily_cost: f64,
    monthly_cost: f64,
    reject: bool,
) -> UsageBudgetSettings {
    UsageBudgetSettings {
        daily_token_limit: daily_token,
        monthly_token_limit: monthly_token,
        daily_cost_limit_usd: daily_cost,
        monthly_cost_limit_usd: monthly_cost,
        reject,
    }
}

fn agg(tokens: u64, cost: f64) -> UsageWindowAgg {
    UsageWindowAgg {
        tokens,
        cost_usd: cost,
    }
}

#[test]
fn all_zero_limits_means_never_breach() {
    let s = settings(0, 0, 0.0, 0.0, true);
    let huge = agg(u64::MAX, f64::MAX / 2.0);
    assert_eq!(budget_breach(&s, &huge, &huge), None, "全 0 = 全关");
}

#[test]
fn daily_token_breach_at_and_over_limit() {
    let s = settings(100, 0, 0.0, 0.0, true);
    assert_eq!(budget_breach(&s, &agg(99, 0.0), &agg(0, 0.0)), None);
    assert!(
        budget_breach(&s, &agg(100, 0.0), &agg(0, 0.0)).is_some(),
        "达限即超（>= 判定——停车在烧新钱之前）"
    );
    assert!(budget_breach(&s, &agg(101, 0.0), &agg(0, 0.0)).is_some());
}

#[test]
fn monthly_token_checked_after_daily() {
    let s = settings(10_000, 100, 0.0, 0.0, true);
    // 日未超、月超 → 报月维度。
    let detail = budget_breach(&s, &agg(50, 0.0), &agg(100, 0.0)).unwrap();
    assert!(detail.contains("本月"), "月超限报月维度: {detail}");
    // 日报序在前：日、月都超 → 报日维度（先到先停）。
    let detail = budget_breach(&s, &agg(10_000, 0.0), &agg(100, 0.0)).unwrap();
    assert!(detail.contains("今日"), "日报序优先: {detail}");
}

#[test]
fn cost_dimensions_fire_on_exact_limit() {
    let s = settings(0, 0, 5.0, 20.0, true);
    assert_eq!(budget_breach(&s, &agg(0, 4.99), &agg(0, 0.0)), None);
    assert!(
        budget_breach(&s, &agg(0, 5.0), &agg(0, 0.0)).is_some(),
        "日费用达限"
    );
    assert!(
        budget_breach(&s, &agg(0, 0.0), &agg(0, 20.0)).is_some(),
        "月费用达限"
    );
    // 日费用报序在月费用前。
    let detail = budget_breach(&s, &agg(0, 6.0), &agg(0, 21.0)).unwrap();
    assert!(detail.contains("今日"), "日费用报序优先: {detail}");
}

#[test]
fn warn_mode_still_reports_breach_detail() {
    // mode=warn 的分流在接线器官（不构造 AgentEvent）；纯裁决只认阈值，
    // reject 与 warn 返回同一 detail（调用方自分流）。
    let s = settings(10, 0, 0.0, 0.0, false);
    assert!(budget_breach(&s, &agg(10, 0.0), &agg(0, 0.0)).is_some());
}

#[test]
fn local_midnight_unix_secs_matches_manual_computation() {
    let now = chrono::Local::now();
    let midnight = unix_secs_at_local_midnight(now.date_naive()).unwrap();
    // 手工口径：now.timestamp() - 本地已过秒数。
    let manual = now.timestamp() - i64::from(now.time().num_seconds_from_midnight());
    assert_eq!(midnight, manual, "日窗口起点 = 本地自然日 00:00");
    // 月 1 日 00:00 必然 <= 日 00:00 且 >= 28 天前。
    let month_first = now.date_naive().with_day(1).unwrap();
    let month_midnight = unix_secs_at_local_midnight(month_first).unwrap();
    assert!(month_midnight <= midnight);
    assert!(midnight - month_midnight <= 31 * 86_400);
}
