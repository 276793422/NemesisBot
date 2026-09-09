//! Swarm M4：验收 agent 编排层（批作业三态处置，§6）。
//!
//! 触发链：worker 回报 → `write_back_board_dispatch` 写回 in_review →
//! gateway 回调闭包 [`spawn_board_review`] → 本模块 [`review_issue`]：
//! 裸提示词 detached LLM 调用（与 planner 同款，`system_prompt` 换
//! 验收提示词 + 注入防线声明）→ `parse_review` 失败回灌重试 ≤2 次 →
//! 仍失败按 UNSURE 诚实处置 → 三态分派（PASS/FAIL/UNSURE）。
//!
//! 纯逻辑（提示词/schema/解析）真相源在 `nemesis-board::review`；本模块
//! 只做装配与副作用（评论/状态转移/重派/配置读取）。
//!
//! 并发验收安全性（§6.1 风险项）：多 issue 同时 in_review 会并发 spawn
//! 多个 review 任务，但 `run_detached` 每次调用建临时 instance（会话
//! `subagent:board-review:{uuid}` 互不相干），共享件均为 `Arc`，无串行
//! 队列必要。评审失败不回滚、不炸写回——评论留痕 + warn，人工兜底。

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use nemesis_board::assignment::Actor;
use nemesis_board::models::{CommentType, IssueStatus, NewComment};
use tracing::{debug, info, warn};

/// 验收依赖集（gateway 回调闭包构造，一次快照传入）。
pub struct BoardReviewDeps {
    pub store: Arc<nemesis_board::BoardStore>,
    /// workspace 根（现 M4 未直接用——评审输入全部来自 board.db；保留给
    /// 后续带工具实核/资产读取的评审形态）。
    #[allow(dead_code)]
    pub workspace: PathBuf,
    /// home 根（config.json 读取：auto_review/auto_accept/max_redispatch
    /// 每次评审时从盘上现读，改配置即时生效，不取装配期快照）。
    pub home: PathBuf,
    /// 主 AgentLoop 后置装配桥（nb_bus/主持人同款；评审任务运行时读）。
    pub moderator_loop: Arc<OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
    pub cluster: Arc<nemesis_cluster::cluster::Cluster>,
}

/// 三态处置动作（由 verdict + 重派预算 + auto_accept 纯函数推导；单测钉死）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewAction {
    /// PASS + auto_accept：评论 + 状态推进 done。
    AutoAccept,
    /// PASS 不自动收货：评论「待人工确认」，状态保持 in_review。
    SuggestManual,
    /// FAIL 且重派预算未耗尽：评论差距 + 重开重派。
    Redispatch,
    /// UNSURE（含解析失败）或 FAIL 预算耗尽：评论转人工，状态保持
    /// in_review（交付线程在场，人工看完直接定 done/重派）。
    EscalateHuman,
}

/// 三态处置决策表（§6.2；`round` = 已发生重派次数 = 派发行数 − 1）。
pub(crate) fn decide_review_action(
    verdict: nemesis_board::ReviewVerdict,
    round: u64,
    max_redispatch: u32,
    auto_accept: bool,
) -> ReviewAction {
    match verdict {
        nemesis_board::ReviewVerdict::Pass if auto_accept => ReviewAction::AutoAccept,
        nemesis_board::ReviewVerdict::Pass => ReviewAction::SuggestManual,
        nemesis_board::ReviewVerdict::Fail if round < u64::from(max_redispatch) => {
            ReviewAction::Redispatch
        }
        _ => ReviewAction::EscalateHuman,
    }
}

/// 触发验收（fire-and-forget；调用方在 write_back in_review 路径调用）。
pub(crate) fn spawn_board_review(deps: BoardReviewDeps, issue_id: i64) {
    tokio::spawn(async move {
        match review_issue(&deps, issue_id).await {
            Ok(outcome) if outcome => {}
            Ok(_) => {} // 跳过类结果已在 review_issue 内记日志
            Err(e) => warn!("[BoardReview] issue {issue_id} 评审失败：{e}（人工兜底）"),
        }
    });
}

