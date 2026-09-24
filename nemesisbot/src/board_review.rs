//! Swarm M4：验收 agent 编排层（批作业三态处置，§6）。
//!
//! 触发链：worker 回报 → `write_back_board_dispatch` 写回 in_review →
//! gateway 回调闭包 [`spawn_board_review`] → 本模块 [`review_issue`]：
//! 裸提示词 detached LLM 调用（与 planner 同款，`system_prompt` 换
//! 验收提示词 + 注入防线声明）→ `parse_review` 失败回灌重试 ≤2 次 →
//! 仍失败按 UNSURE 诚实处置 → 三态分派（PASS/FAIL/UNSURE）。
//!
//! 全自动流转 P4 追加：
//! - **B2a 执行型验收**：最近派发 worker 是本机节点且
//!   `board.review.max_turns > 1` 时，评审 agent 以只读工具白名单最多
//!   N 轮取证再下结论（远端 worker 一律纯文本，防读错工作区）。
//! - **B2b 自检取证**：`board.review.selfcheck=true` 时评审可向 worker
//!   发一轮取证请求（`need_evidence`），回报后触发二段验收（证据只带
//!   一程，防取证-评审乒乓）。
//! - **D3 换节点重派**：同一 worker 连续 ≥2 次派发仍 FAIL → 换历史
//!   未用过的次优匹配节点。
//! - **E1 预算保险丝**：`board.budget` 三维（子单数/累计派发/墙钟）任一
//!   超限即停自动重派转人工；unlimited_mode 下降级为 WARN 继续。
//! - **F3 项目收口**：`board.review.auto_close_project=true` 时项目下全部
//!   顶层父单 done → 汇总验收，PASS → completed；FAIL/UNSURE → 缺口
//!   评论 + completed 回滚 in_progress（不自动重开父单）。
//!
//! 纯逻辑（提示词/schema/解析）真相源在 `nemesis-board::review`；本模块
//! 只做装配与副作用（评论/状态转移/重派/配置读取）。
//!
//! 并发验收安全性（§6.1 风险项）：多 issue 同时 in_review 会并发 spawn
//! 多个 review 任务，但 `run_detached` 每次调用建临时 instance（会话
//! `subagent:board-review:{uuid}` 互不相干），共享件均为 `Arc`，无串行
//! 队列必要。评审失败不回滚、不炸写回——评论留痕 + warn，人工兜底。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use nemesis_board::assignment::Actor;
use nemesis_board::models::{CommentType, IssueStatus, NewComment};
use tracing::{debug, info, warn};

/// estop 停车种类（释放 watcher 据此选复评入口；P4 加 Project 维度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkedKind {
    /// 子单评审。
    Issue,
    /// 父单收口评审。
    Parent,
    /// 项目收口评审（F3）。
    Project,
}

/// 验收依赖集（gateway 回调闭包构造，一次快照传入）。
/// `Clone` 供父单收口钩子（A3）每次触发时从 Arc 快照派生。
#[derive(Clone)]
pub struct BoardReviewDeps {
    pub store: Arc<nemesis_board::BoardStore>,
    /// workspace 根（P2/B1 锚点检查的路径安全边界：[CHECK] file: 类锚点
    /// 只在此子树内解析；后续带工具实核/资产读取的评审形态同用此根）。
    pub workspace: PathBuf,
    /// home 根（config.json 读取：auto_review/auto_accept/max_redispatch
    /// 每次评审时从盘上现读，改配置即时生效，不取装配期快照）。
    pub home: PathBuf,
    /// 主 AgentLoop 后置装配桥（nb_bus/主持人同款；评审任务运行时读）。
    pub moderator_loop: Arc<OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
    pub cluster: Arc<nemesis_cluster::cluster::Cluster>,
    /// 急停句柄（全自动流转 P1/T1-6 保险丝）：挂起时评审冻结（不跑 LLM、
    /// 不重派、不收口），issue 停车登记；release 后由
    /// [`spawn_estop_resume_watcher`] 复评恢复。与 SharedResources 共享
    /// 同一 Arc——CLI/托盘/Dashboard/WSAPI 四入口全部生效。
    pub estop: Arc<nemesis_agent::estop::EstopState>,
    /// estop 冻结停车的 (种类, issue_id) 队列（gateway 装配一份共享，
    /// 评审闭包与 release watcher 共用）。std Mutex：持锁不跨 await。
    pub estop_parked: Arc<std::sync::Mutex<Vec<(ParkedKind, i64)>>>,
    /// B2b 自检取证路由表（gateway 装配一份，callback 闭包与评审任务
    /// 共享）：task_id → issue_id。
    pub selfcheck: SelfcheckRegistry,
}

/// B2b 自检在途注册表：task_id → issue_id。selfcheck 派发**不写**
/// `issue_dispatch`（那是正式派发的写回路由键，写了会被 Route 0 当交付
/// 写回推进状态），callback 回来时凭本表识别这是取证任务并路由到二段
/// 验收——peer_chat_callback payload 只有 task_id，没有 chat_id，字面
/// marker 路由不可行，注册表是唯一可靠路由。std Mutex：持锁不跨 await。
#[derive(Clone, Default)]
pub struct SelfcheckRegistry {
    inner: Arc<std::sync::Mutex<HashMap<String, i64>>>,
}

impl SelfcheckRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(&self, task_id: String, issue_id: i64) {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task_id, issue_id);
    }

    pub(crate) fn take(&self, task_id: &str) -> Option<i64> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(task_id)
    }

    /// 同 issue 是否已有在途取证（防重入：重派循环里评审反复挂起会重复
    /// 发取证任务）。
    pub(crate) fn has_inflight(&self, issue_id: i64) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .any(|&v| v == issue_id)
    }
}

/// B2a 执行型验收的只读工具白名单（评审 agent 取证轮可用工具；全部是
/// 现网注册工具名）。写类/网络副作用类不进名单——评审 agent 是只读角色，
/// 安全 8 层照常在 dispatch 闸拦截（纵深防御，白名单是第一道）。
const READONLY_REVIEW_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "grep",
    "git",
    "web_fetch",
    "lsp",
    "run_checks",
];

/// B2a 评审工具模式：`NoTools` 与历史行为字节等价（max_turns=1 纯文本）；
/// `ReadOnly` 允许只读白名单多轮取证。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewToolMode {
    NoTools,
    ReadOnly { max_turns: u32 },
}

/// B2a 模式选择（纯函数；单测钉死）：仅当「最近一次派发的 worker 就是
/// 本机节点」且配置轮数 >1 时才开执行型取证——远端 worker 的工件不在
/// 本机，评审 agent 就地读文件只会读到 master 自己的工作区（张冠李戴），
/// 一律纯文本。`max_turns=1` 恒为 `NoTools`（与历史行为字节等价）。
pub(crate) fn pick_review_tool_mode(
    last_worker: Option<&str>,
    local_node: &str,
    cfg_max_turns: u32,
) -> ReviewToolMode {
    if cfg_max_turns > 1 && last_worker == Some(local_node) {
        ReviewToolMode::ReadOnly {
            max_turns: cfg_max_turns,
        }
    } else {
        ReviewToolMode::NoTools
    }
}

/// D3 换节点重派目标决策结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RedispatchTargetChoice {
    /// 维持同 worker（首轮/失败未满 2 连/无次优候选回落）。
    Same(String),
    /// 换历史未用过的次优匹配节点。
    Switch(String),
}

impl RedispatchTargetChoice {
    pub(crate) fn into_target(self) -> String {
        match self {
            Self::Same(t) | Self::Switch(t) => t,
        }
    }

    pub(crate) fn is_switch(&self) -> bool {
        matches!(self, Self::Switch(_))
    }
}

/// D3 换节点重派目标选择（决策逻辑集中此处；单测钉死决策表）：
/// - 无历史派发 → Err（调用方诚实报错，同旧行为）。
/// - 尾部连续同 worker 派发 <2 → 维持该 worker（同 worker 整改，会话
///   上下文天然延续）。
/// - 连续 ≥2 次派发同一 worker（同 worker 反复 FAIL/无法定案）→ 向
///   匹配器要全量在线排序，取历史未用过的次优节点；无候选 → 回落同
///   worker（WARN 留痕，不阻塞流程）。
pub(crate) fn pick_redispatch_target(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    dispatches: &[nemesis_board::models::DispatchRecord],
) -> Result<RedispatchTargetChoice, String> {
    let last = match dispatches.last().map(|d| d.worker_id.clone()) {
        Some(w) => w,
        None => return Err("无历史派发记录，无法确定重派目标".to_string()),
    };
    let consecutive = dispatches
        .iter()
        .rev()
        .take_while(|d| d.worker_id == last)
        .count();
    if consecutive < 2 {
        return Ok(RedispatchTargetChoice::Same(last));
    }
    let historical: std::collections::HashSet<&str> =
        dispatches.iter().map(|d| d.worker_id.as_str()).collect();
    let ranked =
        nemesis_web::handlers::board::rank_dispatch_candidates(&deps.store, &deps.cluster, issue);
    match ranked
        .into_iter()
        .find(|w| !historical.contains(w.as_str()))
    {
        Some(next) => {
            warn!(
                "[BoardReview] issue {} 连续 {consecutive} 次派发同一 worker {last} → 换节点重派 {next}",
                issue.id
            );
            Ok(RedispatchTargetChoice::Switch(next))
        }
        None => {
            warn!(
                "[BoardReview] issue {} 连续 {consecutive} 次同 worker {last} 且在线无其他候选节点，维持原目标重派",
                issue.id
            );
            Ok(RedispatchTargetChoice::Same(last))
        }
    }
}

/// E1 预算保险丝检查（`board.budget` 四维：子单数/累计派发/墙钟/token；
/// 单测钉死矩阵）。返回
/// `Some(说明)` = 超预算。0 值 = 该维关闭。unlimited_mode 不改变本函数
/// 结果——调用方决定 breach 时停（默认）还是 WARN 继续（unlimited）。
pub(crate) fn budget_breach(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    cfg: &nemesis_config::BoardFlagConfig,
) -> Option<String> {
    let budget = &cfg.budget;
    // 维度 1：父单子单数上限（planner 过度拆解的硬闸）。
    if budget.max_subissues_per_parent > 0
        && let Ok(children) = deps.store.list_children(issue.id)
        && children.len() > budget.max_subissues_per_parent as usize
    {
        return Some(format!(
            "子单数 {} 超过 board.budget.max_subissues_per_parent={}",
            children.len(),
            budget.max_subissues_per_parent
        ));
    }
    // 维度 2：父单全链累计派发次数（父单 + 全部子单的 dispatch 记录，
    // 含首轮）。查父链失败 = 预算检查不完整 → 诚实按无预算放行（预算是
    // 保险丝不是安全闸，误停链路比漏放行代价高）。
    if budget.max_total_redispatch > 0
        && let Ok(total) = chain_dispatch_count(&deps.store, issue)
        && total > budget.max_total_redispatch as u64
    {
        return Some(format!(
            "父单全链累计派发 {total} 次超过 board.budget.max_total_redispatch={}",
            budget.max_total_redispatch
        ));
    }
    // 维度 3：墙钟预算（created_at 起算；checked 防时钟倒挂下溢——倒挂
    // 视为未超时，不炸流程；超大预算秒数过 i64::try_from 截到 i64::MAX，
    // 防 u64::MAX as i64 回绕成 -1 让任何存活时间都误判超支）。
    if budget.wall_clock_budget_secs > 0 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let budget_ms = i64::try_from(budget.wall_clock_budget_secs)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        if let Some(elapsed_ms) = now_ms.checked_sub(issue.created_at)
            && elapsed_ms >= budget_ms
        {
            return Some(format!(
                "issue 存活 {}s 超过 board.budget.wall_clock_budget_secs={}s",
                elapsed_ms / 1000,
                budget.wall_clock_budget_secs
            ));
        }
    }
    // 维度 4（E1 二期）：父单全链 token 预算。聚合口径 = master 账本里
    // 该父链全部派发（cluster_rpc:{worker}/{task_id} 精确键）的
    // input+output 累计（worker 回调回传，见 gateway.rs record_cluster_usage）。
    // 无 DataStore / 聚合失败 = 预算检查不完整 → 诚实按无预算放行（同维度 2）。
    if budget.max_tokens_per_parent > 0
        && let Some(used) = chain_token_usage(deps, issue)
        && used > budget.max_tokens_per_parent
    {
        return Some(format!(
            "父单全链累计 token {used} 超过 board.budget.max_tokens_per_parent={}",
            budget.max_tokens_per_parent
        ));
    }
    None
}

/// E1 维度 4 的全链 token 聚合入口：从评审依赖解出 master DataStore
///（评审 loop 未装配 / 未后置 → None = 该维诚实放行）。
fn chain_token_usage(deps: &BoardReviewDeps, issue: &nemesis_board::Issue) -> Option<u64> {
    let ds = deps.moderator_loop.get()?.data_store()?;
    Some(chain_token_usage_ds(
        ds.as_ref(),
        deps.store.as_ref(),
        issue.parent_issue_id.unwrap_or(issue.id),
    ))
}

/// E1 维度 4 的纯聚合：`root` 及全部子单的派发行 → worker 回传用量
/// （input+output）之和。按 `/{task_id}` 后缀聚合（[`nemesis_data::DataStore::aggregate_session_usage_by_task`]）：
/// 账本键 worker 段 = 传输层运行时节点 id，派发行 worker_id = 派发时 peer 名，
/// 两者不同源，精确键必失配（T37 run 4 真机实证）——task_id 是唯一稳定锚。
/// 单测直接喂临时 DataStore 钉死矩阵。
pub(crate) fn chain_token_usage_ds(
    ds: &nemesis_data::DataStore,
    store: &nemesis_board::BoardStore,
    root_id: i64,
) -> u64 {
    let mut dispatches = match store.list_dispatches(root_id) {
        Ok(d) => d,
        Err(_) => return 0,
    };
    if let Ok(children) = store.list_children(root_id) {
        for c in children {
            if let Ok(sub) = store.list_dispatches(c.id) {
                dispatches.extend(sub);
            }
        }
    }
    let mut total: u64 = 0;
    for d in &dispatches {
        if let Ok(agg) = ds.aggregate_session_usage_by_task(&d.task_id) {
            total += (agg.input_tokens + agg.output_tokens).max(0) as u64;
        }
    }
    total
}

/// E1 维度 2 的全链派发计数：根（顶层父单）自身 + 全部子单的 dispatch
/// 行数总和。非父单（无子单）= 自身行数。
fn chain_dispatch_count(
    store: &nemesis_board::BoardStore,
    issue: &nemesis_board::Issue,
) -> Result<u64, String> {
    let root_id = issue.parent_issue_id.unwrap_or(issue.id);
    let mut total = store.list_dispatches(root_id)?.len() as u64;
    for c in store.list_children(root_id)? {
        total += store.list_dispatches(c.id)?.len() as u64;
    }
    Ok(total)
}

/// 三态处置动作（由 verdict + 重派预算 + auto_accept 纯函数推导；单测钉死）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewAction {
    /// PASS + auto_accept：评论 + 状态推进 done。
    AutoAccept,
    /// PASS 不自动收货：评论「待人工确认」，状态保持 in_review。
    SuggestManual,
    /// FAIL 且重派预算未耗尽：评论差距 + 重开重派。无限模式下 UNSURE 也
    /// 走此动作（带「无法定案」意见继续派）。
    Redispatch,
    /// UNSURE（含解析失败）或 FAIL 预算耗尽：评论转人工，状态保持
    /// in_review（交付线程在场，人工看完直接定 done/重派）。
    EscalateHuman,
}

