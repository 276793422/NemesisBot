// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 board_review.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

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
/// - P34：`board.fingerprint_weighting=true` 时匹配器排序先过指纹稳定
///   加权（[`fingerprint_weighted_ranked`]：prefer 提前 / avoid 靠后，
///   非排除），再取历史未用者——成功率先行，匹配器名次是基础序。
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
    let ranked = fingerprint_weighted_ranked(deps, issue, ranked);
    // A1（2026-10-21 引入）：耗时估计次级重排——指纹
    // （成功率）先行，速度在成功率中性段内细分；正交信号按此序叠加。
    let ranked = duration_weighted_ranked(&deps.store, issue, ranked);
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
            // 候选为空的两因区分（2026-10-08 NB-4 根修的可观测面）：file:
            // 锚点单是拓扑约束过滤（永远如此，不是瞬态），其余是候选确实
            // 不在线（瞬态，节点上线后换节点面恢复）。
            let why = if nemesis_board::anchor::has_file_anchor(
                issue.acceptance_criteria.as_deref().unwrap_or(""),
            ) {
                "file: 锚点单只能本节点执行（拓扑约束，远端候选已过滤）"
            } else {
                "在线无其他候选节点"
            };
            warn!(
                "[BoardReview] issue {} 连续 {consecutive} 次同 worker {last}，{why}，维持原目标重派",
                issue.id
            );
            Ok(RedispatchTargetChoice::Same(last))
        }
    }
}

/// AOR 批三 5.2（sweep 监察环策略化）：换节点目标选取——在线 ranked 候选
/// 经指纹/耗时加权排序后，取第一个非排除节点（排除 = 刚失败/离线的
/// worker）。`pick_redispatch_target` 的「连续 <2 次 → Same」决策表面向
/// 评审重派（同 worker 偶发失败优先原地重试），监察环的失败形态是
/// 超时/离线，原节点必须排除或降权——独立入口，复用同一加权链
/// （rank → fingerprint → duration）。返回 None = 无可换节点。
pub(crate) fn pick_sweep_switch_target(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    exclude_worker: &str,
) -> Option<String> {
    let ranked =
        nemesis_web::handlers::board::rank_dispatch_candidates(&deps.store, &deps.cluster, issue);
    let ranked = fingerprint_weighted_ranked(deps, issue, ranked);
    let ranked = duration_weighted_ranked(&deps.store, issue, ranked);
    let picked = ranked.into_iter().find(|c| c != exclude_worker);
    if let Some(t) = &picked {
        info!(
            "[BoardReview] issue {} sweep 换节点候选：排除 {exclude_worker} 后取 {t}",
            issue.id
        );
    }
    picked
}

/// P34 指纹稳定加权（`pick_redispatch_target` 的次级重排）：`board.
/// fingerprint_weighting=true` 时按 (worker, 任务类型) 历史成功率把候选
/// 分 prefer → neutral → avoid 三段（段内保匹配器序）。旗标 off / 配置
/// 读失败 / 指纹读取失败 / 候选不足 2 / 无指纹数据 → 原序返回（默认态
/// 与现状行为字节等价；读失败不炸决策——加权是启发式不是正确性不变量）。
fn fingerprint_weighted_ranked(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    ranked: Vec<String>,
) -> Vec<String> {
    let weighting = load_board_flags(&deps.home)
        .map(|c| c.fingerprint_weighting)
        .unwrap_or(false);
    if !weighting || ranked.len() < 2 {
        return ranked;
    }
    let task_type = nemesis_board::task_type_of(&issue.title, &issue.required_tags);
    let fps = match deps.store.worker_fingerprints(&task_type) {
        Ok(fps) if !fps.is_empty() => fps,
        Ok(_) => return ranked, // 无任何指纹数据：无可加权面
        Err(e) => {
            warn!(
                "[BoardReview] issue {} 指纹读取失败（原序继续）：{e}",
                issue.id
            );
            return ranked;
        }
    };
    let ordered = nemesis_board::apply_fingerprint_weights(ranked, &fps);
    info!(
        "[BoardReview] issue {} 指纹加权生效（task_type={task_type}）：候选序 → {:?}",
        issue.id, ordered
    );
    ordered
}