/// 执行一次完整验收。返回 `Ok(false)` = 未评审即跳过（配置关/状态已变/
/// loop 未就绪），`Ok(true)` = 评审闭环完成。
async fn review_issue(deps: &BoardReviewDeps, issue_id: i64) -> Result<bool, String> {
    let cfg = load_board_flags(&deps.home)?;
    if !cfg.auto_review {
        debug!("[BoardReview] issue {issue_id} 跳过：board.auto_review=false");
        return Ok(false);
    }

    let store = &deps.store;
    let issue = store.get_issue(issue_id)?;
    // 只有 in_review 才评：write_back 与人工操作竞态时（人工已改状态）
    // 让位人工，不覆盖。
    if issue.status != IssueStatus::InReview {
        info!(
            "[BoardReview] issue {issue_id} 状态已是 {}（非 in_review），跳过自动验收",
            issue.status
        );
        return Ok(false);
    }

    let Some(agent_loop) = deps.moderator_loop.get() else {
        warn!("[BoardReview] issue {issue_id} 主 agent 未就绪，跳过自动验收（人工兜底）");
        return Ok(false);
    };

    // ---- 组装评审输入：worker 汇报 + 线程争议点 ----
    let comments = store.list_comments(issue_id)?;
    // worker 汇报 = **最新**一次交付（重派轮次取新汇报，不拿首轮旧账）；
    // 无结构化汇报时退最近的 agent 文本评论（诚实降级路径），再没有就
    // 空串（评审 agent 见「未提供」自判）。
    let delivery = comments
        .iter()
        .rev()
        .find(|c| c.ctype == CommentType::Delivery)
        .cloned();
    // worker 汇报 = 交付线程首评；无结构化汇报时退最近的 agent 文本评论
    // （诚实降级路径），再没有就空串（评审 agent 见「未提供」自判）。
    let (delivery_id, worker_report) = match &delivery {
        Some(c) => (Some(c.id), c.content.clone()),
        None => {
            let latest = comments
                .iter()
                .rev()
                .find(|c| c.author.kind == "agent" && c.ctype == CommentType::Comment);
            match latest {
                Some(c) => (Some(c.id), c.content.clone()),
                None => (None, String::new()),
            }
        }
    };
    // 线程 = 除最新交付汇报外的评论，滤掉 status_change/system 机械评论
    // （评审上一轮自己的 FAIL 意见 + 更早轮次的交付汇报保留在场——重派
    // 轮次的完整上下文）。
    let thread: Vec<(String, String)> = comments
        .iter()
        .filter(|c| Some(c.id) != delivery_id)
        .filter(|c| !matches!(c.ctype, CommentType::StatusChange | CommentType::System))
        .map(|c| (format!("{} {}", c.author.kind, c.author.id), c.content.clone()))
        .collect();

    let dispatches = store.list_dispatches(issue_id)?;
    let round = dispatches.len().saturating_sub(1) as u64;

    // 转人工评论的 @人对象 = 创建者是人（admin）时 @ 创建者（impl-plan
    // §6.2「UNSURE @人 请求裁决」）；agent/system 创建者无人类可 @，退化
    // 为普通评论。
    let human_mention = if issue.creator.kind == "admin" {
        format!("@{} ", issue.creator.id)
    } else {
        String::new()
    };

    let mut prompt = nemesis_board::build_review_user_prompt(
        &issue.number,
        &issue.title,
        &issue.description,
        issue.acceptance_criteria.as_deref(),
        &worker_report,
        &thread,
    );

    // ---- LLM 评审（首跑 + 回灌重试 ≤2，planner 同款）----
    let output = match run_review_llm(agent_loop, &mut prompt).await {
        Ok(ok) => ok,
        Err(last_err) => {
            // 解析失败 → 当 UNSURE 诚实处置（§6.1），评论说明是评审输出
            // 本身不合规，防误读成任务问题。
            let comment = format!(
                "{}🤷 验收 agent 无法判定（评审输出连续 3 次无法解析），请人工裁决。\n\n解析错误：{last_err}",
                human_mention
            );
            post_review_comment(store, issue_id, &deps.cluster, &comment)?;
            return Ok(true);
        }
    };

    // 经验落库（M4.5 集体记忆 §6.5.1 蒸馏唯一写闸）：评审 agent 的
    // experience 槽位 → team_memory 表。空壳条目（词表外空白 category /
    // 空 scope/content）丢弃；去重合并与计数在 store 内部；失败只 warn
    // 不炸评审流程（经验沉淀是增值动作，验收结论不受影响）。
    if let Some(exp) = &output.experience {
        let category = exp.category.trim();
        let content = exp.content.trim();
        if category.is_empty() || exp.scope.trim().is_empty() || content.is_empty() {
            info!(
                "[BoardReview] issue {issue_id} 经验槽位为空壳（category/scope/content 有空白），丢弃不入库"
            );
        } else {
            match store.add_team_memory(nemesis_board::models::NewTeamMemory {
                category: category.to_string(),
                scope: exp.scope.trim().to_string(),
                content: content.to_string(),
                source: issue.number.clone(),
                author: deps.cluster.node_id().to_string(),
            }) {
                Ok((id, merged)) => info!(
                    "[BoardReview] issue {issue_id} 经验入库：id={id} category={} scope={} merged={merged}",
                    category,
                    exp.scope.trim()
                ),
                Err(e) => warn!("[BoardReview] issue {issue_id} 经验入库失败：{e}（不影响评审结论）"),
            }
        }
    }

    let reviewer = Actor::agent(deps.cluster.node_id());
    match decide_review_action(output.verdict, round, cfg.max_redispatch, cfg.auto_accept) {
        ReviewAction::AutoAccept => {
            let reasons = render_reasons(&output.reasons);
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: format!("✅ agent 验收通过（board.auto_accept 已开启，自动收货）\n\n{reasons}"),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            store.transition_issue(issue_id, IssueStatus::Done, &reviewer)?;
            info!("[BoardReview] issue {issue_id} PASS → done（auto_accept）");
        }
        ReviewAction::SuggestManual => {
            let reasons = render_reasons(&output.reasons);
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: format!("✅ agent 验收通过，待人工确认\n\n{reasons}"),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            info!("[BoardReview] issue {issue_id} PASS → 待人工确认（保持 in_review）");
        }
        ReviewAction::Redispatch => {
            let gap = output.gap.trim();
            let next_round = round + 1;
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: format!(
                    "❌ agent 验收未通过（第 {next_round}/{} 次重派）\n\n## 差距\n{gap}\n{}",
                    cfg.max_redispatch,
                    render_reasons(&output.reasons)
                ),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            // 重派目标 = 最近一次派发的 worker（同 worker 整改，会话上下文
            // 天然延续——chat_id=board:{number} 收敛同一 worker 会话）。
            let target = dispatches
                .last()
                .map(|d| d.worker_id.clone())
                .ok_or_else(|| "无历史派发记录，无法确定重派目标".to_string())?;
            match nemesis_web::handlers::board::dispatch_issue_core(
                store,
                Some(&deps.cluster),
                issue_id,
                &target,
                &reviewer,
                Some(gap),
            ) {
                Ok(_) => {
                    info!(
                        "[BoardReview] issue {issue_id} FAIL → 重派 #{next_round} → {target}"
                    );
                }
                Err(e) => {
                    let msg = format!("⛔ 验收未通过，自动重派失败：{e}（请人工派发或处置）");
                    store.add_comment(NewComment {
                        issue_id,
                        author: Actor::system("board-review"),
                        content: msg,
                        parent_id: None,
                        ctype: CommentType::System,
                    })?;
                    warn!("[BoardReview] issue {issue_id} 重派失败：{e}");
                }
            }
        }
        ReviewAction::EscalateHuman => {
            let mut comment = format!(
                "{}🤷 验收 agent 无法定案，请人工裁决\n",
                human_mention
            );
            if output.verdict == nemesis_board::ReviewVerdict::Fail {
                comment.push_str(&format!(
                    "\n重派预算已耗尽（{} 次），最新差距仍在：\n{}\n",
                    cfg.max_redispatch,
                    output.gap.trim()
                ));
            }
            comment.push_str(&render_reasons(&output.reasons));
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: comment,
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            info!("[BoardReview] issue {issue_id} verdict={} → 转人工（保持 in_review）", output.verdict.as_str());
        }
    }
    Ok(true)
}

