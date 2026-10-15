//! connector 数据面 WSAPI（行动代理 P0，2026-10-14）。
//!
//! 命令：`connectors.list`（registry 状态）/ `connectors.receipts.list`
//! （凭条流水）/ `connectors.undo`（E3 页内撤销）/ `paperbroker.ledger`
//! （纸面账本）——M3 e2e 断言面、CLI 查询（`connector list` /
//! `connector receipts`）与 Dashboard ConnectorsView 的同一数据面（回执/
//! 账本单点不双写，读皆直读存储）。
//!
//! 执行纪律：常规执行入口是 agent 的 `connector_invoke` 工具（走完整安全
//! 管线）；E3 撤销（`connectors.undo`）是唯一例外写入口——但它不直连
//! executor，而是经 [`resolve_session_loop`] 拿到会话归属的 AgentLoop 后
//! 走 `handle_tool_call` 完整 dispatch 瀑布（安全 8 层/审批/审计照常，
//! 撤销本身也是账）。

use crate::handlers::projects::resolve_session_loop;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_connectors::manifest::{OpBinding, OpClass};
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

/// E3（收官批 2026-10-15）页内撤销：`connectors.undo {receipt_id,
/// session_key?}`。按凭条+当下清单三分派：
/// - read 类 → 诚实说明（不改变外部状态，无需撤销）；
/// - write + 清单声明 undo（域 C）→ 反向 op：凭条主键值回填 arg，经
///   [`resolve_session_loop`] → `handle_tool_call` 完整 dispatch 瀑布
///   （安全 8 层/审批/审计照常——撤销本身也是账）；
/// - write 无 undo 声明（域 B）→ 会话级 `undo_last_turn`（E3a busy 闸
///   守门；需要 session_key——凭条有记录用凭条的，否则取入参）。
///
/// 仅 `executed` 凭条可撤销；registry 按当下裁决（op 已删/清单已改 =
/// 诚实拒，不凭旧行为猜）。
async fn cmd_undo(ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let receipt_id = data
        .and_then(|d| d.get("receipt_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("缺 receipt_id")?
        .to_string();
    let session_key_in = data
        .and_then(|d| d.get("session_key"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    // 凭条回读（近 500 条内按 id 定位，与 receipts.list 上限同源）。
    let r = receipt::list_receipts(&ws, 500, None)?
        .into_iter()
        .find(|x| x.receipt_id == receipt_id)
        .ok_or_else(|| format!("凭条 {receipt_id} 不存在（近 500 条内）"))?;

    // 只有 executed 才有可撤销的效果（denied/blocked/error 本身未生效）。
    if r.outcome != "executed" {
        return Err(format!(
            "凭条 {receipt_id} 的 outcome 是 `{}`（未真实生效），无可撤销",
            r.outcome
        ));
    }

    // 注册表现查。
    let loaded = registry::scan_connectors(&ws);
    let c = loaded
        .iter()
        .filter_map(|l| l.connector.as_ref())
        .find(|c| c.id == r.connector)
        .ok_or_else(|| format!("connector `{}` 不在注册表（已移除或损坏）", r.connector))?;
    let op = c
        .ops
        .iter()
        .find(|o| o.id == r.op)
        .ok_or_else(|| format!("op `{}` 已从 connector `{}` 清单移除", r.op, r.connector))?;

    // read 类：不改变外部状态（诚实说明，不算错）。
    if op.class == OpClass::Read {
        return Ok(Some(json!({
            "receipt_id": receipt_id,
            "undone": false,
            "via": "none",
            "reason": "read 类操作不改变外部状态，无需撤销",
        })));
    }

    // 域 C：清单声明 undo → 反向 op（凭条主键值回填）。
    if let Some(undo) = &op.undo {
        let key_value = r.receipt_value.clone().ok_or_else(|| {
            format!("凭条 {receipt_id} 缺主键值（receipt_value 空），无法回填撤销参数")
        })?;
        let sk = session_key_in
            .or_else(|| r.session_key.clone())
            .unwrap_or_default();
        let loop_ref = resolve_session_loop(ctx, &sk)?;
        let call = nemesis_agent::types::ToolCallInfo {
            id: format!("undo-{receipt_id}"),
            name: nemesis_connectors::INVOKE_TOOL_NAME.to_string(),
            arguments: json!({
                "connector": c.id,
                "op": undo.op,
                "args": { (undo.arg.as_str()): key_value },
            })
            .to_string(),
        };
        let tool_ctx =
            nemesis_agent::context::RequestContext::new("web", &ctx.chat_id, "dashboard-undo", &sk);
        let result_text = loop_ref.handle_tool_call(&call, &tool_ctx).await;
        return Ok(Some(json!({
            "receipt_id": receipt_id,
            "undone": true,
            "via": "reverse_op",
            "undo_op": undo.op,
            "result_text": result_text,
        })));
    }

    // 域 B：write 无 undo 声明 → 会话级 rewind_last_turn。
    if op.class == OpClass::Write {
        let sk = session_key_in
            .or(r.session_key.clone())
            .filter(|s| !s.is_empty())
            .ok_or("该 op 无 undo 声明，会话级撤销需要 session_key（凭条也未记录会话）")?;
        let loop_ref = resolve_session_loop(ctx, &sk)?;
        let report = loop_ref.undo_last_turn(&sk).await?;
        return Ok(Some(json!({
            "receipt_id": receipt_id,
            "undone": true,
            "via": "rewind_last_turn",
            "session_key": sk,
            "report": report,
        })));
    }

    // exec/network 无 undo 声明：无通用撤销语义，诚实拒。
    Err(format!(
        "op `{}`（{} 类）未声明 undo 且非 write 类——无通用撤销语义；如需撤销请在清单为它声明 undo 反向 op",
        op.id,
        op.class.as_str()
    ))
}

/// connector 数据面 handler（`connectors.list` / `connectors.receipts.list` /
/// `connectors.undo`）。
pub struct ConnectorsHandler;

#[async_trait::async_trait]
impl ModuleHandler for ConnectorsHandler {
    fn module_name(&self) -> &str {
        "connectors"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "receipts.list", "undo"]
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
            "undo" => cmd_undo(ctx, data.as_ref()).await,
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