/// A1（2026-10-21 引入）：耗时估计次级重排（指纹加权之后的
/// 第三段排序）：按 (worker, required_profession) 历史耗时中位数把「同类
/// 任务跑得快」的节点提前——借鉴前代实现运行时估计器（滑窗中位数 +
/// 欠采样回落全局 + 无数据零干预）。数据面 = 本仓现成的 `issue_dispatch`
/// 完结派发表（master 侧单钟），无新计时埋点、无新配置旗标（数据即闸：
/// 没有完结派发就自动零干预）。候选不足 2 / 无样本 / 查询失败 → 原序
/// 返回；失败只 warn——启发式不炸决策（与指纹加权同纪律）。
pub(super) fn duration_weighted_ranked(
    store: &nemesis_board::BoardStore,
    issue: &nemesis_board::Issue,
    ranked: Vec<String>,
) -> Vec<String> {
    if ranked.len() < 2 {
        return ranked;
    }
    let category = nemesis_board::normalize_category(issue.required_profession.as_deref());
    match store.recent_completed_dispatch_durations((nemesis_board::WORKER_SAMPLE_CAP * 8) as i64) {
        Ok(samples) if !samples.is_empty() => {
            let ordered = nemesis_board::reorder_by_predicted_duration(ranked, &samples, &category);
            info!(
                "[BoardReview] issue {} 耗时加权生效（category={category}）：候选序 → {:?}",
                issue.id, ordered
            );
            ordered
        }
        Ok(_) => ranked,
        Err(e) => {
            warn!(
                "[BoardReview] issue {} 耗时样本查询失败（原序继续）：{e}",
                issue.id
            );
            ranked
        }
    }
}

/// P34 指纹记账（评审定案唯一写点，见模块头注释）：PASS=成功 / FAIL=
/// 失败记入 (worker, 任务类型) 计数；UNSURE 不记（没定案不配记成败）。
/// 无派发历史（人工转入 in_review）无从记账。幂等键 = task_id（一派发
/// 轮一条：评审重放 / estop 复评同轮重评不重复计数）。记账与
/// fingerprint_weighting 开关解耦（灰度期照常攒数据）；失败只 warn——
/// 增值动作，不炸验收流程。
pub(super) fn record_fingerprint_outcome(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    dispatches: &[nemesis_board::models::DispatchRecord],
    verdict: nemesis_board::ReviewVerdict,
) {
    let success = match verdict {
        nemesis_board::ReviewVerdict::Pass => true,
        nemesis_board::ReviewVerdict::Fail => false,
        nemesis_board::ReviewVerdict::Unsure => return,
    };
    let Some(last) = dispatches.last() else {
        return;
    };
    let task_type = nemesis_board::task_type_of(&issue.title, &issue.required_tags);
    match deps
        .store
        .record_fingerprint_outcome(&last.worker_id, &task_type, &last.task_id, success)
    {
        Ok(true) => debug!(
            "[BoardReview] issue {} 指纹记账：worker={} task_type={task_type} → {}",
            issue.id,
            last.worker_id,
            if success { "成功" } else { "失败" }
        ),
        Ok(false) => debug!(
            "[BoardReview] issue {} 指纹记账幂等跳过（task {} 已记过）",
            issue.id, last.task_id
        ),
        Err(e) => warn!("[BoardReview] issue {} 指纹记账失败（忽略）：{e}", issue.id),
    }
}