/// 三态处置决策表（§6.2；`round` = 已发生重派次数 = 派发行数 − 1）。
/// `unlimited` = board.unlimited_mode（全自动流转 P1/E3，用户裁定
/// 2026-09-10）：FAIL 重派预算视为 ∞、UNSURE 不转人工继续重派——无条件
/// 强制流程运转。estop 不受影响（保险丝非护栏）。
pub(crate) fn decide_review_action(
    verdict: nemesis_board::ReviewVerdict,
    round: u64,
    max_redispatch: u32,
    auto_accept: bool,
    unlimited: bool,
) -> ReviewAction {
    match verdict {
        nemesis_board::ReviewVerdict::Pass if auto_accept => ReviewAction::AutoAccept,
        nemesis_board::ReviewVerdict::Pass => ReviewAction::SuggestManual,
        nemesis_board::ReviewVerdict::Fail if unlimited || round < u64::from(max_redispatch) => {
            ReviewAction::Redispatch
        }
        // 无限模式：UNSURE 带「无法定案」意见继续重派（不转人工）。
        nemesis_board::ReviewVerdict::Unsure if unlimited => ReviewAction::Redispatch,
        _ => ReviewAction::EscalateHuman,
    }
}

// ---------------------------------------------------------------------------
// P2A 能力类失败保护（2026-09-12 双端真机 NB-15 根修）：worker 上报的结构化
// fail_class 驱动重派决策——能力类失败（validation_budget / escalation）在
// 同一 worker/模型上是确定性失败形态，同目标重派大概率原样复现（NB-15：
// 校验预算耗尽连烧两轮重派预算）。
// ---------------------------------------------------------------------------

/// 能力类失败类集：这些 fail_class 同目标重派前必须换节点或人工介入。
const CAPABILITY_FAIL_CLASSES: [&str; 2] = ["validation_budget", "escalation"];

/// 从评论线程提取最新一条结构化失败分类（write_back_board_dispatch 在
/// ⛔ 失败评论尾部落 `fail_class: <class>` 标记行，gateway.rs 唯一写入点）。
/// 只认已知类值（防 worker 文本/人工评论误触发未知值）；找不到 = None
///（旧 worker / 无失败评论路径）。扫描自最新评论起——重派轮次取最新失败。
fn latest_fail_class(comments: &[nemesis_board::Comment]) -> Option<&'static str> {
    const KNOWN: [&str; 6] = [
        "validation_budget",
        "llm_timeout",
        "llm_failure",
        "empty_result",
        "escalation",
        "exec_failed",
    ];
    comments.iter().rev().find_map(|c| {
        c.content.lines().find_map(|line| {
            let t = line.trim();
            let v = t.strip_prefix("fail_class: ")?;
            KNOWN.iter().find(|k| **k == v.trim()).copied()
        })
    })
}

/// 能力类失败的转人工建议文案（与类值一一对应；未知类走兜底）。
fn capability_fail_hint(fc: &str) -> &'static str {
    match fc {
        "validation_budget" => {
            "可选处置：\n- 在 Dashboard「模型」页对 worker 的模型跑 model probe 校准能力档位，或 `model set-tier` 调高档位\n- 人工改派其它节点（不同模型可能胜任）\n- 若确认是偶发，可人工直接重派原节点"
        }
        "escalation" => {
            "可选处置：\n- 改写任务描述或拆小粒度后人工重派（换一种思路打破循环）\n- 人工改派其它节点尝试"
        }
        _ => "请人工检查 worker 状态后裁决（重派 / 换节点 / 调整模型）。",
    }
}

/// 评审上下文（P4）：一段评审 fresh（`allow_selfcheck=true`，配置闸在
/// review_issue 内现读）；B2b 二段评审带 evidence 且 `allow_selfcheck=false`
/// ——取证只带一程，二段结论直接处置（防取证-评审乒乓死循环）。
#[derive(Debug, Clone)]
struct ReviewCtx {
    allow_selfcheck: bool,
    evidence: Option<String>,
}

impl ReviewCtx {
    fn first_stage() -> Self {
        Self {
            allow_selfcheck: true,
            evidence: None,
        }
    }
}

/// 触发验收（fire-and-forget；调用方在 write_back in_review 路径调用）。
pub(crate) fn spawn_board_review(deps: BoardReviewDeps, issue_id: i64) {
    tokio::spawn(async move {
        match review_issue(&deps, issue_id, ReviewCtx::first_stage()).await {
            Ok(outcome) if outcome => {}
            Ok(_) => {} // 跳过类结果已在 review_issue 内记日志
            Err(e) => warn!("[BoardReview] issue {issue_id} 评审失败：{e}（人工兜底）"),
        }
    });
}