/// 裸提示词 detached 评审调用：首跑 + `parse_review` 失败回灌重试 ≤2 次
/// （共 3 轮）。成功返回解析输出；3 轮全败返回末次错误。
async fn run_review_llm(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    prompt: &mut String,
) -> Result<nemesis_board::ReviewOutput, String> {
    let mut last_err = String::new();
    for _ in 0..=2 {
        let raw = agent_loop
            .run_detached(
                prompt,
                nemesis_agent::r#loop::DetachedOpts {
                    system_prompt: Some(nemesis_board::REVIEW_SYSTEM_PROMPT),
                    no_tools: true,
                    max_turns: 1,
                    label: Some("board-review"),
                    ..Default::default()
                },
            )
            .await?;
        match nemesis_board::parse_review(&raw) {
            Ok(out) => return Ok(out),
            Err(e) => {
                // 限定路径：根导出的 build_retry_prompt 是 planner 的
                //（review 同名函数不进根，避免撞名歧义）。
                *prompt = nemesis_board::review::build_retry_prompt(&raw, &e);
                last_err = e.message;
            }
        }
    }
    Err(last_err)
}

/// 从 home 读 board 段旗标（auto_review/auto_accept/max_redispatch）。
/// 配置读失败 → Err（评审诚实放弃，fail-closed 到人工验收——不拿默认值
/// 顶替用户配置）。
fn load_board_flags(home: &std::path::Path) -> Result<nemesis_config::BoardFlagConfig, String> {
    let path = home.join("config.json");
    let cfg = nemesis_config::load_config(&path)
        .map_err(|e| format!("config.json 读取失败（{}）：{e}", path.display()))?;
    Ok(cfg.board.unwrap_or_default())
}

/// reasons 列表渲染成评论小节；空列表给诚实注记。
fn render_reasons(reasons: &[String]) -> String {
    if reasons.is_empty() {
        return "（评审 agent 未给出理由）".to_string();
    }
    let mut s = String::from("理由：\n");
    for r in reasons {
        s.push_str(&format!("- {}\n", r.trim()));
    }
    s
}

/// 验收评论落库（作者 = master 节点 agent；失败仅 warn，不炸评审流程）。
fn post_review_comment(
    store: &nemesis_board::BoardStore,
    issue_id: i64,
    cluster: &nemesis_cluster::cluster::Cluster,
    content: &str,
) -> Result<(), String> {
    store
        .add_comment(NewComment {
            issue_id,
            author: Actor::agent(cluster.node_id()),
            content: content.to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .map(|_| ())
}

#[cfg(test)]
mod tests;
