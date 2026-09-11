//! 额度记账单测（确定时钟注入，无真实等待）。

use super::*;

/// 线程额度：扣满后拒绝，turns_left 随消耗递减。
#[test]
fn test_thread_quota_exhaustion() {
    let ledger = QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: 2,
        hourly_budget_per_node: 0,
        rate_limit_per_min: 0,
    });
    assert_eq!(ledger.turns_left("t1"), 2);
    assert_eq!(ledger.try_consume_post("t1", "node-b", 1000).unwrap(), 1);
    assert_eq!(ledger.try_consume_post("t1", "node-c", 1001).unwrap(), 0);
    // 扣完：任何节点再发都被拒（额度握在 master 手里）。
    assert_eq!(
        ledger.try_consume_post("t1", "node-b", 1002),
        Err(QuotaDenied::ThreadQuotaExhausted)
    );
    assert_eq!(ledger.turns_left("t1"), 0);
    // 其他线程互不影响。
    assert_eq!(ledger.try_consume_post("t2", "node-b", 1003).unwrap(), 1);
    // 0 = 不限。
    let unlimited = QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: 0,
        hourly_budget_per_node: 0,
        rate_limit_per_min: 0,
    });
    for i in 0..50 {
        assert!(unlimited.try_consume_post("t", "n", 1000 + i).is_ok());
    }
}

/// 节点小时额度：滑窗内累计到上限拒绝，窗口滑出后恢复。
#[test]
fn test_hourly_budget_sliding_window() {
    let ledger = QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: 0,
        hourly_budget_per_node: 3,
        rate_limit_per_min: 0,
    });
    for i in 0..3 {
        assert!(ledger.try_consume_post("t", "node-b", 1000 + i).is_ok());
    }
    assert_eq!(
        ledger.try_consume_post("t", "node-b", 1010),
        Err(QuotaDenied::HourlyBudgetExhausted)
    );
    // 其他节点不受影响。
    assert!(ledger.try_consume_post("t", "node-c", 1010).is_ok());
    // 窗口滑出（>3600s 前 → 老时间戳过期）→ 恢复。
    assert!(ledger.try_consume_post("t", "node-b", 1000 + 3601).is_ok());
    // can_post 预检：窗口内攒满 3 条 → 拒绝。
    assert!(ledger.try_consume_post("t", "node-b", 4611).is_ok());
    assert!(ledger.try_consume_post("t", "node-b", 4612).is_ok());
    assert_eq!(
        ledger.can_post("node-b", 4613),
        Err(QuotaDenied::HourlyBudgetExhausted)
    );
}

/// 上行限速：每分钟窗口。
#[test]
fn test_rate_limit_per_minute() {
    let ledger = QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: 0,
        hourly_budget_per_node: 0,
        rate_limit_per_min: 2,
    });
    assert!(ledger.try_consume_post("t", "node-b", 1000).is_ok());
    assert!(ledger.try_consume_post("t", "node-b", 1030).is_ok());
    assert_eq!(
        ledger.try_consume_post("t", "node-b", 1050),
        Err(QuotaDenied::RateLimited)
    );
    // 60s 后窗口滑出恢复；恢复瞬间两条仍在窗内，can_post 预检如实说「限」。
    assert!(ledger.try_consume_post("t", "node-b", 1061).is_ok());
    assert_eq!(
        ledger.can_post("node-b", 1062),
        Err(QuotaDenied::RateLimited)
    );
    assert_eq!(ledger.can_post("node-b", 1092), Ok(()));
}

/// 三闸全过才扣账：被拒绝时不留副作用（不发半条账）。
#[test]
fn test_denial_leaves_no_side_effect() {
    let ledger = QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: 5,
        hourly_budget_per_node: 2,
        rate_limit_per_min: 10,
    });
    // 消耗到小时额度上限。
    assert!(ledger.try_consume_post("t", "node-b", 1000).is_ok());
    assert!(ledger.try_consume_post("t", "node-b", 1001).is_ok());
    // 此时线程额度 3 剩，但小时额度满 → 拒绝，且线程额度不得被扣。
    assert_eq!(
        ledger.try_consume_post("t", "node-b", 1002),
        Err(QuotaDenied::HourlyBudgetExhausted)
    );
    assert_eq!(ledger.turns_left("t"), 3);
    // 拒绝原因映射信封错误码。
    assert_eq!(
        QuotaDenied::HourlyBudgetExhausted.error_code(),
        "quota_exhausted"
    );
    assert_eq!(QuotaDenied::RateLimited.error_code(), "rate_limited");
}

/// live provider 语义（G10 回归）：改额度即时生效，无需重建台账——
/// gateway 接 ConfigStore live 句柄后，config.json 热改额度不重启生效。
#[test]
fn test_live_provider_quota_change_applies_without_rebuild() {
    let cap = std::sync::Arc::new(std::sync::Mutex::new(2u32));
    let cap_for_provider = cap.clone();
    let ledger = QuotaLedger::with_provider(move || QuotaConfig {
        max_agent_turns_per_thread: *cap_for_provider.lock().unwrap(),
        hourly_budget_per_node: 0,
        rate_limit_per_min: 0,
    });
    // cap=2：第 3 条拒绝。
    assert_eq!(ledger.try_consume_post("t1", "node-b", 1000).unwrap(), 1);
    assert_eq!(ledger.try_consume_post("t1", "node-b", 1001).unwrap(), 0);
    assert_eq!(
        ledger.try_consume_post("t1", "node-b", 1002),
        Err(QuotaDenied::ThreadQuotaExhausted)
    );
    // 热调到 4：同一线程立即按新上限放行。
    *cap.lock().unwrap() = 4;
    assert_eq!(ledger.current_config().max_agent_turns_per_thread, 4);
    // used 已是 2：按新上限扣满（剩余=4-used 递减到 0），再发被拒。
    assert_eq!(ledger.try_consume_post("t1", "node-b", 1003).unwrap(), 1);
    assert_eq!(ledger.try_consume_post("t1", "node-b", 1004).unwrap(), 0);
    assert_eq!(ledger.turns_left("t1"), 0);
    assert_eq!(
        ledger.try_consume_post("t1", "node-b", 1005),
        Err(QuotaDenied::ThreadQuotaExhausted)
    );
    // 热调 0 = 不限。
    *cap.lock().unwrap() = 0;
    assert_eq!(ledger.turns_left("t1"), u32::MAX);
}