/// 执行一次完整验收。返回 `Ok(false)` = 未评审即跳过（配置关/状态已变/
/// loop 未就绪/estop 冻结），`Ok(true)` = 评审闭环完成（含 B2b 取证挂起）。
async fn review_issue(
    deps: &BoardReviewDeps,
    issue_id: i64,
    ctx: ReviewCtx,
) -> Result<bool, String> {
    // estop 保险丝检查点 ①：挂起即冻结（不读配置不跑 LLM），停车等释放。
    if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
        return Ok(false);
    }
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

    // ---- 组装评审输入：worker 汇报 + 线程争议点 ----
    let comments = store.list_comments(issue_id)?;
    // worker 汇报 = **最新**一次交付（重派轮次取新汇报，不拿首轮旧账）；
    // 无结构化汇报时退最近的 agent 文本评论（诚实降级路径），再没有就
    // 空串（评审 agent 见「未提供」自判）。
    // worker 汇报 = **最新**一次交付（重派轮次取新汇报，不拿首轮旧账）；
    // 无结构化汇报时退最近的 agent 文本评论（诚实降级路径），再没有就
    // 空串（评审 agent 见「未提供」自判）。
    // 任务卡回显不是交付（2026-09-12 UAT T30 双层实证）：回显模型把派发
    // 任务卡原样抄回当回复，文本里带着验收标准与上轮失败明细引文——锚点
    // 行原文可剥离（anchor.rs 自指防御），但引文散文里的裸词仍会二阶自
    // 命中。凡内容含任务卡头的评论一律不作交付输入（Delivery 与降级
    // agent 评论同滤），回显 worker 诚实走 FAIL → 重派/预算/转人工漏斗。
    let is_card_echo =
        |c: &nemesis_board::Comment| c.content.contains(nemesis_board::TASK_CARD_HEADER);
    let delivery = comments
        .iter()
        .rev()
        .find(|c| c.ctype == CommentType::Delivery && !is_card_echo(c))
        .cloned();
    // worker 汇报 = 交付线程首评；无结构化汇报时退最近的 agent 文本评论
    // （诚实降级路径），再没有就空串（评审 agent 见「未提供」自判）。
    let (delivery_id, worker_report) = match &delivery {
        Some(c) => (Some(c.id), c.content.clone()),
        None => {
            let latest = comments.iter().rev().find(|c| {
                c.author.kind == "agent" && c.ctype == CommentType::Comment && !is_card_echo(c)
            });
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
        .map(|c| {
            (
                format!("{} {}", c.author.kind, c.author.id),
                c.content.clone(),
            )
        })
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
    // 评审评论作者（解析失败分支与三态分派共用）。
    let reviewer = Actor::agent(deps.cluster.node_id());

    let mut prompt = nemesis_board::build_review_user_prompt(
        &issue.number,
        &issue.title,
        &issue.description,
        issue.acceptance_criteria.as_deref(),
        &worker_report,
        &thread,
    );
    // B2b 二段评审：worker 取证回报作为待审数据拼入（证据是 worker LLM
    // 的输出，同样受数据/指令分离声明约束）。
    if let Some(ev) = ctx
        .evidence
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        prompt.push_str(&format!(
            "\n## 自检证据（执行 worker 取证回报）\n{}\n",
            nemesis_utils::truncate(ev, 8000)
        ));
    }
    // F-U3-1 根修（UAT U3 实证）：变更集合并结果作为**系统客观数据**注入。
    // worker 自报的交付路径是一面之词——远程 worker 经集群 exec 在沙箱目录
    // 产文件时，汇报里的沙箱绝对路径让无工具的评审 LLM 误判「文件未落盘」
    // → 假阳 UNSURE 转人工（实际变更集早已合并进项目工作区）。系统合并
    // commit 的实际 diff 才是「产物是否进入项目工作区」的证据真相源。
    // 无合并记录（非文件型任务/本机 worker）或档案目录缺失 = 诚实跳过。
    if let Some(project_id) = issue.project_id
        && let Ok(project) = store.get_project(project_id)
        && let Ok(root) = project_archive_root(project.directory.as_deref())
    {
        let merged_commit = store.list_activity(issue_id).ok().and_then(|acts| {
            acts.iter()
                .rev()
                .find(|a| a.action == crate::board_archive_ingest::ACTION_MERGED)
                .and_then(|a| a.details.as_deref())
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .and_then(|v| v.get("commit").and_then(|c| c.as_str()).map(str::to_string))
        });
        if let Some(commit_oid) = merged_commit {
            match nemesis_board::commit_changed_files(&root, &commit_oid) {
                Ok(files) if !files.is_empty() => {
                    // F-U3-7（2026-09-15 U3 真机 NB-13）：清单只证实「落盘」，
                    // 评审员核对不了「内容」——变更集明明带全文却看不到，
                    // 对「产物内容是否达标」只能 UNSURE 转人工。注入 commit
                    // 内实际 blob 内容节选（非 worker 自述）：小文本全文
                    // 截断呈现，二进制/超限/条数超预算的诚实注记跳过。
                    const BLOB_READ_MAX_BYTES: usize = 64 * 1024;
                    const BLOB_SNIPPET_CHARS: usize = 2000;
                    const BLOB_CONTENT_MAX_FILES: usize = 8;
                    let mut listing = String::new();
                    let mut content_budget = BLOB_CONTENT_MAX_FILES;
                    let mut with_content = 0usize;
                    for (path, status) in &files {
                        listing.push_str(&format!("- {path}（{status}）\n"));
                        if content_budget > 0
                            && let Ok(Some(text)) = nemesis_board::commit_blob_text(
                                &root,
                                &commit_oid,
                                path,
                                BLOB_READ_MAX_BYTES,
                            )
                        {
                            let snippet = nemesis_utils::truncate(text.trim(), BLOB_SNIPPET_CHARS);
                            listing.push_str(&format!(
                                "  内容节选（commit 内 blob 原文）：\n```text\n{snippet}\n```\n"
                            ));
                            content_budget -= 1;
                            with_content += 1;
                        }
                    }
                    if files.len() > with_content {
                        listing.push_str(&format!(
                            "（其余 {} 个文件未附内容：二进制 / 超大 / 节选预算耗尽——仅以清单证实落盘）\n",
                            files.len() - with_content
                        ));
                    }
                    prompt.push_str(&format!(
                        "\n## 变更集合并结果（系统客观数据）\n交付变更集已由系统合并进项目工作区（commit {}）。合并实际落盘的文件清单与内容节选：\n{listing}\n以上为系统合并记录的客观证据（清单=落盘证实；内容节选=commit 内 blob 原文，非 worker 自述）。任务验收标准涉及产物内容时，直接对照节选核对，不要因「内容未随报提供」而判证据不足。\n",
                        &commit_oid[..12.min(commit_oid.len())]
                    ));
                    // 组件5（件4 评审注入，F-U3 同族）：变更集携带
                    // `.discipline/` 时追加纪律闭环证据段（None=非纪律任务
                    // 诚实跳过，不拿陈旧产物污染无关任务）。
                    if let Some(disc) = render_discipline_evidence(&root, &commit_oid, &files) {
                        prompt.push_str(&disc);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("[BoardReview] issue {issue_id} 合并清单读取失败（跳过证据注入）: {e}")
                }
            }
        }
    }

    // ---- 客观锚点双检（P2/B1）：先便宜后昂贵，零 LLM 成本零命令执行 ----
    // acceptance_criteria 的 [CHECK] 行系统先核验：全过 → 摘要拼进 prompt
    // （LLM 仍审语义项）；有 fail → 合成 FAIL 短路（跳过 LLM），照常流经
    // 下方三态决策表（E3 语义零改动：锚点 FAIL = ReviewVerdict::Fail，
    // unlimited/max_redispatch 照旧）。路径形态不安全的锚点行（恶意/越界）
    // 解析期即被拒：不产出锚点、回落语义项 + 系统告警评论（T2-3：标准
    // 自身的毛病不惩罚执行者，也不让验收炸掉）。
    let (anchors, _semantic_fallback, rejected_anchors) =
        nemesis_board::parse_anchors(issue.acceptance_criteria.as_deref().unwrap_or(""));
    if !rejected_anchors.is_empty() {
        let mut warn_body =
            String::from("⚠ 验收标准含不安全/非法锚点行，已忽略（按普通语义标准评审）：\n");
        for r in &rejected_anchors {
            warn_body.push_str(&format!("- `{}`：{}\n", r.raw, r.reason));
        }
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("board"),
            content: warn_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] issue {issue_id} 锚点告警评论落库失败：{e}");
        }
    }
    let anchor_results = nemesis_board::run_anchors(&anchors, &deps.workspace, &worker_report);
    // 锚点全过 → 落 PASS 摘要评论（确定性审计痕迹，T2-1 断言锚点；语义
    // 评审意见另行落评，两层不混）。失败路径的明细由 FAIL 分支评论承载。
    if !anchor_results.is_empty() && nemesis_board::all_passed(&anchor_results) {
        let pass_body = format!(
            "✅ 客观锚点检查通过（{} 条）：\n{}",
            anchor_results.len(),
            nemesis_board::render_anchor_summary(&anchor_results)
        );
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("board"),
            content: pass_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] issue {issue_id} 锚点 PASS 评论落库失败：{e}");
        }
    }
    let output = if !anchor_results.is_empty() && !nemesis_board::all_passed(&anchor_results) {
        warn!("[BoardReview] issue {issue_id} 客观锚点检查失败 → 短路 FAIL（跳过 LLM 语义评审）");
        nemesis_board::ReviewOutput {
            verdict: nemesis_board::ReviewVerdict::Fail,
            reasons: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| format!("锚点失败: {} — {}", r.anchor.raw, r.detail))
                .collect(),
            gap: format!(
                "客观锚点检查失败（确定性核验，未进入 AI 语义评审）：\n{}",
                nemesis_board::render_anchor_failures(&anchor_results)
            ),
            experience: None,
            need_evidence: None,
            evidence_request: None,
        }
    } else {
        if !anchor_results.is_empty() {
            prompt.push_str(&format!(
                "\n## 客观锚点检查（已通过）\n以下客观验收锚点已由系统核验通过，无需重复核验，请专注评审其余语义项：\n{}",
                nemesis_board::render_anchor_summary(&anchor_results)
            ));
        }
        // 色审议需要评审 agent 在场——只有真正要跑 LLM 的路径才要求 loop
        // 就绪（锚点短路 FAIL 是确定性判定，loop 未就绪也成立，人工兜底
        // 照常能看见明细评论）。
        let Some(agent_loop) = deps.moderator_loop.get() else {
            warn!("[BoardReview] issue {issue_id} 主 agent 未就绪，跳过自动验收（人工兜底）");
            return Ok(false);
        };

        // ---- LLM 评审（首跑 + 回灌重试 ≤2，planner 同款）----
        // B2a 工具模式：本机 worker + max_turns>1 → 只读白名单多轮取证；
        // 其余（远端 worker / 默认配置）纯文本单轮（与历史行为字节等价）。
        let tool_mode = pick_review_tool_mode(
            dispatches.last().map(|d| d.worker_id.as_str()),
            deps.cluster.node_id(),
            cfg.review.max_turns,
        );
        match run_review_panel(agent_loop, &prompt, tool_mode, cfg.review.checkers).await {
            Ok(ok) => ok,
            Err(last_err) => {
                // S-O2 迟到评审守卫（评审自身失败路径）：评审期间单据已被
                // 人工改状态 → 不落评论不重派，让位人工（无 verdict 可留痕）。
                if !review_still_relevant(
                    store,
                    issue_id,
                    deps.cluster.node_id(),
                    "parse_failed",
                    "子单验收",
                ) {
                    return Ok(false);
                }
                // 无限模式：解析失败同 UNSURE 语义——带说明继续重派（不转人工）；
                // 差距段给诚实占位（评审系统自身故障不冒充任务差距）。
                if cfg.unlimited_mode {
                    // estop 保险丝检查点 ②：评审 LLM 期间触发急停 → 不再发车
                    // （评审半途的循环由此截断），停车等释放。
                    if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
                        return Ok(false);
                    }
                    // E1 预算保险丝（unlimited 下 WARN 继续，不转人工）。
                    if let Some(breach) = budget_breach(deps, &issue, &cfg) {
                        warn!(
                            "[BoardReview] issue {issue_id} 解析失败重派前预算超限（unlimited_mode 仅告警继续）：{breach}"
                        );
                    }
                    let choice = pick_redispatch_target(deps, &issue, &dispatches)?;
                    let target = choice.into_target();
                    // 发现 D：失败原因文本由 run_review_llm 自带分类前缀
                    //（LLM 调用失败 / 连续 3 轮无法解析），此处不再重复断言轮数。
                    let note =
                        format!("🤷 验收评审未出结论（unlimited_mode 继续重派；{last_err}）");
                    let _ = post_review_comment(store, issue_id, &deps.cluster, &note);
                    match nemesis_web::handlers::board::dispatch_issue_core(
                        store,
                        Some(&deps.cluster),
                        issue_id,
                        &target,
                        &reviewer,
                        Some("验收输出无法定案，请对照验收标准补充客观证据后重新汇报。"),
                    ) {
                        Ok(_) => {
                            warn!(
                                "[BoardReview] issue {issue_id} 解析失败（unlimited_mode）→ 重派 → {target}"
                            );
                            return Ok(true);
                        }
                        Err(e) => {
                            warn!(
                                "[BoardReview] issue {issue_id} 解析失败重派失败：{e}（转人工兜底）"
                            )
                        }
                    }
                }
                // 解析失败 → 当 UNSURE 诚实处置（§6.1），评论说明是评审输出
                // 本身不合规，防误读成任务问题。
                let comment = format!(
                    "{}🤷 验收 agent 无法判定（验收评审自身失败），请人工裁决。\n\n原因：{last_err}",
                    human_mention
                );
                post_review_comment(store, issue_id, &deps.cluster, &comment)?;
                return Ok(true);
            }
        }
    };

    // S-O2 迟到评审守卫：LLM 评审期间单据被人工改状态（reopen/取消等）
    // 则结论整体让位——取证挂起与下方任何处置分支（含最危险的迟到重派）
    // 都不再执行。
    if !review_still_relevant(
        store,
        issue_id,
        deps.cluster.node_id(),
        output.verdict.as_str(),
        "子单验收",
    ) {
        return Ok(false);
    }

    // ---- B2b 取证挂起（P4）：评审需要更多证据 → 向执行 worker 发一轮自检。
    // 挂起在经验落库之前（评审未定案不蒸馏）；锚点短路输出无 need_evidence
    // 槽位，天然不进本分支。二段评审 ctx.allow_selfcheck=false，只带一程。
    if ctx.allow_selfcheck
        && cfg.review.selfcheck
        && let Some(request) = nemesis_board::selfcheck_request_text(&output)
    {
        if deps.selfcheck.has_inflight(issue_id) {
            warn!("[BoardReview] issue {issue_id} 已有在途自检取证，跳过重复挂起，按现有结论处置");
        } else if dispatches.last().map(|d| d.worker_id.as_str()) == Some(deps.cluster.node_id()) {
            // 本机 worker：工件就在本机，取证走 B2a 执行型评审轮（或人工），
            // 不绕 peer_chat 自发自收。
            info!(
                "[BoardReview] issue {issue_id} worker 为本机节点，取证需求交给执行型验收/人工，不另发自检"
            );
        } else {
            // estop 检查点（发车前）：急停中不发自检，停车等释放。
            if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
                return Ok(false);
            }
            let target_worker = dispatches
                .last()
                .map(|d| d.worker_id.clone())
                .ok_or_else(|| "无历史派发记录，无法确定取证目标".to_string())?;
            match dispatch_selfcheck(deps, &issue, &target_worker, &request).await {
                Ok(task_id) => {
                    deps.selfcheck.register(task_id.clone(), issue_id);
                    let note = format!(
                        "⏳ 验收暂缓（board.review.selfcheck）：评审 agent 需要更多证据，已向执行 worker 发起取证（task {task_id}）。\n\n取证请求：{request}\n\nworker 回报后自动进行二段验收。"
                    );
                    let _ = post_review_comment(store, issue_id, &deps.cluster, &note);
                    info!(
                        "[BoardReview] issue {issue_id} 验收挂起等取证（task {task_id}），二段验收待回报"
                    );
                    return Ok(true);
                }
                Err(e) => {
                    warn!(
                        "[BoardReview] issue {issue_id} 取证派发失败：{e}（按现有结论处置，转人工兜底）"
                    );
                    let _ = post_review_comment(
                        store,
                        issue_id,
                        &deps.cluster,
                        &format!("⛔ 取证派发失败（{e}），验收按现有材料处置或人工介入。"),
                    );
                }
            }
        }
    }

    // 经验落库（M4.5 集体记忆 §6.5.1 蒸馏唯一写闸）：评审 agent 的
    // experience 槽位 → team_memory 表。空壳条目（词表外空白 category /
    // 空 scope/content）丢弃；去重合并与计数在 store 内部；失败只 warn
    // 不炸评审流程（经验沉淀是增值动作，验收结论不受影响）。B2b 挂起
    // 路径在上方 return，不蒸馏（评审未定案）。
    store_experience(
        store,
        &issue.number,
        deps.cluster.node_id(),
        &output.experience,
    );

    let unlimited = cfg.unlimited_mode;
    match decide_review_action(
        output.verdict,
        round,
        cfg.max_redispatch,
        cfg.auto_accept,
        unlimited,
    ) {
        ReviewAction::AutoAccept => {
            // EST-03（2026-09-16 横扫加固）：评审 LLM 是长跑——发起时 estop
            // 可能尚未触发、结束时已挂起。收货落 done 前补一道检查点：挂起
            // 即停车入队（release 复评恢复），不越过急停继续自动流转。
            if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
                return Ok(false);
            }
            record_auto_decide(
                store,
                issue_id,
                deps.cluster.node_id(),
                "auto_accept",
                output.verdict.as_str(),
                serde_json::json!({
                    "round": round,
                    "reasons": render_reasons(&output.reasons),
                }),
            );
            auto_accept_and_settle(deps, issue_id, &output.reasons)?;
        }
        ReviewAction::SuggestManual => {
            record_auto_decide(
                store,
                issue_id,
                deps.cluster.node_id(),
                "suggest_manual",
                output.verdict.as_str(),
                serde_json::json!({
                    "round": round,
                    "reasons": render_reasons(&output.reasons),
                }),
            );
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
            // P2A 能力类失败保护（2026-09-12 NB-15 根修）：预判重派目标——
            // 能力类失败（validation_budget / escalation）+ 目标未换（同一
            // worker）= 同模型同环境大概率原样复现。非无限模式不盲派，转
            // 人工并给模型/档位建议；无限模式（无条件流转契约，estop 是
            // 保险丝）仅 WARN 留痕继续。目标预判放在 ❌ 评论/审计之前
            //（pick 纯读无副作用）；下方既有决策点原样保留，通过闸后再走
            // 正常重派（届时二次调用读同一派发历史，结果一致——本闸是
            // 启发式护栏，非正确性不变量）。
            let preview_target = pick_redispatch_target(deps, &issue, &dispatches).ok();
            let capability_fail = (!preview_target.as_ref().is_some_and(|c| c.is_switch()))
                .then(|| latest_fail_class(&comments))
                .flatten()
                .filter(|fc| CAPABILITY_FAIL_CLASSES.contains(fc));
            if let Some(fc) = capability_fail {
                if unlimited {
                    let note = format!(
                        "⚠ 能力类失败（fail_class: {fc}）且重派目标未换——同节点重放大概率原样复现（unlimited_mode 仅告警继续，estop 可随时止血）"
                    );
                    let _ = post_review_comment(store, issue_id, &deps.cluster, &note);
                    warn!(
                        "[BoardReview] issue {issue_id} 能力类失败（{fc}）+ 同目标重派（unlimited_mode 告警继续）"
                    );
                } else {
                    let comment = format!(
                        "{}🤖 能力类失败保护（fail_class: {fc}）：worker 在同一节点反复以相同方式失败，自动重派同节点大概率原样复现，已停止自动重派，请人工裁决。\n\n{}",
                        human_mention,
                        capability_fail_hint(fc)
                    );
                    store.add_comment(NewComment {
                        issue_id,
                        author: reviewer.clone(),
                        content: comment,
                        parent_id: None,
                        ctype: CommentType::Comment,
                    })?;
                    record_auto_decide(
                        store,
                        issue_id,
                        deps.cluster.node_id(),
                        "escalate_human",
                        output.verdict.as_str(),
                        serde_json::json!({
                            "round": round,
                            "fail_class": fc,
                            "reason": "capability_fail_same_target",
                            "reasons": render_reasons(&output.reasons),
                        }),
                    );
                    info!(
                        "[BoardReview] issue {issue_id} 能力类失败（{fc}）+ 同目标 → 转人工（保持 in_review）"
                    );
                    return Ok(true);
                }
            }
            // UNSURE（无限模式）无 gap——给诚实占位意见，重派 prompt 不空转。
            let gap_raw = output.gap.trim();
            let gap = if gap_raw.is_empty() {
                "验收 agent 无法定案（UNSURE）。请对照验收标准逐条自查，补充客观证据后重新汇报。"
            } else {
                gap_raw
            };
            let next_round = round + 1;
            let budget_display = if unlimited {
                "∞".to_string()
            } else {
                cfg.max_redispatch.to_string()
            };
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: format!(
                    "❌ agent 验收未通过（第 {next_round}/{budget_display} 次重派）\n\n## 差距\n{gap}\n{}",
                    render_reasons(&output.reasons)
                ),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            record_auto_decide(
                store,
                issue_id,
                deps.cluster.node_id(),
                "redispatch",
                output.verdict.as_str(),
                serde_json::json!({
                    "round": next_round,
                    "unlimited": unlimited,
                    "reasons": render_reasons(&output.reasons),
                }),
            );
            // 停滞可观测（E3 配套，复审补充）：连续重派且差距文本与上一轮
            // 完全相同 → WARN 告警（只告警不停流程，人可观测不干预）。
            if unlimited && round >= 2 && !gap_raw.is_empty() {
                let stale = comments
                    .iter()
                    .rev()
                    .skip(1) // 跳过刚落的本轮评论
                    .find(|c| c.ctype == CommentType::Comment && c.content.contains("## 差距"))
                    .is_some_and(|c| c.content.contains(gap_raw));
                if stale {
                    warn!(
                        "[BoardReview] issue {issue_id} 停滞嫌疑：第 {next_round} 轮重派差距与上轮相同（unlimited_mode 仅告警不停，estop 可随时止血）"
                    );
                }
            }
            // D3 换节点重派：同一 worker 连续 ≥2 次派发仍 FAIL → 换历史
            // 未用过的次优匹配节点；否则维持同 worker（会话上下文延续）。
            // estop 保险丝检查点 ③：发车前最后一闸（评审 LLM 期间触发急停
            // 的在途轮次在此截断），停车等释放。
            if estop_fuse_engaged(deps, issue_id, ParkedKind::Issue) {
                return Ok(false);
            }
            // E1 预算保险丝（发车前）：三维任一超限 → 停自动重派转人工；
            // unlimited_mode 降级为 WARN 继续（保险丝非护栏的边界一致）。
            if let Some(breach) = budget_breach(deps, &issue, &cfg) {
                if unlimited {
                    warn!(
                        "[BoardReview] issue {issue_id} 预算超限（unlimited_mode 仅告警继续）：{breach}"
                    );
                } else {
                    let msg = format!(
                        "{}🛑 自动重派预算超限，停止自动重派，请人工裁决。\n\n超限项：{breach}",
                        human_mention
                    );
                    post_review_comment(store, issue_id, &deps.cluster, &msg)?;
                    info!("[BoardReview] issue {issue_id} 预算超限 → 停止重派转人工（{breach}）");
                    return Ok(true);
                }
            }
            let target = match pick_redispatch_target(deps, &issue, &dispatches) {
                Ok(c) => {
                    let switched = c.is_switch();
                    let target = c.into_target();
                    if switched {
                        let _ = post_review_comment(
                            store,
                            issue_id,
                            &deps.cluster,
                            &format!("🔁 连续多轮未通过，本次重派换节点执行 → {target}"),
                        );
                    }
                    target
                }
                Err(e) => {
                    let msg = format!("⛔ 验收未通过，确定重派目标失败：{e}（请人工派发或处置）");
                    store.add_comment(NewComment {
                        issue_id,
                        author: Actor::system("board-review"),
                        content: msg,
                        parent_id: None,
                        ctype: CommentType::System,
                    })?;
                    warn!("[BoardReview] issue {issue_id} 重派目标决策失败：{e}");
                    return Ok(true);
                }
            };
            match nemesis_web::handlers::board::dispatch_issue_core(
                store,
                Some(&deps.cluster),
                issue_id,
                &target,
                &reviewer,
                Some(gap),
            ) {
                Ok(_) => {
                    info!("[BoardReview] issue {issue_id} FAIL → 重派 #{next_round} → {target}");
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
            record_auto_decide(
                store,
                issue_id,
                deps.cluster.node_id(),
                "escalate_human",
                output.verdict.as_str(),
                serde_json::json!({
                    "round": round,
                    "reasons": render_reasons(&output.reasons),
                }),
            );
            let mut comment = format!("{}🤷 验收 agent 无法定案，请人工裁决\n", human_mention);
            if output.verdict == nemesis_board::ReviewVerdict::Fail {
                comment.push_str(&format!(
                    "\n重派预算已耗尽（{} 次），最新差距仍在：\n{}\n",
                    cfg.max_redispatch,
                    output.gap.trim()
                ));
            }
            append_reasons_unless_gapped(&mut comment, &output);
            store.add_comment(NewComment {
                issue_id,
                author: reviewer.clone(),
                content: comment,
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            info!(
                "[BoardReview] issue {issue_id} verdict={} → 转人工（保持 in_review）",
                output.verdict.as_str()
            );
        }
    }
    Ok(true)
}

/// 裸提示词 detached 评审调用：首跑 + `parse_review` 失败回灌重试 ≤2 次
/// （共 3 轮）。成功返回解析输出；3 轮全败返回末次错误。`mode` 控制
/// B2a 工具面（NoTools = 历史行为字节等价）。
///
/// 发现 D（2026-09-11 措辞精度）：LLM **调用**失败（网络/超时/上游错误）经
/// `run_detached` 的 Err 在首轮立即返回——不消耗解析重试轮，与本函数内
/// **解析**失败的「连续 3 轮」是两类故障。返回值文本分别带前缀区分，
/// 下游评论不再把调用失败误报成「连续 3 次无法解析」。
async fn run_review_llm(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    prompt: &mut String,
    mode: ReviewToolMode,
) -> Result<nemesis_board::ReviewOutput, String> {
    let mut last_err = String::new();
    for _ in 0..=2 {
        let opts = match mode {
            ReviewToolMode::NoTools => nemesis_agent::r#loop::DetachedOpts {
                system_prompt: Some(nemesis_board::REVIEW_SYSTEM_PROMPT),
                no_tools: true,
                max_turns: 1,
                label: Some("board-review"),
                ..Default::default()
            },
            ReviewToolMode::ReadOnly { max_turns } => nemesis_agent::r#loop::DetachedOpts {
                system_prompt: Some(nemesis_board::REVIEW_SYSTEM_PROMPT),
                no_tools: false,
                allowed_tools: Some(READONLY_REVIEW_TOOLS),
                max_turns,
                label: Some("board-review"),
                ..Default::default()
            },
        };
        let raw = match agent_loop.run_detached(prompt, opts).await {
            Ok(raw) => raw,
            Err(e) => return Err(format!("LLM 调用失败：{e}")),
        };
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
    Err(format!("评审输出连续 3 轮无法解析：{last_err}"))
}

/// 多检查员面板并发上限（P5/B3：N 路评审同时最多 4 路在飞，多余排队）。
const PANEL_CONCURRENCY: usize = 4;
/// 检查员路数收敛上限（配置面同步 clamp；防手滑写 5000 路烧穿 LLM 配额）。
pub(crate) const MAX_REVIEW_CHECKERS: u32 = 5;

/// B3 多检查员面板评审：`checkers > 1` 时并行发出 N 路独立 `run_review_llm`
/// （每路独立 prompt 副本 + 编号后缀去相关，`Semaphore` 限并发，`join_all`
/// 保序），`aggregate_verdicts` 多数票聚合。`checkers = 1`（默认）直接单路
/// 调用，与历史行为字节等价。全路解析失败 → 返回末路错误（与单路 3 轮
/// 全败语义一致）。
async fn run_review_panel(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    base_prompt: &str,
    mode: ReviewToolMode,
    checkers: u32,
) -> Result<nemesis_board::ReviewOutput, String> {
    let checkers = checkers.clamp(1, MAX_REVIEW_CHECKERS) as usize;
    if checkers == 1 {
        let mut prompt = base_prompt.to_string();
        return run_review_llm(agent_loop, &mut prompt, mode).await;
    }
    let sem = Arc::new(tokio::sync::Semaphore::new(PANEL_CONCURRENCY));
    let mut futs = Vec::with_capacity(checkers);
    for i in 0..checkers {
        let mut prompt = format!(
            "{base_prompt}\n\n（你是第 {} 号检查员：请独立评审，不受其他检查员意见影响）",
            i + 1
        );
        let sem = Arc::clone(&sem);
        let agent_loop = Arc::clone(agent_loop);
        futs.push(async move {
            let _permit = sem.acquire_owned().await;
            run_review_llm(&agent_loop, &mut prompt, mode).await
        });
    }
    let results = futures::future::join_all(futs).await;
    let mut oks = Vec::new();
    let mut last_err = String::new();
    for r in results {
        match r {
            Ok(out) => oks.push(out),
            Err(e) => last_err = e,
        }
    }
    if oks.is_empty() {
        return Err(last_err);
    }
    Ok(aggregate_verdicts(checkers, oks))
}

/// 多数票聚合（纯函数，穷举可测）：多数 PASS → Pass；多数 FAIL → Fail；
/// 其余（平票/全 Unsure）→ Unsure（转人工，fail-safe 不偏向自动收货）。
/// reasons 首条注入投票注记（含无效路注记），其余按路合并去重保序；gap
/// 取首个非空；experience 取首个非空；need_evidence/evidence_request 仅在
/// 聚合结论为 Unsure 时保留——多数已定案的结论不被个别证据请求改写
/// （B2b 挂起只服务"评不了"，不推翻"评得了"）。
fn aggregate_verdicts(
    requested: usize,
    outputs: Vec<nemesis_board::ReviewOutput>,
) -> nemesis_board::ReviewOutput {
    let n = outputs.len();
    let pass = outputs
        .iter()
        .filter(|o| o.verdict == nemesis_board::ReviewVerdict::Pass)
        .count();
    let fail = outputs
        .iter()
        .filter(|o| o.verdict == nemesis_board::ReviewVerdict::Fail)
        .count();
    let unsure = n.saturating_sub(pass + fail);
    let verdict = if pass * 2 > n {
        nemesis_board::ReviewVerdict::Pass
    } else if fail * 2 > n {
        nemesis_board::ReviewVerdict::Fail
    } else {
        nemesis_board::ReviewVerdict::Unsure
    };

    let mut reasons = vec![format!(
        "{} 位检查员投票：PASS×{pass} FAIL×{fail} UNSURE×{unsure}（多数票裁决）",
        requested
    )];
    if n < requested {
        reasons.push(format!("另有 {} 位检查员输出无效未计票", requested - n));
    }
    for o in &outputs {
        for r in &o.reasons {
            let r = r.trim();
            if !r.is_empty() && !reasons.iter().any(|x| x.trim() == r) {
                reasons.push(r.to_string());
            }
        }
    }

    let gap = outputs
        .iter()
        .map(|o| o.gap.trim())
        .find(|g| !g.is_empty())
        .unwrap_or("")
        .to_string();
    let experience = outputs.iter().find_map(|o| o.experience.clone());
    let (need_evidence, evidence_request) = if verdict == nemesis_board::ReviewVerdict::Unsure {
        let need = outputs.iter().any(|o| o.need_evidence == Some(true));
        let req = outputs
            .iter()
            .find_map(|o| o.evidence_request.clone().filter(|s| !s.trim().is_empty()));
        (need.then_some(true), req)
    } else {
        (None, None)
    };

    nemesis_board::ReviewOutput {
        verdict,
        reasons,
        gap,
        experience,
        need_evidence,
        evidence_request,
    }
}

/// F3 项目收口验收标准聚合（纯函数）：项目级 AC 排首段（【项目级】标注），
/// 各顶层父单非空 AC 依序拼接（【父单号】标注），段间 `---` 分隔。空白段
/// 跳过（无 AC 的项目/父单不产生空段）。
/// 【label】必须独立成行（2026-09-12 根修）：旧实现拼进 AC 首行
/// （`【项目级】[CHECK] …`），首行不再以 `[CHECK]` 顶格 → 每段第一条
/// 锚点被 parse_anchors 静默吞掉回落语义项（双端真机 S2 实证：项目级
/// AC 的 re:index.html 锚点从未参与客观核验；单行 AC 项目整个锚点面
/// 失效）。
fn join_project_review_ac(project_ac: Option<&str>, parents: &[nemesis_board::Issue]) -> String {
    let mut joined = String::new();
    let mut push = |label: String, ac: &str| {
        let ac = ac.trim();
        if ac.is_empty() {
            return;
        }
        if !joined.is_empty() {
            joined.push_str("\n---\n");
        }
        joined.push_str(&format!("【{label}】\n{ac}\n"));
    };
    if let Some(ac) = project_ac {
        push("项目级".to_string(), ac);
    }
    for p in parents {
        if let Some(ac) = p.acceptance_criteria.as_deref() {
            push(p.number.clone(), ac);
        }
    }
    joined
}

/// 从 home 读 board 段旗标（auto_review/auto_accept/max_redispatch）。
/// 配置读失败 → Err（评审诚实放弃，fail-closed 到人工验收——不拿默认值
/// 顶替用户配置）。P5 冲突漏斗（conflict_auto_resolve）同源消费。
pub(crate) fn load_board_flags(
    home: &std::path::Path,
) -> Result<nemesis_config::BoardFlagConfig, String> {
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

/// gap 与 reasons 的内容重叠守卫（2026-09-12 根修，横扫三处转人工/收口
/// 评论拼装）：锚点短路 FAIL 路径上 `output.gap` 与 `output.reasons` 同源
/// 同构（reasons 每条 = "锚点失败: …"，gap = render_anchor_failures 明细
/// 渲染），两处都拼进评论就是同一条失败锚点重复出现两遍（双端真机 S2
/// 实证：项目收口转人工评论每条失败锚点列出两次）。gap 非空只拼 gap；
/// gap 空（LLM FAIL 未填差距原文的诚实降级）reasons 仍是唯一理由来源，
/// 照常渲染。
fn append_reasons_unless_gapped(comment: &mut String, output: &nemesis_board::ReviewOutput) {
    if output.gap.trim().is_empty() {
        comment.push_str(&render_reasons(&output.reasons));
    }
}

/// E2 决策审计：`auto_decide` 活动落库（决策流视图 `board.audit.list` 的
/// 数据源；`rollback_decision` 只认本 action）。details = JSON：
/// `{decision, verdict, ..extra}`。失败只 warn（审计是增值动作，不炸验收）。
/// P5/F7 冲突四审计（conflict/conflict_auto_resolve/conflict_redispatch/
/// conflict_switch_worker）同走本漏斗——board 冲突防线与验收共用单一审计。
pub(crate) fn record_auto_decide(
    store: &nemesis_board::BoardStore,
    issue_id: i64,
    node_id: &str,
    decision: &str,
    verdict: &str,
    extra: serde_json::Value,
) {
    let mut details = serde_json::json!({ "decision": decision, "verdict": verdict });
    if let (Some(obj), Some(ex)) = (details.as_object_mut(), extra.as_object()) {
        for (k, v) in ex {
            obj.insert(k.clone(), v.clone());
        }
    }
    if let Err(e) = store.add_activity(
        issue_id,
        &Actor::agent(node_id),
        "auto_decide",
        Some(&details.to_string()),
    ) {
        warn!("[BoardReview] issue {issue_id} auto_decide 活动落库失败：{e}");
    }
    // C 里程碑 4（看板项目档案 goal P2）：评审决策 → docs/review/NB-xx.md +
    // timeline。写失败不阻塞评审（writer 内部 WARN+审计）；存量项目静默跳过。
    if let Ok(issue) = store.get_issue(issue_id) {
        nemesis_board::archive_writer::write_review_milestone(
            store, &issue, node_id, decision, verdict, &details,
        );
    }
}

/// S-O2 迟到评审守卫（2026-09-16 showcase 复跑实证）：评审 LLM 异步跑数
/// 分钟，返回时单据可能已被人工 cancel→reopen / 改状态。`transition_issue`
/// 是纯有向边校验——`backlog→done` 是合法边，它不挡迟到结论（实证：NB-2
/// 评审 spawn 后单据被 reopen，迟到 PASS 把 backlog 单直接置 done，依据是
/// 旧交付线程）。结论落地前必须显式复查单据仍在 in_review（评审的法定
/// 起点，与 review_issue 开头的 spawn 前校验对称）；已不在 = 人工干预在先，
/// 自动结论整体让位：丢弃 + 审计留痕 + WARN。返回 false = 结论已丢弃
/// （调用方按「未处置」收尾，与 estop 冻结同语义）。
fn review_still_relevant(
    store: &nemesis_board::BoardStore,
    issue_id: i64,
    node_id: &str,
    verdict: &str,
    phase: &str,
) -> bool {
    let current = match store.get_issue(issue_id) {
        Ok(issue) => issue.status,
        Err(e) => {
            warn!(
                "[BoardReview] {phase} issue {issue_id} 迟到结论复查状态失败（{e}），丢弃让位人工"
            );
            return false;
        }
    };
    if current == IssueStatus::InReview {
        return true;
    }
    warn!(
        "[BoardReview] {phase} issue {issue_id} 评审期间状态已变为 {}（人工干预在先），迟到结论丢弃让位",
        current
    );
    record_auto_decide(
        store,
        issue_id,
        node_id,
        "stale_review_discarded",
        verdict,
        serde_json::json!({
            "phase": phase,
            "current_status": current.as_str(),
            "reason": "issue left in_review while review was in flight",
        }),
    );
    false
}

/// 经验落库（M4.5 蒸馏写闸；子单/父单评审共用单一真相源）。空壳条目
/// 丢弃；失败只 warn 不炸评审流程。
fn store_experience(
    store: &nemesis_board::BoardStore,
    source: &str,
    author_node: &str,
    exp: &Option<nemesis_board::ExperienceNote>,
) {
    let Some(exp) = exp else { return };
    let category = exp.category.trim();
    let content = exp.content.trim();
    if category.is_empty() || exp.scope.trim().is_empty() || content.is_empty() {
        info!(
            "[BoardReview] issue {source} 经验槽位为空壳（category/scope/content 有空白），丢弃不入库"
        );
        return;
    }
    match store.add_team_memory(nemesis_board::models::NewTeamMemory {
        category: category.to_string(),
        scope: exp.scope.trim().to_string(),
        content: content.to_string(),
        source: source.to_string(),
        author: author_node.to_string(),
    }) {
        Ok((id, merged)) => info!(
            "[BoardReview] issue {source} 经验入库：id={id} category={} scope={} merged={merged}",
            category,
            exp.scope.trim()
        ),
        Err(e) => warn!("[BoardReview] issue {source} 经验入库失败：{e}（不影响评审结论）"),
    }
}

/// 全自动流转 P1（A3）：父单收口汇总验收（fire-and-forget）。由 gateway
/// 注册到 `nemesis_web::handlers::board::set_parent_review_hook` 的闭包在
/// sync_parent_status 子单全 done 路径触发；闭包内已读 `auto_close_parent`
/// 旗标，此处只做装配兜底与验收执行。
pub(crate) fn spawn_parent_review(deps: BoardReviewDeps, parent_id: i64) {
    tokio::spawn(async move {
        match review_parent_issue(&deps, parent_id).await {
            Ok(true) => {}
            Ok(false) => {} // 跳过类结果已在 review_parent_issue 内记日志
            Err(e) => warn!("[BoardReview] 父单 {parent_id} 收口验收失败：{e}（人工兜底）"),
        }
    });
}

/// 父单收口评审的项目目录产物证据段（F-U3-6）。清单全列（相对路径 +
/// 大小），小文本文件附内容节选；看板管线目录 `records/`、`.baseline.json`
/// 基线戳与 transfer walk 同源的噪音排除集（SAN-09：venv/缓存等——两处
/// 谓词漂移会让噪音挤占 200 条上限、把真实交付证据挤出评审 prompt）排除。
/// symlink 不跟随（SAN-09：防外部树拉进评审 prompt）。内容条数 / 总条数
/// 双上限防 prompt 爆炸，超出诚实注记。目录不可读时诚实注记（不炸评审）。
fn render_project_artifacts_evidence(dir: &std::path::Path) -> String {
    /// 内容节选的文件大小上限（字节）。
    const CONTENT_MAX_BYTES: u64 = 8 * 1024;
    /// 附内容节选的文件数上限。
    const CONTENT_MAX_FILES: usize = 12;
    /// 清单总条目上限（超出诚实截断）。
    const LISTING_MAX_ENTRIES: usize = 200;
    /// 单文件内容节选字符上限。
    const CONTENT_SNIPPET_CHARS: usize = 2000;

    /// 看板执行档案目录（transfer walk 不知道的看板专属管线产物）。
    fn is_board_pipeline_dir(name: &str) -> bool {
        name == "records"
    }

    fn is_texty(name: &str) -> bool {
        match name.rsplit_once('.') {
            None => true, // 无扩展名（README、Makefile 等）当文本
            Some((_, ext)) => {
                let ext = ext.to_ascii_lowercase();
                matches!(
                    ext.as_str(),
                    "txt"
                        | "md"
                        | "py"
                        | "json"
                        | "csv"
                        | "log"
                        | "toml"
                        | "yaml"
                        | "yml"
                        | "sh"
                        | "bat"
                        | "ps1"
                        | "rs"
                        | "js"
                        | "ts"
                        | "html"
                        | "css"
                        | "cfg"
                        | "ini"
                        | "xml"
                )
            }
        }
    }

    let mut files: Vec<(String, u64)> = Vec::new();
    fn walk(dir: &std::path::Path, rel: &str, out: &mut Vec<(String, u64)>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            let path = e.path();
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            // SAN-09：symlink 不跟随（symlink_metadata 探测；外部树不得
            // 拉进评审 prompt——信息面外泄 + 体积攻击双防）。
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if !is_board_pipeline_dir(&name)
                    && !nemesis_cluster::transfer::is_noise_dir(&child_rel)
                {
                    walk(&path, &child_rel, out)?;
                }
            } else if name != ".baseline.json"
                && !nemesis_cluster::transfer::is_noise_file(&child_rel)
            {
                let size = meta.len();
                out.push((child_rel, size));
            }
            if out.len() > LISTING_MAX_ENTRIES {
                return Ok(());
            }
        }
        Ok(())
    }

    let mut out = String::from(
        "\n## 项目目录实物证据（评审输入 = 本机项目档案目录真实内容，与 worker 声明对照）\n",
    );
    match walk(dir, "", &mut files) {
        Ok(()) => {}
        Err(e) => {
            out.push_str(&format!("（项目目录不可读：{e}）\n"));
            return out;
        }
    }
    if files.is_empty() {
        out.push_str("（项目目录为空：无任何交付产物）\n");
        return out;
    }

    let truncated_listing = files.len() > LISTING_MAX_ENTRIES;
    let mut content_budget = CONTENT_MAX_FILES;
    let mut with_content = 0usize;
    for (rel, size) in &files {
        out.push_str(&format!("- {rel}（{size} 字节）\n"));
        if content_budget > 0
            && *size <= CONTENT_MAX_BYTES
            && is_texty(rel)
            && let Ok(text) = std::fs::read_to_string(dir.join(rel))
        {
            let snippet = nemesis_utils::truncate(text.trim(), CONTENT_SNIPPET_CHARS);
            out.push_str(&format!("  内容节选：\n```text\n{snippet}\n```\n"));
            content_budget -= 1;
            with_content += 1;
        } // 读失败（二进制/编码不符）：只列清单不读内容
    }
    if truncated_listing {
        out.push_str(&format!(
            "（清单超过 {LISTING_MAX_ENTRIES} 条，已诚实截断）\n"
        ));
    }
    if files.len() > with_content {
        out.push_str(&format!(
            "（其余 {} 个文件未附内容节选：大文件 / 非文本 / 节选预算耗尽）\n",
            files.len() - with_content
        ));
    }
    out
}

