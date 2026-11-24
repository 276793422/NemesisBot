//! Durable loop 管理 CLI（`loop` 子命令；`loop` feature 门控；项5）。
//!
//! 与 WSAPI handlers/loops.rs、gateway 驱动器、agent 三工具共用同一状态
//! 真相源 `<workspace>/loops/{id}.json`（nemesis-loop store）。pause/resume
//! 是**人**的操作（agent 工具面无暂停/恢复——防自锁）；resume 是停车场
//! 预算调整后的解除入口（配合 update 语义：本 CLI 的 create 不改既有
//! budget，调预算走 WSAPI `loops.update` 或直接编辑状态文件后 resume）。
//!
//! create 绑定校验：`--session` 必填（sanitize 成 session_key），
//! `session_occupied`（jsonl ∥ meta）不占用 = 拒绝——不静默绑死会话。
//! token 基线：`--max-tokens` >0 时直开 usage 账本（与 gateway 同路径
//! 真相源 workspace_data_dir）记创建时刻会话已耗 token。

use nemesis_agent::cc_hooks::WEB_SESSION_KEY_PREFIX;
use std::path::Path;

use anyhow::{Result, bail};

use crate::common;

// ---------------------------------------------------------------------------
// Clap subcommands
// ---------------------------------------------------------------------------

