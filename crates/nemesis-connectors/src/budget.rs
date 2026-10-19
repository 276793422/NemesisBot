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
//!
//! M3（2026-10-19）真跨进程限额：`BudgetGuard` 可挂**持久化存储**
//! （`<workspace>/data/connectors_budget.json`）——`shared_budget_guard`
//! 自动挂 store，全部读写经「锁文件互斥 → 读盘 → 改 → 原子写回」漏斗，
//! 多 gateway 进程 / 多实例共享同一份账目：
//! - 互斥用**锁文件**（`<store>.lock`，`std::fs::File::lock` 阻塞独占）而
//!   非数据文件自身——数据文件经 `write_file_atomic` tmp+rename 原子换装，
//!   锁句柄存活期与数据文件 inode 解耦（Windows rename-over-open 上限）。
//! - OS 文件锁进程死亡自动释放 = 无陈锁恢复问题；锁持有窗口是毫秒级
//!   读写，阻塞等待可接受（connectors 调用本身低频）。
//! - 存储损坏（半截 JSON 等）= **重置 fail-open**：预算是成本熔断不是
//!   安全边界，宁可重开账也不把 agent 锁死在坏文件上（WARN 留痕）。
//! - 锁文件/写回失败（目录不可写、AV 瞬时占用等）= 回退进程内存态 +
//!   WARN（诚实降级，本次记账跨进程不可见）。
//! - `BudgetGuard::default()`（无 store）行为不变：纯内存，单测/工具路径。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Default, Serialize, Deserialize)]
struct FinanceDay {
    ymd: (i32, u32, u32),
    orders: u32,
    qty: f64,
}

/// 滑窗条目：执行时刻 + 归属预备占用 id（release 按 id 摘除）。
#[derive(Debug, Serialize, Deserialize)]
struct WindowTick {
    at_ms: i64,
    res: u64,
}

/// 预备占用内容（收口/扫尾所需的最小投影）。
#[derive(Debug, Serialize, Deserialize)]
struct Provisional {
    connector: String,
    finance_qty: Option<f64>,
    at_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// connector id → 滑窗（预备 + 已提交统一入列；release 按 res id 摘除）。
    #[serde(default)]
    windows: HashMap<String, Vec<WindowTick>>,
    #[serde(default)]
    finance_day: Option<FinanceDay>,
    /// 预备占用表：res id → 占用内容。commit/release 摘除；24h 扫尾防泄漏
    /// （调用方崩溃/超长路径漏收口时，滑窗一小时滑出、日计数跨日滚转，
    /// 表项只剩内存占用）。
    #[serde(default)]
    provisional: HashMap<u64, Provisional>,
    #[serde(default)]
    next_id: u64,
}

/// 预算护栏实例。跨实例经 [`shared_budget_guard`] 按 workspace 共享
/// （同进程内 agent/loop 重建共用同一 Arc）；挂 store 的实例（M3）账目
/// 落盘，**跨进程**共享同一份（store 是唯一真相，操作经文件锁串行）。
#[derive(Debug, Default)]
pub struct BudgetGuard {
    state: Mutex<State>,
    /// M3：持久化存储路径（None = 纯内存，`default()` 行为不变）。
    store: Option<PathBuf>,
}

/// 预备占用超时扫尾窗口（远大于最长执行路径——RPC 上限 2h，24h 宽裕）。
const PROVISIONAL_TTL_MS: i64 = 24 * HOUR_MS;

/// workspace → 预算存储路径（M3 单一真相源；`shared_budget_guard` 与测试
/// 同源引用，别处不得自行拼路径）。
pub fn budget_store_path(workspace: &Path) -> PathBuf {
    workspace.join("data").join("connectors_budget.json")
}

impl BudgetGuard {
    /// 挂持久化存储的实例（M3）：账目落 `<path>`，跨进程共享（文件锁串行
    /// + 原子写回）。测试直连 `with_store`；生产走 [`shared_budget_guard`]。
    pub fn with_store(path: PathBuf) -> Self {
        Self {
            state: Mutex::new(State::default()),
            store: Some(path),
        }
    }