/// 纪律闭环评审注入（2026-09-24 件4 组件5，F-U3-1/F-U3-7 同族）：变更集
/// 携带 `.discipline/` 纪律产物时，把声明与证伪结果作为系统客观数据注入
/// 评审 prompt。bug-fix 纪律模式下执行者改文件前必须先落六字段声明、收尾
/// 由系统自动跑声明中的证伪命令（nemesis-agent discipline 模块）；**抗糊弄
/// 二道闸就落在这里**——闸门只保证「声明存在、证伪真跑过」，保证不了声明
/// 质量与实验设计，那由评审员对照本段判断。数据源 = 合并 commit 内 blob
/// 原文（非执行者自述，与 F-U3-7 同源）。变更集不含 `.discipline/`（非
/// 纪律任务/产物未随集回流）→ None 诚实跳过（不拿陈旧产物污染无关任务）。
fn render_discipline_evidence(
    root: &std::path::Path,
    commit_oid: &str,
    files: &[(String, String)],
) -> Option<String> {
    const BLOB_READ_MAX_BYTES: usize = 64 * 1024;
    const DECLARATION_SNIPPET_CHARS: usize = 4000;
    const EXCERPT_SNIPPET_CHARS: usize = 2000;
    let disc_paths: Vec<&str> = files
        .iter()
        .map(|(p, _)| p.as_str())
        .filter(|p| p.starts_with(".discipline/"))
        .collect();
    if disc_paths.is_empty() {
        return None;
    }
    let read = |path: &str| -> Option<String> {
        nemesis_board::commit_blob_text(root, commit_oid, path, BLOB_READ_MAX_BYTES)
            .ok()
            .flatten()
    };
    let mut out = String::from(
        "\n## 纪律闭环证据（系统客观数据）\n\
         本任务变更集携带 `.discipline/` 纪律产物：该任务在 bug-fix 纪律模式下执行——\
         修改文件前必须先落六字段声明（根因/真相源/不变量/影响面/单变量/证伪命令），\
         收尾时系统自动执行声明中的证伪命令（exit 0 = 假设未被证伪），预算 2 次封顶。\n\
         以下为合并 commit 内 blob 原文（非执行者自述）：\n",
    );
    // 声明本体（评审对照点：六字段是否具体、单变量是否真单变量）。
    if let Some(text) = read(".discipline/declaration.json") {
        let snippet = nemesis_utils::truncate(text.trim(), DECLARATION_SNIPPET_CHARS);
        out.push_str(&format!(
            "### 声明 declaration.json\n```json\n{snippet}\n```\n"
        ));
    } else {
        out.push_str("### 声明 declaration.json（变更集未携带或读取失败）\n");
    }
    // 证伪结果（falsification-{n}.json，结构化呈现；解析失败附原文）。
    let mut fals: Vec<&str> = disc_paths
        .iter()
        .copied()
        .filter(|p| p.starts_with(".discipline/falsification-") && p.ends_with(".json"))
        .collect();
    fals.sort();
    for path in fals {
        match read(path) {
            Some(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => {
                    let run = v.get("run").and_then(|x| x.as_u64()).unwrap_or(0);
                    let passed = v.get("passed").and_then(|x| x.as_bool()).unwrap_or(false);
                    let cmd = v.get("command").and_then(|x| x.as_str()).unwrap_or("");
                    let excerpt = v
                        .get("output_excerpt")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    out.push_str(&format!(
                        "### 证伪记录 falsification-{run}.json\n- 结果：{}\n- 命令：`{cmd}`\n- 输出节选：\n```text\n{}\n```\n",
                        if passed {
                            "✅ 通过（exit 0）"
                        } else {
                            "❌ 未通过（非零退出/执行失败——预算耗尽即停车升级，本记录即留盘注记）"
                        },
                        nemesis_utils::truncate(excerpt, EXCERPT_SNIPPET_CHARS),
                    ));
                }
                Err(_) => {
                    let snippet = nemesis_utils::truncate(text.trim(), EXCERPT_SNIPPET_CHARS);
                    out.push_str(&format!(
                        "### {path}（非 JSON，原文）\n```text\n{snippet}\n```\n"
                    ));
                }
            },
            None => out.push_str(&format!("### {path}（读取失败）\n")),
        }
    }
    // waive 审计（逃生门留痕——带理由中途退出也进评审视野）。
    if disc_paths.iter().any(|p| p.ends_with("waive-audit.jsonl"))
        && let Some(text) = read(".discipline/waive-audit.jsonl")
    {
        let snippet = nemesis_utils::truncate(text.trim(), EXCERPT_SNIPPET_CHARS);
        out.push_str(&format!(
            "### waive 审计 waive-audit.jsonl\n```jsonl\n{snippet}\n```\n"
        ));
    }
    out.push_str(
        "评审提示：闸门只保证「声明存在、证伪真跑过」，不保证声明质量——请核对：\
         ①六字段是否具体可查（根因到 file:line、单变量是否真单变量）；\
         ②证伪命令是否真能检验根因（而非恒过形式命令）；\
         ③证伪通过不等于验收标准满足，仍需对照验收标准独立判断。\n",
    );
    Some(out)
}

