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

/// gateway 在位守卫（main 派发处对增删改臂调用；`List` 只读不拦）。
///
/// 为什么拦：`jobs.json` 是跨进程真相源，但 gateway 以**内存态写穿**——
/// 任一任务触发 / 任意 WSAPI 变更都会全量保存。gateway 运行中用本 CLI
/// 增删改，改动会在 gateway 下次保存时被整体覆盖（静默丢失，且重启也
/// 救不回——盘上已无此任务）。运行中请走 Dashboard 定时任务页
/// （WSAPI `tasks.cron.*`，与 gateway 同进程，天然无此问题），或先停掉
/// gateway 再用 CLI。
///
/// 检测链复用 estop/dashboard 的跨进程先例：`<workspace>/state/gateway.json`
/// 存在且 `web_port>0` → GET `/api/health` 探活；**只有确认 200 才拦**——
/// 无 state 文件 / 端口无效 / 探活失败（stale 文件、进程已死）一律放行，
/// 宁可漏拦也不误伤 gateway 不在场的正常 CLI 使用。
pub(crate) async fn ensure_no_live_gateway(action: &CronAction, local: bool) -> Result<()> {
    if matches!(action, CronAction::List) {
        return Ok(());
    }
    let home = common::resolve_home(local);
    let state_path =
        nemesis_path::resolve_gateway_state_path_in_workspace(&common::workspace_path(&home));
    let Some(info) = crate::commands::dashboard::read_gateway_state(&state_path) else {
        return Ok(());
    };
    if info.web_port <= 0 {
        return Ok(());
    }
    let base_url = format!("http://{}:{}", info.web_host, info.web_port);
    if crate::commands::dashboard::check_health(&base_url)
        .await
        .is_ok()
    {
        anyhow::bail!(
            "检测到 gateway 正在运行（{base_url}）——运行中的 gateway 以内存态写穿定时任务存储，\
             本 CLI 的增删改会在它下次保存时被覆盖（静默丢失）。\
             请在 Dashboard 的定时任务页操作，或先停掉 gateway 再用 CLI。"
        );
    }
    Ok(())
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
