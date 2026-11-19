//! Assets CLI（W1 #11+#6–10 AI-BOM）：读取 gateway 生成的资产盘点快照。
//!
//! **诚实边界**：CLI 进程不装配 gateway（无 AppState / 工具注册表 /
//! MCP manager），无法现场构建 inventory——这里只读盘上快照
//! `<workspace>/logs/assets/inventory.json`（gateway 启动即刷 +
//! 周期刷新 + WSAPI assets.* 调用即刷）。快照缺席 = 诚实报错提示
//! 先启动 gateway，不做降级编造。BOM 投影与 WSAPI 同源
//! （`nemesis_web::handlers::assets::bom_view`）。

use crate::common;
use anyhow::{Result, bail};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum AssetsAction {
    /// 打印资产盘点快照原文（gateway 生成的 inventory.json）
    Inventory,
    /// 打印 AI-BOM 视图（快照的五类组件投影：framework/model/skill/tool_plugin/mcp）
    Bom,
}

fn load_snapshot(local: bool) -> Result<serde_json::Value> {
    let home = common::resolve_home(local);
    let workspace = common::workspace_path(&home);
    let path = workspace.join("logs").join("assets").join("inventory.json");
    if !path.exists() {
        bail!(
            "资产盘点快照不存在（{}）。\n快照由 gateway 生成：启动 gateway 后自动落盘（启动即刷 + 每 assets.inventory_refresh_secs 刷新），或在 Dashboard 调 assets.inventory 即时生成。",
            path.display()
        );
    }
    let raw = nemesis_utils::fsutil::read_config_string(&path)?;
    serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("快照解析失败（{}）：{}", path.display(), e))
}

pub fn run(action: AssetsAction, local: bool) -> Result<()> {
    let doc = load_snapshot(local)?;
    match action {
        AssetsAction::Inventory => {
            println!("{}", serde_json::to_string_pretty(&doc)?);
        }
        AssetsAction::Bom => {
            let bom = nemesis_web::handlers::assets::bom_view(&doc);
            println!("{}", serde_json::to_string_pretty(&bom)?);
        }
    }
    Ok(())
}