/// 父单收口汇总验收：输入=父单目标/验收标准 + 全部子单状态与最新交付摘要
/// （P1 阶段 LLM 汇总单判；P2 锚点检查器建成后自动升级双检）。三态简化：
/// PASS → done（auto_close_parent 本身即收货授权，评论注明）；FAIL/UNSURE
/// → 转人工保持 in_review（父单不派发，无重派承接单位——无限模式也不
/// 盲转，复审边界）。返回 `Ok(false)` = 未评审即跳过。
async fn review_parent_issue(deps: &BoardReviewDeps, parent_id: i64) -> Result<bool, String> {
    // estop 保险丝检查点 ①：父单收口同样是自动 agent 活动，挂起即冻结。
    if estop_fuse_engaged(deps, parent_id, ParkedKind::Parent) {
        return Ok(false);
    }
    let cfg = load_board_flags(&deps.home)?;
    // 双保险（hook 已读一次）：配置读失败/关闭一律 fail-closed 回人工。
    if !cfg.auto_close_parent || !cfg.auto_review {
        return Ok(false);
    }

    let store = &deps.store;
    let parent = store.get_issue(parent_id)?;
    // 只有 in_review 才收口评审：与人工操作竞态时让位人工，不覆盖。
    if parent.status != IssueStatus::InReview {
        return Ok(false);
    }
    // 范围缺口边界：子树（Subtree 闭包，SAN-08——旧单层 children 扫描看
    // 不见 done 子单之下的 cancelled 孙单，收口会带着隐藏缺口宣称完成）
    // 存在 cancelled → 范围缺口要人裁决，验收补不了。
    let subtree = store.descendants(&[parent_id], nemesis_board::store::DescendantEdges::Subtree);
    if subtree.iter().any(|c| c.status == IssueStatus::Cancelled) {
        info!("[BoardReview] 父单 {parent_id} 子树存在 cancelled 单，不自动收口（转人工）");
        return Ok(false);
    }
    let children = store.list_children(parent_id)?;

    // ---- 组装汇总输入：每子单状态 + 最新交付摘要（截断防 prompt 爆炸）----
    let mut summary = format!("## 子任务完成情况汇总（共 {} 个子任务）\n", children.len());
    for c in &children {
        let latest_delivery = store
            .list_comments(c.id)?
            .iter()
            .rev()
            .find(|cm| cm.ctype == CommentType::Delivery)
            .map(|cm| nemesis_utils::truncate(cm.content.trim(), 1500))
            .unwrap_or_else(|| "（无结构化交付汇报）".to_string());
        summary.push_str(&format!(
            "### 子任务 {}「{}」状态={} 最新交付摘要：\n{}\n\n",
            c.number, c.title, c.status, latest_delivery
        ));
    }

    // ---- F-U3-6（2026-09-15 U3 真机）：项目目录产物证据段 ----
    // 父单验收标准的核心一条 = 「产物齐全于项目目录」，但此前评审输入只有
    // 子单 Delivery 声明摘要（worker 自述）——U3 实证：report.md /
    // pytest_output.txt 等产物全部真实在项目目录，评审员却因「取证文件
    // 内容未随报提供、无法客观确认产物齐全」判 UNSURE 转人工。注入真实
    // 清单 + 内容节选后，声明 vs 实物可对照（无绑定目录的存量项目诚实跳过）。
    if let Some(pid) = parent.project_id
        && let Ok(project) = store.get_project(pid)
        && let Some(dir) = project.directory.as_deref()
    {
        summary.push_str(&render_project_artifacts_evidence(std::path::Path::new(
            dir,
        )));
    }

    let mut prompt = nemesis_board::build_review_user_prompt(
        &parent.number,
        &parent.title,
        &parent.description,
        parent.acceptance_criteria.as_deref(),
        &summary,
        // 父单线程评论以 status_change/system 机械记录为主（子单结论已在
        // 汇总段），不带入。
        &[],
    );

    // ---- 客观锚点双检（P2/B1，父单同款）：先便宜后昂贵 ----
    // 父单验收输入 = 子单交付汇总段；锚点全过 → 摘要拼进 prompt 仍审语义；
    // 有 fail → 合成 FAIL 短路（跳过 LLM），流经下方 verdict 分臂（父单
    // 不派发、无重派承接单位——转人工语义与 LLM FAIL 一致）。不安全锚点
    // 行同款解析期拒绝 + 系统告警评论（T2-3）。
    let (anchors, _semantic_fallback, rejected_anchors) =
        nemesis_board::parse_anchors(parent.acceptance_criteria.as_deref().unwrap_or(""));
    if !rejected_anchors.is_empty() {
        let mut warn_body =
            String::from("⚠ 验收标准含不安全/非法锚点行，已忽略（按普通语义标准评审）：\n");
        for r in &rejected_anchors {
            warn_body.push_str(&format!("- `{}`：{}\n", r.raw, r.reason));
        }
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id: parent_id,
            author: nemesis_board::Actor::system("board"),
            content: warn_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] 父单 {parent_id} 锚点告警评论落库失败：{e}");
        }
    }
    let anchor_results = nemesis_board::run_anchors(&anchors, &deps.workspace, &summary);
    // 锚点全过 → PASS 摘要评论（父单同款，确定性审计痕迹）。
    if !anchor_results.is_empty() && nemesis_board::all_passed(&anchor_results) {
        let pass_body = format!(
            "✅ 客观锚点检查通过（{} 条）：\n{}",
            anchor_results.len(),
            nemesis_board::render_anchor_summary(&anchor_results)
        );
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id: parent_id,
            author: nemesis_board::Actor::system("board"),
            content: pass_body,
            parent_id: None,
            ctype: CommentType::System,
        }) {
            warn!("[BoardReview] 父单 {parent_id} 锚点 PASS 评论落库失败：{e}");
        }
    }
    let output = if !anchor_results.is_empty() && !nemesis_board::all_passed(&anchor_results) {
        warn!("[BoardReview] 父单 {parent_id} 客观锚点检查失败 → 短路 FAIL（跳过 LLM 语义评审）");
        nemesis_board::ReviewOutput {
            verdict: nemesis_board::ReviewVerdict::Fail,
            reasons: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| format!("锚点失败: {} — {}", r.anchor.raw, r.detail))
                .collect(),
            gap: format!(
                "客观锚点检查失败（确定性核验，未进入 AI 语义评审）：\n{}",
                nemesis_board::render_anchor_failures(&anchor_results)
            ),
            experience: None,
            need_evidence: None,
            evidence_request: None,
        }
    } else {
        if !anchor_results.is_empty() {
            prompt.push_str(&format!(
                "\n## 客观锚点检查（已通过）\n以下客观验收锚点已由系统核验通过，无需重复核验，请专注评审其余语义项：\n{}",
                nemesis_board::render_anchor_summary(&anchor_results)
            ));
        }
        // 色审议需要评审 agent 在场；锚点短路 FAIL 是确定性判定，loop
        // 未就绪也成立（明细评论照常落库，人工兜底可见）。
        let Some(agent_loop) = deps.moderator_loop.get() else {
            warn!("[BoardReview] 父单 {parent_id} 主 agent 未就绪，跳过收口验收（人工兜底）");
            return Ok(false);
        };
        // 父单收口恒纯文本（B2a 执行型验收只适用子单——父单工件分散在
        // 多个子单的 worker 会话，master 就地读文件无从对应）。
        match run_review_panel(
            agent_loop,
            &prompt,
            ReviewToolMode::NoTools,
            cfg.review.checkers,
        )
        .await
        {
            Ok(ok) => ok,
            Err(last_err) => {
                let comment = format!(
                    "🤷 父单收口验收无法完成（验收评审自身失败），请人工裁决。\n\n原因：{last_err}"
                );
                post_review_comment(store, parent_id, &deps.cluster, &comment)?;
                return Ok(true);
            }
        }
    };

    store_experience(
        store,
        &parent.number,
        deps.cluster.node_id(),
        &output.experience,
    );

    let reviewer = Actor::agent(deps.cluster.node_id());
    // S-O2 迟到评审守卫（同子单验收）：收口评审期间父单被人工改状态则
    // 结论整体让位（PASS 直推 done 是 backlog→done 合法边的同型竞态面）。
    if !review_still_relevant(
        store,
        parent_id,
        deps.cluster.node_id(),
        output.verdict.as_str(),
        "父单收口",
    ) {
        return Ok(false);
    }
    match output.verdict {
        nemesis_board::ReviewVerdict::Pass => {
            store.add_comment(NewComment {
                issue_id: parent_id,
                author: reviewer.clone(),
                content: format!(
                    "✅ 父单收口验收通过（board.auto_close_parent 已开启，自动收口）\n\n{}",
                    render_reasons(&output.reasons)
                ),
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            record_auto_decide(
                store,
                parent_id,
                deps.cluster.node_id(),
                "parent_auto_close",
                output.verdict.as_str(),
                serde_json::json!({ "reasons": render_reasons(&output.reasons) }),
            );
            store.transition_issue(parent_id, IssueStatus::Done, &reviewer)?;
            info!("[BoardReview] 父单 {parent_id} 收口验收 PASS → done（auto_close_parent）");
            // C-F5（复核 2026-09-16）：父单落 done 改走与子单落定同源的
            // on_issue_settled 单一出口。此前只显式调项目收口触发
            // （notify_project_review_on_parent_done，on_issue_settled 内部
            // 同点调用，行为等价不重复），漏掉两条联动：多级看板沿祖先链
            // 向上 sync_parent_status（父单的父单收口感知）+ dependents
            // 依赖补派。收敛安全：收口 hook 是异步 spawn 无同步递归；
            // sync 带 visited 防环；终态单 terminal 早退不重复触发收口。
            nemesis_web::handlers::board::on_issue_settled(
                store,
                Some(&deps.cluster),
                parent_id,
                &reviewer,
            );
        }
        verdict @ (nemesis_board::ReviewVerdict::Fail | nemesis_board::ReviewVerdict::Unsure) => {
            let human_mention = if parent.creator.kind == "admin" {
                format!("@{} ", parent.creator.id)
            } else {
                String::new()
            };
            let mut comment = format!(
                "{human_mention}🤷 父单收口验收未定案（{ver}），请人工裁决。\n",
                ver = verdict.as_str()
            );
            if verdict == nemesis_board::ReviewVerdict::Fail {
                comment.push_str(&format!("\n差距：\n{}\n", output.gap.trim()));
            }
            append_reasons_unless_gapped(&mut comment, &output);
            comment.push_str(
                "\n（父单不派发、无重派承接单位——请检查各子单结论后人工定 done 或重开子单）",
            );
            store.add_comment(NewComment {
                issue_id: parent_id,
                author: reviewer.clone(),
                content: comment,
                parent_id: None,
                ctype: CommentType::Comment,
            })?;
            record_auto_decide(
                store,
                parent_id,
                deps.cluster.node_id(),
                "parent_escalate_human",
                verdict.as_str(),
                serde_json::json!({ "reasons": render_reasons(&output.reasons) }),
            );
            info!(
                "[BoardReview] 父单 {parent_id} 收口验收 verdict={} → 转人工（保持 in_review）",
                verdict.as_str()
            );
        }
    }
    Ok(true)
}

/// B2b 二段验收（fire-and-forget）：gateway 回调闭包在 [`SelfcheckRegistry`]
/// 命中后调用（issue_id 由闭包 `take` 出，避免双取竞态）。worker 回报失败
/// （status=error）→ 诚实转人工；正常回报 → 证据带入二段评审
/// （`allow_selfcheck=false`：取证只带一程，二段结论直接三态处置）。
pub(crate) fn spawn_selfcheck_second_stage(
    deps: BoardReviewDeps,
    issue_id: i64,
    status: String,
    response: String,
) {
    tokio::spawn(async move {
        if status == "error" {
            warn!(
                "[BoardReview] issue {issue_id} 自检取证回报失败（status=error）→ 转人工（评审保持 in_review）"
            );
            let _ = post_review_comment(
                &deps.store,
                issue_id,
                &deps.cluster,
                "⚠ 自检取证回报失败（worker 侧出错），取证闭环无法完成，请人工裁决或重新派发。",
            );
            return;
        }
        info!("[BoardReview] issue {issue_id} 取证回报到位 → 二段验收");
        let ctx = ReviewCtx {
            allow_selfcheck: false,
            evidence: Some(response.to_string()),
        };
        match review_issue(&deps, issue_id, ctx).await {
            Ok(true) => {}
            Ok(false) => {} // 跳过类结果已在 review_issue 内记日志
            Err(e) => warn!("[BoardReview] issue {issue_id} 二段验收失败：{e}（人工兜底）"),
        }
    });
}

/// B2b 自检取证提示词：给执行 worker 的指令——只收集证据如实回报，不下
/// 验收结论、不改已交付内容。marker 仅供人读（路由凭 SelfcheckRegistry）。
fn build_selfcheck_prompt(issue: &nemesis_board::Issue, request: &str) -> String {
    format!(
        "[取证请求 board_selfcheck:{number}] 验收方对照验收标准评审你的交付后，需要以下证据才能定案。\
请在本机完成取证并如实回报证据内容（逐项说明，含关键输出原文），不要下验收结论、不要修改已交付内容：\n\n{request}\n\n## 验收标准（取证对照）\n{ac}\n",
        number = issue.number,
        request = request,
        ac = issue
            .acceptance_criteria
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("（未提供）"),
    )
}

/// B2b 自检派发：向执行 worker 发 peer_chat 取证请求。与正式派发
/// （fire-and-forget）不同，这里**同步等 RPC 送达**（30s）——送不到就不
/// 挂起评审（回调永远不会来，挂起即悬挂），诚实返回 Err 转人工兜底。
/// 会话键与正式派发一致（`board:{number}`）：取证请求落在 worker 的既有
/// 任务会话里，上下文天然延续。返回 task_id（调用方注册进
/// [`SelfcheckRegistry`]）。
async fn dispatch_selfcheck(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    worker: &str,
    request: &str,
) -> Result<String, String> {
    let cluster = &deps.cluster;
    let prompt = build_selfcheck_prompt(issue, request);
    let source_node_id = cluster.node_id().to_string();
    let chat_id = format!("board:{}", issue.number);
    let source_payload = serde_json::json!({
        "node_id": source_node_id,
        "channel": "board",
        "chat_id": chat_id,
    });
    let task_id = cluster.submit_peer_chat(
        worker,
        "peer_chat",
        serde_json::json!({ "content": prompt, "_source": source_payload }),
        "board",
        &chat_id,
    )?;
    let rpc_client = cluster.rpc_client_arc().ok_or("RPC client not available")?;
    let request_rpc = nemesis_cluster::rpc_types::RPCRequest {
        id: task_id.clone(),
        action: nemesis_cluster::rpc_types::ActionType::Known(
            nemesis_cluster::rpc_types::KnownAction::PeerChat,
        ),
        payload: serde_json::json!({
            "content": prompt,
            "task_id": task_id,
            "_source": source_payload,
            "_source_rpc_port": cluster.rpc_port(),
        }),
        source: source_node_id,
        target: Some(worker.to_string()),
    };
    rpc_client
        .call_with_timeout(worker, request_rpc, std::time::Duration::from_secs(30))
        .await
        .map_err(|e| format!("取证 RPC 送达失败：{e}"))?;
    Ok(task_id)
}

/// F3 项目收口汇总验收（fire-and-forget）：gateway 注册到
/// `nemesis_web::handlers::board::set_project_review_hook`；触发前置
/// （全部顶层父单 done + 项目非空 + 状态 active/in_progress）在 web 侧
/// notify 做过，此处 estop/配置双保险 + 验收执行 + 三态处置。
pub(crate) fn spawn_project_review(deps: BoardReviewDeps, project_id: i64) {
    tokio::spawn(async move {
        match review_project_completion(&deps, project_id).await {
            Ok(true) => {}
            Ok(false) => {} // 跳过类结果已在 review_project_completion 内记日志
            Err(e) => warn!("[BoardReview] 项目 {project_id} 收口验收失败：{e}（人工兜底）"),
        }
    });
}

/// F3 项目收口汇总验收。返回 `Ok(false)` = 未评审即跳过（配置关/项目已
/// 收口/竞态防御复核不过/estop 冻结），`Ok(true)` = 评审闭环完成。
async fn review_project_completion(
    deps: &BoardReviewDeps,
    project_id: i64,
) -> Result<bool, String> {
    // estop 保险丝检查点 ①：项目收口同样是自动 agent 活动。
    if estop_fuse_engaged(deps, project_id, ParkedKind::Project) {
        return Ok(false);
    }
    let cfg = load_board_flags(&deps.home)?;
    // 双保险（hook 已读一次）：auto_review 总闸 + review.auto_close_project。
    if !cfg.auto_review || !cfg.review.auto_close_project {
        return Ok(false);
    }

    let store = &deps.store;
    let project = store.get_project(project_id)?;
    if matches!(project.status.as_str(), "completed" | "archived") {
        debug!(
            "[BoardReview] 项目 {project_id} 已是 {}，跳过收口验收",
            project.status
        );
        return Ok(false);
    }

    // 触发前置防御性复核（触发到评审排队之间人工可能又动了单子）。
    let parents: Vec<nemesis_board::Issue> = store
        .list_issues(&nemesis_board::models::IssueFilter {
            project_id: Some(project_id),
            ..Default::default()
        })?
        .into_iter()
        .filter(|i| i.parent_issue_id.is_none())
        .collect();
    if parents.is_empty() {
        return Ok(false);
    }
    if parents.iter().any(|p| p.status != IssueStatus::Done) {
        info!("[BoardReview] 项目 {project_id} 存在非 done 顶层父单（竞态），跳过收口验收");
        return Ok(false);
    }
    // 范围缺口边界（与父单收口同款语义 + SAN-08 子树闭包）：任何子孙
    // cancelled（含 done 父单之下的隐藏缺口）→ 范围缺口要人裁决，项目级
    // 验收补不了。
    for p in &parents {
        let subtree = store.descendants(&[p.id], nemesis_board::store::DescendantEdges::Subtree);
        if subtree.iter().any(|c| c.status == IssueStatus::Cancelled) {
            info!(
                "[BoardReview] 项目 {project_id} 父单 {}（{}）子树存在 cancelled 单，不自动收口（转人工）",
                p.number, p.title
            );
            return Ok(false);
        }
    }

    // ---- 组装汇总输入：每顶层父单段 = 自身最新交付（若有）+ 子单交付
    // 摘要（截断防 prompt 爆炸）。2026-09-12 根修：旧实现只取父单**自身**
    // 最新 Delivery——实际数据流中 worker 交付评论落在叶子子单上（对比
    // review_parent_issue：它遍历的就是子单），父单自身常无 Delivery，
    // 聚合到空集 → 项目级锚点对「（无结构化交付汇报）」实核必然全 FAIL
    // （双端真机 S2 实证：三条 re: 锚点全败，子单真实交付从未进入实核
    // 文本）。聚合深度对齐 review_parent_issue（单层子单——planner 拆解
    // 纪律即单层），不引入新的嵌套深度语义。
    let mut summary = format!("## 顶层任务完成情况汇总（共 {} 个）\n", parents.len());
    for p in &parents {
        let children = store.list_children(p.id)?;
        summary.push_str(&format!(
            "### 任务 {}「{}」状态={}（子单 {} 个）\n",
            p.number,
            p.title,
            p.status,
            children.len(),
        ));
        if let Some(cm) = store
            .list_comments(p.id)?
            .iter()
            .rev()
            .find(|cm| cm.ctype == CommentType::Delivery)
        {
            summary.push_str(&format!(
                "父单最新交付摘要：\n{}\n\n",
                nemesis_utils::truncate(cm.content.trim(), 1500)
            ));
        }
        if children.is_empty() {
            summary.push_str("（无子单）\n\n");
        }
        for c in &children {
            let latest_delivery = store
                .list_comments(c.id)?
                .iter()
                .rev()
                .find(|cm| cm.ctype == CommentType::Delivery)
                .map(|cm| nemesis_utils::truncate(cm.content.trim(), 1500))
                .unwrap_or_else(|| "（无结构化交付汇报）".to_string());
            // F-U6-2：附子单独立验收结论（最新 auto_decide）——此前评审员
            // 只看到「状态=done」会误读为「worker 自评」，U6 实证 UNSURE
            // 理由即含「子单仅有自评 done、无独立验收结论」（事实错误：
            // 每张子单都有 agent 评审 auto_accept 决策入账）。只带决策词
            // 不带理由全文（防 prompt 爆炸）。
            let verdict_note = store
                .list_activity(c.id)?
                .iter()
                .rev()
                .find(|a| a.action == "auto_decide")
                .and_then(|a| a.details.as_deref())
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .and_then(|v| {
                    v.get("decision")
                        .and_then(|d| d.as_str())
                        .map(|s| s.to_string())
                })
                .map(|d| format!("（子单评审结论：{d}）"))
                .unwrap_or_default();
            summary.push_str(&format!(
                "- 子任务 {}「{}」状态={}{} 最新交付摘要：\n{}\n\n",
                c.number, c.title, c.status, verdict_note, latest_delivery
            ));
        }
    }

    // ---- F-U6-2（2026-09-15 U6 真机）：项目目录实物证据段（review_parent_issue
    // 的 F-U3-6 同款——横向扫同类补齐）。项目收口验收标准与父单同形（「项目
    // 目录含 xxx」），但此前评审输入只有各子单 Delivery 声明摘要（worker 自述，
    // 且 exec 工作副本路径 Linux/Windows 混杂）——U6 实证：父单收口评审凭
    // 实物证据四条标准全过 PASS，项目收口评审却因「无项目目录实物直接取证 /
    // 子单仅有自评」判 UNSURE 转人工，两道评审同一标准结论矛盾。注入真实
    // 清单 + 内容节选后与父单收口同源对照（无绑定目录的存量项目诚实跳过）。
    if let Some(dir) = project.directory.as_deref() {
        summary.push_str(&render_project_artifacts_evidence(std::path::Path::new(
            dir,
        )));
    }

    // 验收标准聚合（F3 项目收口判定依据）：项目级 AC 在前（v10 列——此前
    // 只聚合父单 AC，project.create 存的项目级标准被静默丢弃），各顶层父单
    // 非空 AC 依序拼接；锚点行照常参与客观双检。
    let joined_ac = join_project_review_ac(Some(&project.acceptance_criteria), &parents);

    let mut prompt = nemesis_board::build_review_user_prompt(
        &format!("PROJECT-{}", project.id),
        &project.name,
        &project.description,
        Some(&joined_ac),
        &summary,
        &[],
    );

    // ---- 客观锚点双检（父单/子单同款）：workspace 根，先便宜后昂贵 ----
    let (anchors, _semantic_fallback, rejected_anchors) = nemesis_board::parse_anchors(&joined_ac);
    if !rejected_anchors.is_empty() {
        warn!(
            "[BoardReview] 项目 {project_id} 验收标准含 {} 条不安全锚点行（已忽略，按普通语义标准评审）",
            rejected_anchors.len()
        );
    }
    let anchor_results = nemesis_board::run_anchors(&anchors, &deps.workspace, &summary);
    let output = if !anchor_results.is_empty() && !nemesis_board::all_passed(&anchor_results) {
        warn!("[BoardReview] 项目 {project_id} 客观锚点检查失败 → 短路 FAIL（跳过 LLM 语义评审）");
        nemesis_board::ReviewOutput {
            verdict: nemesis_board::ReviewVerdict::Fail,
            reasons: anchor_results
                .iter()
                .filter(|r| !r.passed)
                .map(|r| format!("锚点失败: {} — {}", r.anchor.raw, r.detail))
                .collect(),
            gap: format!(
                "客观锚点检查失败（确定性核验，未进入 AI 语义评审）：\n{}",
                nemesis_board::render_anchor_failures(&anchor_results)
            ),
            experience: None,
            need_evidence: None,
            evidence_request: None,
        }
    } else {
        if !anchor_results.is_empty() {
            prompt.push_str(&format!(
                "\n## 客观锚点检查（已通过）\n以下客观验收锚点已由系统核验通过，无需重复核验，请专注评审其余语义项：\n{}",
                nemesis_board::render_anchor_summary(&anchor_results)
            ));
        }
        let Some(agent_loop) = deps.moderator_loop.get() else {
            warn!("[BoardReview] 项目 {project_id} 主 agent 未就绪，跳过收口验收（人工兜底）");
            return Ok(false);
        };
        // 项目级恒纯文本（同父单边界：工件分散在各 worker 会话）。
        match run_review_panel(
            agent_loop,
            &prompt,
            ReviewToolMode::NoTools,
            cfg.review.checkers,
        )
        .await
        {
            Ok(ok) => ok,
            Err(last_err) => {
                // 解析失败 → 全部顶层父单写转人工评论（项目无评论表，
                // 落在父单线程里人必然看得见）。
                let comment = format!(
                    "🏛 项目「{}」收口验收无法完成（验收评审自身失败），请人工裁决。\n\n原因：{last_err}",
                    project.name
                );
                for p in &parents {
                    let _ = post_review_comment(store, p.id, &deps.cluster, &comment);
                }
                return Ok(true);
            }
        }
    };

    store_experience(
        store,
        &format!("project:{}", project.name),
        deps.cluster.node_id(),
        &output.experience,
    );

    apply_project_review_outcome(deps, &project, &parents, &output)?;
    Ok(true)
}

/// F3 三态落盘（独立 fn 供单测直测，不经 LLM）：
/// - PASS → 项目 completed（`board.review.auto_close_project` 本身即收口
///   授权，评论注明）。
/// - FAIL/UNSURE → completed 回滚 in_progress（F2 状态机合法转移，P3
///   预留）；已在 in_progress 的不动；archived 完全不动（只 warn）。缺口
///   评论落全部顶层父单（@各自创建者；LLM gap 无法可靠映射到具体父单，
///   诚实全量留痕），**不自动重开父单、不重派**（F3 边界）。
fn apply_project_review_outcome(
    deps: &BoardReviewDeps,
    project: &nemesis_board::models::Project,
    parents: &[nemesis_board::Issue],
    output: &nemesis_board::ReviewOutput,
) -> Result<(), String> {
    let store = &deps.store;
    let reviewer = Actor::agent(deps.cluster.node_id());
    match output.verdict {
        nemesis_board::ReviewVerdict::Pass => {
            // active 不可直达 completed（状态机只认 in_progress→completed）：
            // 人工路径下父单全 done 但项目仍 active 时先补 in_progress 中转
            // （两跳都合法），自动流（F2 首派联动）本就 in_progress 不进此臂。
            if project.status == "active" {
                store.update_project(
                    project.id,
                    &nemesis_board::models::ProjectPatch {
                        status: Some("in_progress".to_string()),
                        ..Default::default()
                    },
                )?;
            }
            store.update_project(
                project.id,
                &nemesis_board::models::ProjectPatch {
                    status: Some("completed".to_string()),
                    ..Default::default()
                },
            )?;
            info!(
                "[BoardReview] 项目 {}「{}」收口验收 PASS → completed（review.auto_close_project）",
                project.id, project.name
            );
            for p in parents {
                record_auto_decide(
                    store,
                    p.id,
                    deps.cluster.node_id(),
                    "project_complete",
                    output.verdict.as_str(),
                    serde_json::json!({
                        "project_id": project.id,
                        "reasons": render_reasons(&output.reasons),
                    }),
                );
            }
            // F9 收口总结（P6）：completed 落定后异步生成档案 summary.md
            //（失败诚实留痕，不影响已落定的收口状态）。
            crate::board_review::spawn_project_summary(deps.clone(), project.id);
        }
        verdict @ (nemesis_board::ReviewVerdict::Fail | nemesis_board::ReviewVerdict::Unsure) => {
            // completed → in_progress 回滚（合法转移）；in_progress 保持；
            // archived 不动。
            if project.status == "completed" {
                store.update_project(
                    project.id,
                    &nemesis_board::models::ProjectPatch {
                        status: Some("in_progress".to_string()),
                        ..Default::default()
                    },
                )?;
                info!(
                    "[BoardReview] 项目 {} 验收未定案 → completed 回滚 in_progress（F3）",
                    project.id
                );
            } else if project.status == "archived" {
                warn!(
                    "[BoardReview] 项目 {} 已 archived，验收未定案结论只留评论不回滚状态",
                    project.id
                );
            }
            let mut comment = format!(
                "🏛 项目「{name}」收口验收未定案（{ver}），请人工裁决。\n",
                name = project.name,
                ver = verdict.as_str()
            );
            if output.verdict == nemesis_board::ReviewVerdict::Fail {
                comment.push_str(&format!("\n差距：\n{}\n", output.gap.trim()));
            }
            append_reasons_unless_gapped(&mut comment, output);
            comment.push_str(
                "\n（项目级验收不自动重开父单、不重派——请检查各任务结论后人工处置；\
处置完成后项目可人工设 completed）",
            );
            for p in parents {
                let human_mention = if p.creator.kind == "admin" {
                    format!("@{} ", p.creator.id)
                } else {
                    String::new()
                };
                store.add_comment(NewComment {
                    issue_id: p.id,
                    author: reviewer.clone(),
                    content: format!("{human_mention}{comment}"),
                    parent_id: None,
                    ctype: CommentType::Comment,
                })?;
            }
            info!(
                "[BoardReview] 项目 {} 收口验收 verdict={} → 缺口评论 {} 条父单，转人工",
                project.id,
                verdict.as_str(),
                parents.len()
            );
            for p in parents {
                record_auto_decide(
                    store,
                    p.id,
                    deps.cluster.node_id(),
                    "project_escalate_human",
                    verdict.as_str(),
                    serde_json::json!({
                        "project_id": project.id,
                        "reasons": render_reasons(&output.reasons),
                    }),
                );
            }
        }
    }
    Ok(())
}

/// AutoAccept 收货 + settle 联动（P2 #0 断链修复）：评论「自动收货」→
/// 落 done → **必须**走 `on_issue_settled`（父单状态同步 + 依赖子单补派）
/// ——与 WSAPI issue.status 落 done/cancelled 同一真相源；漏调即 T1-2
/// 断链（子单 done 后续依赖子单永不派发、父单永不收口）。独立成 fn 供
/// 回归测试直测联动（review_issue 主链需 LLM 才能到本分支）。
fn auto_accept_and_settle(
    deps: &BoardReviewDeps,
    issue_id: i64,
    reasons: &[String],
) -> Result<(), String> {
    let store = &deps.store;
    let reviewer = Actor::agent(deps.cluster.node_id());
    store.add_comment(NewComment {
        issue_id,
        author: reviewer.clone(),
        content: format!(
            "✅ agent 验收通过（board.auto_accept 已开启，自动收货）\n\n{}",
            render_reasons(reasons)
        ),
        parent_id: None,
        ctype: CommentType::Comment,
    })?;
    store.transition_issue(issue_id, IssueStatus::Done, &reviewer)?;
    nemesis_web::handlers::board::on_issue_settled(store, Some(&deps.cluster), issue_id, &reviewer);
    info!("[BoardReview] issue {issue_id} PASS → done（auto_accept）+ settle 联动");
    Ok(())
}

/// estop 保险丝（全自动流转 P1/T1-6）：急停挂起 = true 并把 issue 停车
/// （release 后 [`spawn_estop_resume_watcher`] 复评恢复）。评论失败只
/// warn——冻结本身不依赖落库。重复停车按 issue_id 去重（防多检查点重复
/// 入队/刷评论）。
fn estop_fuse_engaged(deps: &BoardReviewDeps, issue_id: i64, kind: ParkedKind) -> bool {
    if !deps.estop.is_engaged() {
        return false;
    }
    {
        let mut q = deps
            .estop_parked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !q.iter().any(|&(_, id)| id == issue_id) {
            q.push((kind, issue_id));
        }
    }
    warn!("[BoardReview] issue {issue_id} estop 冻结：自动验收/重派停车（急停释放后自动恢复）");
    let _ = post_review_comment(
        &deps.store,
        issue_id,
        &deps.cluster,
        "⛔ estop 急停中：自动验收/重派已冻结（保险丝）。急停释放后自动恢复。",
    );
    true
}

/// 停车场 sweep 触发闸（estop × 节流；集群完备性加固 2026-09-11 从
/// gateway 内联闭包抽取，可测）：急停挂起 → false 且**不消耗节流窗口**
/// —— 急停期间的 announce 刷新不吃掉释放后的首次重试机会。非急停时按
/// `min_interval` 节流（抗 announce 风暴），放行即盖章。gateway 装配的
/// sweep 回调把「本节点 announce」与「cluster 槽位未填」两条件留在调用
/// 侧（闭包捕获相关，与本闸正交）。
pub(crate) fn park_sweep_gate(
    estop_engaged: bool,
    last: &mut Option<std::time::Instant>,
    now: std::time::Instant,
    min_interval: std::time::Duration,
) -> bool {
    if estop_engaged {
        return false;
    }
    let ok = match *last {
        Some(t) => now.duration_since(t) >= min_interval,
        None => true,
    };
    if ok {
        *last = Some(now);
    }
    ok
}

/// estop 释放 watcher：订阅急停状态 watch，true→false 沿（释放）把停车
/// 队列逐条复评——无限模式循环从断点恢复（T1-6「release 后恢复」）；再
/// 按态补扫一轮 `replay_stuck_reviews`（R5-BUG-1：在 LLM 中途被打断的
/// 在途评审不进停车场，release 后靠扫态兜底复活）。gateway 装配期调用
/// 一次；四入口（CLI/托盘/Dashboard/WSAPI）最终都走同一
/// `EstopState::release()`，watch 全覆盖。
pub(crate) fn spawn_estop_resume_watcher(deps: BoardReviewDeps) {
    let mut rx = deps.estop.subscribe();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            if *rx.borrow_and_update() {
                continue; // 触发沿不动作（冻结在各检查点生效）
            }
            let parked: Vec<(ParkedKind, i64)> = deps
                .estop_parked
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .drain(..)
                .collect();
            for (kind, issue_id) in &parked {
                info!(
                    "[BoardReview] estop 释放 → 恢复{} {issue_id} 自动验收",
                    match kind {
                        ParkedKind::Issue => "",
                        ParkedKind::Parent => "父单",
                        ParkedKind::Project => "项目",
                    }
                );
                match kind {
                    ParkedKind::Issue => spawn_board_review(deps.clone(), *issue_id),
                    ParkedKind::Parent => spawn_parent_review(deps.clone(), *issue_id),
                    ParkedKind::Project => spawn_project_review(deps.clone(), *issue_id),
                }
            }
            // P4/E4（看板项目档案 goal）：急停挂起的合并补跑——PLACED 注册表
            // 里尚存的条目（estop 触发时合并被拒、条目保留）逐个重试；在途
            // 派发自然按 Waiting* 跳过。
            crate::board_archive_ingest::retry_placed_merges(&deps);
            // R5-BUG-1 根修：停车场只收「评审入口检查点被闸」的登记；已过
            // 检查点、在 LLM 中途被 estop 打断的评审不进队列（直接死亡），
            // drain 完停车场后按态补扫一遍兜底（复用启动重放/评审重放的
            // 守卫矩阵：无结论才重放、转人工 sticky、读失败 fail-closed）。
            // skip 刚唤醒的条目——入口竞态守卫拦不住并发双跑，必须去重。
            replay_stuck_reviews(&deps, &parked);
        }
    });
}

