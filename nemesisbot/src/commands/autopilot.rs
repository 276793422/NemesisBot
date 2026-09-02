//! Autopilot command — 看板定时自动化 CLI（W2 P4）。
//!
//! 与 `issue` 命令同样直接操作共享 SQLite store（`{workspace}/board/board.db`）。
//! 注意：cron 调度由 gateway 的 live CronService 挂载——CLI 建/改的规则在
//! gateway 启动同步（`sync_autopilot_jobs`）时生效；Dashboard 端创建则即时挂载。
//! `run` 子命令在本进程没有集群连接，配置了派发目标的规则会明确拒绝（仅
//! 建单规则可正常触发），与 `fire_autopilot` 的诚实拒绝语义一致。

use crate::common;
use anyhow::Result;
use nemesis_board::BoardStore;
use nemesis_board::models::{AutopilotPatch, NewAutopilot};

#[derive(clap::Subcommand)]
pub enum AutopilotAction {
    /// List all autopilot rules
    List,
    /// Create a new autopilot rule
    Create {
        /// Rule name (unique)
        name: String,
        /// cron 表达式（5 段，如 `0 9 * * *`）
        #[arg(long)]
        cron: String,
        /// 建 issue 的标题模板（`{date}` → 当地日期）
        #[arg(long)]
        title: String,
        /// Issue 描述
        #[arg(long, default_value = "")]
        description: String,
        /// Priority: 0=low 1=medium 2=high 3=urgent
        #[arg(long, default_value_t = 1)]
        priority: i32,
        /// Project id
        #[arg(long)]
        project_id: Option<i64>,
        /// 派活目标节点 id（空 = 仅建单不派活）
        #[arg(long, default_value = "")]
        target: String,
        /// 创建为停用状态（默认启用）
        #[arg(long)]
        disabled: bool,
    },
    /// Update rule fields (only provided fields change)
    Update {
        /// Rule id（见 `autopilot list`）
        id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        cron: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        priority: Option<i32>,
        #[arg(long)]
        project_id: Option<i64>,
        #[arg(long)]
        target: Option<String>,
    },
    /// Enable a rule
    Enable { id: i64 },
    /// Disable a rule
    Disable { id: i64 },
    /// Remove a rule
    Remove { id: i64 },
    /// Trigger a rule once (targeted rules need a running cluster — rejected here)
    Run { id: i64 },
    /// Show run history (issues created by this rule, newest first)
    Runs { id: i64 },
}

/// BoardStore 返回 `Result<_, String>` → anyhow（同 issue::err）。
fn err(e: String) -> anyhow::Error {
    anyhow::Error::msg(e)
}

fn open_store(local: bool) -> Result<std::sync::Arc<BoardStore>> {
    let home = common::resolve_home(local);
    let db = common::workspace_path(&home).join("board").join("board.db");
    Ok(std::sync::Arc::new(
        BoardStore::open(&db, "NB").map_err(|e| anyhow::anyhow!("打开看板库失败: {e}"))?,
    ))
}

fn print_autopilot(ap: &nemesis_board::Autopilot) {
    let cron_state = if ap.cron_job_id.is_some() {
        "已挂载"
    } else {
        "未挂载（等 gateway 启动同步）"
    };
    let last_run = ap
        .last_run_at
        .map(|t| format!("{t}"))
        .unwrap_or_else(|| "从未运行".to_string());
    println!(
        "#{} [{}] {} — cron `{}` 标题 `{}` 目标 {}（{}；最近运行: {last_run}）",
        ap.id,
        if ap.enabled { "启用" } else { "停用" },
        ap.name,
        ap.cron,
        ap.title,
        if ap.target.is_empty() {
            "仅建单"
        } else {
            &ap.target
        },
        cron_state
    );
}

