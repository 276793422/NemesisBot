//! Autopilot command — 看板定时自动化 CLI。
//!
//! 与 `issue` 命令同样直接操作共享 SQLite store（`{workspace}/board/board.db`）。
//! 注意：cron 调度由 gateway 的 live CronService 挂载——CLI 建/改的规则在
//! gateway 启动同步（`sync_autopilot_jobs`）时生效；Dashboard 端创建则即时挂载。
//! `run` 子命令在本进程没有集群连接，配置了派发目标的规则会明确拒绝（仅
//! 建单规则可正常触发），与 `fire_autopilot` 的诚实拒绝语义一致。

use crate::common;
use anyhow::Result;
use nemesis_board::BoardStore;
use nemesis_board::models::{
    AUTOPILOT_MODE_LIVE, AUTOPILOT_MODE_SHADOW, AutopilotPatch, NewAutopilot,
};

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
        /// 验收标准（触发建单时透传；空 = 不填，评审将保守转人工）
        #[arg(long, default_value = "")]
        acceptance_criteria: String,
        /// 绑定发起会话 session_key（父会话存活时建单继承其上下文；父会话
        /// clear/删除后规则自动暂停。空 = 不绑定，行为与旧版一致）
        #[arg(long, default_value = "")]
        session_key: String,
        /// 直上全真模式（live）。缺省 = 影子模式（shadow，D3 拍板）：首轮
        /// 触发只建单 + 决策意图评论 + 本地留痕，不派发不拆解；确认行为
        /// 后 `autopilot update <id> --live` 转全真
        #[arg(long)]
        live: bool,
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
        /// 验收标准（提供即覆盖；`--acceptance-criteria ""` = 清空）
        #[arg(long)]
        acceptance_criteria: Option<String>,
        /// 绑定发起会话 session_key（提供即覆盖；`--session-key ""` = 解除绑定）
        #[arg(long)]
        session_key: Option<String>,
        /// 切到全真模式（live；影子转正。与 --shadow 互斥）
        #[arg(long)]
        live: bool,
        /// 切到影子模式（shadow；live 回退观测。与 --live 互斥）
        #[arg(long)]
        shadow: bool,
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
        "#{} [{}] [{}] {} — cron `{}` 标题 `{}` 目标 {}（{}；最近运行: {last_run}）{}",
        ap.id,
        if ap.enabled { "启用" } else { "停用" },
        // B34 触发模式显示（D3：新规则默认影子）。
        if ap.mode == AUTOPILOT_MODE_SHADOW {
            "影子"
        } else {
            "全真"
        },
        ap.name,
        ap.cron,
        ap.title,
        if ap.target.is_empty() {
            "仅建单"
        } else {
            &ap.target
        },
        cron_state,
        // 父会话绑定显示（绑定 = 建单继承父会话上下文）。
        ap.session_key
            .as_deref()
            .map(|k| format!(" 父会话: {k}"))
            .unwrap_or_default(),
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
            acceptance_criteria,
            session_key,
            live,
            disabled,
        } => {
            // 语法 + 语义双闸：解析通过但永不匹配的
            // 表达式在写 store 前拒绝，与 WSAPI 同一校验单点。
            nemesis_web::handlers::board::validate_autopilot_schedule(&cron, !disabled)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            // D3 拍板：新规则默认影子起步，显式 --live 才直上全真。
            let mode = if live {
                AUTOPILOT_MODE_LIVE
            } else {
                AUTOPILOT_MODE_SHADOW
            };
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
                    auto_plan: false,
                    // F-：验收标准透传（CLI 空串 = 不填）。
                    acceptance_criteria: Some(acceptance_criteria).filter(|s| !s.trim().is_empty()),
                    // 父会话绑定（CLI 空串 = 不绑定）。
                    session_key: Some(session_key).filter(|s| !s.trim().is_empty()),
                    mode: mode.to_string(),
                })
                .map_err(err)?;
            if mode == AUTOPILOT_MODE_SHADOW {
                println!(
                    "已创建 autopilot #{}（影子模式：触发只建单留痕不派发；确认行为后 `autopilot update {} --live` 转全真；cron 挂载于 gateway 启动同步时生效）",
                    ap.id, ap.id
                );
            } else {
                println!(
                    "已创建 autopilot #{}（全真模式；cron 挂载于 gateway 启动同步时生效）",
                    ap.id
                );
            }
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
            acceptance_criteria,
            session_key,
            live,
            shadow,
        } => {
            // 语法 + 语义双闸：按「合并后的有效
            // cron + 有效 enabled」校验，与 WSAPI 同一校验单点。
            let old = store.get_autopilot(id).map_err(err)?;
            let eff_cron = cron.clone().unwrap_or(old.cron);
            let eff_enabled = old.enabled;
            nemesis_web::handlers::board::validate_autopilot_schedule(&eff_cron, eff_enabled)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            // B34 模式切换（影子⇄全真双向；不提供 = 不改）。
            let mode = match (live, shadow) {
                (true, true) => {
                    return Err(anyhow::anyhow!("--live 与 --shadow 互斥，一次只切一个方向"));
                }
                (true, false) => Some(AUTOPILOT_MODE_LIVE.to_string()),
                (false, true) => Some(AUTOPILOT_MODE_SHADOW.to_string()),
                (false, false) => None,
            };
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
                        auto_plan: None,
                        acceptance_criteria,
                        // `--session-key ""` = 解除绑定；不提供 = 不改。
                        session_key: session_key.map(|s| s.trim().to_string()),
                        // 模式切换走 update_autopilot 的 validate（update
                        // 判 mode 是推进授权版本的内容 patch——updated_at
                        // bump → 血统版本戳推进，语义正确）。
                        mode,
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
            let ap = store.get_autopilot(id).map_err(err)?;
            // 死排程闸：启用死规则 = 复活静默僵尸，
            // loud 拒绝（先修 cron 再启用）。
            nemesis_web::handlers::board::validate_autopilot_schedule(&ap.cron, true)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
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
            // B34：影子留痕落 `<workspace>/logs/autopilot_shadow/`——gateway
            // 启动时已装配工作区根，CLI 手跑此前缺位，这里补装（幂等，
            // 重复装配保留首份；未装 = 影子只留看板评论不留本地文件）。
            {
                let ws = common::workspace_path(&common::resolve_home(local));
                nemesis_web::handlers::board::install_handoff_workspace(ws);
            }
            let out = {
                #[cfg(feature = "cluster")]
                {
                    // CLI 进程无集群连接：目标规则由 fire_autopilot 明确拒绝；
                    // auto_plan 需 moderator/事件上下文，同样诚实降级（None）。
                    nemesis_web::handlers::board::fire_autopilot(&store, None, &ap, &actor, None)
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
            if out.get("shadow") == Some(&serde_json::Value::Bool(true)) {
                println!(
                    "（影子模式：仅建单 + 决策意图评论 + 本地留痕，未派发未拆解；确认行为后 `autopilot update {id} --live` 转全真）"
                );
            } else {
                let dispatch = out.get("dispatch");
                if dispatch
                    .and_then(|d| d.get("parked"))
                    .and_then(|v| v.as_str())
                    == Some("inflight_full")
                {
                    println!("（人优先槽位：节点在途派发满载，本单已停车待自动重试）");
                } else if dispatch.is_some() {
                    println!("（本次为仅建单：CLI 进程不连集群；到点自动触发由 gateway 派发）");
                }
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

#[cfg(test)]
mod autopilot_tests;
