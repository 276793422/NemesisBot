//! U-B（§9.2 无人值守）：LLM 端点不可用**恢复探针**。
//!
//! 封存判定与中断档案在 web 侧（[`nemesis_web::handlers::board_unavailable`]，
//! master 拆解/评审/重派三闸同 crate）。本模块补全恢复闭环：
//!
//! - **探测**：封存期间周期性发最小真实 completion（TCP 通 ≠ LLM 活——
//!   半开连接/网关假活只有真推理能戳穿）。成功 = 端点恢复的直接证据，
//!   恰好解封（边沿）时触发中断续做。
//! - **续做**：读 `<workspace>/board/interruptions/` 待续档案，按
//!   `pending_action` 分派既有恢复路径（review→复评 / plan→续拆 /
//!   project→项目收口复评），处理完归档（归档不删）。
//! - **优先级**：estop 人工急停 > 探针（estop 挂起时不探测不恢复——
//!   恢复动作也是 agent 活动）；封存独立于 estop（release 不解封）。
//! - **probe_interval_secs=0** = 只封存不探测（恢复靠人工
//!   `board.unavailable.release` 或端点恢复后的自然评审成功）。
//!
//! 探针 spawn 于 gateway 装配（post_agent.rs N1 恢复同块）；全部节点装配
//! 但 loop 内角色闸——非 master 只记账不续做（board 写路径是 master 专属，
//! worker 镜像不跑评审/拆解）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use nemesis_web::handlers::board_unavailable as ub;

use crate::board_review::BoardReviewDeps;

/// 探针 + 恢复依赖（gateway 装配一次快照传入）。
pub struct UnavailableProbeDeps {
    pub review: BoardReviewDeps,
    /// home 根（probe_interval_secs 等 config 现读——改配置即时生效）。
    pub home: PathBuf,
    /// 事件中枢（续拆链 SSE 事件用；None 诚实降级）。
    pub hub: Option<Arc<nemesis_web::events::EventHub>>,
}

/// 装配入口（gateway 调用；tokio runtime 内）。
pub fn spawn_unavailable_probe(deps: UnavailableProbeDeps) {
    tokio::spawn(async move {
        probe_loop(deps).await;
    });
}

/// tick 粒度（秒）：min(配置间隔, 30) 再 clamp 5..=30——状态变化（estop/
/// 手动放行/配置热改）快速感知，真实探测仍按配置间隔节流。配置间隔小于
/// 5s 的极端值按 5s 下限（不空转）。
fn probe_tick_secs(interval_secs: u64) -> u64 {
    interval_secs.min(30).clamp(5, 30)
}

/// 单条档案是否可由本进程发起续做（纯函数便于单测）：worker 镜像不跑
/// 评审/拆解/收口；未知 pending_action 不猜。
fn resume_dispatchable(pending: &str, is_master: bool) -> bool {
    is_master && matches!(pending, "review" | "plan" | "project")
}

async fn probe_loop(deps: UnavailableProbeDeps) {
    let is_master = matches!(
        deps.review.cluster.role().as_str(),
        "coordinator" | "master" | "manager"
    );
    info!(
        "[Board][U-B] 恢复探针已装配（master={is_master}，tick={}s）",
        probe_tick_secs(ub::unavailable_cfg(Some(&deps.home)).probe_interval_secs)
    );
    let mut last_probe: Option<Instant> = None;
    loop {
        let cfg = ub::unavailable_cfg(Some(&deps.home));
        tokio::time::sleep(Duration::from_secs(probe_tick_secs(
            cfg.probe_interval_secs,
        )))
        .await;

        if !ub::frozen_now() {
            // 未封存：探测计时清零（封存从不连续两段共享计时）。
            last_probe = None;
            continue;
        }
        // estop 优先级更高：人工急停挂起时不探测不恢复（恢复动作本身是
        // agent 活动，急停语义下必须冻结）。
        if deps.review.estop.is_engaged() {
            continue;
        }
        // 0 = 只封存不探测（恢复靠人工 release 或端点恢复后的自然成功）。
        if cfg.probe_interval_secs == 0 {
            continue;
        }
        if last_probe.is_some_and(|t| t.elapsed() < Duration::from_secs(cfg.probe_interval_secs)) {
            continue;
        }
        last_probe = Some(Instant::now());
        let Some((provider, model, _)) = nemesis_providers::default_slot::current() else {
            continue;
        };
        // 最小真实 completion（"ping" 单轮、零工具、默认参数——探测流量
        // 最小化；模型答什么都算活，只要推理链路通）。
        let messages = [nemesis_providers::types::Message {
            role: "user".to_string(),
            content: nemesis_providers::types::MessageContent::Text("ping".to_string()),
            ..Default::default()
        }];
        match provider
            .chat(
                &messages,
                &[],
                &model,
                &nemesis_providers::types::ChatOptions::default(),
            )
            .await
        {
            Ok(_) => {
                if ub::record_llm_success(&cfg) {
                    info!("[Board][U-B] 探针确认端点恢复，开始中断续做");
                    resume_interruptions(&deps, is_master);
                }
            }
            Err(e) => {
                // 探针失败照常记账（连接类连击——已封存无边沿副作用，
                // freeze=false 时零成本）；非连接类（超时=慢）不计数。
                ub::record_llm_failure(&cfg, &e.to_string());
                tracing::debug!("[Board][U-B] 探针仍未恢复：{e}");
            }
        }
    }
}

