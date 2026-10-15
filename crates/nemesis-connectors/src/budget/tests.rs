use super::*;

const LIMITS: Limits = Limits {
    max_ops_per_hour: 3,
    finance_max_order_qty: 100.0,
    finance_max_orders_per_day: 2,
};

#[test]
fn rate_limit_window_and_expiry_recovery() {
    let g = BudgetGuard::default();
    let t0 = 1_700_000_000_000i64;

    for i in 0..3 {
        let v = g.check(&LIMITS, "c", t0 + i, None);
        assert!(v.is_allow(), "第 {} 次应放行", i + 1);
        g.record(&LIMITS, "c", t0 + i, None);
    }
    // 第 4 次撞限
    let v = g.check(&LIMITS, "c", t0 + 3, None);
    match v {
        BudgetVerdict::RateLimited { used, limit } => {
            assert_eq!((used, limit), (3, 3));
        }
        other => panic!("expect RateLimited, got {other:?}"),
    }
    assert!(!g.check(&LIMITS, "c", t0 + 3, None).is_allow());

    // 一小时后窗口滑出 → 恢复
    assert!(g.check(&LIMITS, "c", t0 + HOUR_MS + 1, None).is_allow());
    // 独立 connector 不受影响
    assert!(g.check(&LIMITS, "other", t0, None).is_allow());
    // max_ops_per_hour=0 = 限频关
    let off = Limits {
        max_ops_per_hour: 0,
        ..LIMITS
    };
    for i in 0..10 {
        assert!(g.check(&off, "c", t0 + i, None).is_allow());
    }
}

#[test]
fn finance_qty_fail_closed() {
    let g = BudgetGuard::default();
    let t0 = 1_700_000_000_000i64;

    // 未配置（0）= 拒绝所有
    let no_qty = Limits {
        finance_max_order_qty: 0.0,
        ..LIMITS
    };
    let v = g.check(&no_qty, "pb", t0, Some(1.0));
    match &v {
        BudgetVerdict::FinanceQtyExceeded { qty, limit } => {
            assert_eq!((*qty, *limit), (1.0, 0.0));
            let msg = v.denial_message();
            assert!(
                msg.contains("fail-closed"),
                "未配置文案要点名 fail-closed: {msg}"
            );
            assert!(msg.contains("finance.max_order_qty"));
        }
        other => panic!("expect FinanceQtyExceeded, got {other:?}"),
    }

    // 超过限额
    let v = g.check(&LIMITS, "pb", t0, Some(101.0));
    assert!(matches!(v, BudgetVerdict::FinanceQtyExceeded { qty, .. } if qty == 101.0));
    assert!(v.denial_message().contains("超过"));

    // 边界 = 恰好限额放行
    assert!(g.check(&LIMITS, "pb", t0, Some(100.0)).is_allow());
    // 无资金类检查不受影响
    assert!(g.check(&LIMITS, "pb", t0, None).is_allow());
}

#[test]
fn finance_daily_orders_and_midnight_reset() {
    let g = BudgetGuard::default();
    // t0 = 2023-11-14 UTC；+24h = 次日
    let t0 = 1_700_000_000_000i64;
    let next_day = t0 + 24 * HOUR_MS;

    assert!(g.check(&LIMITS, "pb", t0, Some(10.0)).is_allow());
    g.record(&LIMITS, "pb", t0, Some(10.0));
    assert!(g.check(&LIMITS, "pb", t0 + 1, Some(10.0)).is_allow());
    g.record(&LIMITS, "pb", t0 + 1, Some(10.0));

    // 第 3 笔撞日累计
    let v = g.check(&LIMITS, "pb", t0 + 2, Some(10.0));
    match &v {
        BudgetVerdict::FinanceDailyOrdersExceeded { today, limit } => {
            assert_eq!((*today, *limit), (2, 2));
        }
        other => panic!("expect FinanceDailyOrdersExceeded, got {other:?}"),
    }
    let msg = v.denial_message();
    assert!(msg.contains("今日"), "{msg}");

    // 跨日重置（记录时间戳用次日，滑窗 retain 不误伤日计数）
    assert!(g.check(&LIMITS, "pb", next_day, Some(10.0)).is_allow());
    let (orders, qty) = g.finance_today(next_day);
    assert_eq!((orders, qty), (0, 0.0), "UTC 日滚转后计数清零");
}

#[test]
fn finance_daily_zero_fail_closed_with_config_hint() {
    let g = BudgetGuard::default();
    let t0 = 1_700_000_000_000i64;
    let no_daily = Limits {
        finance_max_orders_per_day: 0,
        ..LIMITS
    };
    let v = g.check(&no_daily, "pb", t0, Some(1.0));
    assert!(matches!(
        v,
        BudgetVerdict::FinanceDailyOrdersExceeded { limit: 0, .. }
    ));
    let msg = v.denial_message();
    assert!(msg.contains("finance.max_orders_per_day"), "{msg}");
}

#[test]
fn rate_denial_message_mentions_quota() {
    let g = BudgetGuard::default();
    let t0 = 1_700_000_000_000i64;
    for i in 0..3 {
        g.record(&LIMITS, "c", t0 + i, None);
    }
    let v = g.check(&LIMITS, "c", t0 + 3, None);
    let msg = v.denial_message();
    assert!(msg.contains("限频"), "{msg}");

    let (used, _) = (g.rate_used("c", t0 + 3), ());
    assert_eq!(used, 3);
}
