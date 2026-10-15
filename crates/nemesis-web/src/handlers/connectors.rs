//! connector 数据面 WSAPI（行动代理 P0，2026-10-14）。
//!
//! 命令：`connectors.list`（registry 状态）/ `connectors.receipts.list`
//! （凭条流水）/ `paperbroker.ledger`（纸面账本）——M3 e2e 断言面、CLI
//! 查询（`connector list` / `connector receipts`）与 Dashboard
//! ConnectorsView 的同一数据面（回执/账本单点不双写，读皆直读存储）。
//!
//! 只读数据面：执行入口是 agent 的 `connector_invoke` 工具（走完整安全
//! 管线），本 handler 不提供任何写路径。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_connectors::manifest::OpBinding;
use nemesis_connectors::{ledger, receipt, registry};
use serde_json::{Value, json};
use std::path::PathBuf;

fn workspace_of(ctx: &RequestContext) -> Result<PathBuf, String> {
    ctx.workspace
        .clone()
        .map(PathBuf::from)
        .ok_or_else(|| "workspace not configured".to_string())
}

/// 总闸（`config.json` `connectors.enabled`，缺省 true——与供给闸同源语义）。
fn enabled_now(ctx: &RequestContext) -> bool {
    ctx.home
        .clone()
        .ok_or(())
        .and_then(|h| crate::handlers::config::load_config(&h).map_err(|_| ()))
        .ok()
        .and_then(|cfg| cfg.connectors)
        .map(|c| c.enabled)
        .unwrap_or(true)
}

/// 单 connector → JSON（校验通过形态）。
fn connector_json(dir_name: &str, c: &nemesis_connectors::manifest::ValidatedConnector) -> Value {
    let ops: Vec<Value> = c
        .ops
        .iter()
        .map(|op| {
            let binding = match &op.binding {
                OpBinding::Http { method, path, .. } => json!({
                    "type": "http", "method": format!("{method:?}").to_uppercase(), "path": path,
                }),
                OpBinding::Tool { tool } => json!({ "type": "tool", "tool": tool }),
                OpBinding::Action { action } => json!({ "type": "action", "action": action }),
            };
            json!({
                "id": op.id,
                "intent": op.intent,
                "risk": op.risk.as_str(),
                "class": op.class.as_str(),
                "confirm_required": op.confirm_required,
                "finance_budget": op.finance_budget,
                "binding": binding,
            })
        })
        .collect();
    json!({
        "id": c.id,
        "name": c.name,
        "version": c.version,
        "kind": format!("{:?}", c.kind).to_lowercase(),
        "enabled": c.enabled,
        "description": c.description,
        "base_url": c.base_url,
        "dir": dir_name,
        "ops": ops,
        "error": Value::Null,
    })
}

/// 注册表扫描 → 列表（损坏 connector 诚实入列带 error）。
fn cmd_list(ctx: &RequestContext) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let loaded = registry::scan_connectors(&ws);
    let connectors: Vec<Value> = loaded
        .iter()
        .map(|l| {
            let dir_name = l
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match &l.connector {
                Some(c) => connector_json(&dir_name, c),
                None => json!({
                    "id": dir_name,
                    "dir": dir_name,
                    "enabled": false,
                    "error": l.error.clone().unwrap_or_else(|| "未知损坏".to_string()),
                }),
            }
        })
        .collect();
    let count = connectors.len();
    Ok(Some(json!({
        "enabled": enabled_now(ctx),
        "workspace": ws.display().to_string(),
        "count": count,
        "connectors": connectors,
    })))
}

/// 凭条流水（新→旧；limit 缺省 50、connector 可选过滤）。
fn cmd_receipts_list(ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let limit = data
        .and_then(|d| d.get("limit"))
        .and_then(|v| v.as_u64())
        .map(|v| (v as usize).min(500))
        .unwrap_or(50);
    let connector = data
        .and_then(|d| d.get("connector"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let rows: Vec<Value> = receipt::list_receipts(&ws, limit, connector)?
        .into_iter()
        .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
        .collect();
    let count = rows.len();
    Ok(Some(json!({
        "workspace": ws.display().to_string(),
        "count": count,
        "limit": limit,
        "connector": connector,
        "receipts": rows,
    })))
}

/// 纸面账本（缺文件 = 全新模拟账本常态，不报错）。
fn cmd_ledger(ctx: &RequestContext) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let l = ledger::load_ledger(&ws)?;
    let positions: Vec<Value> = l
        .positions
        .values()
        .map(|p| json!({ "symbol": p.symbol, "qty": p.qty, "avg_cost": p.avg_cost }))
        .collect();
    let orders: Vec<Value> = l
        .orders
        .iter()
        .map(|o| {
            json!({
                "order_id": o.order_id,
                "ts": o.ts,
                "side": format!("{:?}", o.side).to_lowercase(),
                "symbol": o.symbol,
                "qty": o.qty,
                "price": o.price,
                "status": format!("{:?}", o.status).to_lowercase(),
            })
        })
        .collect();
    Ok(Some(json!({
        "cash": l.cash,
        "positions": positions,
        "orders": orders,
        "order_count": orders.len(),
        "ledger_path": ledger::ledger_path(&ws).display().to_string(),
    })))
}

/// connector 数据面 handler（`connectors.list` / `connectors.receipts.list`）。
pub struct ConnectorsHandler;

#[async_trait::async_trait]
impl ModuleHandler for ConnectorsHandler {
    fn module_name(&self) -> &str {
        "connectors"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "receipts.list"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        match cmd {
            "list" => cmd_list(ctx),
            "receipts.list" => cmd_receipts_list(ctx, data.as_ref()),
            _ => Err(format!("unknown command: connectors.{cmd}")),
        }
    }
}

/// 纸面券商账本查询（`paperbroker.ledger`）。
pub struct PaperBrokerHandler;

#[async_trait::async_trait]
impl ModuleHandler for PaperBrokerHandler {
    fn module_name(&self) -> &str {
        "paperbroker"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["ledger"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        match cmd {
            "ledger" => cmd_ledger(ctx),
            _ => Err(format!("unknown command: paperbroker.{cmd}")),
        }
    }
}

#[cfg(test)]
mod tests;
