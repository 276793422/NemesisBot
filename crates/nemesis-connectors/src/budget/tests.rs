//! budget 单测：reserve/commit/release 三段原子语义（R3）。
//! 既有五测迁移到 reserve 形态 + 新增 并发闸 / 共享单例 / 四路径收口。

use std::sync::Arc;
use std::thread;

use super::{BudgetGuard, BudgetVerdict, Limits};

const T0: i64 = 1_700_000_000_000;
const HOUR_MS: i64 = 3_600_000;
/// UTC 2023-11-14T23:59:59Z / 次日 00:00:01Z（跨日滚转边界）。
const DAY1_LATE: i64 = 1_700_006_399_000;
const DAY2_EARLY: i64 = 1_700_006_401_000;

fn limits(hour: u32, qty: f64, day: u32) -> Limits {
    Limits {
        max_ops_per_hour: hour,
        finance_max_order_qty: qty,
        finance_max_orders_per_day: day,
    }
}

#[test]
fn rate_limit_window_and_expiry_recovery() {
    let g = BudgetGuard::default();
    let lm = limits(3, f64::INFINITY, 0);
    for i in 0..3 {
        assert!(
            g.reserve(&lm, "paper", T0 + i, None).is_ok(),
            "第 {i} 次占用应在限额内"
        );
    }
    assert_eq!(
        g.reserve(&lm, "paper", T0, None),
        Err(BudgetVerdict::RateLimited { used: 3, limit: 3 })
    );
    // 滑窗外（>1h）恢复
    assert!(g.reserve(&lm, "paper", T0 + HOUR_MS + 1, None).is_ok());
    // 独立 connector 不互相挤占
    assert!(g.reserve(&lm, "gitea", T0, None).is_ok());
}

#[test]
fn finance_qty_fail_closed() {
    let g = BudgetGuard::default();
    // qty 上限 0 = 拒绝所有资金 op（不设限 = 不成交）
    let lm = limits(0, 0.0, 0);
    assert_eq!(
        g.reserve(&lm, "paper", T0, Some(1.0)),
        Err(BudgetVerdict::FinanceQtyExceeded {
            qty: 1.0,
            limit: 0.0
        })
    );
    // 显式上限存在时，超额拒绝
    let lm = limits(0, 100.0, 10);
    assert_eq!(
        g.reserve(&lm, "paper", T0, Some(100.5)),
        Err(BudgetVerdict::FinanceQtyExceeded {
            qty: 100.5,
            limit: 100.0
        })
    );
    // 恰好等于上限放行
    assert!(g.reserve(&lm, "paper", T0, Some(100.0)).is_ok());
}

#[test]
fn finance_daily_orders_and_midnight_reset() {
    let g = BudgetGuard::default();
    let lm = limits(0, 100.0, 2);
    assert!(g.reserve(&lm, "paper", T0, Some(1.0)).is_ok());
    assert!(g.reserve(&lm, "paper", T0 + 1, Some(2.0)).is_ok());
    assert_eq!(
        g.reserve(&lm, "paper", T0 + 2, Some(3.0)),
        Err(BudgetVerdict::FinanceDailyOrdersExceeded { today: 2, limit: 2 })
    );
    assert_eq!(g.finance_today(T0 + 2), (2, 3.0));
    // 跨 UTC 自然日滚转
    assert!(
        g.reserve(&lm, "paper", T0 + 24 * HOUR_MS, Some(5.0))
            .is_ok()
    );
    assert_eq!(g.finance_today(T0 + 24 * HOUR_MS), (1, 5.0));
}

#[test]
fn finance_daily_zero_fail_closed_with_config_hint() {
    let g = BudgetGuard::default();
    let lm = limits(0, 100.0, 0);
    assert_eq!(
        g.reserve(&lm, "paper", T0, Some(1.0)),
        Err(BudgetVerdict::FinanceDailyOrdersExceeded { today: 0, limit: 0 })
    );
    // 拒绝文案提示显式配置（fail-closed 默认拒绝，不静默当「未超额」）
    assert!(
        g.reserve(&lm, "paper", T0, Some(1.0))
            .unwrap_err()
            .denial_message()
            .contains("显式配置")
    );
}

