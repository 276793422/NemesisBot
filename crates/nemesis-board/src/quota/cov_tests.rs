// quota.rs 覆盖率补充测试（Default 快照 28-34 / message+error_code 全变体
// 55/57 / can_post 小时额度耗尽臂 202-204）。

use super::*;

/// Default 快照值锁定（28-34 的三个初始化行）。
#[test]
fn default_config_snapshot() {
    let cfg = QuotaConfig::default();
    assert_eq!(cfg.max_agent_turns_per_thread, 8);
    assert_eq!(cfg.hourly_budget_per_node, 20);
    assert_eq!(cfg.rate_limit_per_min, 12);
}

/// 三种拒绝的人读文案（55/57 的两个未覆盖臂 + error_code 分组）。
#[test]
fn denied_messages_and_error_codes_cover_all_variants() {
    assert!(
        QuotaDenied::ThreadQuotaExhausted
            .message()
            .contains("agent-turn quota exhausted")
    );
    assert!(
        QuotaDenied::HourlyBudgetExhausted
            .message()
            .contains("hourly discussion budget exhausted")
    );
    assert!(QuotaDenied::RateLimited.message().contains("rate limited"));

    assert_eq!(
        QuotaDenied::ThreadQuotaExhausted.error_code(),
        "quota_exhausted"
    );
    assert_eq!(
        QuotaDenied::HourlyBudgetExhausted.error_code(),
        "quota_exhausted"
    );
    assert_eq!(QuotaDenied::RateLimited.error_code(), "rate_limited");
}

/// can_post 的小时额度闸（202 的 return + 204 的块收尾）：小时额度记满
/// 后 can_post 拒绝；窗口滑走后放行。
#[test]
fn can_post_hourly_budget_exhausted() {
    let ledger = QuotaLedger::new(QuotaConfig {
        rate_limit_per_min: 0,
        hourly_budget_per_node: 1,
        max_agent_turns_per_thread: 0,
    });
    // 消耗掉小时额度（rate/thread 闸关闭，只走小时账）。
    ledger.try_consume_post("t1", "node-a", 1_000).unwrap();
    // 小时窗口内 → 拒绝。
    assert_eq!(
        ledger.can_post("node-a", 1_500),
        Err(QuotaDenied::HourlyBudgetExhausted)
    );
    // 窗口外（>3600s）→ 滑动淘汰后放行。
    assert!(ledger.can_post("node-a", 5_000).is_ok());
}
