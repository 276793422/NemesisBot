//! 纸面券商账本（域 C，PLAN）：模拟资金 + 持仓 + 成交记录。
//!
//! 单文件 JSON 存 `<workspace>/data/paper_broker.json`。v1 即时成交（无挂
//! 单）：`place_order` 按给定价格立即 fill，`cancel` 诚实报错（v1 无可撤订
//! 单——诚实边界是 demo 叙事的一部分：撤销走「反向单」或聊天「撤销」动词
//! 回退整轮）。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 账本相对路径（workspace 下）。
pub const LEDGER_REL: &str = "data/paper_broker.json";
/// 模拟资金（零账号 demo 缺省）。可被已有文件覆盖。
pub const DEFAULT_CASH: f64 = 1_000_000.0;

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    Buy,
    Sell,
}

/// v1 只有即时成交一态（保留 enum 形态钉拼写，未来挂单态向后兼容加）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderStatus {
    Filled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub symbol: String,
    pub qty: f64,
    pub avg_cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderRecord {
    /// `NB-ORD-<n>`，全账本单调。
    pub order_id: String,
    /// RFC3339 UTC。
    pub ts: String,
    pub side: OrderSide,
    pub symbol: String,
    pub qty: f64,
    pub price: f64,
    pub status: OrderStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerLedger {
    pub cash: f64,
    pub positions: BTreeMap<String, Position>,
    pub orders: Vec<OrderRecord>,
}

impl Default for BrokerLedger {
    fn default() -> Self {
        Self {
            cash: DEFAULT_CASH,
            positions: BTreeMap::new(),
            orders: Vec::new(),
        }
    }
}

/// 账本路径。
pub fn ledger_path(workspace: &Path) -> std::path::PathBuf {
    workspace.join(LEDGER_REL)
}

// 账本写互斥（进程级）：下单是调用方组装的 load→mutate→save RMW，单文件
// JSON 无内存缓存——两个写者交错 = 后 save 者整文件覆盖（前者的订单与
// 资金变动静默丢失，纸面资金凭空重复）。写者不止一个 ConnectorInvokeTool
// 实例：主 loop 与每个项目 loop 各持一份工具实例（治理资产锚同一主
// workspace，见 register_connector_governance）——per-instance 锁罩不住
// 跨实例交错（BUG-20261120-035），必须进程级单例。镜像
// nemesis-config::config_write_lock 的取锁纪律：**只在命令臂最外层取
// 一次**，guard 存活覆盖整段 RMW（从 load 到 save 落盘）；Mutex 非重入，
// 勿嵌套；读路径不入锁（rename 原子性保证读到完整旧值或完整新值）。
// 跨进程（CLI connector invoke 单发）仍是既有边界，不在本锁射程。
static LEDGER_WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// 取账本写串行锁。用法：写臂首行 `let _serial =
/// nemesis_connectors::ledger::ledger_write_lock();`，guard 存活到 save
/// 落盘为止。见上方锁注释的取锁纪律（只在写臂取、勿嵌套）。
pub fn ledger_write_lock() -> parking_lot::MutexGuard<'static, ()> {
    LEDGER_WRITE_LOCK.lock()
}

