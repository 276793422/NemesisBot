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

/// BUG-20261114-023 判别性回归：闸关闭（0=关）时滑窗扫尾（retain）不得
/// 缺席——push 恒在而 retain 只在闸开时跑，关闭配置下 minute/hour 账本
/// 只进不出无界堆积（对齐 connectors/budget.rs 同族纪律「滑窗扫尾无条件
/// 执行」）。61s 步进跨越多个窗口后，minute 内只允许残留最后 60s 内的
/// 条目；旧形态（retain 在闸内）下全部 N 条滞留 → 本测试红。
#[test]
fn disabled_gates_still_prune_sliding_windows() {
    let ledger = QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: 0,
        hourly_budget_per_node: 0,
        rate_limit_per_min: 0,
    });
    // 30 次发言、每次 +61s：跨 29 个分钟窗。全部应 Ok（双闸全关）。
    let n = 30;
    for k in 0..n {
        let now = k * 61;
        ledger
            .try_consume_post("thread-1", "node-a", now)
            .unwrap_or_else(|e| panic!("第 {k} 次发言被拒（双闸全关不应拒绝）: {e:?}"));
    }
    let final_now = (n - 1) * 61;
    let inner = ledger.lock();
    let node = inner.nodes.get("node-a").expect("node-a 应已入账");
    // 剪枝后 minute 只剩窗口内条目（61s 步进 ⇒ 至多最后 1 条）。
    assert!(
        node.minute.len() <= 1,
        "minute 账本未随时间剪枝（BUG-20261114-023）：{} 条滞留（应 ≤1）",
        node.minute.len()
    );
    // hour 窗口同理（61s×30 ≈ 30 分钟，全在 1h 内——hour 剪枝后应保留全部，
    // 这里只验证「unbounded的是 minute」这一失守面；hour 失守面单独压测：
    // 步进改为 3601s 时 hour 同样只允许残留 1 条）。
    let ledger_h = QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: 0,
        hourly_budget_per_node: 0,
        rate_limit_per_min: 0,
    });
    let m = 12;
    for k in 0..m {
        let now = k * 3601;
        ledger_h
            .try_consume_post("thread-2", "node-b", now)
            .unwrap();
    }
    let inner_h = ledger_h.lock();
    let node_h = inner_h.nodes.get("node-b").expect("node-b 应已入账");
    assert!(
        node_h.hour.len() <= 1,
        "hour 账本未随时间剪枝（BUG-20261114-023）：{} 条滞留（应 ≤1）",
        node_h.hour.len()
    );
    let _ = final_now;
}