/// 启动重放守卫：该单据当前 in_review 态是否已有评审结论。判据 = 最新
/// `auto_decide` 活动 vs 最近一次**正常**进入 in_review 的转移，按
/// activity_log 插入序（append-only 单调 id，秒级时间戳同秒不可分）比先后：
/// - 无任何 auto_decide → 从未评审过（崩溃杀掉 spawn 的典型窗口）→ 重放；
/// - 结论在转移之后 → 当前态已定案（含转人工——stickiness 必须保持，重放
///   不得翻盘；含 audit.rollback 的人工纠错窗口——rollback 落的
///   `audit_rollback:…` 活动不是 JSON、不被计为正常转移，其 in_review
///   归属上一次已定案的转移 → 不重放）；
/// - 结论在转移之前 → 旧轮结论（如重派后再交付），本轮评审还没跑 → 重放。
///
/// 读失败 fail-closed 按「已定案」处理（不重放）。
fn review_already_concluded(store: &nemesis_board::BoardStore, issue_id: i64) -> bool {
    let Ok(acts) = store.list_activity(issue_id) else {
        return true;
    };
    let Some(last_decide) = acts.iter().rposition(|a| a.action == "auto_decide") else {
        return false;
    };
    let last_enter = acts.iter().rposition(|a| {
        a.action == "status_changed"
            && a.details
                .as_deref()
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .and_then(|v| {
                    v.get("to")
                        .and_then(|t| t.as_str())
                        .map(|t| t == IssueStatus::InReview.as_str())
                })
                .unwrap_or(false)
    });
    match last_enter {
        Some(pos) => last_decide >= pos,
        // 有结论但查不到进入转移（旧数据/手工造数）→ 保守不重放。
        None => true,
    }
}

