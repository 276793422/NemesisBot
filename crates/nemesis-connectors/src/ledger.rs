//! 纸面券商账本（域 C，PLAN §4）：模拟资金 + 持仓 + 成交记录。
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
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写账本临时文件失败: {e}"))?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("替换账本失败（remove）: {e}"))?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("账本落位失败（rename）: {e}"))?;
    Ok(())
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
        // NaN-safe 非正判断：NaN 不满足 > 0.0，必须拒绝。
        if qty <= 0.0 || qty.is_nan() {
            return Err(format!("买入数量必须为正（收到 {qty}）"));
        }
        if price < 0.0 || price.is_nan() {
            return Err(format!("价格非法（收到 {price}）"));
        }
        let cost = r2(qty * price);
        if self.cash < cost {
            return Err(format!("模拟资金不足：需 {cost:.2}，可用 {:.2}", self.cash));
        }
        self.cash = r2(self.cash - cost);
        let pos = self
            .positions
            .entry(symbol.to_string())
            .or_insert_with(|| Position {
                symbol: symbol.to_string(),
                qty: 0.0,
                avg_cost: 0.0,
            });
        let total_cost = pos.avg_cost * pos.qty + cost;
        pos.qty += qty;
        pos.avg_cost = r2(total_cost / pos.qty);
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
        // NaN-safe 非正判断：NaN 不满足 > 0.0，必须拒绝。
        if qty <= 0.0 || qty.is_nan() {
            return Err(format!("卖出数量必须为正（收到 {qty}）"));
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
        self.cash = r2(self.cash + proceeds);
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