/// 解封边沿的中断续做：读待续档案 → 逐条分派既有恢复路径 → 归档。
/// 单条失败（worker 侧越权/未知形态/续做发起失败）保留档案下轮重试，
/// 不中断其余档案的处理。
fn resume_interruptions(deps: &UnavailableProbeDeps, is_master: bool) {
    // 中断档案统一落 handoff workspace（U-A 停车档案同根；未装配 =
    // 档案本来就写不进去，无事可续）。
    let Some(ws) = nemesis_web::handlers::board::handoff_workspace_root().map(|p| p.as_path())
    else {
        return;
    };
    let docs = ub::read_pending_interruptions(ws);
    if docs.is_empty() {
        return;
    }
    info!("[Board][U-B] 中断续做：{} 份待续档案", docs.len());
    for doc in docs {
        let pending = doc["pending_action"].as_str().unwrap_or("").to_string();
        let issue_key = doc["issue"].as_str().unwrap_or("?").to_string();
        let issue_id = doc["issue_id"].as_i64();
        if !resume_dispatchable(&pending, is_master) {
            warn!(
                "[Board][U-B] {issue_key} 续做未发起（pending={pending}，master={is_master}）——档案保留"
            );
            continue;
        }
        let Some(issue_id) = issue_id else {
            warn!("[Board][U-B] {issue_key} 档案缺 issue_id，无法续做——档案保留（人工处理）");
            continue;
        };
        let handled = match pending.as_str() {
            "review" => {
                crate::board_review::spawn_board_review(deps.review.clone(), issue_id);
                true
            }
            "project" => {
                crate::board_review::spawn_project_review(deps.review.clone(), issue_id);
                true
            }
            "plan" => resume_plan_chain(deps, issue_id),
            _ => false,
        };
        // 归档已发起续做的档案（归档不删可追溯；续做自身再失败会经正常
        // 挂起路径重写档案）。档案被改名/移动时找不到原件——保留现场，
        // 下轮探针会再次读到并重复分派（评审/续拆入口各自幂等：状态闸/
        // RUNNING_PLANS 同单在途让位）。
        let path = ub::interruption_path(ws, &issue_key);
        if handled && path.exists() {
            ub::archive_interruption(ws, &path);
        } else if !handled {
            warn!("[Board][U-B] {issue_key} 续做发起失败——档案保留下轮重试");
        }
    }
}

/// plan 档案续拆：经 web 侧 [`nemesis_web::handlers::board::respawn_plan_chain`]
/// （RUNNING_PLANS 在途同单让位 + 恢复评论 + 新 plan_id 链）。agent 槽
/// 未就绪/单子已查不到 = false（档案保留）。
fn resume_plan_chain(deps: &UnavailableProbeDeps, issue_id: i64) -> bool {
    let Ok(issue) = deps.review.store.get_issue(issue_id) else {
        return false;
    };
    let Some(agent_loop) = deps.review.moderator_loop.get().cloned() else {
        return false;
    };
    nemesis_web::handlers::board::respawn_plan_chain(
        &deps.review.store,
        Some(deps.review.cluster.clone()),
        agent_loop,
        issue,
        Some(deps.home.clone()),
        deps.hub.clone(),
    )
}

#[cfg(test)]
mod tests;
