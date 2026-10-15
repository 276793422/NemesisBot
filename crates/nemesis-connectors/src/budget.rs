//! 预算护栏（PLAN §3.4）：per-connector 滑窗限频 + 资金类单笔/日累计限额。
//!
//! 语义（资金类 fail-closed）：
//! - `max_ops_per_hour = 0` = 限频关闭；资金类检查**不受** 0 豁免。
//! - `finance_max_order_qty = 0` = 拒绝所有资金 op（不设限 = 不成交）。
//! - `finance_max_orders_per_day = 0` = 拒绝所有资金 op（同 fail-closed 族）。
//!
//! 计数器进程内存态（与既有风险限制骨架同边界；跨进程聚合挂远期）。
//! `now_ms` 由调用方传入（可测性），日期按 UTC 自然日滚转。

use std::collections::HashMap;

use parking_lot::Mutex;

const HOUR_MS: i64 = 3_600_000;

/// 预算限额配置（config `connectors.*` 键的消费形态）。
#[derive(Debug, Clone)]
pub struct Limits {
    /// per-connector 每小时执行上限（0=关）。
    pub max_ops_per_hour: u32,
    /// 资金类单笔数量上限（0=拒绝所有）。
    pub finance_max_order_qty: f64,
    /// 资金类日累计笔数上限（0=拒绝所有）。
    pub finance_max_orders_per_day: u32,
}

/// 预算裁决结果。
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetVerdict {
    Allow,
    RateLimited { used: u32, limit: u32 },
    FinanceQtyExceeded { qty: f64, limit: f64 },
    FinanceDailyOrdersExceeded { today: u32, limit: u32 },
}

impl BudgetVerdict {
    pub fn is_allow(&self) -> bool {
        matches!(self, BudgetVerdict::Allow)
    }

    /// 诚实拒绝文案（回执 / 回灌给模型用）。
    pub fn denial_message(&self) -> String {
        match self {
            BudgetVerdict::Allow => String::new(),
            BudgetVerdict::RateLimited { used, limit } => {
                format!("已达该 connector 的限频上限（本小时已执行 {used}/{limit} 次），请稍后再试")
            }
            BudgetVerdict::FinanceQtyExceeded { qty, limit } => {
                if *limit <= 0.0 {
                    "资金类操作未配置单笔数量上限（finance.max_order_qty=0，fail-closed 默认拒绝所有）——请先在 config 显式配置限额".into()
                } else {
                    format!("单笔数量 {qty} 超过资金限额上限 {limit}，已拒绝")
                }
            }
            BudgetVerdict::FinanceDailyOrdersExceeded { today, limit } => {
                if *limit == 0 {
                    "资金类操作未配置日累计笔数上限（finance.max_orders_per_day=0，fail-closed 默认拒绝所有）——请先在 config 显式配置限额".into()
                } else {
                    format!("今日资金类操作已达上限（{today}/{limit}），已拒绝（明日自动恢复）")
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct FinanceDay {
    ymd: (i32, u32, u32),
    orders: u32,
    qty: f64,
}

#[derive(Debug, Default)]
struct State {
    /// connector id → 执行时刻 ms（滑窗）。
    windows: HashMap<String, Vec<i64>>,
    finance_day: Option<FinanceDay>,
}

/// 预算护栏（进程内单例，BotService 装配）。
#[derive(Debug, Default)]
pub struct BudgetGuard {
    state: Mutex<State>,
}

impl BudgetGuard {
    /// 执行前检查（不记账）。`finance_qty` = Some 时叠加资金类检查。
    pub fn check(
        &self,
        limits: &Limits,
        connector: &str,
        now_ms: i64,
        finance_qty: Option<f64>,
    ) -> BudgetVerdict {
        let mut st = self.state.lock();
        if let Some(qty) = finance_qty {
            if limits.finance_max_order_qty <= 0.0 || qty > limits.finance_max_order_qty {
                return BudgetVerdict::FinanceQtyExceeded {
                    qty,
                    limit: limits.finance_max_order_qty,
                };
            }
            let day = ensure_day(&mut st, now_ms);
            if limits.finance_max_orders_per_day == 0
                || day.orders >= limits.finance_max_orders_per_day
            {
                return BudgetVerdict::FinanceDailyOrdersExceeded {
                    today: day.orders,
                    limit: limits.finance_max_orders_per_day,
                };
            }
        }
        if limits.max_ops_per_hour > 0 {
            let win = st.windows.entry(connector.to_string()).or_default();
            win.retain(|t| *t > now_ms - HOUR_MS);
            if win.len() as u32 >= limits.max_ops_per_hour {
                return BudgetVerdict::RateLimited {
                    used: win.len() as u32,
                    limit: limits.max_ops_per_hour,
                };
            }
        }
        BudgetVerdict::Allow
    }

    /// 执行成功后记账（滑窗推入 + 资金计数）。
    pub fn record(&self, _limits: &Limits, connector: &str, now_ms: i64, finance_qty: Option<f64>) {
        let mut st = self.state.lock();
        st.windows
            .entry(connector.to_string())
            .or_default()
            .push(now_ms);
        if let Some(qty) = finance_qty {
            let day = ensure_day(&mut st, now_ms);
            day.orders += 1;
            day.qty += qty;
        }
    }

    /// 当前 connector 滑窗内已执行次数（凭条/状态面素材）。
    pub fn rate_used(&self, connector: &str, now_ms: i64) -> u32 {
        let mut st = self.state.lock();
        let win = st.windows.entry(connector.to_string()).or_default();
        win.retain(|t| *t > now_ms - HOUR_MS);
        win.len() as u32
    }

    /// 今日资金类（笔数, 累计数量）。
    pub fn finance_today(&self, now_ms: i64) -> (u32, f64) {
        let mut st = self.state.lock();
        let day = ensure_day(&mut st, now_ms);
        (day.orders, day.qty)
    }
}

/// 取/滚转当日资金计数（UTC 自然日）。
fn ensure_day(st: &mut State, now_ms: i64) -> &mut FinanceDay {
    let date = chrono::TimeZone::timestamp_millis_opt(&chrono::Utc, now_ms)
        .single()
        .map(|dt| dt.date_naive())
        .unwrap_or_else(|| chrono::Utc::now().date_naive());
    use chrono::Datelike;
    let ymd = (date.year(), date.month(), date.day());
    match &mut st.finance_day {
        Some(d) if d.ymd == ymd => {}
        _ => {
            st.finance_day = Some(FinanceDay {
                ymd,
                orders: 0,
                qty: 0.0,
            });
        }
    }
    st.finance_day.as_mut().expect("刚已填充")
}

#[cfg(test)]
mod tests;