    /// 状态操作统一漏斗（M3）：无 store = 进程内互斥；有 store = 锁文件
    /// 互斥 → 读盘 → `f` → 原子写回。锁/写失败诚实降级内存态（WARN 留痕
    /// ——预算是成本熔断不是安全边界，不因基础设施抖动把执行面锁死）。
    fn operate<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        let Some(path) = &self.store else {
            let mut st = self.state.lock();
            return f(&mut st);
        };
        match StoreLock::acquire(path) {
            Ok(_lock) => {
                let mut st = load_state_file(path);
                let out = f(&mut st);
                match serde_json::to_vec(&st) {
                    Ok(bytes) => {
                        if let Err(e) = nemesis_utils::file::write_file_atomic(
                            &path.to_string_lossy(),
                            &bytes,
                            0o600,
                        ) {
                            tracing::warn!(
                                "[Connector] 预算存储写回失败（本次记账仅进程内可见）: {e}"
                            );
                        }
                    }
                    Err(e) => tracing::warn!("[Connector] 预算状态序列化失败: {e}"),
                }
                out
            }
            Err(e) => {
                tracing::warn!("[Connector] 预算存储锁不可用，回退进程内存态: {e}");
                let mut st = self.state.lock();
                f(&mut st)
            }
        }
    }

    /// 执行前原子占用：单锁内检查 小时滑窗 + 资金限额，**通过即写入预备
    /// 账目**（计入限额）。拒绝时返回 [`BudgetVerdict`]（调用方出凭条/文案）。
    pub fn reserve(
        &self,
        limits: &Limits,
        connector: &str,
        now_ms: i64,
        finance_qty: Option<f64>,
    ) -> Result<Reservation, BudgetVerdict> {
        self.operate(|st| {
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
                let day = ensure_day(st, now_ms);
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
                let day = ensure_day(st, now_ms);
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
        })
    }

    /// 预备转正式（记账保留）。幂等：重复 commit / 未知 id 静默忽略——
    /// 账目在 reserve 时一次落定，重复收口不重复计数。
    pub fn commit(&self, res: &Reservation) {
        self.operate(|st| {
            st.provisional.remove(&res.id);
        });
    }

    /// 扣减预备账目（仅「确定未产生外部效果」的失败路径调用）。幂等：已
    /// commit / 未知 id 静默忽略——commit 后账目是既成事实，release 不回吐。
    pub fn release(&self, res: &Reservation) {
        self.operate(|st| {
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
        });
    }

    /// 当前 connector 滑窗内已执行次数（凭条/状态面素材）。
    pub fn rate_used(&self, connector: &str, now_ms: i64) -> u32 {
        self.operate(|st| {
            let win = st.windows.entry(connector.to_string()).or_default();
            win.retain(|t| t.at_ms > now_ms - HOUR_MS);
            win.len() as u32
        })
    }

    /// 今日资金类（笔数, 累计数量）。
    pub fn finance_today(&self, now_ms: i64) -> (u32, f64) {
        self.operate(|st| {
            let day = ensure_day(st, now_ms);
            (day.orders, day.qty)
        })
    }
}

/// workspace 共享预算单例（R3）：进程级 `Mutex<HashMap>` 按 canonical
/// workspace 路径键控——agent/loop 重建走同一 `Arc`，滑窗与日累计不清零
/// （旧 per-instance `BudgetGuard::default()` 让 agent 重启即重置额度，
/// 限额形同虚设）。canonicalize 失败（目录不存在等）回落原路径作键。
///
/// M3 起实例自动挂 store（`<workspace>/data/connectors_budget.json`）：
/// 同进程共享 Arc，**跨进程**经文件锁 + 原子写回共享同一份账目——多
/// gateway 实例（多 `--local` 部署除外，workspace 不同天然隔离）不再
/// 各记各账。
pub fn shared_budget_guard(workspace: &Path) -> Arc<BudgetGuard> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<BudgetGuard>>>> = OnceLock::new();
    let map = REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    let key = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    map.lock()
        .entry(key.clone())
        .or_insert_with(|| Arc::new(BudgetGuard::with_store(budget_store_path(&key))))
        .clone()
}

/// M3 存储互斥句柄：锁文件（`<store>.lock`）上的 OS 级阻塞独占锁。锁文件
/// 与数据文件分离——数据文件走 `write_file_atomic` tmp+rename，锁句柄不受
/// rename 影响。进程死亡 = OS 释放锁，无陈锁恢复问题。
struct StoreLock(std::fs::File);

impl StoreLock {
    fn acquire(store_path: &Path) -> Result<Self, String> {
        let lock_path = store_path.with_extension("lock");
        // 与 write_file_atomic 同纪律：父目录不存在就地建（首次占用时
        // `<workspace>/data/` 尚未生成是常态，不是错误）。
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建预算锁目录 {} 失败: {e}", parent.display()))?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| format!("打开预算锁文件 {} 失败: {e}", lock_path.display()))?;
        file.lock()
            .map_err(|e| format!("预算锁 {} 获取失败: {e}", lock_path.display()))?;
        Ok(Self(file))
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// 读盘 → State。缺文件（首次）= 全新账；损坏（半截 JSON 等）= 重置
/// fail-open + WARN（预算是成本熔断不是安全边界——宁可重开账，不把执行
/// 面锁死在坏文件上）。
fn load_state_file(path: &Path) -> State {
    match std::fs::read_to_string(path) {
        Err(_) => State::default(),
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(
                "[Connector] 预算存储 {} 损坏（{e}），重置 fail-open",
                path.display()
            );
            State::default()
        }),
    }
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