pub fn run(action: AutopilotAction, local: bool) -> Result<()> {
    let store = open_store(local)?;
    let actor = nemesis_board::assignment::Actor::admin("cli");
    match action {
        AutopilotAction::List => {
            let autopilots = store.list_autopilots().map_err(err)?;
            if autopilots.is_empty() {
                println!("（无 autopilot 规则；用 `autopilot create` 创建）");
                return Ok(());
            }
            for ap in &autopilots {
                print_autopilot(ap);
            }
            println!("共 {} 条", autopilots.len());
        }
        AutopilotAction::Create {
            name,
            cron,
            title,
            description,
            priority,
            project_id,
            target,
            disabled,
        } => {
            nemesis_cron::CronService::validate_schedule(&cron)
                .map_err(|e| anyhow::anyhow!("cron 表达式无效: {e}"))?;
            let ap = store
                .create_autopilot(&NewAutopilot {
                    name,
                    cron,
                    title,
                    description,
                    priority,
                    project_id,
                    target,
                    enabled: !disabled,
                })
                .map_err(err)?;
            println!(
                "已创建 autopilot #{}（cron 挂载于 gateway 启动同步时生效）",
                ap.id
            );
            print_autopilot(&ap);
        }
        AutopilotAction::Update {
            id,
            name,
            cron,
            title,
            description,
            priority,
            project_id,
            target,
        } => {
            if let Some(c) = cron.as_deref() {
                nemesis_cron::CronService::validate_schedule(c)
                    .map_err(|e| anyhow::anyhow!("cron 表达式无效: {e}"))?;
            }
            let ap = store
                .update_autopilot(
                    id,
                    &AutopilotPatch {
                        name,
                        cron,
                        title,
                        description,
                        priority,
                        project_id,
                        target,
                        enabled: None,
                    },
                )
                .map_err(err)?;
            println!(
                "已更新 autopilot #{}（cron 挂载于 gateway 启动同步时生效）",
                ap.id
            );
            print_autopilot(&ap);
        }
        AutopilotAction::Enable { id } => {
            let ap = store
                .update_autopilot(
                    id,
                    &AutopilotPatch {
                        enabled: Some(true),
                        ..Default::default()
                    },
                )
                .map_err(err)?;
            println!("已启用");
            print_autopilot(&ap);
        }
        AutopilotAction::Disable { id } => {
            let ap = store
                .update_autopilot(
                    id,
                    &AutopilotPatch {
                        enabled: Some(false),
                        ..Default::default()
                    },
                )
                .map_err(err)?;
            println!("已停用");
            print_autopilot(&ap);
        }
        AutopilotAction::Remove { id } => {
            let removed = store.remove_autopilot(id).map_err(err)?;
            if removed {
                println!("已删除 autopilot #{}", id);
            } else {
                println!("autopilot #{} 不存在", id);
            }
        }
        AutopilotAction::Run { id } => {
            let ap = store.get_autopilot(id).map_err(err)?;
            let out = {
                #[cfg(feature = "cluster")]
                {
                    // CLI 进程无集群连接：目标规则由 fire_autopilot 明确拒绝。
                    nemesis_web::handlers::board::fire_autopilot(&store, None, &ap, &actor)
                        .map_err(err)?
                }
                #[cfg(not(feature = "cluster"))]
                {
                    nemesis_web::handlers::board::fire_autopilot(&store, &ap, &actor)
                        .map_err(err)?
                }
            };
            println!(
                "已触发：建单 {}",
                out.get("issue_number")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
            );
            if out.get("dispatch").is_some() {
                println!("（本次为仅建单：CLI 进程不连集群；到点自动触发由 gateway 派发）");
            }
        }
        AutopilotAction::Runs { id } => {
            let issues = store
                .list_issues_by_origin("autopilot", &id.to_string(), 20)
                .map_err(err)?;
            if issues.is_empty() {
                println!("（该规则从未运行）");
                return Ok(());
            }
            for issue in &issues {
                println!("#{} [{}] {}", issue.number, issue.status, issue.title);
            }
            println!("共 {} 条", issues.len());
        }
    }
    Ok(())
}