#[test]
fn rate_denial_message_mentions_quota() {
    let g = BudgetGuard::default();
    let lm = limits(1, f64::INFINITY, 0);
    assert!(g.reserve(&lm, "paper", T0, None).is_ok());
    let msg = g
        .reserve(&lm, "paper", T0 + 1, None)
        .unwrap_err()
        .denial_message();
    assert!(msg.contains("限频"), "文案应提及限频：{msg}");
    assert!(msg.contains("1/1"), "文案应带已用/上限：{msg}");
}

// --- R3 新增：原子占用 / 幂等收口 / 共享单例 ---------------------------------

#[test]
fn concurrent_reserves_respect_limit() {
    // limit=1 + 20 线程并发 reserve：恰好 1 个 Ok——check/record 两段式的
    // 并发旁路（裁决与记账之间窗口）被 reserve 单锁原子占用封死。
    let g = Arc::new(BudgetGuard::default());
    let lm = limits(1, f64::INFINITY, 0);
    let results: Vec<_> = thread::scope(|s| {
        let handles: Vec<_> = (0..20)
            .map(|i| {
                let g = g.clone();
                let lm = lm.clone();
                s.spawn(move || g.reserve(&lm, "paper", T0 + i, None).is_ok())
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(results.iter().filter(|ok| **ok).count(), 1);
}

/// 远程 outcome → 收口动作的四路径账目语义（R3 决策表的账本侧验证）。
#[test]
fn outcome_to_settlement_four_paths() {
    // ① Done → commit：记账保留，release 无效
    let g = BudgetGuard::default();
    let lm = limits(1, 100.0, 10);
    let r = g.reserve(&lm, "paper", T0, Some(10.0)).unwrap();
    g.commit(&r);
    g.release(&r); // commit 后 release = 无操作（既成事实不回吐）
    assert_eq!(g.finance_today(T0), (1, 10.0));
    assert_eq!(g.rate_used("paper", T0), 1);

    // ② 本地 DispatchRejected / Failed / Err → release：额度回吐可重试
    let g = BudgetGuard::default();
    let r = g.reserve(&lm, "paper", T0, Some(10.0)).unwrap();
    g.release(&r);
    assert_eq!(g.finance_today(T0), (0, 0.0));
    assert_eq!(g.rate_used("paper", T0), 0);
    // 回吐后可再次占用
    assert!(g.reserve(&lm, "paper", T0, Some(10.0)).is_ok());

    // ③ 远程 f() Err（超时/传输错误）→ 保留账目（不收口）：
    //    结果未知——外部写可能已生效，放行额度 = 绕过限额。
    //    未收口的占用自然计入限额（下一条断言在 ③ 的 guard 上继续）。
    let g = BudgetGuard::default();
    assert!(g.reserve(&lm, "paper", T0, Some(10.0)).is_ok());
    assert_eq!(g.finance_today(T0), (1, 10.0));
    assert_eq!(g.rate_used("paper", T0), 1);

    // ④ 其他 outcome（对端明确回执 denied/error/budget_blocked）→ release：
    //    同 ②，由 release 幂等性保证——已在 ② 覆盖，不重复。
}

#[test]
fn provisional_counts_toward_limit_before_settlement() {
    // 预备占用未收口即计入限额：并发第二请求在第一请求执行期间被拒
    // （日限 1：第一笔尚未执行完，第二笔就得排队）。
    let g = BudgetGuard::default();
    let lm = limits(2, 100.0, 1);
    assert!(g.reserve(&lm, "paper", T0, Some(10.0)).is_ok());
    assert_eq!(
        g.reserve(&lm, "paper", T0 + 1, Some(10.0)),
        Err(BudgetVerdict::FinanceDailyOrdersExceeded { today: 1, limit: 1 })
    );
    assert_eq!(g.finance_today(T0 + 1), (1, 10.0));
}

#[test]
fn shared_budget_guard_same_workspace_same_arc() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let a = super::shared_budget_guard(&ws);
    let b = super::shared_budget_guard(&ws);
    assert!(Arc::ptr_eq(&a, &b), "同 workspace 必须共享同一 Arc");
    // 计数跨实例可见（agent/loop 重建不清零）
    let lm = limits(1, f64::INFINITY, 0);
    assert!(a.reserve(&lm, "paper", T0, None).is_ok());
    assert!(b.reserve(&lm, "paper", T0 + 1, None).is_err());
    // 不同 workspace = 不同实例
    let other = super::shared_budget_guard(&dir.path().join("other"));
    assert!(!Arc::ptr_eq(&a, &other));
    // 不存在的目录回落原路径作键，不 panic
    let ghost = super::shared_budget_guard(&dir.path().join("ghost"));
    assert!(!Arc::ptr_eq(&a, &ghost));
}

#[test]
fn release_across_day_boundary_does_not_refund_new_day() {
    let g = BudgetGuard::default();
    let lm = limits(0, 100.0, 10);
    // day1 尾占用；随后滚到 day2 且新占用一笔（FinanceDay 已滚转为 day2）
    let r1 = g.reserve(&lm, "paper", DAY1_LATE, Some(10.0)).unwrap();
    let r2 = g.reserve(&lm, "paper", DAY2_EARLY, Some(5.0)).unwrap();
    assert_eq!(g.finance_today(DAY2_EARLY), (1, 5.0));
    // 释放 day1 的旧占用：day2 的账不得被扣（ymd 对比不匹配 → 不回吐）
    g.release(&r1);
    assert_eq!(g.finance_today(DAY2_EARLY), (1, 5.0));
    // day2 自身占用的正常收口仍生效
    g.release(&r2);
    assert_eq!(g.finance_today(DAY2_EARLY), (0, 0.0));
}

#[test]
fn repeated_settlement_is_idempotent() {
    let g = BudgetGuard::default();
    let lm = limits(3, 100.0, 10);
    let r = g.reserve(&lm, "paper", T0, Some(10.0)).unwrap();
    g.release(&r);
    g.release(&r); // 二次 release 静默
    assert_eq!(g.finance_today(T0), (0, 0.0));
    let r2 = g.reserve(&lm, "paper", T0, Some(10.0)).unwrap();
    g.commit(&r2);
    g.commit(&r2); // 二次 commit 静默
    assert_eq!(g.finance_today(T0), (1, 10.0));
    // 未知 id 收口 = 无操作不 panic
    g.commit(&super::Reservation { id: 0 });
}

#[test]
fn rate_limit_off_still_prunes_window_ticks() {
    // 限频关闭（max_ops_per_hour=0）也要清过期滑窗 tick：记账是无条件入窗
    // 的，retain 若只在限频开启时跑，长驻 gateway 的 windows vec 无界增长
    //（2026-10-16 四轮复查）。修复后：下一次 reserve 扫掉全部过期 tick。
    let g = BudgetGuard::default();
    let lm = limits(0, f64::INFINITY, 0);
    for i in 0..5 {
        assert!(g.reserve(&lm, "paper", T0 + i, None).is_ok());
    }
    // 2h 后再占用：旧 tick 全部滑出窗口被扫，窗内只剩新 tick。
    assert!(g.reserve(&lm, "paper", T0 + 2 * HOUR_MS, None).is_ok());
    let st = g.state.lock();
    let win = st.windows.get("paper").unwrap();
    assert_eq!(win.len(), 1, "过期 tick 必须被扫除，不无限堆积");
}
