//! Dossier command — 任务档案导出 CLI（2026-09-29 goal）。
//!
//! 把一个任务树（`NB-N` 及全部后代）或整个项目相关的全部记录整合成
//! 自包含目录（默认 `<workspace>/logs/dossiers/`）。数据面全 master 本机
//! （board.db + 项目档案 + 收件箱 + 用量账本），零 RPC。导出核心与
//! WSAPI `dossier.export` 同源（`nemesis_board::dossier`）。

use crate::common;
use anyhow::Result;
use nemesis_board::BoardStore;
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum DossierAction {
    /// Export an issue tree (number like NB-1) or a whole project's records
    Export {
        /// Issue number like NB-1（与 --project-id 二选一）
        issue: Option<String>,
        /// Export all issues of this project instead
        #[arg(long)]
        project_id: Option<i64>,
        /// Output root dir (default: <workspace>/logs/dossiers)
        #[arg(long)]
        out: Option<String>,
    },
}

fn open_store(local: bool) -> Result<BoardStore> {
    let home = common::resolve_home(local);
    let db = common::workspace_path(&home).join("board").join("board.db");
    BoardStore::open(&db, "NB").map_err(|e| anyhow::anyhow!("打开看板库失败: {e}"))
}

/// 用量闭包：直开 DataStore（与 gateway 同一路径真相源 workspace_data_dir）；
/// 打开失败 = 无账（usage.csv 缺行诚实，不阻塞导出）。
fn usage_closer(local: bool) -> impl Fn(&str) -> Option<nemesis_board::dossier::DossierUsage> {
    let home = common::resolve_home(local);
    let db_path = nemesis_path::workspace_data_dir(&home).join("nemesisbot_data.db");
    let ds = nemesis_data::DataStore::open(&db_path).ok();
    if ds.is_none() {
        eprintln!(
            "⚠ 用量账本不可用（{}），usage.csv 将缺行",
            db_path.display()
        );
    }
    move |task_id: &str| {
        let ds = ds.as_ref()?;
        ds.aggregate_session_usage_by_task(task_id).ok().map(|agg| {
            nemesis_board::dossier::DossierUsage {
                input_tokens: agg.input_tokens,
                output_tokens: agg.output_tokens,
            }
        })
    }
}

pub fn run(action: DossierAction, local: bool) -> Result<()> {
    match action {
        DossierAction::Export {
            issue,
            project_id,
            out,
        } => {
            let store = open_store(local)?;
            let home = common::resolve_home(local);
            let workspace = common::workspace_path(&home);
            let out_root: PathBuf = match out {
                Some(p) => PathBuf::from(p),
                None => workspace.join("logs").join("dossiers"),
            };
            let usage_of = usage_closer(local);

            let outcome = if let Some(number) = issue {
                nemesis_board::dossier::export_issue_tree(
                    &store,
                    number.trim(),
                    &workspace,
                    &out_root,
                    &usage_of,
                )
                .map_err(anyhow::Error::msg)?
            } else if let Some(pid) = project_id {
                nemesis_board::dossier::export_project(
                    &store, pid, &workspace, &out_root, &usage_of,
                )
                .map_err(anyhow::Error::msg)?
            } else {
                anyhow::bail!("需要任务编号（NB-N）或 --project-id <ID> 之一");
            };

            println!("✓ 任务档案已导出: {}", outcome.root.display());
            println!(
                "  任务 {} 个；缺口注记 {} 条",
                outcome.issue_numbers.len(),
                outcome.notes.len()
            );
            for n in &outcome.notes {
                println!("  ⚠ {n}");
            }
            Ok(())
        }
    }
}
