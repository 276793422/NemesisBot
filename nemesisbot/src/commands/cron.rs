//! Cron command - manage scheduled jobs.
//!
//! 存储读写统一走 `nemesis-cron`（`CronService` + `store` 模块单一真相源，
//! 与 gateway 同一形态；旧版手拼裸数组是 docs/REPORT/2026-10-13_cron-
//! store-format-split-cli-vs-gateway-fix.md 记录的格式分裂 BUG，已修）。
//! 本文件不再出现任何存储 JSON 拼装。

use crate::common;
use anyhow::Result;
use nemesis_cron::{CronSchedule, CronService};

#[derive(clap::Subcommand)]
pub enum CronAction {
    /// List all scheduled jobs
    List,
    /// Add a new scheduled job
    Add {
        /// Job name
        #[arg(short, long)]
        name: String,
        /// Message for agent
        #[arg(short, long)]
        message: String,
        /// Run every N seconds
        #[arg(short, long)]
        every: Option<u64>,
        /// Cron expression
        #[arg(short, long)]
        cron: Option<String>,
        /// Deliver response to channel
        #[arg(short, long)]
        deliver: bool,
        /// Recipient for delivery
        #[arg(long)]
        to: Option<String>,
        /// Channel for delivery
        #[arg(long)]
        channel: Option<String>,
    },
    /// Remove a job by ID
    Remove {
        /// Job ID
        id: String,
    },
    /// Enable a job
    Enable {
        /// Job ID
        id: String,
    },
    /// Disable a job
    Disable {
        /// Job ID
        id: String,
    },
}

pub fn run(action: CronAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let store_path = common::cron_store_path(&home);

    // REL-002 损坏守卫：存储存在但解析失败（含旧形态迁移失败）⇒ 拒绝一切
    // 改动——后续 save 会拿空/半态 store 顶掉盘上未知数据。
    if store_path.exists() {
        nemesis_cron::store::load(&store_path).map_err(|e| {
            anyhow::anyhow!(
                "定时任务存储已损坏，已中止操作以免覆盖既有任务: {} ({e})",
                store_path.display()
            )
        })?;
    }

    // 变更/查询全部经 CronService（id 生成、时间戳、state、落盘回滚与
    // gateway 同一实现）。service 不 spawn 后台线程（start() 才会），drop 安全。
    let svc = CronService::new(&store_path.to_string_lossy());

    match action {
        CronAction::List => {
            println!("Scheduled Jobs");
            println!("===============");
            if !store_path.exists() {
                println!("  No scheduled jobs.");
                println!("  Add one with: nemesisbot cron add -n <name> -m <message> -e <seconds>");
                return Ok(());
            }
            let jobs = svc.list_jobs(true);
            if jobs.is_empty() {
                println!("  No scheduled jobs.");
            } else {
                for job in jobs {
                    println!("  {} ({})", job.name, job.id);
                    println!("    Schedule: {}", describe_schedule(&job.schedule));
                    println!(
                        "    Status: {}",
                        if job.enabled { "enabled" } else { "disabled" }
                    );
                    println!();
                }
            }
        }
        CronAction::Add {
            name,
            message,
            every,
            cron,
            deliver,
            to,
            channel,
        } => {
            let schedule = if let Some(secs) = every {
                if secs == 0 {
                    anyhow::bail!("--every 必须 > 0 秒（0 秒间隔的任务永远不会触发）");
                }
                CronSchedule {
                    kind: "every".to_string(),
                    at_ms: None,
                    every_ms: Some((secs * 1000) as i64),
                    expr: None,
                    tz: None,
                }
            } else if let Some(ref cron_expr) = cron {
                CronService::validate_schedule(cron_expr)
                    .map_err(|e| anyhow::anyhow!("cron 表达式无效: {e}"))?;
                CronSchedule {
                    kind: "cron".to_string(),
                    at_ms: None,
                    every_ms: None,
                    expr: Some(cron_expr.clone()),
                    tz: None,
                }
            } else {
                println!("Error: Either --every or --cron must be specified.");
                return Ok(());
            };

            let job = svc
                .add_job_ext(
                    &name,
                    schedule,
                    &message,
                    deliver,
                    channel.as_deref(),
                    to.as_deref(),
                    None,
                    None,
                    true,
                )
                .map_err(anyhow::Error::msg)?;
            println!("Added job '{}' ({})", name, job.id);
        }
        CronAction::Remove { id } => match svc.remove_job(&id) {
            Ok(true) => println!("Removed job {}", id),
            Ok(false) => println!("Job {} not found.", id),
            Err(e) => return Err(anyhow::anyhow!("删除任务落盘失败（任务保留）: {e}")),
        },
        CronAction::Enable { id } => toggle(&svc, &id, true),
        CronAction::Disable { id } => toggle(&svc, &id, false),
    }
    Ok(())
}

fn toggle(svc: &CronService, id: &str, enabled: bool) {
    match svc.enable_job(id, enabled) {
        Ok(_) => println!(
            "Job {} {}",
            id,
            if enabled { "enabled" } else { "disabled" }
        ),
        Err(_) => println!("Job {} not found.", id),
    }
}

/// 人类可读的调度描述（与 gateway 侧语义同源：every/at/cron 三态）。
fn describe_schedule(schedule: &CronSchedule) -> String {
    match schedule.kind.as_str() {
        "every" => format!("every {}s", schedule.every_ms.unwrap_or(0) / 1000),
        "at" => schedule
            .at_ms
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| "at (已过期)".to_string()),
        "cron" => schedule
            .expr
            .as_deref()
            .map(CronService::describe_schedule)
            .unwrap_or_else(|| "cron".to_string()),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