/// 项目级收口结论探测：任一顶层父单携带本项目的 auto_decide
///（`project_complete` / `project_escalate_human` 记在各父单名下，details
/// 带 `project_id`）= 项目收口评审已跑过（转人工 sticky 同样成立）→ 不重放。
fn project_review_already_concluded(
    store: &nemesis_board::BoardStore,
    parent_ids: &[i64],
    project_id: i64,
) -> bool {
    parent_ids.iter().any(|pid| {
        store
            .list_activity(*pid)
            .map(|acts| {
                acts.iter().any(|a| {
                    a.action == "auto_decide"
                        && a.details
                            .as_deref()
                            .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                            .and_then(|v| v.get("project_id").and_then(|p| p.as_i64()))
                            .map(|p| p == project_id)
                            .unwrap_or(false)
                })
            })
            .unwrap_or(true) // 读失败按已结论处理（不重放）
    })
}

/// 评审重放（2026-09-15 R4-BUG-2 根修；R5-BUG-1 扩展到 estop 释放沿）：
/// 评审 spawn 是纯内存 fire-and-forget 任务，in-flight 评审死于两种场景
/// 都无重触发——① master 重启杀掉（单据/父单永久卡 in_review、项目永久
/// 卡「验收中」，真机实证：父单收口验收 spawn 15s 后 master 被杀，重启后
/// NB-3 永卡 in_review）；② estop 在评审**入口检查点之后**打断（LLM 中途
/// 被杀）——此路不走 `estop_fuse_engaged` 停车登记，release 后 resume
/// watcher 的停车场队列是空的，单据同样永卡（真机实证：NB-16 收口评审
/// LLM 中被打断，release 后 10s 零复活）。与 estop resume watcher（冻结
/// 恢复）互补，这里管「评审死了没结论」的按态恢复：
///
/// - 叶子 in_review 且从未评审（有 Delivery = writeback 真发生过；人工
///   手动挪 in_review 的纯看板单无 Delivery，不扫）→ 重放子单验收；
/// - 父单 in_review 且全子单 done 且从未收口评审 → 重放收口（部分完成态
///   的父单 in_review 属人工/缺口语义，不自动收口）；
/// - 项目 active 且全顶层父单 done 且无项目级结论 → 重放项目收口。
///
/// 级联自愈：重放验收 PASS → on_issue_settled / notify_project_review 链
/// 逐层点火——只要最深的断链复活，上层自动跟上；重复触发由三个评审函数
/// 内建守卫（estop 保险丝 / auto_review 旗标现读 / status==InReview 竞态
/// 让位 / cancelled 缺口转人工）吸收，幂等安全。两个调用点：gateway
/// 装配期（三类钩子注册完成后，`skip` 传空）+ estop 释放沿（resume
/// watcher drain 完停车场后，`skip` 传刚唤醒的条目防同一单双重 spawn——
/// 入口竞态守卫拦不住并发双跑，两个 spawn 都能在写回前通过 in_review
/// 检查）。
pub(crate) fn replay_stuck_reviews(deps: &BoardReviewDeps, skip: &[(ParkedKind, i64)]) {
    let store = &deps.store;
    let parked_awake = |kind: ParkedKind, id: i64| skip.iter().any(|&(k, i)| k == kind && i == id);
    let mut replayed = 0usize;
    match store.list_issues(&nemesis_board::models::IssueFilter {
        status: Some(IssueStatus::InReview),
        ..Default::default()
    }) {
        Ok(issues) => {
            for issue in issues {
                if review_already_concluded(store, issue.id) {
                    continue;
                }
                let Ok(children) = store.list_children(issue.id) else {
                    continue;
                };
                if children.is_empty() {
                    if parked_awake(ParkedKind::Issue, issue.id) {
                        continue;
                    }
                    // 叶子单：仅 writeback 交付过的（有 Delivery）才重放。
                    let delivered = store
                        .list_comments(issue.id)
                        .map(|cs| cs.iter().any(|c| c.ctype == CommentType::Delivery))
                        .unwrap_or(false);
                    if !delivered {
                        continue;
                    }
                    info!(
                        "[BoardReview] 评审重放：issue {}（{}）in_review 无评审结论 → 重放验收",
                        issue.number, issue.title
                    );
                    spawn_board_review(deps.clone(), issue.id);
                    replayed += 1;
                } else if children.iter().all(|c| c.status == IssueStatus::Done) {
                    if parked_awake(ParkedKind::Parent, issue.id) {
                        continue;
                    }
                    info!(
                        "[BoardReview] 评审重放：父单 {}（{}）全子单 done 无收口结论 → 重放收口验收",
                        issue.number, issue.title
                    );
                    spawn_parent_review(deps.clone(), issue.id);
                    replayed += 1;
                }
            }
        }
        Err(e) => warn!("[BoardReview] 评审重放扫描 in_review 单据失败：{e}"),
    }
    // 项目级：全顶层父单已 done（父单若还卡 in_review，上面的重放链会
    // 在收口落 done 时经 notify_project_review 点火，这里不必抢跑）。
    if let Ok(projects) = store.list_projects() {
        for project in projects {
            if matches!(project.status.as_str(), "completed" | "archived") {
                continue;
            }
            let Ok(parents) = store.list_issues(&nemesis_board::models::IssueFilter {
                project_id: Some(project.id),
                ..Default::default()
            }) else {
                continue;
            };
            let tops: Vec<i64> = parents
                .iter()
                .filter(|i| i.parent_issue_id.is_none())
                .map(|i| i.id)
                .collect();
            if tops.is_empty()
                || parents
                    .iter()
                    .filter(|i| i.parent_issue_id.is_none())
                    .any(|p| p.status != IssueStatus::Done)
            {
                continue;
            }
            if project_review_already_concluded(store, &tops, project.id) {
                continue;
            }
            if parked_awake(ParkedKind::Project, project.id) {
                continue;
            }
            info!(
                "[BoardReview] 评审重放：项目 {}「{}」全父单 done 无收口结论 → 重放项目收口",
                project.id, project.name
            );
            spawn_project_review(deps.clone(), project.id);
            replayed += 1;
        }
    }
    if replayed > 0 {
        info!("[BoardReview] 评审重放完成：{replayed} 个评审/收口任务已重新触发");
    }
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

// ---------------------------------------------------------------------------
// 看板项目档案 P6（F9）：收口总结生成
//
// 项目 completed 后异步生成档案总结 `<archive>/docs/summary.md`（「AI 生成件」
// 标注 + 生成时间 + 模型名）。复用评审通道（detached 纯文本单轮）+ tier 闸
// （拒 mini，同冲突硬解——小模型写长文不可靠）。**生成失败诚实留痕，不阻塞
// 收口状态机**（completed 在调用前已落定，本函数只做锦上添花）。
// ---------------------------------------------------------------------------

/// 收口总结系统提示词（自由 Markdown，不走评审 verdict JSON——与评审
/// 通道共用的是 LLM 调用通道本身，不是输出格式）。
const PROJECT_SUMMARY_SYSTEM_PROMPT: &str = "\
你是 NemesisBot 看板的项目档案管理员。项目刚刚收口（completed），请依据下方事实清单写一份收口回顾总结，Markdown 输出。

硬性要求：
1. 恰好三个小节：「## 各任务做法」「## 决策流摘要」「## 最终结构」。
2. 严格依据事实清单，不虚构清单之外的文件/决策/结论；总长不超过 600 字。
3. 只输出 Markdown 正文，不要代码围栏，不要任何额外说明。";

/// F9 触发入口：项目收口（自动 PASS / 人工 project.update → completed）
/// 两条路径都汇到这里。异步不阻塞调用方；内部自守门（estop/tier/档案
/// 目录缺失全部诚实跳过）。
pub(crate) fn spawn_project_summary(deps: BoardReviewDeps, project_id: i64) {
    tokio::spawn(async move {
        if let Err(e) = run_project_summary(&deps, project_id).await {
            warn!("[BoardReview] 项目 {project_id} 收口总结生成失败（不阻塞收口）: {e}");
            summarize_fail_note(&deps, project_id, &e);
        }
    });
}

/// 失败留痕：顶层父单评论 + 档案 timeline（能写哪个写哪个，都失败只 log）。
fn summarize_fail_note(deps: &BoardReviewDeps, project_id: i64, err: &str) {
    let comment =
        format!("📝 AI 收口总结生成失败（不阻塞收口，可人工补写档案 docs/summary.md）：{err}");
    if let Ok(project) = deps.store.get_project(project_id) {
        if let Ok(root) = project_archive_root(project.directory.as_deref()) {
            let _ = nemesis_board::archive::append_timeline(
                &root,
                "summary",
                None,
                "board",
                &format!("收口总结生成失败：{err}"),
            );
        }
        if let Ok(issues) = deps.store.list_issues(&nemesis_board::models::IssueFilter {
            project_id: Some(project_id),
            ..Default::default()
        }) {
            for p in issues.iter().filter(|i| i.parent_issue_id.is_none()) {
                let _ = deps.store.add_comment(NewComment {
                    issue_id: p.id,
                    author: Actor::agent(deps.cluster.node_id()),
                    content: comment.clone(),
                    parent_id: None,
                    ctype: CommentType::System,
                });
            }
        }
    }
}

/// 档案目录解析：Project.directory（绝对路径）存在才可用；缺失 = 存量
/// 项目/目录被手删——诚实跳过（None 语义）。
fn project_archive_root(directory: Option<&str>) -> Result<std::path::PathBuf, String> {
    let raw = directory.ok_or("项目未绑定档案目录（存量项目不参与档案管线）")?;
    let root = std::path::PathBuf::from(raw);
    if !root.is_dir() {
        return Err(format!("档案目录不存在: {}", root.display()));
    }
    Ok(root)
}

/// F9 主体。前置检查全部通过才调 LLM；任何失败向上返回（由调用方留痕）。
async fn run_project_summary(deps: &BoardReviewDeps, project_id: i64) -> Result<(), String> {
    // 急停中不跑 LLM（ cosmetic 件不值得占保险丝语义，跳过即可）。
    if deps.estop.is_engaged() {
        return Err("急停（E-STOP）生效中，收口总结跳过".to_string());
    }
    let project = deps.store.get_project(project_id)?;
    if project.status != "completed" {
        // 触发与落定之间的竞态（人工又改回）：诚实跳过。
        return Err(format!(
            "项目状态已非 completed（{}），收口总结跳过",
            project.status
        ));
    }
    let root = project_archive_root(project.directory.as_deref())?;
    let agent_loop = deps
        .moderator_loop
        .get()
        .cloned()
        .ok_or("主 agent 未就绪，收口总结跳过")?;
    // tier 闸拒 mini（同冲突硬解；提示切模型后……无自动重试入口——下次
    // 收口的项目会再试，本单诚实留痕）。
    if matches!(
        agent_loop.tier(),
        nemesis_types::capability::ModelTier::Mini
    ) {
        return Err("当前模型能力档为 mini，收口总结需要 normal/big 档模型".to_string());
    }

    let issues = deps
        .store
        .list_issues(&nemesis_board::models::IssueFilter {
            project_id: Some(project_id),
            ..Default::default()
        })?;
    if issues.is_empty() {
        return Err("项目无任务，无可总结内容".to_string());
    }
    let decisions = deps
        .store
        .list_recent_activity(500, Some("auto_decide"))
        .unwrap_or_default();
    let facts = build_summary_facts(&deps.store, &project, &issues, &decisions);

    let prompt = format!(
        "# 项目「{name}」收口事实清单\n\n{facts}",
        name = project.name
    );
    let raw = agent_loop
        .run_detached(
            &prompt,
            nemesis_agent::r#loop::DetachedOpts {
                system_prompt: Some(PROJECT_SUMMARY_SYSTEM_PROMPT),
                no_tools: true,
                max_turns: 1,
                label: Some("project-summary"),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("LLM 调用失败：{e}"))?;
    let text = raw.trim();
    if text.is_empty() {
        return Err("LLM 返回空内容".to_string());
    }

    let model = agent_loop.active_model();
    let header = format!(
        "> 本文件为 **AI 生成件**（NemesisBot 看板收口总结）。\n\
         > 生成时间：{} ｜ 模型：{model} ｜ 内容由 AI 归纳，请以 records/ 执行档案为准。\n",
        chrono::Local::now().to_rfc3339(),
    );
    let appendix = format!(
        "\n\n---\n\n## 附录：档案结构\n\n{}",
        render_archive_tree(&root, 2)
    );
    let doc = format!("{header}\n{text}{appendix}\n");
    let target = root.join("docs").join("summary.md");
    std::fs::create_dir_all(root.join("docs")).map_err(|e| format!("创建 docs/ 失败: {e}"))?;
    std::fs::write(&target, doc).map_err(|e| format!("写 summary.md 失败: {e}"))?;
    let _ = nemesis_board::archive::append_timeline(
        &root,
        "summary",
        None,
        "board",
        &format!("收口总结生成（模型 {model}）"),
    );
    info!(
        "[BoardReview] 项目 {}「{}」收口总结已生成 → {}",
        project_id,
        project.name,
        target.display()
    );
    Ok(())
}

/// F9 事实清单（纯函数，单测直测）：项目行 + 逐单状态/末评摘录 + 决策流
/// 压缩行。LLM 只看这份清单——不读盘、不猜。
fn build_summary_facts(
    store: &nemesis_board::BoardStore,
    project: &nemesis_board::models::Project,
    issues: &[nemesis_board::models::Issue],
    decisions: &[nemesis_board::models::AuditDecisionRow],
) -> String {
    let ids: std::collections::HashSet<i64> = issues.iter().map(|i| i.id).collect();
    let mut out = String::new();
    out.push_str(&format!(
        "- 项目：{}（状态 {}）{}\n",
        project.name,
        project.status,
        if project.description.is_empty() {
            String::new()
        } else {
            format!("：{}", project.description)
        }
    ));
    out.push_str("\n## 任务清单\n");
    for i in issues {
        let depth = if i.parent_issue_id.is_some() {
            "  - "
        } else {
            "- "
        };
        out.push_str(&format!(
            "{depth}{} {}（状态 {}）\n",
            i.number,
            i.title,
            i.status.as_str()
        ));
        // 末条评论摘录（交付/评审结论落点；取 300 字防清单爆炸）。
        if let Ok(comments) = store.list_comments(i.id)
            && let Some(last) = comments.last()
        {
            let excerpt: String = last.content.chars().take(300).collect();
            out.push_str(&format!(
                "    末评[{}/{}]: {excerpt}\n",
                last.author.kind, last.author.id
            ));
        }
    }
    out.push_str("\n## 决策流（自动化决策）\n");
    let mut n = 0usize;
    for d in decisions {
        if !ids.contains(&d.activity.issue_id) {
            continue;
        }
        let kind =
            serde_json::from_str::<serde_json::Value>(d.activity.details.as_deref().unwrap_or(""))
                .ok()
                .and_then(|v| {
                    v.get("decision")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "auto_decide".to_string());
        out.push_str(&format!("- {} {}：{kind}\n", d.issue_number, d.issue_title));
        n += 1;
    }
    if n == 0 {
        out.push_str("（无自动化决策记录）\n");
    }
    out
}

/// 档案结构树（确定性附录；depth 层目录 + 文件行，超深目录以 … 截断）。
fn render_archive_tree(root: &std::path::Path, depth: usize) -> String {
    let mut out = String::new();
    fn walk(dir: &std::path::Path, prefix: &str, depth: usize, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            out.push_str(&format!(
                "{prefix}{}{}\n",
                name,
                if is_dir { "/" } else { "" }
            ));
            if is_dir && depth > 1 {
                walk(&e.path(), &format!("{prefix}  "), depth - 1, out);
            }
        }
    }
    out.push_str("archive/\n");
    walk(root, "  ", depth, &mut out);
    out
}

#[cfg(test)]
mod tests;
