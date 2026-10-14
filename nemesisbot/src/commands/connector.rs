//! connector 注册表 CLI（行动代理 P0，2026-10-14）。
//!
//! `nemesisbot connector <action>`：list / receipts / ledger。与 WSAPI
//! `connectors.*` / `paperbroker.ledger` 同源——直读 nemesis-connectors
//! 存储（`<workspace>/connectors/` 注册表 + `data/receipts/` 凭条流水 +
//! `data/paper_broker.json` 账本，单点不双写）。执行入口是 agent 的
//! `connector_invoke` 工具（走完整安全管线），CLI 只做只读查询。

use anyhow::Result;
use clap::Subcommand;

use crate::common;
use nemesis_connectors::manifest::OpBinding;

#[derive(Debug, Subcommand)]
pub enum ConnectorAction {
    /// List registered connectors (corrupt dirs honestly listed with cause)
    List,
    /// Show recent receipts (newest first)
    Receipts {
        /// Max rows to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Filter by connector id
        #[arg(long)]
        connector: Option<String>,
    },
    /// Show the paper-broker ledger (cash / positions / orders)
    Ledger,
}

pub fn run(action: ConnectorAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let workspace = common::workspace_path(&home);
    match action {
        ConnectorAction::List => {
            let enabled = nemesis_config::load_config(&common::config_path(&home))
                .ok()
                .and_then(|cfg| cfg.connectors)
                .map(|c| c.enabled)
                .unwrap_or(true);
            let loaded = nemesis_connectors::registry::scan_connectors(&workspace);
            println!("connectors: {} (enabled: {})", loaded.len(), enabled);
            for l in &loaded {
                let dir_name = l
                    .dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                match &l.connector {
                    Some(c) => {
                        println!(
                            "  {} v{} [{}] {} (dir: {})",
                            c.id,
                            c.version,
                            format!("{:?}", c.kind).to_lowercase(),
                            c.name,
                            dir_name
                        );
                        for op in &c.ops {
                            let binding = match &op.binding {
                                OpBinding::Http { method, path, .. } => format!(
                                    "http {} {}",
                                    format!("{method:?}").to_uppercase(),
                                    path
                                ),
                                OpBinding::Tool { tool } => format!("tool {tool}"),
                                OpBinding::Action { action } => format!("action {action}"),
                            };
                            println!(
                                "    - {:<14} risk={:<8} class={:<8} {}",
                                op.id,
                                op.risk.as_str(),
                                op.class.as_str(),
                                binding
                            );
                        }
                    }
                    None => {
                        println!(
                            "  {dir_name} [corrupt] {}",
                            l.error.clone().unwrap_or_default()
                        );
                    }
                }
            }
        }
        ConnectorAction::Receipts { limit, connector } => {
            let rows =
                nemesis_connectors::receipt::list_receipts(&workspace, limit, connector.as_deref())
                    .map_err(anyhow::Error::msg)?;
            println!("receipts ({}):", rows.len());
            for r in &rows {
                let receipt = r
                    .receipt_value
                    .as_deref()
                    .map(|v| format!(" {}={}", r.receipt_key.as_deref().unwrap_or_default(), v))
                    .unwrap_or_default();
                println!(
                    "  {} {} {} {} {}{}",
                    r.receipt_id, r.ts, r.connector, r.op, r.outcome, receipt
                );
                println!("      {}", r.summary);
            }
        }
        ConnectorAction::Ledger => {
            let l =
                nemesis_connectors::ledger::load_ledger(&workspace).map_err(anyhow::Error::msg)?;
            println!("cash: {:.2}", l.cash);
            println!("positions ({}):", l.positions.len());
            for p in l.positions.values() {
                println!("  {} qty={} avg_cost={:.2}", p.symbol, p.qty, p.avg_cost);
            }
            println!("orders ({}):", l.orders.len());
            for o in &l.orders {
                println!(
                    "  {} {} {} {} qty={} price={:.2} {:?}",
                    o.order_id,
                    o.ts,
                    format!("{:?}", o.side).to_lowercase(),
                    o.symbol,
                    o.qty,
                    o.price,
                    o.status
                );
            }
        }
    }
    Ok(())
}