/// P34：重派换节点评论的档位注记（可审计：「档位 prefer，成功率 5/6」
/// 形态，调用方包括号）。开关关 / 配置读失败 → None（评论与现状字节
/// 一致）；开关开 → 有样本给档位+成功率，无样本诚实注明（不虚构 0/0）。
pub(super) fn fingerprint_note_text(
    deps: &BoardReviewDeps,
    issue: &nemesis_board::Issue,
    target: &str,
) -> Option<String> {
    let weighting = load_board_flags(&deps.home)
        .map(|c| c.fingerprint_weighting)
        .unwrap_or(false);
    if !weighting {
        return None;
    }
    let task_type = nemesis_board::task_type_of(&issue.title, &issue.required_tags);
    let (success, total) = match deps.store.worker_fingerprint(target, &task_type) {
        Ok(Some(pair)) => pair,
        Ok(None) | Err(_) => (0, 0),
    };
    Some(nemesis_board::tier_note(success, total))
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
    // D9-1（2026-10-13 第 6 轮审计）：created_at 是 store::now() 的 unix
    // **秒**（store.rs:111），曾直接从毫秒 now 里减——差值恒带 ~1.77e12
    // ms 的单位偏移，配了墙钟预算的父单**刚建单即报超支**，E1 保险丝第
    // 三维形同虚设且永远误熔断。秒→毫秒显式换算后作差。
    if budget.wall_clock_budget_secs > 0 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let created_ms = issue.created_at.saturating_mul(1000);
        let budget_ms = i64::try_from(budget.wall_clock_budget_secs)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        if let Some(elapsed_ms) = now_ms.checked_sub(created_ms)
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

/// ② 分类→重派策略映射（AOR 批一b 泛化，替代原 CAPABILITY_FAIL_CLASSES
/// 三值集合）：fail_class → 重派处置语义的单一真相源。分类存在时映射表
/// 优先于纯预算逻辑；无分类（None）保持现状预算逻辑（兼容不变）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailClassPolicy {
    /// 瞬态：同节点重试有意义（llm_timeout / llm_rate_limit / llm_failure /
    /// empty_result）——现状预算逻辑，不加闸。
    TransientRetry,
    /// 能力/授权类：同目标重放大概率复现——换节点优先（validation_budget /
    /// escalation / mandate_violation / exec_failed），无可换节点转人工。
    /// 原三值能力集（P2A）语义 + exec_failed（保守：同能力类处理）。
    CapabilitySwitch,
    /// 环境类：换节点重试（environmental / resource_conflict）——不同节点
    /// 环境不同，同目标重放无意义；无可换节点转人工（装配/错峰建议）。
    SwitchNode,
    /// 冻结：副作用后果不明，不自动重派转人工（unknown_side_effect；
    /// token_budget 语义同族——其专用闸在 Redispatch 臂更早处拦截，此处
    /// 收录是为映射矩阵完整性）——unlimited 不豁免。
    Freeze,
}

/// 纯函数映射（单测钉死全矩阵）：类值 → 重派策略。值集与
/// latest_fail_class 的 KNOWN 白名单一一对应（双端同步纪律）。
pub(crate) fn redispatch_policy(fc: &str) -> FailClassPolicy {
    match fc {
        "environmental" | "resource_conflict" => FailClassPolicy::SwitchNode,
        "unknown_side_effect" | "token_budget" => FailClassPolicy::Freeze,
        "llm_timeout" | "llm_rate_limit" | "llm_failure" | "empty_result" => {
            FailClassPolicy::TransientRetry
        }
        // validation_budget / escalation / mandate_violation / exec_failed
        _ => FailClassPolicy::CapabilitySwitch,
    }
}

/// 从评论线程提取最新一条结构化失败分类（write_back_board_dispatch 在
/// ⛔ 失败评论尾部落 `fail_class: <class>` 标记行，gateway.rs 唯一写入点）。
/// 只认已知类值（防 worker 文本/人工评论误触发未知值）；找不到 = None
///（旧 worker / 无失败评论路径）。扫描自最新评论起——重派轮次取最新失败。
pub(crate) fn latest_fail_class(comments: &[nemesis_board::Comment]) -> Option<&'static str> {
    // 与 classify_terminal_failure 的值集同步演化（该函数发出什么，这里才
    // 认什么；llm_rate_limit 2026-09-17 补认、token_budget 2026-10-10 P8、
    // ② AOR 批一b 2026-11-04 补 environmental/resource_conflict/
    // unknown_side_effect——环境特征细分 + worker 六词自报桥）。
    const KNOWN: [&str; 12] = [
        "validation_budget",
        "llm_timeout",
        "llm_failure",
        "llm_rate_limit",
        "empty_result",
        "escalation",
        "token_budget",
        // L13-b（2026-11-03 三期）：worker 侧 mandate 执行面守卫终局
        //（授权面/时限/预算违规——同目标重派授权状态不变，大概率复现）。
        "mandate_violation",
        "exec_failed",
        // ② AOR 批一b（2026-11-04）：环境/资源类（worker 六词自报 + exec
        // 兜底环境特征细分——换节点重试有意义）与副作用不明类（当前无 B 端
        // 自动生产者，识别即冻结不自动重派，防伪造面同步收口）。
        "environmental",
        "resource_conflict",
        "unknown_side_effect",
    ];
    comments.iter().rev().find_map(|c| {
        c.content.lines().find_map(|line| {
            let t = line.trim();
            let v = t.strip_prefix("fail_class: ")?;
            KNOWN.iter().find(|k| **k == v.trim()).copied()
        })
    })
}

