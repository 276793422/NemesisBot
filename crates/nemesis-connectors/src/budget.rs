//! 预算护栏（PLAN §3.4 + R3 整改）：per-connector 滑窗限频 + 资金类
//! 单笔/日累计限额，**reserve/commit/release 三段原子占用**。
//!
//! 语义（资金类 fail-closed）：
//! - `max_ops_per_hour = 0` = 限频关闭；资金类检查**不受** 0 豁免。
//! - `finance_max_order_qty = 0` = 拒绝所有资金 op（不设限 = 不成交）。
//! - `finance_max_orders_per_day = 0` = 拒绝所有资金 op（同 fail-closed 族）。
//!
//! R3 原子占用（修复旧 check→await→record 的并发旁路与执行即失账）：
//! - [`BudgetGuard::reserve`]：单锁内完成全部限额检查并**立即记账**（预备
//!   账目计入限额）——裁决与外部执行之间的窗口不再被并发请求钻空；
//! - [`BudgetGuard::commit`]：预备转正式（记账保留；幂等，重复收口不重复计数）；
//! - [`BudgetGuard::release`]：仅「确定未产生外部效果」的失败路径调用
//!   （幂等；commit 后调用 = 无操作，既成事实不回吐）；
//! - **超时/传输错误 = 结果未知：不 release**（账目保留）+ 调用方 WARN
//!   ——外部写可能已生效，放行额度等于绕过限额。
//!
//! 计数器进程内存态；跨实例经 [`shared_budget_guard`] 按 workspace 共享
//! （agent/loop 重建不清零）。`now_ms` 由调用方传入（可测性），日期按
//! UTC 自然日滚转。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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

/// 预备占用句柄（[`BudgetGuard::reserve`] 产物；`commit`/`release` 按 id
/// 收口，二者幂等）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub(crate) id: u64,
}

#[derive(Debug, Default)]
struct FinanceDay {
    ymd: (i32, u32, u32),
    orders: u32,
    qty: f64,
}

/// 滑窗条目：执行时刻 + 归属预备占用 id（release 按 id 摘除）。
#[derive(Debug)]
struct WindowTick {
    at_ms: i64,
    res: u64,
}

/// 预备占用内容（收口/扫尾所需的最小投影）。
#[derive(Debug)]
struct Provisional {
    connector: String,
    finance_qty: Option<f64>,
    at_ms: i64,
}

#[derive(Debug, Default)]
struct State {
    /// connector id → 滑窗（预备 + 已提交统一入列；release 按 res id 摘除）。
    windows: HashMap<String, Vec<WindowTick>>,
    finance_day: Option<FinanceDay>,
    /// 预备占用表：res id → 占用内容。commit/release 摘除；24h 扫尾防泄漏
    /// （调用方崩溃/超长路径漏收口时，滑窗一小时滑出、日计数跨日滚转，
    /// 表项只剩内存占用）。
    provisional: HashMap<u64, Provisional>,
    next_id: u64,
}

/// 预算护栏实例。跨实例经 [`shared_budget_guard`] 按 workspace 共享
/// （同进程内 agent/loop 重建共用同一 Arc）。
#[derive(Debug, Default)]
pub struct BudgetGuard {
    state: Mutex<State>,
}

/// 预备占用超时扫尾窗口（远大于最长执行路径——RPC 上限 2h，24h 宽裕）。
const PROVISIONAL_TTL_MS: i64 = 24 * HOUR_MS;