#[derive(clap::Subcommand)]
pub enum LoopAction {
    /// 创建 loop（绑定已有会话；不静默创建会话）
    Create {
        /// 目标文本（唤醒消息原文引用）
        goal: String,
        /// 绑定的会话 ID（sanitize 成 agent:main:session:{id}；会话必须在场）
        #[arg(long)]
        session: String,
        /// 首次唤醒延迟秒（缺省 = 不排程，等 agent loop_wakeup 自配速）
        #[arg(long, default_value_t = 0)]
        delay_secs: u64,
        /// 周期 cron 表达式（与 --delay-secs 互斥；本地时区）
        #[arg(long)]
        cron: Option<String>,
        /// 唤醒次数预算（0 = 不限）
        #[arg(long, default_value_t = 0)]
        max_turns: u64,
        /// 墙钟预算秒（创建起算，跨重启；0 = 不限）
        #[arg(long, default_value_t = 0)]
        wall_clock_secs: u64,
        /// token 预算（usage 台账喂入，只算 loop 期间；0 = 不限）
        #[arg(long, default_value_t = 0)]
        max_tokens: u64,
    },
    /// 列出全部 loop
    List,
    /// 查看单个 loop 详情（含 progress_log）
    Show {
        /// loop ID（如 L1）
        id: String,
    },
    /// 暂停（人操作；驱动器停止投递）
    Pause {
        /// loop ID
        id: String,
    },
    /// 恢复（paused/parked → active；停车场解除入口）
    Resume {
        /// loop ID
        id: String,
    },
    /// 删除 loop 状态文件
    Remove {
        /// loop ID
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

pub async fn run(action: LoopAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let ws = common::workspace_path(&home);
    let dir = nemesis_loop::loops_dir(&ws);

    match action {
        LoopAction::Create {
            goal,
            session,
            delay_secs,
            cron,
            max_turns,
            wall_clock_secs,
            max_tokens,
        } => cmd_create(
            &dir,
            &home,
            &goal,
            &session,
            delay_secs,
            cron,
            max_turns,
            wall_clock_secs,
            max_tokens,
        ),
        LoopAction::List => cmd_list(&dir),
        LoopAction::Show { id } => cmd_show(&dir, &id),
        LoopAction::Pause { id } => cmd_pause_resume(&dir, &id, true),
        LoopAction::Resume { id } => cmd_pause_resume(&dir, &id, false),
        LoopAction::Remove { id } => cmd_remove(&dir, &id),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn cmd_create(
    dir: &Path,
    home: &Path,
    goal: &str,
    session: &str,
    delay_secs: u64,
    cron: Option<String>,
    max_turns: u64,
    wall_clock_secs: u64,
    max_tokens: u64,
) -> Result<()> {
    let goal = goal.trim();
    if goal.is_empty() {
        bail!("goal 不能为空");
    }
    let session_key = format!(
        "{}{}",
        WEB_SESSION_KEY_PREFIX,
        nemesis_agent::session::SessionStore::sanitize_session_id(session)
    );
    if !nemesis_agent::chat_log::session_occupied(&session_key) {
        bail!("会话 {session_key} 不存在（先建会话再绑定 loop；不静默创建）");
    }
    let ts = now_ms();
    // 创建串行化（BUG-20261119-186）：alloc+save 收进 create_loop 进程级
    // 锁——CLI 与 gateway 内 WSAPI 并发创建时，进程内路径同锁互斥；跨进程
    // 窗口是诚实边界（见 store.rs CREATE_LOCK 注释）。
    let st = nemesis_loop::create_loop(dir, |id| {
        let mut st =
            nemesis_loop::LoopState::new(id.clone(), goal.to_string(), session_key.clone(), ts);
        st.budget = nemesis_loop::LoopBudget {
            max_turns_total: max_turns,
            wall_clock_budget_secs: wall_clock_secs,
            max_tokens,
        };
        // token 基线：显式配了 max_tokens 且账本在场 → 记创建时刻已耗 token
        // （spent.tokens = 现值 − 基线，只算 loop 期间；账本缺席 = 基线 0 诚实
        // 降级，驱动器按全量现值计——warn 提示）。
        if max_tokens > 0 {
            let db_path = nemesis_path::workspace_data_dir(home).join("nemesisbot_data.db");
            match nemesis_data::DataStore::open(&db_path) {
                Ok(ds) => match ds.aggregate_session_usage(&session_key) {
                    Ok(agg) => {
                        st.tokens_base =
                            (agg.input_tokens.max(0) + agg.output_tokens.max(0)) as u64;
                    }
                    Err(e) => eprintln!("⚠ token 基线读取失败（{e}）；预算按全量现值计"),
                },
                Err(e) => eprintln!(
                    "⚠ 用量账本不可用（{}：{e}）；token 预算按全量现值计",
                    db_path.display()
                ),
            }
        }
        // 初始排程（可选）：给了 delay/cron 就立刻排，不给 = 等 agent 自配速。
        if delay_secs > 0 || cron.is_some() {
            let (wake, cron_set) = nemesis_loop::compute_next_wake(
                ts,
                if delay_secs == 0 {
                    None
                } else {
                    Some(delay_secs)
                },
                cron.as_deref(),
            )?;
            st.next_wake_ms = wake;
            st.cron = cron_set;
        }
        st.push_progress(ts, format!("[create] 目标：{goal}（人经 CLI 创建）"));
        Ok(st)
    })
    .map_err(anyhow::Error::msg)?;
    let id = st.id.clone();

    println!("Loop {id} created");
    println!("  目标:       {goal}");
    println!("  会话:       {session_key}");
    println!(
        "  预算:       turns={} wall_clock={}s tokens={}",
        max_turns, wall_clock_secs, max_tokens
    );
    match st.next_wake_ms {
        Some(ms) => println!("  首次唤醒:   {}", nemesis_loop::format_wake_time(ms)),
        None => println!("  首次唤醒:   未排程（等 agent loop_wakeup 自配速）"),
    }
    println!();
    println!("gateway 在跑时驱动器按排程注入唤醒；急停期间冻结，release 后补投。");
    Ok(())
}

// ---------------------------------------------------------------------------
// List / Show
// ---------------------------------------------------------------------------

fn cmd_list(dir: &Path) -> Result<()> {
    let all = nemesis_loop::list_loops(dir);
    if all.is_empty() {
        println!("（无 loop）");
        return Ok(());
    }
    println!(
        "{:<5} {:<8} {:<28} {:<24} 目标",
        "ID", "状态", "下次唤醒", "绑定会话"
    );
    for st in &all {
        let wake = st
            .next_wake_ms
            .map(nemesis_loop::format_wake_time)
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<5} {:<8} {:<28} {:<24} {}",
            st.id,
            st.status.as_str(),
            wake,
            st.session_key,
            truncate(&st.goal, 40)
        );
    }
    println!();
    println!("共 {} 条", all.len());
    Ok(())
}

fn cmd_show(dir: &Path, id: &str) -> Result<()> {
    let st = nemesis_loop::load_loop(dir, id)
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow::anyhow!("loop {id} 不存在"))?;
    println!("Loop {}", st.id);
    println!("─────────────────────────────────────");
    println!("  目标:       {}", st.goal);
    println!("  状态:       {}", st.status.as_str());
    println!("  会话:       {}", st.session_key);
    if !st.plan.is_empty() {
        println!("  计划:       {}", st.plan);
    }
    println!(
        "  下次唤醒:   {}",
        st.next_wake_ms
            .map(nemesis_loop::format_wake_time)
            .unwrap_or_else(|| "-".to_string())
    );
    if let Some(ref cron) = st.cron {
        println!("  cron:       {cron}");
    }
    println!(
        "  预算:       turns {}/{} · wall_clock {}/{}s · tokens {}/{}",
        st.spent.turns,
        st.budget.max_turns_total,
        st.spent.wall_clock_secs,
        st.budget.wall_clock_budget_secs,
        st.spent.tokens,
        st.budget.max_tokens
    );
    println!(
        "  创建:       {}",
        nemesis_loop::format_wake_time(st.started_at_ms)
    );
    if let Some(ref summary) = st.finish_summary {
        println!("  收尾总结:   {summary}");
    }
    println!("  进度 (最近 {}):", st.progress_log.len().min(10));
    for e in st.progress_tail(10) {
        println!(
            "    [{}] {}",
            nemesis_loop::format_wake_time(e.at_ms),
            e.text
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pause / Resume / Remove
// ---------------------------------------------------------------------------

fn cmd_pause_resume(dir: &Path, id: &str, pause: bool) -> Result<()> {
    // 锁内 RMW（BUG-20261125-003）：状态机校验与落盘用锁内最新状态，
    // 防与驱动器/agent 工具交错丢更新（pause 被旧快照写回复活）。
    let ts = now_ms();
    let st = nemesis_loop::update_loop(dir, id, |st| {
        if pause {
            if st.status != nemesis_loop::LoopStatus::Active {
                return Err(format!(
                    "loop {} 当前状态 {}，仅 active 可暂停",
                    st.id,
                    st.status.as_str()
                ));
            }
            st.status = nemesis_loop::LoopStatus::Paused;
            st.push_progress(ts, "[paused] 人经 CLI 暂停".to_string());
        } else {
            if st.status != nemesis_loop::LoopStatus::Paused
                && st.status != nemesis_loop::LoopStatus::Parked
            {
                return Err(format!(
                    "loop {} 当前状态 {}，仅 paused/parked 可恢复",
                    st.id,
                    st.status.as_str()
                ));
            }
            st.status = nemesis_loop::LoopStatus::Active;
            st.push_progress(ts, "[resumed] 人经 CLI 恢复".to_string());
            if st.next_wake_ms.is_none() && st.cron.is_none() {
                println!("  注意：当前未排程——agent 下一次 loop_wakeup 或编辑状态文件后才会被唤醒");
            }
        }
        st.updated_at_ms = ts;
        Ok(true)
    })
    .map_err(anyhow::Error::msg)?;
    if pause {
        println!("Loop {} paused", st.id);
    } else {
        println!("Loop {} resumed", st.id);
    }
    Ok(())
}

fn cmd_remove(dir: &Path, id: &str) -> Result<()> {
    let existed = nemesis_loop::remove_loop(dir, id).map_err(anyhow::Error::msg)?;
    if !existed {
        bail!("loop {id} 不存在");
    }
    println!("Loop {id} removed");
    Ok(())
}

/// 目标列截断（显示用，rune 安全）。
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}
