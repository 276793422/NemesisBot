//! connector 数据面 WSAPI（行动代理 P0，2026-10-14；管理面 2026-10-19）。
//!
//! 命令：`connectors.list`（registry 状态）/ `connectors.receipts.list`
//! （凭条流水）/ `connectors.undo`（E3 页内撤销）/ `connectors.create` /
//! `connectors.set_enabled` / `connectors.remove`（Dashboard 管理面三命令，
//! 逻辑在 nemesis-connectors registry）/ `paperbroker.ledger`（纸面账本）
//! ——M3 e2e 断言面、CLI 查询（`connector list` /
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
use std::path::{Path, PathBuf};

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

/// 单 connector → JSON（校验通过形态）。trust 徽标随行（M1 消费面收口：
/// 前端徽标三态 verified/community/unsigned；损坏条目由调用方置 null）。
/// workspace config.mcp.json 已注册 server 的工具前缀集（M4b 在位探测）。
/// 前缀形态与 nemesis-mcp adapter 挂载面同源：`mcp_{sanitize(name)}_`。
/// config 缺失/不可解析 = 空集（没有 server 在场，tool 绑定全部视为未就绪
/// ——诚实展示，不假装可用）。
fn registered_mcp_prefixes(workspace: &Path) -> Vec<String> {
    let path = nemesis_path::resolve_mcp_config_path_in_workspace(workspace);
    match nemesis_config::load_mcp_config(&path) {
        Ok(cfg) => cfg
            .servers
            .iter()
            .map(|s| format!("mcp_{}_", nemesis_mcp::adapter::sanitize_name(&s.name)))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// tool 绑定在位判定（M4b）：绑定工具名被任一已注册前缀覆盖 = 就绪。
/// 无 tool 绑定的 connector 返回 null（探测不适用——http/action/script
/// 绑定不挂 MCP）。
fn tool_bindings_ready(
    c: &nemesis_connectors::manifest::ValidatedConnector,
    prefixes: &[String],
) -> Option<bool> {
    let tool_ops: Vec<&str> = c
        .ops
        .iter()
        .filter_map(|op| match &op.binding {
            OpBinding::Tool { tool } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    if tool_ops.is_empty() {
        return None;
    }
    Some(
        tool_ops
            .iter()
            .all(|tool| prefixes.iter().any(|p| tool.starts_with(p.as_str()))),
    )
}

fn connector_json(
    dir_name: &str,
    c: &nemesis_connectors::manifest::ValidatedConnector,
    trust: nemesis_connectors::signing::ConnectorTrust,
    mcp_prefixes: &[String],
    exclusive_rows: &[Value],
) -> Value {
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
                OpBinding::Script {
                    command,
                    timeout_secs,
                    workdir,
                    capture,
                    ..
                } => json!({
                    "type": "script", "command": command,
                    "timeout_secs": timeout_secs, "workdir": workdir, "capture": capture,
                }),
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
        "trust": trust.as_str(),
        "description": c.description,
        "base_url": c.base_url,
        "dir": dir_name,
        "ops": ops,
        // M4b 在位探测：tool 绑定的 MCP server 是否注册（null=无 tool 绑定
        // 不适用）。false = 列表诚实展示「MCP 未注册」，不假装可用。
        "mcp_ready": tool_bindings_ready(c, mcp_prefixes),
        // B2：独占接管面——声明的独占 server 清单 + per-tool 策略行
        // （mirrors[].policy = default|exempt|blocked，前端三态选择器）。
        "exclusive_mcp": c.exclusive_mcp,
        "mirrors": exclusive_rows,
        "error": Value::Null,
    })
}

/// 主 loop 的 MCP 工具快照行（name + parameters + B3 hints）。loop 不在
/// 场 = 空表（镜像行只剩 sidecar 记录的例外，args_schema 缺席——诚实展示）。
struct McpToolRow {
    name: String,
    params: Value,
    read_only_hint: Option<bool>,
    destructive_hint: Option<bool>,
}

fn main_loop_mcp_tools(ctx: &RequestContext) -> Vec<McpToolRow> {
    let Some(loop_arc) = ctx.state.agent_loop.read().clone() else {
        return Vec::new();
    };
    loop_arc
        .mcp_tool_snapshot()
        .read()
        .iter()
        .map(|e| McpToolRow {
            name: e.name.clone(),
            params: e.parameters.clone(),
            read_only_hint: e.read_only_hint,
            destructive_hint: e.destructive_hint,
        })
        .collect()
}

/// B2：单 connector 的独占接管 per-tool 策略行（Dashboard 三态选择器数据
/// 面）。行 = 独占前缀命中的**当下在场**工具（policy 按当下 sidecar 裁）
/// ∪ sidecar 记录但当下不在场的例外（server 离线也可见可撤——例外是用户
/// 显式意志，不因 server 重启消失）。按工具名字典序。
fn exclusive_tool_rows(
    ws: &Path,
    c: &nemesis_connectors::manifest::ValidatedConnector,
    mcp_tools: &[McpToolRow],
) -> Vec<Value> {
    use nemesis_connectors::registry::mirror_baseline;
    use nemesis_connectors::state::{ToolPolicy, load_tool_state, tool_policy};
    if c.exclusive_mcp.is_empty() {
        return Vec::new();
    }
    let state = load_tool_state(ws, &c.id);
    let prefixes: Vec<String> = c
        .exclusive_mcp
        .iter()
        .map(|s| registry::exclusive_prefix(s))
        .collect();
    let mut rows: Vec<Value> = mcp_tools
        .iter()
        .filter(|t| prefixes.iter().any(|p| t.name.starts_with(p.as_str())))
        .map(|t| {
            // B3：镜像基线档位随行（前端 risk 徽标 + 确认卡提示）。
            let (risk, class, confirm) = mirror_baseline(t.read_only_hint, t.destructive_hint);
            json!({
                "id": t.name,
                "args_schema": t.params,
                "risk": risk.as_str(),
                "class": class.as_str(),
                "confirm_required": confirm,
                "policy": match tool_policy(&state, &t.name) {
                    ToolPolicy::Default => "default",
                    ToolPolicy::Exempt => "exempt",
                    ToolPolicy::Blocked => "blocked",
                },
            })
        })
        .collect();
    let recorded = state
        .blocked
        .iter()
        .map(|t| (t, "blocked"))
        .chain(state.exempt.iter().map(|t| (t, "exempt")));
    for (t, p) in recorded {
        if rows.iter().any(|r| r["id"] == json!(t)) {
            continue;
        }
        rows.push(json!({ "id": t, "args_schema": Value::Null, "policy": p }));
    }
    rows.sort_by(|a, b| {
        a["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["id"].as_str().unwrap_or_default())
    });
    rows
}

/// 注册表扫描 → 列表（损坏 connector 诚实入列带 error）。
fn cmd_list(ctx: &RequestContext) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let loaded = registry::scan_connectors(&ws);
    let mcp_prefixes = registered_mcp_prefixes(&ws);
    let mcp_tools = main_loop_mcp_tools(ctx);
    let connectors: Vec<Value> = loaded
        .iter()
        .map(|l| {
            let dir_name = l
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match &l.connector {
                Some(c) => {
                    let rows = exclusive_tool_rows(&ws, c, &mcp_tools);
                    connector_json(&dir_name, c, l.trust, &mcp_prefixes, &rows)
                }
                None => json!({
                    "id": dir_name,
                    "dir": dir_name,
                    "enabled": false,
                    // M1：损坏条目无有效清单可评估——trust 置 null（不是
                    // "unsigned"：没解析出的清单谈不上「未签名」的判断）。
                    "trust": Value::Null,
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
///   （安全 8 层/审批/审计照常——撤销本身也是账）；**结果按行动账本
///   新凭条结构化判定**（R5）：反向 op 的凭条 `outcome == "executed"`
///   才 `undone: true`，不嗅探文本；
/// - write 无 undo 声明（域 B）→ 会话级 `undo_last_turn`（E3a busy 闸
///   守门；需要 session_key——凭条有记录用凭条的，否则取入参）。**只
///   回退本地会话**，外部副作用未被撤销——`undone: false` 如实标注。
///
/// 仅 `executed` 凭条可撤销；registry 按当下裁决（op 已删/清单已改 =
/// 诚实拒，不凭旧行为猜）。响应统一 `{receipt_id, undone, via,
/// capability, reason?}`（R5 能力建模：`capability = reverse_op |
/// local_rewind | none`）。
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

    // 只有 executed 才有可撤销的效果；unknown（结果未知——超时/回执缺失）
    // 保守拒撤但措辞必须区分：不是「未生效」而是「无法确认是否已生效」。
    if r.outcome != "executed" {
        let reason = if r.outcome == "unknown" {
            "结果未知（操作可能已在对端生效），撤销被保守拒绝"
        } else {
            "未真实生效"
        };
        return Err(format!(
            "凭条 {receipt_id} 的 outcome 是 `{}`（{reason}），无可撤销",
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
            "capability": "none",
            "reason": "read 类操作不改变外部状态，无需撤销",
        })));
    }

    // 域 C：清单声明 undo → 反向 op（凭条主键值回填）。R5：结果按行动
    // 账本新凭条结构化判定——不嗅探文本。
    if let Some(undo) = &op.undo {
        let key_value = r.receipt_value.clone().ok_or_else(|| {
            format!("凭条 {receipt_id} 缺主键值（receipt_value 空），无法回填撤销参数")
        })?;
        let sk = session_key_in
            .or_else(|| r.session_key.clone())
            .unwrap_or_default();
        let loop_ref = resolve_session_loop(ctx, &sk)?;
        // 派发前快照在册凭条 id：派发后新增的 = 本次反向 op 产生的账。
        let before: std::collections::HashSet<String> = receipt::list_receipts(&ws, 500, None)?
            .into_iter()
            .map(|x| x.receipt_id)
            .collect();
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
        let after = receipt::list_receipts(&ws, 500, None)?;
        let base = json!({
            "receipt_id": receipt_id,
            "via": "reverse_op",
            "capability": "reverse_op",
            "undo_op": undo.op,
            "result_text": result_text,
        });
        return Ok(Some(
            match undo_ledger_verdict(&before, &after, &c.id, &undo.op) {
                Ok(undo_rid) => json!({
                    "receipt_id": receipt_id,
                    "undone": true,
                    "via": "reverse_op",
                    "capability": "reverse_op",
                    "undo_op": undo.op,
                    "undo_receipt_id": undo_rid,
                    "result_text": result_text,
                }),
                Err(reason) => {
                    let mut v = base;
                    v["undone"] = json!(false);
                    v["reason"] = json!(reason);
                    v
                }
            },
        ));
    }

    // 域 B：write 无 undo 声明 → 会话级 rewind_last_turn。R5：只回退
    // 本地会话，外部副作用未被撤销——undone=false 如实标注（不宣称
    // 「撤销完成」）。
    if op.class == OpClass::Write {
        let sk = session_key_in
            .or(r.session_key.clone())
            .filter(|s| !s.is_empty())
            .ok_or("该 op 无 undo 声明，会话级撤销需要 session_key（凭条也未记录会话）")?;
        let loop_ref = resolve_session_loop(ctx, &sk)?;
        let report = loop_ref.undo_last_turn(&sk).await?;
        return Ok(Some(json!({
            "receipt_id": receipt_id,
            "undone": false,
            "via": "local_rewind",
            "capability": "local_rewind",
            "session_key": sk,
            "reason": "外部副作用未被撤销，仅本地会话回退",
            "report": report,
        })));
    }

    // exec/network 无 undo 声明：无通用撤销语义（能力建模 = none，诚实
    // 说明而非报错——前端按 capability 隐藏/明示）。
    Ok(Some(json!({
        "receipt_id": receipt_id,
        "undone": false,
        "via": "none",
        "capability": "none",
        "reason": format!(
            "op `{}`（{} 类）未声明 undo 且非 write 类——无通用撤销语义；如需撤销请在清单为它声明 undo 反向 op",
            op.id,
            op.class.as_str()
        ),
    })))
}

/// R5 撤销判定（纯函数，便于三臂单测）：对比反向 op 派发前后的行动账本
/// 快照，只认「新增凭条 + `outcome == "executed"`」为撤销成功。不嗅探
/// 文本——派发回显是给人看的，账才是结构化真相。
///
/// - `Ok(undo_receipt_id)`：新凭条已 executed，`undone: true`；
/// - `Err(reason)`：新凭条 outcome 非 executed（被拦/失败），或压根没落
///   新凭条——外部状态未撤销，`undone: false`。
fn undo_ledger_verdict(
    before: &std::collections::HashSet<String>,
    after: &[nemesis_connectors::receipt::Receipt],
    connector: &str,
    undo_op: &str,
) -> Result<String, String> {
    after
        .iter()
        .find(|x| !before.contains(&x.receipt_id) && x.connector == connector && x.op == undo_op)
        .map(|nr| {
            if nr.outcome == "executed" {
                Ok(nr.receipt_id.clone())
            } else {
                // 措辞与 cmd_undo 前置闸同纪律：unknown 是「结果未知（可能
                // 已生效）」不是「未生效」——同函数族内不破口径。
                let note = if nr.outcome == "unknown" {
                    "结果未知，外部状态可能未被撤销"
                } else {
                    "未真实生效，外部状态未撤销"
                };
                Err(format!("反向 op 凭条 outcome=`{}`（{note}）", nr.outcome))
            }
        })
        .unwrap_or_else(|| Err("反向 op 未落新凭条（被拦或执行失败）——外部状态未撤销".to_string()))
}

/// Dashboard 管理面（2026-10-19）：`connectors.create {dir_name, toml}` /
/// `connectors.set_enabled {dir, enabled}` / `connectors.remove {dir}`。
/// 逻辑在 nemesis-connectors registry（[`registry::create_connector`] 等三
/// 函数），此处只做参数抽取——热生效免费：`scan_connectors` 每次重读目录，
/// 下一次 `connectors.list` / `connector_invoke` 即见，零重启。语义注记：
/// 新装条目默认启用与否由清单自身的 `enabled` 键决定；启停（A3-a）恒写
/// sidecar、清单字节不动——签名条目可正常启停。
fn cmd_create(ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let data = data.ok_or("缺参数")?;
    let dir_name = data
        .get("dir_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("缺 dir_name")?;
    let toml_text = data
        .get("toml")
        .and_then(|v| v.as_str())
        .ok_or("缺 toml（connector.toml 全文）")?;
    let c = registry::create_connector(&ws, dir_name, toml_text)?;
    Ok(Some(json!({ "ok": true, "id": c.id, "dir": dir_name })))
}

fn cmd_set_enabled(ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let data = data.ok_or("缺参数")?;
    let dir = data
        .get("dir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("缺 dir")?;
    let enabled = data
        .get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or("缺 enabled（bool）")?;
    registry::set_connector_enabled(&ws, dir, enabled)?;
    Ok(Some(json!({ "ok": true, "dir": dir, "enabled": enabled })))
}

fn cmd_remove(ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let dir = data
        .and_then(|d| d.get("dir"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("缺 dir")?;
    registry::remove_connector(&ws, dir)?;
    Ok(Some(json!({
        "ok": true,
        "dir": dir,
        // 行动账本是追加式流水：删 connector 不抹历史账（前端文案注明）。
        "receipts_retained": true,
    })))
}

/// B4（2026-10-19 总开关）：`connectors.set_master {enabled}` —— 写
/// config.json `connectors.enabled`（raw RMW，只动这一键）。热生效免费：
/// 供给闸与 dispatch 闸每次现读 config（F8 hidden_tools 同族），无需重启。
/// 闸只裁 connector_find/connector_invoke 两把治理工具——原生 MCP/文件等
/// 工具不受影响（连接器≠闸门，A 批正名后的架构语义）。
fn cmd_set_master(ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
    let home = ctx.home.clone().ok_or("home not configured")?;
    let enabled = data
        .and_then(|d| d.get("enabled"))
        .and_then(|v| v.as_bool())
        .ok_or("缺 enabled（bool）")?;
    // seeded 变体：config.json 缺席（新装配态）从空对象起，总闸一键照样落键。
    crate::handlers::config::update_raw_config_seeded(&home, |cfg| {
        let obj = cfg.as_object_mut().ok_or("config.json 顶层不是对象")?;
        match obj.get_mut("connectors") {
            Some(connectors) => {
                connectors["enabled"] = json!(enabled);
            }
            None => {
                obj.insert("connectors".to_string(), json!({ "enabled": enabled }));
            }
        }
        Ok(())
    })?;
    Ok(Some(json!({ "ok": true, "enabled": enabled })))
}

/// B2（2026-10-19 独占接管）：`connectors.tool_policy {connector, tool,
/// policy}` —— per-tool 例外表 sidecar 写（`config/connector_state/<id>
/// .json`）。`exempt` = 恢复原生直调（用户显式放弃这一个接口的监管）；
/// `blocked` = 原生照隐 + 镜像不提供（能力对 agent 消失）；`default` =
/// 镜像接管基线。热生效免费：独占闸/镜像枚举每次现读 sidecar。
fn cmd_tool_policy(ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
    let ws = workspace_of(ctx)?;
    let data = data.ok_or("缺参数")?;
    let connector = data
        .get("connector")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("缺 connector")?;
    let tool = data
        .get("tool")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("缺 tool")?;
    let policy_s = data
        .get("policy")
        .and_then(|v| v.as_str())
        .ok_or("缺 policy（default|exempt|blocked）")?;
    let policy = match policy_s {
        "default" => nemesis_connectors::state::ToolPolicy::Default,
        "exempt" => nemesis_connectors::state::ToolPolicy::Exempt,
        "blocked" => nemesis_connectors::state::ToolPolicy::Blocked,
        other => return Err(format!("未知 policy `{other}`（default|exempt|blocked）")),
    };
    nemesis_connectors::state::set_tool_policy(&ws, connector, tool, policy)?;
    Ok(Some(json!({
        "ok": true,
        "connector": connector,
        "tool": tool,
        "policy": policy_s,
    })))
}

/// connector 数据面 handler（`connectors.list` / `connectors.receipts.list` /
/// `connectors.undo` / `connectors.create` / `connectors.set_enabled` /
/// `connectors.remove` / `connectors.set_master`（B4 总开关）/
/// `connectors.tool_policy`（B2 per-tool 例外表））。
pub struct ConnectorsHandler;

#[async_trait::async_trait]
impl ModuleHandler for ConnectorsHandler {
    fn module_name(&self) -> &str {
        "connectors"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "receipts.list",
            "undo",
            "create",
            "set_enabled",
            "remove",
            "set_master",
            "tool_policy",
        ]
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
            "create" => cmd_create(ctx, data.as_ref()),
            "set_enabled" => cmd_set_enabled(ctx, data.as_ref()),
            "remove" => cmd_remove(ctx, data.as_ref()),
            "set_master" => cmd_set_master(ctx, data.as_ref()),
            "tool_policy" => cmd_tool_policy(ctx, data.as_ref()),
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