impl BudgetGuard {
    /// 执行前原子占用：单锁内检查 小时滑窗 + 资金限额，**通过即写入预备
    /// 账目**（计入限额）。拒绝时返回 [`BudgetVerdict`]（调用方出凭条/文案）。
    pub fn reserve(
        &self,
        limits: &Limits,
        connector: &str,
        now_ms: i64,
        finance_qty: Option<f64>,
    ) -> Result<Reservation, BudgetVerdict> {
        let mut st = self.state.lock();
        // 泄漏扫尾：超时未收口的预备占用过期摘除（纯内存卫生；对应额度
        // 已随滑窗/日滚转自然失效）。
        st.provisional
            .retain(|_, p| p.at_ms > now_ms - PROVISIONAL_TTL_MS);

        if let Some(qty) = finance_qty {
            if limits.finance_max_order_qty <= 0.0 || qty > limits.finance_max_order_qty {
                return Err(BudgetVerdict::FinanceQtyExceeded {
                    qty,
                    limit: limits.finance_max_order_qty,
                });
            }
            let day = ensure_day(&mut st, now_ms);
            if limits.finance_max_orders_per_day == 0
                || day.orders >= limits.finance_max_orders_per_day
            {
                return Err(BudgetVerdict::FinanceDailyOrdersExceeded {
                    today: day.orders,
                    limit: limits.finance_max_orders_per_day,
                });
            }
        }
        // 滑窗扫尾无条件执行：tick 记账（下方 push）不因限频关闭而缺席，
        // retain 若只在限频开启时跑，`max_ops_per_hour=0` 的 connector
        // windows 会无界堆积（长驻 gateway 内存卫生）。
        let win = st.windows.entry(connector.to_string()).or_default();
        win.retain(|t| t.at_ms > now_ms - HOUR_MS);
        if limits.max_ops_per_hour > 0 && win.len() as u32 >= limits.max_ops_per_hour {
            return Err(BudgetVerdict::RateLimited {
                used: win.len() as u32,
                limit: limits.max_ops_per_hour,
            });
        }
        // 通过 → 立即记账（预备计入限额）：滑窗 + 资金日计数同步落。
        let id = st.next_id;
        st.next_id += 1;
        st.windows
            .entry(connector.to_string())
            .or_default()
            .push(WindowTick {
                at_ms: now_ms,
                res: id,
            });
        if let Some(qty) = finance_qty {
            let day = ensure_day(&mut st, now_ms);
            day.orders += 1;
            day.qty += qty;
        }
        st.provisional.insert(
            id,
            Provisional {
                connector: connector.to_string(),
                finance_qty,
                at_ms: now_ms,
            },
        );
        Ok(Reservation { id })
    }

    /// 预备转正式（记账保留）。幂等：重复 commit / 未知 id 静默忽略——
    /// 账目在 reserve 时一次落定，重复收口不重复计数。
    pub fn commit(&self, res: &Reservation) {
        self.state.lock().provisional.remove(&res.id);
    }

    /// 扣减预备账目（仅「确定未产生外部效果」的失败路径调用）。幂等：已
    /// commit / 未知 id 静默忽略——commit 后账目是既成事实，release 不回吐。
    pub fn release(&self, res: &Reservation) {
        let mut st = self.state.lock();
        let Some(p) = st.provisional.remove(&res.id) else {
            return;
        };
        if let Some(win) = st.windows.get_mut(&p.connector)
            && let Some(pos) = win.iter().position(|t| t.res == res.id)
        {
            win.remove(pos);
        }
        if let Some(qty) = p.finance_qty {
            // 只在占用当日回吐：跨日滚转后新 FinanceDay 没吃过这笔，不能扣。
            let reserved_ymd = day_ymd_of(p.at_ms);
            match &mut st.finance_day {
                Some(d) if d.ymd == reserved_ymd => {
                    d.orders = d.orders.saturating_sub(1);
                    d.qty = (d.qty - qty).max(0.0);
                }
                _ => {}
            }
        }
    }

    /// 当前 connector 滑窗内已执行次数（凭条/状态面素材）。
    pub fn rate_used(&self, connector: &str, now_ms: i64) -> u32 {
        let mut st = self.state.lock();
        let win = st.windows.entry(connector.to_string()).or_default();
        win.retain(|t| t.at_ms > now_ms - HOUR_MS);
        win.len() as u32
    }

    /// 今日资金类（笔数, 累计数量）。
    pub fn finance_today(&self, now_ms: i64) -> (u32, f64) {
        let mut st = self.state.lock();
        let day = ensure_day(&mut st, now_ms);
        (day.orders, day.qty)
    }
}

/// workspace 共享预算单例（R3）：进程级 `Mutex<HashMap>` 按 canonical
/// workspace 路径键控——agent/loop 重建走同一 `Arc`，滑窗与日累计不清零
/// （旧 per-instance `BudgetGuard::default()` 让 agent 重启即重置额度，
/// 限额形同虚设）。canonicalize 失败（目录不存在等）回落原路径作键。
pub fn shared_budget_guard(workspace: &Path) -> Arc<BudgetGuard> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<BudgetGuard>>>> = OnceLock::new();
    let map = REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    let key = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    map.lock()
        .entry(key)
        .or_insert_with(|| Arc::new(BudgetGuard::default()))
        .clone()
}

/// `now_ms` → (年, 月, 日)（UTC）。
fn day_ymd_of(now_ms: i64) -> (i32, u32, u32) {
    use chrono::Datelike;
    chrono::TimeZone::timestamp_millis_opt(&chrono::Utc, now_ms)
        .single()
        .map(|dt| {
            let d = dt.date_naive();
            (d.year(), d.month(), d.day())
        })
        .unwrap_or_else(|| {
            let d = chrono::Utc::now().date_naive();
            (d.year(), d.month(), d.day())
        })
}

/// 取/滚转当日资金计数（UTC 自然日）。
fn ensure_day(st: &mut State, now_ms: i64) -> &mut FinanceDay {
    let ymd = day_ymd_of(now_ms);
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