/// 失败类别的转人工建议文案（与类值一一对应；未知类走兜底）。② 批一b
/// 起 caption 文案随 redispatch_policy 映射同步扩展（映射新增闸类别 =
/// hint 必有对应条目）。
pub(super) fn capability_fail_hint(fc: &str) -> &'static str {
    match fc {
        "validation_budget" => {
            "可选处置：\n- 在 Dashboard「模型」页对 worker 的模型跑 model probe 校准能力档位，或 `model set-tier` 调高档位\n- 人工改派其它节点（不同模型可能胜任）\n- 若确认是偶发，可人工直接重派原节点"
        }
        "escalation" => {
            "可选处置：\n- 改写任务描述或拆小粒度后人工重派（换一种思路打破循环）\n- 人工改派其它节点尝试"
        }
        "mandate_violation" => {
            "可选处置：\n- 换节点重派（master 将重签新 mandate，重新授权）\n- 人工检查授权模板/时限/预算设置，或检查节点用户权限"
        }
        "exec_failed" => {
            "可选处置：\n- 查看失败评论中的现象与失败类别，人工判断环境/任务形态问题\n- 换节点重派；确认是任务形态问题则改写任务描述后重派"
        }
        "environmental" => {
            "可选处置：\n- 换节点重派（目标节点可能具备缺失依赖）\n- 人工在本节点装配缺失环境（worker 契约已授权便携装配，装配事实应已留证于失败评论）"
        }
        "resource_conflict" => {
            "可选处置：\n- 换节点或错峰重派（避开资源冲突窗口）\n- 人工检查 [TOUCH] 声明与并行任务是否冲突，必要时调整派发时序"
        }
        "unknown_side_effect" => {
            "可选处置：\n- 人工检查失败报告中的副作用描述，确认影响范围\n- 处置干净（清理/回滚）后人工重派；影响不明时不要盲目重派"
        }
        _ => "请人工检查 worker 状态后裁决（重派 / 换节点 / 调整模型）。",
    }
}

/// 评审上下文（P4）：一段评审 fresh（`allow_selfcheck=true`，配置闸在
/// review_issue 内现读）；B2b 二段评审带 evidence 且 `allow_selfcheck=false`
/// ——取证只带一程，二段结论直接处置（防取证-评审乒乓死循环）。
/// N2（2026-10-12）：`retry` = 已消耗的评审自身失败自动重评次数
///（failure_retries 预算记账，随重评链传递）。
#[derive(Debug, Clone)]
pub(super) struct ReviewCtx {
    pub(super) allow_selfcheck: bool,
    pub(super) evidence: Option<String>,
    pub(super) retry: u32,
}

impl ReviewCtx {
    pub(super) fn first_stage() -> Self {
        Self {
            allow_selfcheck: true,
            evidence: None,
            retry: 0,
        }
    }
}

/// 触发验收（fire-and-forget；调用方在 write_back in_review 路径调用）。
/// 评审/收口链后台任务的统一观察壳（panic 留痕闭环）：决策链的
/// fire-and-forget spawn 共用。任务 panic（JoinError）不再无声——ERROR
/// 日志 + issue 系统评论留痕（`issue_id=None` 的项目级任务只记日志——
/// 项目没有单一 issue 可挂）；任务被取消（关机/estop）= 正常流不留痕。
/// 2026-10-11 事故固化：planner 截断 panic 杀死规划任务、看板零留痕，
/// 用户体感「建了不动」——无人值守链路的后台任务死亡必须看板可见
/// （日志只有翻日志的人看得到）。
/// 后台任务 panic 看护（评审/重规划等 fire-and-forget 任务共用）：任务
/// panic → 日志 + issue 评论显形；取消（关机/estop）= 正常流静默。
/// pub(crate)：board_replan.rs 重规划起草共用（AOR 批三 5.3）。
pub(crate) fn spawn_watched<F>(
    store: Arc<nemesis_board::BoardStore>,
    cluster: Arc<nemesis_cluster::cluster::Cluster>,
    issue_id: Option<i64>,
    what: &'static str,
    fut: F,
) where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = tokio::spawn(fut);
    tokio::spawn(async move {
        if let Err(je) = handle.await {
            if !je.is_panic() {
                return; // 任务被取消（关机/estop）= 正常流
            }
            let msg = nemesis_utils::string_utils::panic_message(je.into_panic());
            let short: String = msg.chars().take(300).collect();
            error!("[BoardReview] {what}后台任务 panic：{short}");
            if let Some(issue_id) = issue_id {
                let _ = post_review_comment(
                    &store,
                    issue_id,
                    &cluster,
                    &format!(
                        "❌ {what}后台任务异常终止（内部错误 panic）：{short}——请人工检查或重试"
                    ),
                );
            }
        }
    });
}