/// 载入账本：缺文件 = 全新模拟账本（零账号 demo 常态）；损坏 = 诚实 Err
/// （损坏静默当空会让后续 save 销毁真账本——与全仓数据纪律一致）。
pub fn load_ledger(workspace: &Path) -> Result<BrokerLedger, String> {
    let path = ledger_path(workspace);
    if !path.exists() {
        return Ok(BrokerLedger::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取账本失败 {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| {
        format!(
            "账本损坏 {}: {e}（拒绝当空处理，请人工修复或删除重建）",
            path.display()
        )
    })
}

/// 保存账本（tmp 写入 + rename 原子落位）。
pub fn save_ledger(workspace: &Path, ledger: &BrokerLedger) -> Result<(), String> {
    let path = ledger_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建账本目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(ledger).map_err(|e| format!("序列化账本失败: {e}"))?;
    // 委托唯一临时名权威 helper——旧固定 `.json.tmp` 并发
    // 互踩 + 先 remove 后 rename 的崩溃窗口（两步间断电 = 账本消失）一并根除。
    nemesis_utils::file::write_file_atomic(&path.to_string_lossy(), text.as_bytes(), 0o644)
        .map_err(|e| format!("账本落盘失败: {e}"))
}

impl BrokerLedger {
    /// 下一订单号（按现存最大序号 +1，删除不重号）。
    pub fn next_order_id(&self) -> String {
        let mut max = 0u64;
        for o in &self.orders {
            if let Some(n) = o.order_id.strip_prefix("NB-ORD-")
                && let Ok(v) = n.parse::<u64>()
            {
                max = max.max(v);
            }
        }
        format!("NB-ORD-{}", max + 1)
    }

    /// 模拟买入（即时成交）。
    pub fn buy(
        &mut self,
        symbol: &str,
        qty: f64,
        price: f64,
        ts: &str,
    ) -> Result<OrderRecord, String> {
        // 非有限值全拒（BUG-20261121-030）：NaN 不满足 > 0.0；±inf 漏过旧
        // `qty <= 0.0 || qty.is_nan()` 闸——inf 数量入成本核算只会产出失真
        // 文案，闸收口为 is_finite 单点。
        if !qty.is_finite() || qty <= 0.0 {
            return Err(format!("买入数量非法（须为正的有限数，收到 {qty}）"));
        }
        if !price.is_finite() || price < 0.0 {
            return Err(format!("价格非法（须为非负的有限数，收到 {price}）"));
        }
        let cost = r2(qty * price);
        // r2 的 ×100 放大对近界有限输入可溢出（qty=1、price=1e307 →
        // 1e309=inf）：拒单而不是把 inf 记进账本——cash=inf 后资金闸
        // `cash < cost` 对一切有限 cost 永假（inf < inf == false），账本
        // 资金检查永久失效（BUG-20261121-030 主体）。
        if !cost.is_finite() {
            return Err(format!(
                "订单金额溢出（qty×price ≈ {qty:.3e} × {price:.3e}），已拒单"
            ));
        }
        if self.cash < cost {
            return Err(format!("模拟资金不足：需 {cost:.2}，可用 {:.2}", self.cash));
        }
        // 二阶 r2 溢出闸（BUG-20261128-044）：一阶闸只保订单金额（cost 的
        // r2 输出）有限；现金/均价的**累计形态** r2(现值±金额) 与
        // r2(总成本/数量) 的 ×100 放大可对合法有限输入二次溢出（如现金
        // 1.8e307 的存量文件扣 1 元 → (1.8e307×100)=inf）。inf 入账 =
        // 双重破坏：资金闸 `cash < cost` 永假（BUG-20261121-030 注释点名的
        // 毒态从第二条路到达）+ serde_json 把非有限 f64 序列化为 null =
        // 账本文件砖化（下次 load 诚实报损坏）。先算后写：任一累计值非
        // 有限 → 拒单不动账。
        let new_cash = r2(self.cash - cost);
        let (pos_qty, pos_avg) = match self.positions.get(symbol) {
            Some(p) => (p.qty, p.avg_cost),
            None => (0.0, 0.0),
        };
        let new_qty = pos_qty + qty;
        let new_avg = r2((pos_avg * pos_qty + cost) / new_qty);
        if !new_cash.is_finite() || !new_avg.is_finite() {
            return Err(format!(
                "累计值溢出（现金 → {new_cash:e}，均价 → {new_avg:e}），已拒单不动账"
            ));
        }
        self.cash = new_cash;
        let pos = self
            .positions
            .entry(symbol.to_string())
            .or_insert_with(|| Position {
                symbol: symbol.to_string(),
                qty: 0.0,
                avg_cost: 0.0,
            });
        pos.qty = new_qty;
        pos.avg_cost = new_avg;
        let order = OrderRecord {
            order_id: self.next_order_id(),
            ts: ts.to_string(),
            side: OrderSide::Buy,
            symbol: symbol.to_string(),
            qty,
            price,
            status: OrderStatus::Filled,
        };
        self.orders.push(order.clone());
        Ok(order)
    }

    /// 模拟卖出（即时成交；无挂单）。
    pub fn sell(
        &mut self,
        symbol: &str,
        qty: f64,
        price: f64,
        ts: &str,
    ) -> Result<OrderRecord, String> {
        // 非有限值全拒（BUG-20261121-030，与 buy 同款 is_finite 收口）。
        if !qty.is_finite() || qty <= 0.0 {
            return Err(format!("卖出数量非法（须为正的有限数，收到 {qty}）"));
        }
        // 负价/非有限价与 buy 同款闸——缺闸时负价卖出
        // proceeds 为负直接扣现金（账本可被工具调用面推成负余额）；+inf
        // 价卖出 proceeds=inf 记账同样毁账（BUG-20261121-030 同族）。
        if !price.is_finite() || price < 0.0 {
            return Err(format!("价格非法（须为非负的有限数，收到 {price}）"));
        }
        let Some(pos) = self.positions.get_mut(symbol) else {
            return Err(format!("无 {symbol} 持仓，无法卖出"));
        };
        if pos.qty < qty {
            return Err(format!(
                "{symbol} 持仓不足：持有 {:.0}，尝试卖出 {:.0}",
                pos.qty, qty
            ));
        }
        let proceeds = r2(qty * price);
        // r2 ×100 放大溢出同闸（见 buy）：拒单不动账。
        if !proceeds.is_finite() {
            return Err(format!(
                "卖出金额溢出（qty×price ≈ {qty:.3e} × {price:.3e}），已拒单"
            ));
        }
        // 二阶 r2 溢出闸（BUG-20261128-044 同族）：proceeds 一阶有限 ≠
        // 现金累计有限——cash 1e6 + proceeds 1.79e308 ≈ 1.79e308 有限，
        // 但 r2 的 ×100 放大 → 1.79e310 = inf 入账（资金闸永假 + 账本
        // 序列化 null 砖化）。先算后写，非有限拒单不动账。
        let new_cash = r2(self.cash + proceeds);
        if !new_cash.is_finite() {
            return Err(format!(
                "账户现金累计溢出（{:.3e} + {:.3e} 经 r2 ×100 放大越界），已拒单不动账",
                self.cash, proceeds
            ));
        }
        self.cash = new_cash;
        pos.qty -= qty;
        if pos.qty <= 0.0 {
            self.positions.remove(symbol);
        }
        let order = OrderRecord {
            order_id: self.next_order_id(),
            ts: ts.to_string(),
            side: OrderSide::Sell,
            symbol: symbol.to_string(),
            qty,
            price,
            status: OrderStatus::Filled,
        };
        self.orders.push(order.clone());
        Ok(order)
    }

    /// 撤单：v1 即时成交，诚实报错（demo 诚实边界）。
    pub fn cancel(&self, order_id: &str) -> Result<OrderRecord, String> {
        if let Some(o) = self.orders.iter().find(|o| o.order_id == order_id) {
            let _ = o; // 订单存在且已成交——两态同一诚实结论：无可撤订单
            return Err(format!(
                "订单 {order_id} 已即时成交（v1 纸面券商无挂单），无可撤销订单；如需反向请下反向单，或用聊天「撤销」回退整轮"
            ));
        }
        Err(format!("订单 {order_id} 不存在于账本中"))
    }
}

#[cfg(test)]
mod tests;