pub(crate) fn spawn_board_review(deps: BoardReviewDeps, issue_id: i64) {
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    spawn_watched(store, cluster, Some(issue_id), "评审", async move {
        match review_issue(&deps, issue_id, ReviewCtx::first_stage()).await {
            Ok(outcome) if outcome => {}
            Ok(_) => {} // 跳过类结果已在 review_issue 内记日志
            Err(e) => warn!("[BoardReview] issue {issue_id} 评审失败：{e}（人工兜底）"),
        }
    });
}

/// N2（2026-10-12）：评审自身失败自动重评的延迟（秒）。给评审侧基础设施
/// 故障（LLM 限流/端点抖动）一个自愈窗口——立即重跑大概率撞同一个故障。
pub(super) const REVIEW_RETRY_DELAY_SECS: u64 = 60;

/// N2：延迟重评（fire-and-forget）。睡眠后重入 review_issue 同一入口——
/// estop 挂起闸、in_review 状态闸（人工已改状态让位）、S-O2 迟到守卫全部
/// 天然生效；ctx.retry 记账让重评再失败时预算判定继续累积。
pub(super) fn spawn_review_retry(deps: BoardReviewDeps, issue_id: i64, attempt: u32) {
    let store = deps.store.clone();
    let cluster = deps.cluster.clone();
    spawn_watched(store, cluster, Some(issue_id), "评审重试", async move {
        tokio::time::sleep(std::time::Duration::from_secs(REVIEW_RETRY_DELAY_SECS)).await;
        let ctx = ReviewCtx {
            allow_selfcheck: true,
            evidence: None,
            retry: attempt,
        };
        match review_issue(&deps, issue_id, ctx).await {
            Ok(_) => {} // 跳过/闭环结果已在 review_issue 内记日志
            Err(e) => warn!("[BoardReview] issue {issue_id} 评审重试失败：{e}（人工兜底）"),
        }
    });
}

/// U-A（2026-10-12 停车现场工程 §9.1）：停车现场落盘（机器可读侧车
/// `<workspace>/board/handoffs/<NB-x>.json`）+ 返回人可读评论的档案注记
/// 尾巴。verdict_chain 从 U-C timeline 聚合提取（每轮决策全程链，单一
/// 真相源）。写失败只 WARN + 返回空串——停车评论照发（停车优先，档案
/// 缺席诚实）；全部停车分支收口本函数（单一真相源）。
pub(crate) fn park_with_handoff(
    deps: &BoardReviewDeps,
    issue_number: &str,
    issue_id: i64,
    spec: nemesis_board::handoff::HandoffSpec,
) -> String {
    let chain = nemesis_board::handoff::verdict_chain_from(&deps.store, issue_id);
    match nemesis_board::handoff::write_handoff(
        &deps.workspace,
        issue_number,
        issue_id,
        &spec,
        chain,
    ) {
        Ok(path) => format!(
            "\n\n📋 停车现场已存档：{}（人工续做：对本节点说「继续 {issue_number}」；\
             自动化续做：WSAPI `board.resume {{\"number\": \"{issue_number}\"}}`——\
             独立 ops 会话读档执行，不落用户聊天，结论回贴本单）",
            path.display()
        ),
        Err(e) => {
            warn!("[BoardReview] {issue_number} 停车档案写失败（评论照发）：{e}");
            String::new()
        }
    }
}

/// 环境快照默认字段（最后派发 worker + 状态）——handoff `environment`
/// 的公共底座；调用方可按需补节点/模型细节。
pub(super) fn last_dispatch_env(
    dispatches: &[nemesis_board::models::DispatchRecord],
) -> serde_json::Value {
    match dispatches.last() {
        Some(d) => serde_json::json!({
            "last_worker": d.worker_id,
            "last_dispatch_state": d.state,
        }),
        None => serde_json::json!({ "last_worker": null }),
    }
}
