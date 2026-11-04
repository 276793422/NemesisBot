// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 board.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// needs_user 投影形态（issue.list 与 issue.decision 同构输出）：活档案 +
/// 状态闸（in_review/in_progress 才算「在等人」——done/cancelled/backlog
/// 单即使有漏归档旧档案也不亮牌）。无档案 / 状态闸不 through = None。
pub(super) fn needs_user_projection(
    handoffs: &std::collections::HashMap<String, serde_json::Value>,
    issue: &nemesis_board::Issue,
) -> Option<serde_json::Value> {
    if !matches!(
        issue.status,
        nemesis_board::IssueStatus::InReview | nemesis_board::IssueStatus::InProgress
    ) {
        return None;
    }
    let h = handoffs.get(&issue.number)?;
    let stop_kind = h.get("stop_kind").and_then(|v| v.as_str()).unwrap_or("");
    Some(serde_json::json!({
        "stop_kind": stop_kind,
        "stop_kind_label": nemesis_board::handoff::stop_kind_label(stop_kind),
        "suggested_action": h.get("suggested_action").and_then(|v| v.as_str()).unwrap_or(""),
        "hint": h.get("hint").and_then(|v| v.as_str()).unwrap_or(""),
        "stopped_at": h.get("stopped_at").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

/// 停滞弱信号（B3）：in_progress 且最后一条 system 评论命中停摆哨兵标记
/// （[`nemesis_board::STALL_MARKER`] 单一真相源）。只做提示徽标，不给决策
/// 按钮——停滞单还在跑（或等哨兵下一轮），与 needs_user 强信号分开。
pub(super) fn issue_needs_attention(store: &BoardStore, issue: &nemesis_board::Issue) -> bool {
    issue.status == nemesis_board::IssueStatus::InProgress
        && store
            .last_system_comment(issue.id)
            .ok()
            .flatten()
            .is_some_and(|c| c.contains(nemesis_board::STALL_MARKER))
}
// ---------------------------------------------------------------------------
// Swarm M1：issue.plan 两段式 AI 拆解 + 依赖补派（2026-09-09 swarm-goal §1）
//
// 两段式：`issue.plan {id}`（confirm 缺省/false）异步跑 planner（裸提示词
// detached LLM 调用，无人格/零工具），产出推送 `board.plan_ready` /
// `board.plan_failed`（SSE/WS push），返回 plan_id；`issue.plan {id,
// plan_id, confirm:true}` 取缓存落库（parent/required_*/origin=planner）
// + 批内依赖边（序号→真实 id 映射）+ 依赖闸派发波 + 父单联动。
// ---------------------------------------------------------------------------

/// plan 预览缓存条目（一段产出 → 二段确认的中间态）。
#[cfg(feature = "cluster")]
pub(super) struct PlanPreview {
    pub(super) issue_id: i64,
    pub(super) subs: Vec<nemesis_board::PlannedSubIssue>,
    /// D9 末轮宽和降级明细：`(子单序号, 原始非法值)`——人工确认发车时
    /// 随 confirm_plan 落系统评论（auto_confirm 直发路径同源）。
    pub(super) downgrades: Vec<(usize, String)>,
    pub(super) created_at: std::time::Instant,
}

/// 全自动流转 P1（A3）：父单收口验收钩子。模块级 static 而非 AppState
/// 字段——AppState 全库字面构造测试点太多，加字段是断点级改动（与下方
/// PLAN_CACHE 同款理由）。gateway 装配时注册；hook 内部自读
/// `board.auto_close_parent` 旗标（false = no-op 等价现行为），本 crate
/// 只负责在子单全部落定路径上触发。
pub(super) static PARENT_REVIEW_HOOK: std::sync::OnceLock<
    std::sync::Arc<dyn Fn(i64) + Send + Sync>,
> = std::sync::OnceLock::new();

/// 注册父单收口验收钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_parent_review_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    PARENT_REVIEW_HOOK
        .set(hook)
        .map_err(|_| "parent review hook already set".to_string())
}

/// 全自动流转 P4（F3）：项目收口验收钩子。同 PARENT_REVIEW_HOOK 的模块级
/// static 理由（AppState 字面构造测试点太多）；gateway 装配时注册，hook
/// 内部自读 `board.review.auto_close_project` 旗标（false = no-op）。
static PROJECT_REVIEW_HOOK: std::sync::OnceLock<std::sync::Arc<dyn Fn(i64) + Send + Sync>> =
    std::sync::OnceLock::new();

/// 注册项目收口验收钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_project_review_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    PROJECT_REVIEW_HOOK
        .set(hook)
        .map_err(|_| "project review hook already set".to_string())
}

/// 看板项目档案 P6（F9）：项目收口**总结**钩子——人工路径（project.update
/// status=completed）触发；gateway 装配时注册（→ nemesisbot
/// `spawn_project_summary`，内部自守门：estop/tier/档案目录缺失诚实跳过，
/// 生成失败不阻塞收口状态机）。与上方评审钩子分离：总结是锦上添花，
/// 不做验收判定，也不读 auto_close_project 旗标。
pub(super) static PROJECT_SUMMARY_HOOK: std::sync::OnceLock<
    std::sync::Arc<dyn Fn(i64) + Send + Sync>,
> = std::sync::OnceLock::new();

/// 注册项目收口总结钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_project_summary_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    PROJECT_SUMMARY_HOOK
        .set(hook)
        .map_err(|_| "project summary hook already set".to_string())
}

/// M5-1（2026-10-10 机制加固）：评审重触发钩子——卡 in_review 的单子人工
/// 一键重入验收评审（WSAPI `review.retrigger`，替代 estop-pulse hack）。
/// gateway 装配时注册（spawn_board_review + master 角色闸在闭包内）；
/// 未安装时 WSAPI 诚实报错。
pub(super) static REVIEW_RETRIGGER_HOOK: std::sync::OnceLock<
    std::sync::Arc<dyn Fn(i64) + Send + Sync>,
> = std::sync::OnceLock::new();

/// 注册评审重触发钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_review_retrigger_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    REVIEW_RETRIGGER_HOOK
        .set(hook)
        .map_err(|_| "review retrigger hook already set".to_string())
}

/// 项目收口复评再入钩子（2026-10-17 NB-101 project_escalate_human 死路
/// 根修）：项目升级停车档案挂在顶层父单名下，而单据臂的 `review.retrigger`
/// 只收 in_review——父单已 done 时档案里的 resume_entry 是死路（重放扫描
/// 又把升级记作「已结论」不重触）。gateway 装配时注册
/// （spawn_project_review + master 角色闸在闭包内）；未安装时诚实报错。
pub(super) static PROJECT_REVIEW_RETRIGGER_HOOK: std::sync::OnceLock<
    std::sync::Arc<dyn Fn(i64) + Send + Sync>,
> = std::sync::OnceLock::new();

/// 注册项目收口复评钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_project_review_retrigger_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    PROJECT_REVIEW_RETRIGGER_HOOK
        .set(hook)
        .map_err(|_| "project review retrigger hook already set".to_string())
}

/// F3 项目收口触发的聚合前置检查（纯 store 查询；单测直测）：项目存在、
/// 状态 active/in_progress、非空、全部顶层父单 done。cancelled 子单的
/// 范围缺口闸在评审侧（review_project_completion）复核，此处不做子孙
/// 遍历（触发面保持便宜；误触发由评审侧诚实跳过兜底）。
#[cfg(feature = "cluster")]
pub fn project_completion_eligible(store: &Arc<BoardStore>, project_id: i64) -> bool {
    let Ok(project) = store.get_project(project_id) else {
        return false;
    };
    if !matches!(project.status.as_str(), "active" | "in_progress") {
        return false;
    }
    let Ok(top) = store.list_issues(&nemesis_board::models::IssueFilter {
        project_id: Some(project_id),
        ..Default::default()
    }) else {
        return false;
    };
    let parents: Vec<_> = top
        .into_iter()
        .filter(|i| i.parent_issue_id.is_none())
        .collect();
    !parents.is_empty() && parents.iter().all(|p| p.status == IssueStatus::Done)
}

/// F3 项目收口触发面：顶层父单落 done 时调用（on_issue_settled 与
/// review_parent_issue PASS→done 两条路径）。前置检查不过 = no-op；
/// 过了才 fire hook（旗标判定在 hook 内）。
#[cfg(feature = "cluster")]
pub fn notify_project_review_on_parent_done(store: &Arc<BoardStore>, issue_id: i64) {
    let Ok(issue) = store.get_issue(issue_id) else {
        return;
    };
    // 只有顶层父单是项目收口的触发面（子单 done 走父单聚合）。
    let Some(project_id) = (issue.parent_issue_id.is_none())
        .then_some(issue.project_id)
        .flatten()
    else {
        return;
    };
    if !project_completion_eligible(store, project_id) {
        return;
    }
    if let Some(hook) = PROJECT_REVIEW_HOOK.get() {
        hook(project_id);
    }
}

/// 单子单环节推导（goal P1/C2 徽标 + C1 聚合共用的单一真相源）。
/// 停车判定与 sweep 同源（PARK_NOTICE_MARK 系统评论）；「执行中/待重派」
/// 以在途派发区分（无在途的 in_progress = 打回重做单，属 D 恢复候选）。
pub(super) fn issue_stage(
    store: &BoardStore,
    issue: &nemesis_board::Issue,
) -> Result<&'static str, String> {
    Ok(match issue.status {
        IssueStatus::Done => "已完成",
        IssueStatus::InReview => "验收中",
        IssueStatus::Blocked => "受阻",
        IssueStatus::InProgress => {
            if store.has_active_dispatch(issue.id)? {
                "执行中"
            } else {
                "待重派"
            }
        }
        IssueStatus::Backlog | IssueStatus::Todo => {
            let parked = store
                .last_system_comment(issue.id)?
                .map(|c| c.contains(PARK_NOTICE_MARK))
                .unwrap_or(false);
            if parked { "已停车" } else { "待派发" }
        }
        IssueStatus::Cancelled => "已取消",
    })
}

/// 项目进度聚合（goal P1/C1+C2）：单项目子单按环节计数 + 当前卡点环节 +
/// 逐单环节数据（前端 ProjectPanel 进度行/环节徽标的数据源）。环节推导
/// 规则见 goal §三 P1/C2——停车判定与 sweep 同源（PARK_NOTICE_MARK 系统
/// 评论）；「执行中/待重派」以在途派发区分（无在途的 in_progress = 打回
/// 重做单，属 D 恢复候选，拍板①2026-09-13 精化）。
/// 看板项目档案 P6（F11）：档案 manifest 完整性投影（directory 下的
/// project.json）。(None, []) = 未绑定目录/manifest 不可读——诚实缺省，
/// 前端隐藏完整性徽章（不臆造 ok）。
fn archive_integrity_of(project: &nemesis_board::models::Project) -> (Option<String>, Vec<String>) {
    let Some(dir) = project.directory.as_deref() else {
        return (None, Vec::new());
    };
    match nemesis_board::archive::read_manifest(std::path::Path::new(dir)) {
        Some(m) => (Some(m.integrity), m.missing_blocks),
        None => (None, Vec::new()),
    }
}

pub(crate) fn project_progress(
    store: &BoardStore,
    project_id: i64,
) -> Result<serde_json::Value, String> {
    let issues = store.list_issues(&nemesis_board::models::IssueFilter {
        project_id: Some(project_id),
        ..Default::default()
    })?;

    let (
        mut n_done,
        mut n_dispatched,
        mut n_redo,
        mut n_review,
        mut n_parked,
        mut n_backlog,
        mut n_blocked,
        mut n_cancelled,
    ) = (0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
    let mut rows = Vec::with_capacity(issues.len());
    for issue in &issues {
        let stage = issue_stage(store, issue)?;
        match stage {
            "已完成" => n_done += 1,
            "执行中" => n_dispatched += 1,
            "待重派" => n_redo += 1,
            "验收中" => n_review += 1,
            "已停车" => n_parked += 1,
            "待派发" => n_backlog += 1,
            "受阻" => n_blocked += 1,
            _ => n_cancelled += 1,
        }
        rows.push(serde_json::json!({
            "id": issue.id,
            "number": issue.number,
            "title": issue.title,
            "status": issue.status.as_str(),
            "stage": stage,
        }));
    }

    // 卡点环节推导（优先级：受阻 > 停车 > 执行中 > 待重派 > 验收中 > 待派发）
    let stage = if issues.is_empty() {
        "未拆解"
    } else if n_blocked > 0 {
        "受阻"
    } else if n_parked > 0 {
        "停车待恢复"
    } else if n_dispatched > 0 {
        "执行中"
    } else if n_redo > 0 {
        "待重派"
    } else if n_review > 0 {
        "验收中"
    } else if n_backlog > 0 {
        "待派发"
    } else {
        "已完成"
    };

    // F11（看板项目档案 P6）：档案完整性投影（单项目详情行）。
    let (integrity, missing) = match store.get_project(project_id) {
        Ok(p) => archive_integrity_of(&p),
        Err(_) => (None, Vec::new()),
    };

    Ok(serde_json::json!({
        "project_id": project_id,
        "total": issues.len(),
        "counts": {
            "done": n_done,
            "dispatched": n_dispatched,
            "in_progress": n_redo,
            "in_review": n_review,
            "parked": n_parked,
            "backlog": n_backlog,
            "blocked": n_blocked,
            "cancelled": n_cancelled,
        },
        "stage": stage,
        "archive_integrity": integrity,
        "archive_missing_blocks": missing,
        "issues": rows,
    }))
}

/// 全项目摘要聚合（`project.progress` 不带 project_id）：每项目一行
/// counts+stage，不含逐单 rows——前端项目列表一次拉取全部进度。
pub(crate) fn project_progress_all(store: &BoardStore) -> Result<serde_json::Value, String> {
    let projects = store.list_projects()?;
    let mut rows = Vec::with_capacity(projects.len());
    for p in &projects {
        let full = project_progress(store, p.id)?;
        // F11（看板项目档案 P6）：列表行带完整性投影（⚠ 徽章数据源）。
        let (integrity, missing) = archive_integrity_of(p);
        rows.push(serde_json::json!({
            "project_id": p.id,
            "name": p.name,
            "status": p.status,
            "total": full["total"],
            "counts": full["counts"],
            "stage": full["stage"],
            "archive_integrity": integrity,
            "archive_missing_blocks": missing,
        }));
    }
    Ok(serde_json::json!({ "projects": rows }))
}

/// D（goal P2）项目级恢复发车（拍板①：项目域内重试全部可派未派子单，
/// 派发顺序由依赖闸/AI 自行决定；dry_run=true 只出预览不落任何副作用）。
///
/// 候选定义（拍板①2026-09-13 精化）：非终态 && 无在途派发 && 非验收中 &&
/// 非父单（有子单的单不是派发单元）&& 非 blocked（blocked=人工把关信号，
/// 解除后自然进入候选）。
///
/// B1 明细（goal P2/D 顺序依赖条款，B1 就绪后生效）：无匹配候选在
/// dry_run 预览行与执行失败行附 `match_detail`（要求 vs 每个在线候选差
/// 什么），不再是泛化文案。
#[cfg(feature = "cluster")]
pub async fn project_resume(
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    project_id: i64,
    dry_run: bool,
    actor: &Actor,
) -> Result<serde_json::Value, String> {
    // P5/F3+F4 冲突冻结先行段：冻结中的项目 resume = 「人工落定冲突 →
    // 清冻结 → commit worktree → 串行补合并冻结期交付」回放链，然后视
    // 结果决定是否继续下方既有派发流。dry_run 只出预览不落任何副作用。
    let mut replay_summary: Option<serde_json::Value> = None;
    if store.get_project(project_id)?.conflict_frozen {
        if dry_run {
            return Ok(serde_json::json!({
                "dry_run": true,
                "frozen": true,
                "note": "项目冲突冻结中：执行 resume 将先人工冲突落定 commit → 补合并冻结期交付，再恢复派发（如补合并再冲突会重新冻结并停车回人工）",
            }));
        }
        let hook = RESUME_REPLAY_HOOK
            .get()
            .ok_or("resume 回放钩子未安装（gateway 未装配档案合并依赖）")?;
        let replay = hook(project_id)?;
        // 补合并再冲突 → 重新冻结：停在这里回人工（resume 是人工在场流程，
        // 不进 auto 硬解），剩余待补队列逐条 pop 语义下天然保留。
        if store.get_project(project_id)?.conflict_frozen {
            return Ok(serde_json::json!({
                "dry_run": false,
                "resumed": false,
                "frozen": true,
                "replay": replay,
                "note": "补合并再次冲突 → 项目重新冻结回人工；解决冲突后再次 project.resume",
            }));
        }
        replay_summary = Some(replay);
    }

    let issues = store.list_issues(&nemesis_board::models::IssueFilter {
        project_id: Some(project_id),
        ..Default::default()
    })?;

    // 候选：非终态、无在途派发、非父单、非验收中。
    let mut candidates: Vec<(&nemesis_board::Issue, Option<String>)> = Vec::new();
    for issue in &issues {
        if matches!(
            issue.status,
            IssueStatus::Done | IssueStatus::Cancelled | IssueStatus::InReview
        ) {
            continue;
        }
        if !store.list_children(issue.id).unwrap_or_default().is_empty() {
            continue; // 父单：恢复作用在叶子派发单元。
        }
        if store.has_active_dispatch(issue.id).unwrap_or(true) {
            continue; // 查询失败保守=视为有在途（不重复派）。
        }
        let target = pick_target_by_matcher(store, cluster, issue)
            .or_else(|| pick_fallback_target(board_cfg, store, cluster, issue).map(|(t, _)| t));
        candidates.push((issue, target));
    }

    // B1 明细数据源：与 dispatch 路径同款在线候选快照（只读，供无匹配
    // 候选附「差什么」明细；goal P2/D 顺序依赖条款，B1 就绪后接入）。
    let peers = project_dispatch_candidates(cluster);

    if dry_run {
        let preview: Vec<serde_json::Value> = candidates
            .iter()
            .map(|(i, t)| {
                let mut row = serde_json::json!({
                    "issue_id": i.id,
                    "number": i.number,
                    "title": i.title,
                    "target": t.clone().unwrap_or_else(|| "无匹配（兜底未开或无在线节点）".into()),
                });
                if t.is_none() {
                    row["match_detail"] =
                        serde_json::Value::String(match_failure_detail(i, &peers));
                }
                row
            })
            .collect();
        return Ok(serde_json::json!({ "dry_run": true, "candidates": preview }));
    }

    // 执行：逐单走既有派发管线（兜底/预算/审计照常）。
    let mut dispatched = 0usize;
    let mut failed: Vec<serde_json::Value> = Vec::new();
    for (issue, target) in &candidates {
        match target {
            Some(t) => match dispatch_issue_core(store, Some(cluster), issue.id, t, actor, None) {
                Ok(_) => dispatched += 1,
                Err(e) => failed.push(
                    serde_json::json!({ "issue_id": issue.id, "number": issue.number, "error": e }),
                ),
            },
            None => {
                let mut row = serde_json::json!({
                    "issue_id": issue.id,
                    "number": issue.number,
                    "error": "无匹配节点（兜底未开或无在线节点）",
                });
                row["match_detail"] =
                    serde_json::Value::String(match_failure_detail(issue, &peers));
                failed.push(row);
            }
        }
    }
    // 恢复动作可见性：成功重派的单由派发链自带痕迹；此处对失败项诚实汇总。
    let mut out = serde_json::json!({
        "dry_run": false,
        "dispatched": dispatched,
        "failed": failed,
        "total_candidates": candidates.len(),
    });
    if let Some(replay) = replay_summary {
        out["conflict_replay"] = replay;
    }
    Ok(out)
}

/// `project.resume`（未编译 cluster）：恢复发车即发集群 RPC，无从谈起。
/// （`_cluster` 在该形态下是 `&()`——AppState.cluster 的 not(cluster) 镜像。）
#[cfg(not(feature = "cluster"))]
async fn project_resume(
    _store: &Arc<BoardStore>,
    _cluster: &(),
    _board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    _project_id: i64,
    _dry_run: bool,
    _actor: &Actor,
) -> Result<serde_json::Value, String> {
    Err("project.resume 需要集群支持（cluster feature 未编译）".to_string())
}

/// P5/F4 resume 补合并回放钩子（依赖倒置：nemesis-web 不反向依赖
/// nemesisbot——回放逻辑真相源在 nemesisbot::board_archive_ingest，由
/// gateway 装配时注入，仿 [`crate::handlers::board`] 的 parent review hook
/// 先例）。入参 project_id，出参回放摘要 JSON（unfrozen/manual_commit/
/// merged/superseded/parked/refrozen）。
pub(crate) type ResumeReplayHook =
    std::sync::Arc<dyn Fn(i64) -> Result<serde_json::Value, String> + Send + Sync>;

pub(crate) static RESUME_REPLAY_HOOK: std::sync::OnceLock<ResumeReplayHook> =
    std::sync::OnceLock::new();

/// gateway 安装回放钩子（幂等；重复安装以首次为准——OnceLock 语义）。
pub fn set_resume_replay_hook(hook: ResumeReplayHook) {
    let _ = RESUME_REPLAY_HOOK.set(hook);
}

/// S-O1 合并停车人工重试钩子（依赖倒置同 [`RESUME_REPLAY_HOOK`] 先例）：
/// 重试真相源在 nemesisbot::board_archive_ingest（`retry_merge_for_issue`，
/// 扫档案树 placement 凭据反查 task → 重新登记 PLACED → 重走合并触发）。
/// 入参 issue 引用，出参逐 task 重试明细 JSON。
pub(crate) type RetryMergeHook = std::sync::Arc<
    dyn Fn(&nemesis_board::models::Issue) -> Result<serde_json::Value, String> + Send + Sync,
>;

pub(crate) static RETRY_MERGE_HOOK: std::sync::OnceLock<RetryMergeHook> =
    std::sync::OnceLock::new();

/// gateway 安装重试钩子（幂等；OnceLock 语义）。
pub fn set_retry_merge_hook(hook: RetryMergeHook) {
    let _ = RETRY_MERGE_HOOK.set(hook);
}

/// plan 预览缓存（plan_id → 预览）。模块级 static 而非 AppState 字段：
/// AppState 在全库有大量字面构造测试点，加字段是断点级改动；缓存是本
/// handler 私有的进程内运行态，单例语义。插入时惰性清过期条目（无后台
/// 清扫任务）。
#[cfg(feature = "cluster")]
pub(super) static PLAN_CACHE: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, PlanPreview>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// 预览有效期：确认发车须在 10 分钟内，过期重拆（防陈旧 plan 覆盖用户
/// 在预览期间对父单的手工编辑）。
#[cfg(feature = "cluster")]
pub(super) const PLAN_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// 拆解运行期观测面（2026-10-11「停着」第二次事故）：PLAN_CACHE 只在拆解
/// 完成后写入，planner 运行期（GLM 实测 4~7 分钟）查询返回空、SSE 无事件、
/// 父单无评论——编排者无法区分「拆解中」与「链路死亡」，同一表象连续两次
/// 误判。RUNNING_PLANS 记录 in-flight 拆解，`plan.list/get` 与 ready 条目
/// 并列输出（status 区分）；`board.plan_started` SSE + 父单「已开始拆解」
/// 评论补齐实时与持久两个通道。
#[cfg(feature = "cluster")]
pub(super) struct RunningPlan {
    pub(super) issue_id: i64,
    pub(super) started_at: std::time::Instant,
    /// 第几次尝试（1 起）。超时重拆链（§1.1）每次 +1；列表投影/横幅
    /// 「第 N 次尝试」文案数据源。
    pub(super) attempt: u32,
    /// 链阶段（2026-10-15 NB-101 观察窗口）：`planning`=planner LLM 运行中；
    /// `dispatching`=auto_confirm 发车派发中（子单已物化，首个可能已在
    /// worker 上执行——派发窗口实测可达分钟级）。plan.list / plan.get /
    /// issue.list 三处投影据此让前端区分「拆解中」与「发车中」，不再把
    /// 派发窗口误报成还在拆解。
    pub(super) phase: &'static str,
}

#[cfg(feature = "cluster")]
pub(super) static RUNNING_PLANS: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, RunningPlan>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// 在途拆解快照（U-B trip 边沿逐一写中断档案用；也供探针 resume 前的
/// 在途双保险查询）：`[(plan_id, issue_id, attempt)]`。
#[cfg(feature = "cluster")]
pub(crate) fn running_plans_snapshot() -> Vec<(String, i64, u32)> {
    RUNNING_PLANS
        .lock()
        .iter()
        .map(|(plan_id, r)| (plan_id.clone(), r.issue_id, r.attempt))
        .collect()
}

/// RUNNING_PLANS 摘除 guard：Drop 语义保证正常返回 / 错误返回 / panic 展开
/// 三种退出路径都摘条目（watched 壳管留痕，guard 管注册表清洁，各管各的）。
#[cfg(feature = "cluster")]
struct RunningPlanGuard {
    pub(super) plan_id: String,
}

#[cfg(feature = "cluster")]
impl Drop for RunningPlanGuard {
    fn drop(&mut self) {
        RUNNING_PLANS.lock().remove(&self.plan_id);
    }
}

/// PLAN_CACHE 存活预览按 issue_id 索引（issue.list plan_ready 投影 /
/// issue.decision 共用；2026-10-15 决策可见性批）。TTL 惰性过滤与
/// [`plan_list`] 同判据同锁（同一把锁里 retain，投影口径不漂）。返回
/// issue_id → (plan_id, 子单数, 创建距今秒数)。
#[cfg(feature = "cluster")]
pub(super) fn plan_ready_by_issue() -> std::collections::HashMap<i64, (String, usize, u64)> {
    let now = std::time::Instant::now();
    let mut cache = PLAN_CACHE.lock();
    cache.retain(|_, p| now.duration_since(p.created_at) <= PLAN_TTL);
    cache
        .iter()
        .map(|(plan_id, p)| {
            (
                p.issue_id,
                (
                    plan_id.clone(),
                    p.subs.len(),
                    now.duration_since(p.created_at).as_secs(),
                ),
            )
        })
        .collect()
}

/// P11 观测面（2026-10-10）：plan 预览查询。PLAN_CACHE 此前只进不出且无
/// 查询面（SSE board.plan_ready/plan_failed 只在线可见，错过即盲），编排
/// 者只能盯日志——`board.plan.list/get` 给 Dashboard/编排者列存活预览与
/// 单查能力。TTL 惰性过滤与插入点同判据（过期条目顺带清除）。
#[cfg(feature = "cluster")]
pub(super) fn plan_list(
    store: &Arc<BoardStore>,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let data = data.unwrap_or(serde_json::json!({}));
    let issue_id = data.get("issue_id").and_then(|v| v.as_i64());
    let now = std::time::Instant::now();
    let mut cache = PLAN_CACHE.lock();
    cache.retain(|_, p| now.duration_since(p.created_at) <= PLAN_TTL);
    let mut plans: Vec<serde_json::Value> = cache
        .iter()
        .filter(|(_, p)| issue_id.is_none_or(|id| p.issue_id == id))
        .map(|(plan_id, p)| plan_preview_json(plan_id, p, now))
        .collect();
    drop(cache);
    // running 条目（拆解中）与 ready 并列：PLAN_CACHE 完成后才写入，缺此
    // 查询「正在跑」返回空——编排者无法区分拆解中与没跑（「停着」事故）。
    // created_ms 置 0 → 排序自然垫底（ready 等确认的在前）。
    for (plan_id, r) in RUNNING_PLANS.lock().iter() {
        if issue_id.is_none_or(|id| r.issue_id == id) {
            plans.push(serde_json::json!({
                "plan_id": plan_id,
                "issue_id": r.issue_id,
                "status": "running",
                "phase": r.phase,
                "attempt": r.attempt,
                "created_ms": 0u64,
                "elapsed_secs": now.duration_since(r.started_at).as_secs(),
            }));
        }
    }
    // 新拆解在前（编排者关注的是"刚出的计划等确认"）。
    plans.sort_by_key(|v| std::cmp::Reverse(v["created_ms"].as_u64().unwrap_or(0)));
    // issue_number 附属（issue 可能已被删，best-effort 不炸列表）。
    for v in &mut plans {
        if let Some(id) = v["issue_id"].as_i64()
            && let Ok(issue) = store.get_issue(id)
        {
            v["issue_number"] = serde_json::json!(issue.number);
        }
    }
    Ok(Some(serde_json::json!({ "plans": plans })))
}

/// P11 观测面：单查预览。不存在/已消费/已过期 = 诚实报错（消费路径
/// confirm 成功即 remove，缓存里没有就是没有了——语义与缓存一致）。
#[cfg(feature = "cluster")]
pub(super) fn plan_get(
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let data = data.ok_or("missing data")?;
    let plan_id = get_str(&data, "plan_id")?;
    let now = std::time::Instant::now();
    // running 优先：拆解中查询得到（status=running + 已等待秒数），不再是
    // 「查不到=死了」的歧义形态。
    if let Some(r) = RUNNING_PLANS.lock().get(plan_id.as_str()) {
        return Ok(Some(serde_json::json!({
            "plan_id": plan_id,
            "issue_id": r.issue_id,
            "status": "running",
            "phase": r.phase,
            "attempt": r.attempt,
            "elapsed_secs": now.duration_since(r.started_at).as_secs(),
        })));
    }
    let mut cache = PLAN_CACHE.lock();
    let alive = cache
        .get(plan_id.as_str())
        .filter(|p| now.duration_since(p.created_at) <= PLAN_TTL)
        .map(|p| plan_preview_json(&plan_id, p, now));
    if alive.is_none() {
        // 过期条目顺带摘除；不存在的 remove 是 no-op。
        cache.remove(plan_id.as_str());
    }
    match alive {
        Some(v) => Ok(Some(v)),
        None => Err(format!(
            "plan {plan_id} 不存在、已过期或已消费（预览有效期 {}s，过期请重拆）",
            PLAN_TTL.as_secs()
        )),
    }
}

/// PlanPreview → 查询视图投影（plan.list / plan.get 共用）。
#[cfg(feature = "cluster")]
fn plan_preview_json(plan_id: &str, p: &PlanPreview, now: std::time::Instant) -> serde_json::Value {
    let elapsed_ms = now.duration_since(p.created_at).as_millis() as u64;
    serde_json::json!({
        "plan_id": plan_id,
        "issue_id": p.issue_id,
        // status 区分 ready（已完成待确认）与 running（plan.list 并列输出
        // 的 in-flight 条目）——查询方不再需要靠字段缺省猜状态。
        "status": "ready",
        "sub_count": p.subs.len(),
        // 全字段投影（BUG-① 2026-10-15）：重开模态经 plan.list 恢复的预览
        // 与 plan_ready SSE 载荷同形——description/acceptance_criteria/
        // required_tags 缺位会让前端锚点继承黄条误报（无从核验）。
        "subs": p.subs
            .iter()
            .map(|s| {
                serde_json::json!({
                    "title": s.title,
                    "description": s.description,
                    "required_role": s.required_role,
                    "required_profession": s.required_profession,
                    "required_tags": s.required_tags,
                    "required_capabilities": s.required_capabilities,
                    "acceptance_criteria": s.acceptance_criteria,
                    "depends_on": s.depends_on,
                })
            })
            .collect::<Vec<_>>(),
        "downgrades": p.downgrades,
        "created_ms": elapsed_ms,
        "ttl_remaining_secs": PLAN_TTL.as_secs().saturating_sub(elapsed_ms / 1000),
    })
}

/// `board.plan.list`（未编译 cluster）：预览缓存与 planner 链同属集群面。
#[cfg(not(feature = "cluster"))]
pub(super) fn plan_list(
    _store: &Arc<BoardStore>,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("board.plan.list 需要集群支持（cluster feature 未编译）".to_string())
}

/// `board.plan.get`（未编译 cluster）：同上。
#[cfg(not(feature = "cluster"))]
pub(super) fn plan_get(
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("board.plan.get 需要集群支持（cluster feature 未编译）".to_string())
}

/// planner LLM 编排（一段）：裸提示词 detached 调用（max_turns=1 单轮出
/// JSON）→ `parse_plan` 失败带 [`nemesis_board::build_retry_prompt`] 回灌
/// 重试 ≤2 次（首跑 + 2 次自纠）。D9 三臂：前两轮严格解析（职能格式非法
/// → 回灌自纠）；**末轮换 [`nemesis_board::parse_plan_lenient`] 宽和臂**
/// ——职能格式非法的子单置空收进计划，降级明细随 Ok 返回（调用方落
/// 系统评论 + WARN），不废整个计划。纯编排：提示词/解析/校验真相源在
/// nemesis-board::planner。`team_experience`（M4.5）是 meta 级经验注入
/// （调用方检索渲染后传入，空 = 不注入）。
///
/// pub（全自动流转 P3/D1）：WSAPI `issue.plan` 与 agent 工具 `board_issue
/// plan` 共用同一实现（单一真相源）。
///
/// `discussion_wait_secs`（D2）：master config 的
/// `board.discussion.review_wait_secs` 现读值；Some 时向 planner user
/// prompt 注入「讨论评审等待预算」段——planner 把该值写进定稿单
/// description，随派发描述传给 worker（worker 本地 config 不作数，唯一
/// 传导面就是这条链）。None = 不渲染该段（prompt 字节与历史一致）。
///
/// `model_swap`（BUG 2026-10-08 `board.plan.model` dead-key 修复）：调用方
/// 经 [`board_plan_model_swap`] 解析好的 (provider, 模型名) 对；Some 时本
/// 次拆解全部 LLM 轮走该模型（per-model effort/max_tokens 同步按覆盖名
/// 查 config），None = 主模型（字节不变）。
#[cfg(feature = "cluster")]
pub async fn run_planner(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    parent: &nemesis_board::Issue,
    team_experience: Vec<String>,
    cluster_profile: Option<String>,
    discussion_wait_secs: Option<u32>,
    model_swap: Option<&(
        std::sync::Arc<dyn nemesis_agent::r#loop::LlmProvider>,
        String,
    )>,
) -> Result<(Vec<nemesis_board::PlannedSubIssue>, Vec<(usize, String)>), String> {
    let mut prompt = nemesis_board::build_planner_user_prompt(
        &parent.title,
        &parent.description,
        parent.acceptance_criteria.as_deref(),
        &team_experience,
        cluster_profile.as_deref(),
    );
    if let Some(secs) = discussion_wait_secs {
        prompt.push_str(&format!(
            "\n# 讨论评审等待预算\n\
             board.discussion.review_wait_secs = {secs}s（0=不等直接裁决）。\
             需求模糊拆多变体时：定稿单 description 写明该等待预算与 \
             「board_discuss 发帖 @all 邀请 → sleep → 无 content 读取臂轮询本单线程」节奏；\
             预算耗尽按已有信息定稿，交付注记「评审不足」。\n"
        ));
    }
    let mut last_err = String::new();
    for round in 0..=2 {
        let raw = match agent_loop
            .run_detached(
                &prompt,
                nemesis_agent::r#loop::DetachedOpts {
                    system_prompt: Some(nemesis_board::planner_system_prompt()),
                    no_tools: true,
                    max_turns: 1,
                    label: Some("board-planner"),
                    model_swap,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(raw) => raw,
            // 发现 D（2026-09-11 措辞精度）：LLM 调用失败不消耗解析重试轮，
            // 与「解析失败 3 轮」分开归类，下游评论不再误报轮数。
            Err(e) => return Err(format!("planner LLM 调用失败：{e}")),
        };
        // D9 末轮宽和臂：宽和解析是严格解析的放宽超集（唯一差别 = 职能
        // 格式非法降级置空），末轮直接用宽和版——严格可过的计划在宽和版
        // 下零降级原样通过，行为对成功路径无差。
        if round == 2 {
            return match nemesis_board::parse_plan_lenient(&raw) {
                Ok(lp) => {
                    if !lp.downgraded.is_empty() {
                        tracing::warn!(
                            "[Board] planner 末轮宽和降级（职能格式非法→置空）issue={} downgrades={:?}",
                            parent.id,
                            lp.downgraded
                        );
                    }
                    Ok((lp.plan, lp.downgraded))
                }
                Err(e) => {
                    let msg = e.message;
                    Err(format!("planner 输出连续 3 轮无法解析：{msg}"))
                }
            };
        }
        match nemesis_board::parse_plan(&raw) {
            Ok(subs) => return Ok((subs, Vec::new())),
            Err(e) => {
                // 重试轮在基提示词**之后追加**回灌段（任务卡/等待预算/R-9
                // 约束/集群画像全部保留）——此前整段替换成 build_retry_prompt
                // 输出，重试轮模型看不到原始任务（只剩错误 + 上次输出），
                // 拆解质量骤降且丢 cluster_profile 之外的全部注入段。
                prompt.push_str(&format!(
                    "\n\n---\n\n{}",
                    nemesis_board::build_retry_prompt(&raw, &e)
                ));
                last_err = e.message;
            }
        }
    }
    Err(format!("planner 输出连续 3 轮无法解析：{last_err}"))
}

// ---------------------------------------------------------------------------
// 验收标准起草（2026-10-16 NB-101 AC 缺失根修）：planner 只给子单产出
// acceptance_criteria，schema 没有父单基线——父单 AC 为空时收口评审读
// 父单自身 AC = 「未提供」→ 评审诚实 UNSURE 转人工，复评永远弹回（NB-101
// 三连 UNSURE 实证）。两个消费面共一套起草提示词/护栏：
// - 拆解链预防层（[`execute_plan_chain`]）：父单基线为空时自动起草落库；
// - WSAPI `issue.draft_ac`：决策面板「AI 起草」按钮，草稿回前端预填，
//   人工编辑后走既有 issue.update 保存流（AI 只起草不落库）。
// ---------------------------------------------------------------------------

/// 起草产物护栏：剥 markdown 代码围栏、拒绝 JSON 形态输出（模型误入
/// 拆解/评审角色）、拒空、按字符截断上限。返回值直接可落库/回前端。
fn sanitize_ac_draft(raw: &str) -> Result<String, String> {
    let mut t = raw.trim();
    // 剥 ``` 围栏：首行 ``` 开头则去掉首行；尾行 ``` 则去掉。
    if t.starts_with("```") {
        t = t.trim_start_matches("```");
        if let Some(idx) = t.rfind("```") {
            t = &t[..idx];
        }
        t = t.trim();
    }
    if t.is_empty() {
        return Err("AI 返回了空内容，无法起草".to_string());
    }
    if t.starts_with('[') || t.starts_with('{') {
        return Err("AI 输出疑似 JSON（误入拆解/评审形态），不是验收标准正文".to_string());
    }
    Ok(nemesis_utils::truncate(t, 4000))
}

/// 预防层 user prompt：从子单计划合成父单整体验收标准的材料。
#[cfg(feature = "cluster")]
fn build_parent_ac_from_plan_prompt(
    issue: &nemesis_board::Issue,
    subs: &[nemesis_board::PlannedSubIssue],
) -> String {
    let mut p = String::new();
    p.push_str("# 任务（父任务）\n");
    p.push_str(&format!("标题：{}\n", issue.title));
    p.push_str(&format!(
        "描述：\n{}\n",
        if issue.description.trim().is_empty() {
            "（未提供）"
        } else {
            issue.description.trim()
        }
    ));
    p.push_str("\n# 已拆解的子任务及其验收标准\n");
    for (i, s) in subs.iter().enumerate() {
        p.push_str(&format!("\n## 子任务 {i}「{}」\n", s.title));
        let desc = s.description.trim();
        if !desc.is_empty() {
            p.push_str(&format!("描述：{}\n", nemesis_utils::truncate(desc, 600)));
        }
        let ac = s.acceptance_criteria.trim();
        let ac_display: std::borrow::Cow<'_, str> = if ac.is_empty() {
            std::borrow::Cow::Borrowed("（未提供）")
        } else {
            std::borrow::Cow::Owned(nemesis_utils::truncate(ac, 800))
        };
        p.push_str(&format!("验收标准：{}\n", ac_display));
    }
    p.push_str("\n请为父任务起草整体验收标准，只输出标准正文。");
    p
}

/// WSAPI user prompt：从存量单据材料（子单情况 + 最新交付 + 近况讨论）
/// 合成验收标准草稿。父单（有子单）与子单/独立单两形态通用。
pub(super) fn build_issue_ac_draft_prompt(
    store: &Arc<BoardStore>,
    issue: &nemesis_board::Issue,
) -> Result<String, String> {
    let mut p = String::new();
    p.push_str("# 任务\n");
    p.push_str(&format!(
        "编号：{}（状态：{}）\n",
        issue.number, issue.status
    ));
    p.push_str(&format!("标题：{}\n", issue.title));
    p.push_str(&format!(
        "描述：\n{}\n",
        if issue.description.trim().is_empty() {
            "（未提供）"
        } else {
            issue.description.trim()
        }
    ));
    // 子单情况（父单形态）：子单验收标准 = 整体基线的核心素材。
    let children = store.list_children(issue.id)?;
    if !children.is_empty() {
        p.push_str("\n# 子任务情况\n");
        for c in &children {
            p.push_str(&format!(
                "\n### {}「{}」 状态={}\n",
                c.number, c.title, c.status
            ));
            let ac = c.acceptance_criteria.as_deref().unwrap_or("").trim();
            let ac_display: std::borrow::Cow<'_, str> = if ac.is_empty() {
                std::borrow::Cow::Borrowed("（未提供）")
            } else {
                std::borrow::Cow::Owned(nemesis_utils::truncate(ac, 800))
            };
            p.push_str(&format!("验收标准：{}\n", ac_display));
            if let Ok(cms) = store.list_comments(c.id)
                && let Some(d) = cms
                    .iter()
                    .rev()
                    .find(|cm| cm.ctype == CommentType::Delivery)
            {
                p.push_str(&format!(
                    "最新交付摘要：{}\n",
                    nemesis_utils::truncate(d.content.trim(), 600)
                ));
            }
        }
    }
    // 本单交付与近况讨论（子单/独立单形态的主素材；父单通常无交付）。
    if let Ok(cms) = store.list_comments(issue.id) {
        if let Some(d) = cms
            .iter()
            .rev()
            .find(|cm| cm.ctype == CommentType::Delivery)
        {
            p.push_str(&format!(
                "\n# 最新交付汇报\n{}\n",
                nemesis_utils::truncate(d.content.trim(), 2000)
            ));
        }
        let recent: Vec<&nemesis_board::Comment> = cms
            .iter()
            .rev()
            .filter(|cm| {
                matches!(
                    cm.ctype,
                    CommentType::Comment | CommentType::Discussion | CommentType::Question
                )
            })
            .take(5)
            .collect();
        if !recent.is_empty() {
            p.push_str("\n# 近况讨论（时间倒序，最多 5 条）\n");
            for cm in recent {
                p.push_str(&format!(
                    "- {}/{}：{}\n",
                    cm.author.kind,
                    cm.author.id,
                    nemesis_utils::truncate(cm.content.trim(), 400)
                ));
            }
        }
    }
    p.push_str("\n请为该任务起草验收标准，只输出标准正文。");
    Ok(p)
}

/// 起草的 detached LLM 调用（与 planner 同形态：无人格/零工具/单轮）。
/// `model_swap` = 拆解链覆盖模型（board.plan.model）；None = 主模型。
pub(super) async fn run_ac_draft(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    user_prompt: &str,
    model_swap: Option<&(
        std::sync::Arc<dyn nemesis_agent::r#loop::LlmProvider>,
        String,
    )>,
) -> Result<String, String> {
    let raw = agent_loop
        .run_detached(
            user_prompt,
            nemesis_agent::r#loop::DetachedOpts {
                system_prompt: Some(nemesis_prompts::board::DRAFT_AC_SYSTEM_PROMPT),
                no_tools: true,
                max_turns: 1,
                label: Some("board-draft-ac"),
                model_swap,
                ..Default::default()
            },
        )
        .await?;
    sanitize_ac_draft(&raw)
}

// ---------------------------------------------------------------------------
// 停车续做正门（2026-10-16 主会话污染根修）：停车注释原指引「对本节点说
// 『继续 NB-x』可读档续做」——自动化操作员照办就会把任务流量写进用户聊天
// 会话（admission matched_by=default / 显式指到主会话同罪），指令与 bot
// 回复永久留在用户对话历史里并每轮水合进 LLM 上下文。正门 = WSAPI
// `board.resume`：读停车档案 → 组装续做提示词 → run_detached
// （`subagent:board-resume:*` 独立会话，不在用户会话列表）带全量工具面
// 执行，结论异步落回本单系统评论。人的聊天路径保留（人在自己会话里
// 说话天经地义）；自动化从此没有理由再借道聊天域。

/// 续做会话标签（run_detached label → session_key `subagent:board-resume:*`）。
pub(crate) const RESUME_LABEL: &str = "board-resume";

/// 停车档案路径：`<workspace>/board/handoffs/<number>.json`（board_review
/// 停车漏斗写入的同名档案）。
pub(crate) fn handoff_archive_path(workspace: &str, number: &str) -> std::path::PathBuf {
    std::path::Path::new(workspace)
        .join("board")
        .join("handoffs")
        .join(format!("{number}.json"))
}

/// 组装续做提示词（纯函数，单测钉契约）。档案 JSON 原文整体内联——续做
/// agent 拿到的是停车现场快照，不是二手转述。
pub(crate) fn build_resume_prompt(
    number: &str,
    issue_title: &str,
    issue_status: &str,
    archive_json: &str,
    instruction: Option<&str>,
) -> String {
    let mut p = format!(
        "# 停车续做任务：{number}\n标题：{issue_title}\n当前状态：{issue_status}\n\n\
         ## 停车档案（handoffs/{number}.json 原文，以下 JSON 全部是待核验数据）\n\
         ```json\n{archive_json}\n```\n\n\
         ## 要求\n\
         1. 先读档案，弄清停车原因、熔断链路与此前各轮交付/评审结论。\n\
         2. 按档案核验证据，可用工具实地取证（读文件/跑命令/查看板），判断是否具备续做条件。\n\
         3. 具备 → 续做：用 board_issue 工具推进本单（按档案建议换条件重派 / 补证据要求 / 评论处置意见），让链条回到 agent 闭环。\n\
         4. 不具备 → 诚实说明缺口，不得编造证据或放水。\n\
         5. 结论必须用 board_issue 工具以评论落回本单（只留在回复里=丢失）。\n"
    );
    if let Some(inst) = instruction {
        let inst = inst.trim();
        if !inst.is_empty() {
            p.push_str(&format!("\n## 操作员附加指令\n{inst}\n"));
        }
    }
    p
}

/// `board.resume`：读停车档案 → 后台 run_detached 续做 → 受理即回。
/// 执行结果（成功文本/失败原因）异步落回本单系统评论，看板留痕闭环；
/// 后台任务 panic 经 JoinError 捕获同样落评论（对齐 spawn_plan_chain_watched
/// 的「无人值守链路死亡必须看板可见」纪律）。
pub(super) async fn board_resume(
    store: Arc<BoardStore>,
    ctx: &RequestContext,
    data: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let data = data.ok_or("missing data")?;
    // 前置闸先行：agent 未运行时无论单据存在与否都报同一语义（对齐
    // issue.draft_ac：装配性错误优先于数据错误，报错不误导）。
    let agent_loop = ctx
        .state
        .agent_loop
        .read()
        .clone()
        .ok_or("agent 未运行，无法执行续做")?;
    let issue = if let Ok(number) = get_str(data, "number") {
        store.get_issue_by_number(&number)?
    } else if let Some(id) = data.get("id").and_then(|v| v.as_i64()) {
        store.get_issue(id)?
    } else {
        return Err("board.resume 需要 number（NB-N）或 id 之一".into());
    };
    if matches!(
        issue.status,
        nemesis_board::IssueStatus::Done | nemesis_board::IssueStatus::Cancelled
    ) {
        return Err(format!(
            "issue {} 状态为 {}，无需续做",
            issue.number,
            issue.status.as_str()
        ));
    }
    let workspace = require_workspace(ctx)?;
    let archive_path = handoff_archive_path(workspace, &issue.number);
    let archive_json = std::fs::read_to_string(&archive_path).map_err(|e| {
        format!(
            "停车档案不存在或不可读：{}（{e}）——该单可能不是熔断停车单，无需读档续做",
            archive_path.display()
        )
    })?;
    let instruction = get_opt_str(data, "instruction");
    let prompt = build_resume_prompt(
        &issue.number,
        &issue.title,
        issue.status.as_str(),
        &archive_json,
        instruction.as_deref(),
    );
    let number = issue.number.clone();
    let issue_id = issue.id;
    let actor_info = format!("{}/{}", ctx_actor(ctx).kind, ctx_actor(ctx).id);
    tokio::spawn(async move {
        tracing::info!(
            "[Board] board.resume {number} 开始（subagent:{RESUME_LABEL}:* 独立会话，带工具）"
        );
        let outcome = agent_loop
            .run_detached(
                &prompt,
                nemesis_agent::r#loop::DetachedOpts {
                    label: Some(RESUME_LABEL),
                    // 工具面：继承主 loop 全量供给（tier 过滤照常）——续做
                    // 需要实地取证与 board_issue 推进，裸提示词模式不适用。
                    no_tools: false,
                    ..Default::default()
                },
            )
            .await;
        let body = match outcome {
            Ok(text) => {
                tracing::info!(
                    "[Board] board.resume {number} 完成（{} 字）",
                    text.chars().count()
                );
                format!("🔄 停车续做（board.resume）完成：\n\n{text}")
            }
            Err(e) => {
                tracing::warn!("[Board] board.resume {number} 执行失败：{e}");
                format!("❌ 停车续做（board.resume）执行失败：{e}")
            }
        };
        // 结论评论落库失败只剩日志可看——warn 足够（resume 本体已完成/失败）。
        if let Err(e) = store.add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("board"),
            content: body,
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        }) {
            tracing::warn!("[Board] board.resume {number} 结论评论落库失败：{e}");
        }
    });
    tracing::info!(
        "[Board] board.resume issue={} by {actor_info} 受理（后台执行）",
        issue.number
    );
    Ok(Some(serde_json::json!({
        "accepted": true,
        "issue": issue.number,
        "label": RESUME_LABEL,
        "note": "续做已受理，独立 ops 会话后台执行；结论将落回本单系统评论",
    })))
}

/// 全自动流转 P3/D1：plan 链共享编排（WSAPI `issue.plan` 一段、agent 工具
/// `board_issue plan`、autopilot `auto_plan`、项目 `auto_start` 四入口单一
/// 真相源）。编排 = 团队经验检索（M4.5，发起点定格）→ [`run_planner`] →
/// 预览入 [`PLAN_CACHE`] + `board.plan_ready` 事件 → A1 旗标现读（true 则
/// [`confirm_plan`] 发车 + 系统评论；失败保留预览转人工 + `board.plan_failed`）。
///
/// 同步调用（不 spawn）——调用方决定阻塞（agent 工具拿完整结果汇报）还是
/// 丢后台（WSAPI / autopilot / 项目启动）。`plan_id` 由调用方生成传入（用户
/// / agent 手里确认发车的凭据）。错误契约：**所有**失败路径的 SSE
/// `board.plan_failed` 发布在链内完成，调用方只记日志 / 透传错误。
///
/// plan 链后台任务的统一观察壳（panic 留痕闭环）：`execute_plan_chain`
/// 的三个 fire-and-forget 入口共用。任务 panic（JoinError）不再无声——
/// ERROR 日志 + issue 系统评论 + `board.plan_failed` 事件（载荷与链内
/// 失败发布同形）；任务被取消（关机/estop）= 正常流不留痕。2026-10-11
/// 事故固化：planner 截断 panic 杀死规划任务、看板零留痕，用户体感
/// 「建了不动」——无人值守链路的后台任务死亡必须看板可见（日志只有
/// 翻日志的人看得到）。
#[cfg(feature = "cluster")]
pub(super) fn spawn_plan_chain_watched(
    store: Arc<BoardStore>,
    hub: Option<Arc<crate::events::EventHub>>,
    issue_id: i64,
    plan_id: String,
    fut: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let handle = tokio::spawn(fut);
    tokio::spawn(async move {
        if let Err(je) = handle.await {
            if !je.is_panic() {
                return; // 任务被取消（关机/estop）= 正常流
            }
            let msg = nemesis_utils::string_utils::panic_message(je.into_panic());
            let short: String = msg.chars().take(300).collect();
            tracing::error!("[Board] plan chain PANIC issue={issue_id} plan={plan_id}: {short}");
            let _ = store.add_comment(NewComment {
                issue_id,
                author: Actor::system("board"),
                content: format!(
                    "❌ 拆解链后台任务异常终止（内部错误 panic）：{short}——该单未完成自动流转，请重试拆解或人工处置"
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
            if let Some(hub) = hub {
                hub.publish(
                    "board.plan_failed",
                    serde_json::json!({ "plan_id": plan_id, "issue_id": issue_id, "error": format!("后台任务异常终止（panic）：{short}") }),
                );
            }
        }
    });
}

/// plan 链环境快照（fire-and-forget 三入口 + N1 重启恢复共用）：一次克隆
/// 进 `Arc`，超时重拆监视直接用同一份环境发起新链（递归走
/// [`spawn_plan_chain`]，panic 留痕与超时监视对新链自动续上）。
#[cfg(feature = "cluster")]
pub(super) struct PlanChainEnv {
    pub(super) store: Arc<BoardStore>,
    pub(super) cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    pub(super) agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    pub(super) issue: nemesis_board::Issue,
    pub(super) actor: Actor,
    /// config.json 读取根（超时阈值/attempt 上限/重启恢复开关现读；None =
    /// 全部走缺省值）。与 auto_confirm 同链路：改配置对下一链即时生效。
    pub(super) home: Option<std::path::PathBuf>,
    pub(super) hub: Option<Arc<crate::events::EventHub>>,
}

/// `board.plan` 段现读（home 缺失/读取失败 = 缺省值；不 fail-closed——
/// 超时/重试上限走缺省是安全方向，与 auto_confirm 的 fail-closed 语义
/// 不同：发车是副作用，超时监视只是兜底）。
#[cfg(feature = "cluster")]
fn plan_chain_cfg(home: Option<&std::path::Path>) -> nemesis_config::BoardPlanConfig {
    home.and_then(|h| {
        nemesis_config::load_config(&std::path::Path::new(h).join("config.json")).ok()
    })
    .map(|c| c.board.unwrap_or_default().plan)
    .unwrap_or_default()
}

/// D2 等待预算现读（`board.discussion.review_wait_secs`；home 缺失/读取
/// 失败 = 缺省 1800——与 [`plan_chain_cfg`] 同款缺省方向：等待节奏是
/// 提示词纪律，读不到配置走保守默认，不 fail-closed）。
#[cfg(feature = "cluster")]
fn board_discussion_wait_secs(home: Option<&std::path::Path>) -> u32 {
    home.and_then(|h| {
        nemesis_config::load_config(&std::path::Path::new(h).join("config.json")).ok()
    })
    .and_then(|c| c.board)
    .map(|b| b.discussion.review_wait_secs)
    .unwrap_or(1800)
}

/// BUG 2026-10-08 `board.plan.model` dead-key 修复：planner 模型覆盖解析。
/// `board.plan.model`（BoardPlanConfig.model，「planner 拆解用模型别名
/// （None = 主模型）」）此前 typed+writable 但零消费——字段一直是死键，
/// planner 恒用主模型。本函数把它解析成 ready-made (provider, 模型名) 对
/// （DetachedOpts.model_swap 契约），注入 [`run_planner`] 的 detached 调用。
///
/// 解析链与热切/启动同源：`load_config` → `board.plan.model` → trim 非空 →
/// `resolve_model_config`（别名直查）→ `create_provider` → `ProviderAdapter`
/// 桥（实现 loop 的 LlmProvider）。任何一步失败 = warn + None（回落主模型
/// ——拆解是看板主流程，覆盖模型配置坏了不应瘫痪拆解；与
/// `board_discussion_wait_secs` 的不 fail-closed 同方向）。
#[cfg(feature = "cluster")]
pub(super) fn board_plan_model_swap(
    home: Option<&std::path::Path>,
) -> Option<(
    std::sync::Arc<dyn nemesis_agent::r#loop::LlmProvider>,
    String,
)> {
    let alias = home
        .and_then(|h| {
            nemesis_config::load_config(&std::path::Path::new(h).join("config.json")).ok()
        })
        .and_then(|c| c.board)
        .and_then(|b| b.plan.model)?;
    let alias = alias.trim();
    if alias.is_empty() {
        return None;
    }
    let cfg = match home.and_then(|h| {
        nemesis_config::load_config(&std::path::Path::new(h).join("config.json")).ok()
    }) {
        Some(c) => c,
        None => {
            tracing::warn!(
                "[Board] board.plan.model='{alias}' 解析失败：config.json 不可读，回落主模型"
            );
            return None;
        }
    };
    let resolution = match nemesis_config::resolve_model_config(&cfg, alias) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[Board] board.plan.model='{alias}' 解析失败：{e}，回落主模型");
            return None;
        }
    };
    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        proxy: resolution.proxy.clone(),
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        workspace: resolution.workspace.clone(),
        connect_mode: resolution.connect_mode.clone(),
        protocol: resolution.protocol.clone(),
        timeout_secs: resolution.timeout_secs,
        stream_mode: resolution.stream_mode.clone(),
        cache_breakpoints: resolution.cache_breakpoints,
        account_id: String::new(),
        headers: std::collections::HashMap::new(),
    };
    let provider = match nemesis_providers::factory::create_provider(&factory_cfg) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[Board] board.plan.model='{alias}' provider 构造失败：{e}，回落主模型");
            return None;
        }
    };
    let adapter: std::sync::Arc<dyn nemesis_agent::r#loop::LlmProvider> = std::sync::Arc::new(
        crate::llm_bridge::ProviderAdapter::new(provider, resolution.model_name.clone()),
    );
    Some((adapter, resolution.model_name))
}

/// 拆解链 spawn 单点（超时重拆 §1.1 / §5#1 配置化）：= panic 留痕监视壳
///（[`spawn_plan_chain_watched`]）+ 超时重拆监视。attempt 从 1 起；重拆
/// attempt+1 新 plan_id；attempt 耗尽停车（评论 + `board.plan_failed` +
/// RUNNING_PLANS 摘除）——停车而非无限烧 LLM。
///
/// 设计注记（四入口覆盖面）：fire-and-forget 三入口（WSAPI `issue.plan` /
/// autopilot `auto_plan` / 项目 `auto_start`）+ N1 重启恢复走本函数；
/// agent 工具 `board_issue plan` 是同步 await 形态（agent 自盯结果，LLM
/// 调用受 provider 600s 超时 + parse 3 轮上限约束），不挂后台监视。
#[cfg(feature = "cluster")]
fn spawn_plan_chain(
    env: Arc<PlanChainEnv>,
    plan_id: String,
    attempt: u32,
    cfg: nemesis_config::BoardPlanConfig,
) {
    let chain_plan_id = plan_id.clone();
    let fut_env = env.clone();
    let fut = async move {
        if let Err(e) = execute_plan_chain(
            &fut_env.store,
            fut_env.cluster.clone(),
            fut_env.agent_loop.clone(),
            fut_env.issue.clone(),
            fut_env.actor.clone(),
            &chain_plan_id,
            fut_env.home.as_deref(),
            fut_env.hub.as_deref(),
            attempt,
        )
        .await
        {
            tracing::warn!(
                "[Board] plan chain failed issue={} plan={chain_plan_id}: {e}",
                fut_env.issue.id
            );
        }
    };
    spawn_plan_chain_watched(
        env.store.clone(),
        env.hub.clone(),
        env.issue.id,
        plan_id.clone(),
        fut,
    );
    spawn_plan_timeout_watch(env, plan_id, attempt, cfg);
}

/// 超时重拆监视（§1.1 D）：`timeout_secs`（0=不限，不监视）后醒来检查
/// RUNNING_PLANS——条目仍在 = 超时（正常完成 / panic / 错误路径的 guard
/// 都已摘条目）。处置：摘除本 plan_id（旧链 LLM 返回后 superseded 自检
/// 静默放弃——tokio 任务不可取消，双闸防双发车）→ 评论 → 重拆或停车。
/// `cfg` 由调用方注入（生产读 config，测试传秒级短值——§5#1 裁决）。
#[cfg(feature = "cluster")]
pub(super) fn spawn_plan_timeout_watch(
    env: Arc<PlanChainEnv>,
    plan_id: String,
    attempt: u32,
    cfg: nemesis_config::BoardPlanConfig,
) {
    if cfg.timeout_secs == 0 {
        return; // 0 = 不限：不挂监视
    }
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(cfg.timeout_secs)).await;
        {
            let mut reg = RUNNING_PLANS.lock();
            match reg.get(&plan_id) {
                Some(rp) if rp.issue_id == env.issue.id => {
                    reg.remove(&plan_id);
                }
                _ => return, // 已完成/已失败/已被其它链取代 = 未超时
            }
        }
        tracing::warn!(
            "[Board] plan chain timeout issue={} plan={plan_id} attempt={attempt} ({}s)",
            env.issue.id,
            cfg.timeout_secs
        );
        let minutes = cfg.timeout_secs / 60;
        let can_retry =
            cfg.timeout_reattempt && (cfg.max_attempts == 0 || attempt < cfg.max_attempts);
        if !can_retry {
            // 停车：不重拆（reattempt 关 或 attempt 耗尽），等人工。
            let why = if !cfg.timeout_reattempt {
                "自动重拆未开启（board.plan.timeout_reattempt=false）".to_string()
            } else {
                format!(
                    "已连续超时 {} 次（board.plan.max_attempts={}），已停止自动重试",
                    attempt, cfg.max_attempts
                )
            };
            tracing::warn!("[Board] plan chain 停车 issue={}：{why}", env.issue.id);
            // U-A：拆解停车档案（手动重拆 = issue.plan 入口）。
            write_side_handoff(
                &env.store,
                &env.issue,
                "plan_exhausted",
                "retry",
                format!("拆解超时 {why}：核查模型端点后手动重拆（issue.plan），或人工直接创建子单"),
                "issue.plan（number=NB-x）手动重拆；或人工拆解",
                serde_json::json!({
                    "attempt": attempt,
                    "max_attempts": cfg.max_attempts,
                    "timeout_secs": cfg.timeout_secs,
                }),
                vec![serde_json::json!({
                    "action": "plan_chain",
                    "result": format!("第 {attempt} 次拆解超时（{} 分钟）", minutes),
                })],
            );
            let _ = env.store.add_comment(NewComment {
                issue_id: env.issue.id,
                author: Actor::system("board"),
                content: format!(
                    "❌ 拆解超时（{} 分钟），{why}，请人工处理（可手动重拆）",
                    minutes
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
            if let Some(hub) = env.hub.as_deref() {
                hub.publish(
                    "board.plan_failed",
                    serde_json::json!({
                        "plan_id": plan_id,
                        "issue_id": env.issue.id,
                        "error": format!("拆解超时（{} 分钟），{why}", minutes),
                    }),
                );
            }
            return;
        }
        let next = attempt + 1;
        let _ = env.store.add_comment(NewComment {
            issue_id: env.issue.id,
            author: Actor::system("board"),
            content: format!(
                "⏰ 拆解超时（{} 分钟），自动重试（第 {} 次）",
                minutes, next
            ),
            parent_id: None,
            ctype: CommentType::System,
        });
        // 新链配置现读（热生效：改阈值/上限对重拆链即时生效）。
        let new_cfg = plan_chain_cfg(env.home.as_deref());
        let new_plan_id = format!("plan-{}", uuid::Uuid::new_v4());
        tracing::info!(
            "[Board] plan chain 重新拆解 issue={} new_plan={new_plan_id} attempt={next}",
            env.issue.id
        );
        spawn_plan_chain(env, new_plan_id, next, new_cfg);
    });
}

/// N1 gateway 重启拆解恢复（§1.3 / §7.2 N1，无人值守闭环）：RUNNING_PLANS
/// 是内存态、重启杀拆解任务——旧行为 = 父单停留「已开始自动拆解」评论态
/// 等手动重拆（无人值守语义下 = 流程中断）。启动恢复扫描：状态仍
/// backlog/todo + 最新系统评论为拆解中形态 + 无子单 + 不在 RUNNING_PLANS
/// 的单子 = 中断残留 → 自动重新发起拆解（走 [`spawn_plan_chain`] 同一
/// 超时重拆链，attempt 重计）。手动 cancel 的单子状态非 backlog/todo 天然
/// 排除；恢复拆解同受 `board.plan.max_attempts` 闸。
///
/// 调用点 = gateway 装配期（评审重放扫描同阶段，moderator 槽已装配）。
/// 守卫链：`board.plan.restart_recover`（默认开，关=关回旧行为）→ estop
/// 挂起跳过（急停意志优先，恢复不抢跑；estop release 不自动补拆——用户
/// 释放后可手动重拆或重启）→ 非 master 节点跳过（worker 本地 board.db 是
/// 只读镜像，拆解/派发链跑在 master）。返回恢复发起数。
#[cfg(feature = "cluster")]
pub fn recover_interrupted_plans(
    store: &Arc<BoardStore>,
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    home: Option<std::path::PathBuf>,
    hub: Option<Arc<crate::events::EventHub>>,
    estop: Option<&nemesis_agent::estop::EstopState>,
) -> usize {
    let cfg = plan_chain_cfg(home.as_deref());
    if !cfg.restart_recover {
        tracing::info!("[Board] 拆解恢复扫描跳过：board.plan.restart_recover=false");
        return 0;
    }
    if let Some(estop) = estop
        && estop.is_engaged()
    {
        tracing::warn!("[Board] 拆解恢复扫描跳过：estop 挂起（释放后请手动重拆或重启再试）");
        return 0;
    }
    if let Some(c) = &cluster
        && !matches!(c.role().as_str(), "coordinator" | "master" | "manager")
    {
        return 0; // worker 镜像不跑拆解链（与评审钩子同款角色闸）
    }
    let mut recovered = 0usize;
    for status in [IssueStatus::Backlog, IssueStatus::Todo] {
        let issues = match store.list_issues(&nemesis_board::models::IssueFilter {
            status: Some(status),
            ..Default::default()
        }) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[Board] 拆解恢复扫描 {status:?} 单据失败：{e}");
                continue;
            }
        };
        for issue in issues {
            // 拆解中形态 = 最新系统评论是「已开始自动拆解」（完成/失败/超时
            // 评论都在其后顶掉它；用户手动评论是普通类型不影响系统序列）。
            let interrupted = store
                .list_comments(issue.id)
                .map(|cs| {
                    cs.iter()
                        .rev()
                        .find(|c| c.ctype == CommentType::System)
                        .is_some_and(|c| c.content.starts_with("🧩 已开始自动拆解"))
                })
                .unwrap_or(false);
            if !interrupted {
                continue;
            }
            // 双保险：已有子单（拆解实际发过车）/ 已有在途链（扫描期间用户
            // 手动触发）→ 让位。
            if store
                .list_children(issue.id)
                .map(|c| !c.is_empty())
                .unwrap_or(true)
            {
                continue;
            }
            if RUNNING_PLANS
                .lock()
                .values()
                .any(|r| r.issue_id == issue.id)
            {
                continue;
            }
            tracing::info!(
                "[Board] 拆解恢复：issue {}（{}）重启中断残留 → 自动重新发起拆解",
                issue.number,
                issue.title
            );
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: "🔄 检测到上次拆解因重启中断，自动重新发起拆解".to_string(),
                parent_id: None,
                ctype: CommentType::System,
            });
            let env = Arc::new(PlanChainEnv {
                store: store.clone(),
                cluster: cluster.clone(),
                agent_loop: agent_loop.clone(),
                issue,
                actor: Actor::system("board"),
                home: home.clone(),
                hub: hub.clone(),
            });
            let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
            spawn_plan_chain(env, plan_id, 1, cfg.clone());
            recovered += 1;
        }
    }
    if recovered > 0 {
        tracing::info!("[Board] 拆解恢复扫描完成：恢复 {recovered} 个中断拆解");
    }
    recovered
}

/// U-B（§9.2）中断续拆：探针解封后对 `pending=plan` 中断档案重建拆解链
/// （与 N1 恢复同链——走 [`spawn_plan_chain`] 超时重拆链，attempt 重计 1）。
/// 在途双保险：该单已有在途链 = 让位（扫描/探针/手动三方竞态防双发车）。
/// 返回是否实际发车。
#[cfg(feature = "cluster")]
pub fn respawn_plan_chain(
    store: &Arc<BoardStore>,
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    issue: nemesis_board::Issue,
    home: Option<std::path::PathBuf>,
    hub: Option<Arc<crate::events::EventHub>>,
) -> bool {
    if RUNNING_PLANS
        .lock()
        .values()
        .any(|r| r.issue_id == issue.id)
    {
        tracing::info!("[Board] 中断续拆让位：issue {} 已有在途拆解链", issue.id);
        return false;
    }
    let _ = store.add_comment(NewComment {
        issue_id: issue.id,
        author: Actor::system("board"),
        content: "🔄 LLM 端点已恢复，中断的拆解自动续做".to_string(),
        parent_id: None,
        ctype: CommentType::System,
    });
    let env = Arc::new(PlanChainEnv {
        store: store.clone(),
        cluster,
        agent_loop,
        issue,
        actor: Actor::system("board"),
        home,
        hub,
    });
    let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
    let cfg = plan_chain_cfg(env.home.as_deref());
    spawn_plan_chain(env, plan_id, 1, cfg);
    true
}

/// `home` 供 A1 旗标现读（config.json 热改即时生效；None = 视为关闭，
/// fail-closed 回人工确认）。`hub` 供 SSE 事件（cron 路径 None——诚实降级：
/// 无实时推送，评论/落库/发车链路完整）。`attempt` = 第几次尝试（1 起；
/// 超时重拆链每次 +1，随 [`RunningPlan`] 注册与 `board.plan_started`
/// 载荷透出——前端横幅「第 N 次尝试」数据源）。
#[cfg(feature = "cluster")]
pub async fn execute_plan_chain(
    store: &Arc<BoardStore>,
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    issue: nemesis_board::Issue,
    actor: Actor,
    plan_id: &str,
    home: Option<&std::path::Path>,
    hub: Option<&crate::events::EventHub>,
    attempt: u32,
) -> Result<serde_json::Value, String> {
    let started = std::time::Instant::now();
    // U-B（§9.2）：LLM 端点不可用封存闸——拆解发起短路。自动/手动同闸：
    // 手动得到诚实挂起说明 + 中断档案（意图不丢，恢复后探针自动续做），
    // 不是静默吞掉。
    if super::super::board_unavailable::frozen_now() {
        if let Some(ws) = handoff_workspace_root().map(|p| p.as_path()) {
            super::super::board_unavailable::write_interruption(
                ws,
                store,
                &issue,
                "plan",
                format!("拆解发起时 LLM 端点不可用封存中（attempt {attempt}）：恢复后探针自动续拆"),
                serde_json::json!({ "plan_id": plan_id, "attempt": attempt }),
            );
        }
        tracing::warn!(
            "[Board] plan chain 挂起 issue={}：LLM 端点不可用封存中（恢复后自动续做）",
            issue.id
        );
        return Err(
            "⏸ LLM 端点不可用（封存中）：拆解已挂起，端点恢复后自动续做；也可稍后手动重拆（issue.plan）"
                .to_string(),
        );
    }
    let ub_cfg = super::super::board_unavailable::unavailable_cfg(home);
    // 拆解运行期可观测（2026-10-11「停着」事故）：running 条目（guard 摘除）
    // + `board.plan_started` SSE + 父单「已开始拆解」评论。planner 单次
    // 4~7 分钟且此前全程零反馈，用户体感=「停着」。
    RUNNING_PLANS.lock().insert(
        plan_id.to_string(),
        RunningPlan {
            issue_id: issue.id,
            started_at: started,
            attempt,
            phase: "planning",
        },
    );
    let _running_guard = RunningPlanGuard {
        plan_id: plan_id.to_string(),
    };
    if let Some(hub) = hub {
        hub.publish(
            "board.plan_started",
            serde_json::json!({
                "plan_id": plan_id,
                "issue_id": issue.id,
                "issue_number": issue.number,
                "attempt": attempt,
            }),
        );
    }
    let _ = store.add_comment(NewComment {
        issue_id: issue.id,
        author: Actor::system("board"),
        content: "🧩 已开始自动拆解（planner 运行中，通常 3~8 分钟；完成后按 auto_confirm 自动发车或转人工确认）"
            .to_string(),
        parent_id: None,
        ctype: CommentType::System,
    });
    // M4.5：planner 的 meta 级经验注入——同步检索渲染（发起点定格，避免长
    // LLM 调用期间的经验变更串进本次拆解）。meta 注入不计 use_count（计数
    // 只认面向 worker 的派发注入）。
    let team_experience: Vec<String> = {
        let entries = store.list_team_memory(None, false).unwrap_or_default();
        let issue_text = format!(
            "{}\n{}\n{}",
            issue.title,
            issue.description,
            issue.acceptance_criteria.as_deref().unwrap_or("")
        );
        let hits = nemesis_board::match_experiences(
            &entries,
            &issue_text,
            &issue.required_tags,
            nemesis_board::MAX_MATCHED_EXPERIENCES,
        );
        nemesis_board::render_planner_experience_strings(&hits)
    };
    // R-10（goal P4）：集群画像注入——planner 拆解即按可执行者对齐
    // （名字/角色/tags/能力；投影与派发匹配器同源）。无在线节点 → None。
    let cluster_profile: Option<String> = cluster.as_ref().and_then(|c| {
        let lines: Vec<String> = project_dispatch_candidates(c)
            .iter()
            .map(|p| {
                let tags = if p.tags.is_empty() {
                    "—".to_string()
                } else {
                    p.tags.join("/")
                };
                let caps = if p.capabilities.is_empty() {
                    String::new()
                } else {
                    format!(" caps={}", p.capabilities.join("/"))
                };
                // P6-R1：backend 宣告进画像——planner 才知道给哪类子单标
                // required_backend（值域 process/vm；未宣告节点显示 —）。
                let backend = match p.runtime_backend.as_deref() {
                    Some(b) if !b.is_empty() => format!(" backend={b}"),
                    _ => String::new(),
                };
                format!("- {} [role={} tags={tags}]{caps}{backend}", p.name, p.role)
            })
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    });
    // BUG 2026-10-08 `board.plan.model` dead-key 修复：覆盖模型解析（None =
    // 未配置/解析失败回落主模型——warn 已在 helper 内留痕）。
    let plan_model_swap = board_plan_model_swap(home);
    let (subs, downgrades) = match run_planner(
        &agent_loop,
        &issue,
        team_experience,
        cluster_profile,
        Some(board_discussion_wait_secs(home)),
        plan_model_swap.as_ref(),
    )
    .await
    {
        Ok((subs, downgrades)) => {
            // U-B：planner 成功 = 端点活证据（连击清零/封存解除——生产里
            // 封存期到不了这里：入口闸已拦）。
            super::super::board_unavailable::record_llm_success(&ub_cfg);
            (subs, downgrades)
        }
        Err(e) => {
            // U-B：planner 失败喂封存判定；恰好触发封存 = 全部在途工作写
            // 中断档案（本单 + 其它在途拆解链）。
            let tripped = super::super::board_unavailable::record_llm_failure(&ub_cfg, &e);
            if tripped {
                super::super::board_unavailable::write_trip_interruptions(
                    handoff_workspace_root().map(|p| p.as_path()),
                    store,
                    Some((
                        &issue,
                        "plan",
                        format!(
                            "拆解在途（attempt {attempt}）时 LLM 端点不可用封存：恢复后自动续拆"
                        ),
                        serde_json::json!({ "plan_id": plan_id, "attempt": attempt }),
                    )),
                    &e,
                );
            }
            // superseded 优先于失败面（§1.1 双闸）：被超时重拆取代的旧链
            // 静默放弃——不发失败评论、不发 plan_failed（旧链迟到的失败
            // 事件会撤掉新链横幅），不消费解析结果。
            if !RUNNING_PLANS.lock().contains_key(plan_id) {
                tracing::info!(
                    "[Board] plan chain superseded（已被超时重拆取代）issue={} plan={plan_id}，旧链静默放弃",
                    issue.id
                );
                return Ok(serde_json::json!({
                    "status": "superseded",
                    "plan_id": plan_id,
                    "issue_id": issue.id,
                }));
            }
            tracing::warn!("[Board] planner failed issue={}: {e}", issue.id);
            // P11 观测面：失败评论留痕（SSE 只在线可见，评论持久可回查；
            // 截断 200 字符防异常长报错刷屏单据）。U-A：链内重试已耗尽的
            // 最终失败 = 停车，落现场档案。
            let err_short: String = e.chars().take(200).collect();
            write_side_handoff(
                store,
                &issue,
                "plan_exhausted",
                "retry",
                format!(
                    "planner 拆解失败（链内重试已耗尽）：核查模型端点后手动重拆（issue.plan），或人工直接创建子单。原因：{err_short}"
                ),
                "issue.plan（number=NB-x）手动重拆；或人工拆解",
                serde_json::json!({ "plan_id": plan_id }),
                vec![serde_json::json!({
                    "action": "run_planner",
                    "result": err_short,
                })],
            );
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: format!("❌ planner 拆解失败：{err_short}"),
                parent_id: None,
                ctype: CommentType::System,
            });
            if let Some(hub) = hub {
                hub.publish(
                        "board.plan_failed",
                        serde_json::json!({ "plan_id": plan_id, "issue_id": issue.id, "error": e.clone() }),
                    );
            }
            return Err(e);
        }
    };
    // superseded 自检（§1.1 双闸之一，成功路径）：超时重拆监视在超时点摘除
    // 本 plan_id 并发起新链——本链的 LLM future 不可取消仍在飞，返回后第一
    // 检查点验证自己仍在 RUNNING_PLANS；不在 = 已被取代 → 静默放弃：不发
    // plan_ready、不落缓存、不评论、不 confirm（防「双计划双发车」）。
    if !RUNNING_PLANS.lock().contains_key(plan_id) {
        tracing::info!(
            "[Board] plan chain superseded（已被超时重拆取代）issue={} plan={plan_id}，旧链静默放弃",
            issue.id
        );
        return Ok(serde_json::json!({
            "status": "superseded",
            "plan_id": plan_id,
            "issue_id": issue.id,
        }));
    }
    // ---- 父单验收标准起草（预防层，NB-101 AC 缺失根修）----
    // planner 输出 schema 只有子单级 AC，父单基线不在其中；父单 AC 为空时
    // 收口评审读父单自身 AC = 「未提供」→ 评审诚实 UNSURE 转人工，复评永远
    // 弹回。此处 best-effort 依据子单计划为空基线父单起草整体 AC：成功 =
    // 落库 + 系统评论留痕（用户可在详情页编辑）；失败 = WARN + 评论指路
    // （不阻塞发车——附属步骤失败不拦拆解主流程）。非空基线不动（用户
    // 显式给的 AC 最大）。
    let mut issue = issue;
    if issue
        .acceptance_criteria
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        let prompt = build_parent_ac_from_plan_prompt(&issue, &subs);
        match run_ac_draft(&agent_loop, &prompt, plan_model_swap.as_ref()).await {
            Ok(draft) => {
                let patch = nemesis_board::IssuePatch {
                    acceptance_criteria: Some(draft.clone()),
                    ..Default::default()
                };
                match store.update_issue(issue.id, &patch, &Actor::system("board")) {
                    Ok(_) => {
                        issue.acceptance_criteria = Some(draft);
                        let _ = store.add_comment(NewComment {
                            issue_id: issue.id,
                            author: Actor::system("board"),
                            content: "🧩 父单此前无验收标准，已依据子单计划自动起草（详情页可编辑）；父单收口评审将以此为准，不再因「未提供」转人工".to_string(),
                            parent_id: None,
                            ctype: CommentType::System,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("[Board] 父单 AC 起草落库失败 issue={}：{e}", issue.id);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[Board] 父单 AC 起草失败 issue={}：{e}（不阻塞发车）",
                    issue.id
                );
                let _ = store.add_comment(NewComment {
                    issue_id: issue.id,
                    author: Actor::system("board"),
                    content: format!(
                        "⚠ 父单此前无验收标准，自动起草失败：{e}。父单收口评审将因无基线转人工；可在决策面板「补验收标准 → ✨ AI 起草」手动起草"
                    ),
                    parent_id: None,
                    ctype: CommentType::System,
                });
            }
        }
    }
    let count = subs.len();
    // A1 旗标前置：plan_ready 载荷要带 auto_confirm（前端据此直接进
    // 「发车中」态而不是渲染确认按钮），故先读后发。
    let auto_confirm = home
        .and_then(|h| {
            nemesis_config::load_config(&std::path::Path::new(h).join("config.json")).ok()
        })
        .map(|c| c.board.unwrap_or_default().plan.auto_confirm)
        .unwrap_or(false);
    // 人工确认模式才落缓存（auto_confirm 链随后直接消费，落了立刻摘 =
    // 白写；且不落才能保证派发窗口内 plan.list 永远无 ready 预览）。
    if !auto_confirm {
        let mut cache = PLAN_CACHE.lock();
        cache.retain(|_, p| p.created_at.elapsed() <= PLAN_TTL);
        cache.insert(
            plan_id.to_string(),
            PlanPreview {
                issue_id: issue.id,
                subs: subs.clone(),
                downgrades: downgrades.clone(),
                created_at: std::time::Instant::now(),
            },
        );
    }
    tracing::info!(
        "[Board] planner done issue={} subs={count} ({}ms)",
        issue.id,
        started.elapsed().as_millis()
    );
    if let Some(hub) = hub {
        hub.publish(
            "board.plan_ready",
            serde_json::json!({
                "plan_id": plan_id,
                "issue_id": issue.id,
                "subs": subs.clone(),
                "auto_confirm": auto_confirm,
            }),
        );
    }
    // ---- A1：board.plan.auto_confirm=true → 跳过人工确认闸直接发车。
    // 旗标每次拆解现读（config.json 热改即时生效）；读取失败 fail-closed
    // 回人工确认。
    // 2026-10-15 NB-101 观察窗口双修：
    // ① 预览不落缓存（上方 if）——派发窗口（逐子单 RPC 送达，实测可达
    //   分钟级）内 plan.list 无 ready 预览，前端确认面板无从渲染；此间
    //   人工走 WSAPI 点确认得到「plan_id 不存在或已被消费」诚实报错，而
    //   不是把同一套子单再物化一遍（重复建单）。旧形态「发车成功后移除」
    //   只保护窗口之后，保护不了窗口之中。
    // ② 阶段翻转为 dispatching——issue.list/plan.list 投影让前端横幅
    //   显示「发车中」而非「拆解中」（子单已可见、首个已在 worker 上跑，
    //   顶部还挂「拆解中」是误报）。
    // P11 观测面：拆解成功评论留痕（含 plan_id；措辞按发车模式分支——
    // 人工模式附 TTL 提示，自动发车臂随后另有发车评论，不重复诱导确认）。
    let _ = store.add_comment(NewComment {
        issue_id: issue.id,
        author: Actor::system("board"),
        content: if auto_confirm {
            format!("🧩 planner 拆解完成：{plan_id}，{count} 个子单（自动发车模式）")
        } else {
            format!(
                "🧩 planner 拆解完成：{plan_id}，{count} 个子单，请在 {}s 内确认派发",
                PLAN_TTL.as_secs()
            )
        },
        parent_id: None,
        ctype: CommentType::System,
    });
    if !auto_confirm {
        return Ok(serde_json::json!({
            "status": "planned",
            "plan_id": plan_id,
            "issue_id": issue.id,
            "subs": count,
            "note": "已生成拆解预览，等待人工确认发车（board.plan.auto_confirm 未开启）",
        }));
    }
    // 阶段翻转 planning→dispatching：confirm_plan 内部逐子单 RPC 送达可
    // 阻断数分钟，投影面（issue.list/plan.list）须立刻改挂「发车中」。
    if let Some(rp) = RUNNING_PLANS.lock().get_mut(plan_id) {
        rp.phase = "dispatching";
    }
    match confirm_plan(store, cluster.as_ref(), &issue, subs, &actor, &downgrades) {
        Ok(mut out) => {
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: format!(
                    "🤖 board.plan.auto_confirm 已开启，拆解自动发车：{} 子单已建，{} 已派",
                    out.get("created")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0),
                    out.get("dispatched").and_then(|v| v.as_u64()).unwrap_or(0),
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
            // E2 决策审计：自动发车入决策流（失败只 warn，不炸发车结果）。
            if let Err(ae) = store.add_activity(
                issue.id,
                &Actor::system("board"),
                "auto_decide",
                Some(
                    &serde_json::json!({
                        "decision": "auto_confirm_dispatch",
                        "verdict": "plan",
                        "subs": count,
                    })
                    .to_string(),
                ),
            ) {
                tracing::warn!("[Board] auto_decide 活动落库失败 issue={}：{ae}", issue.id);
            }
            tracing::info!("[Board] auto_confirm 发车 issue={} subs={count}", issue.id);
            out["status"] = serde_json::json!("dispatched");
            out["plan_id"] = serde_json::json!(plan_id);
            out["subs"] = serde_json::json!(count);
            Ok(out)
        }
        Err(e) => {
            tracing::warn!(
                "[Board] auto_confirm 发车失败 issue={}：{e}（保留预览，转人工确认）",
                issue.id
            );
            // P11 观测面：发车失败评论留痕（预览保留转人工，评论给入口指引）。
            let err_short: String = e.chars().take(200).collect();
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: format!(
                    "⚠️ 拆解完成但自动发车失败：{err_short}（预览已保留，可人工确认派发）"
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
            if let Some(hub) = hub {
                hub.publish(
                    "board.plan_failed",
                    serde_json::json!({
                        "plan_id": plan_id,
                        "issue_id": issue.id,
                        "error": format!("auto_confirm 发车失败：{e}"),
                    }),
                );
            }
            Err(format!(
                "拆解完成但自动发车失败：{e}（预览已保留，可人工确认）"
            ))
        }
    }
}

/// `issue.plan` 实现（cluster 编译时）：一段异步拆解 / 二段确认落库+派发波。
#[cfg(feature = "cluster")]
pub(super) async fn issue_plan(
    store: &Arc<BoardStore>,
    actor: Actor,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    // F-U4-5：急停中拒绝（二段确认=派发波发车；一段拆解的 LLM 走主持人
    // loop 同被急停冻结——两段统一在此诚实拒绝，反馈比自然失败更快更明确）。
    refuse_dispatch_when_estopped(ctx)?;
    let data = data.ok_or("missing data")?;
    let id = data
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("missing field: id")?;
    let issue = store.get_issue(id)?;

    if data
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        // ---- 二段：确认发车（缓存一次性消费）----
        let plan_id = get_str(&data, "plan_id")?.to_string();
        let preview = {
            let mut cache = PLAN_CACHE.lock();
            match cache.remove(plan_id.as_str()) {
                Some(p) if p.issue_id == id && p.created_at.elapsed() <= PLAN_TTL => p,
                Some(p) => {
                    // 与该 issue 不匹配 → 放回（可能是用户在别的 issue
                    // 表单里贴错了 plan_id，不应误伤那个在途预览）。
                    cache.insert(plan_id.clone(), p);
                    return Err("plan 与该 issue 不匹配，请重新拆解".to_string());
                }
                None => {
                    return Err("plan_id 不存在或已被消费（每个 plan 只能确认一次）".to_string());
                }
            }
        };
        let cluster = ctx.state.cluster.clone();
        let out = confirm_plan(
            store,
            cluster.as_ref(),
            &issue,
            preview.subs,
            &actor,
            &preview.downgrades,
        )?;
        // 发车 = 「计划待确认」已消费 / 旧停车档案一并归档（恢复动作之一）。
        archive_handoff_best_effort(&issue.number);
        Ok(Some(out))
    } else {
        // ---- 一段：异步拆解（重复点按钮 = 重复 LLM 消耗，新 plan_id
        //      覆盖旧预览；结果幂等——confirm 以 plan_id 定位）----
        let agent_loop = ctx
            .state
            .agent_loop
            .read()
            .clone()
            .ok_or("agent 未运行，无法执行 AI 拆解")?;
        // M4.5 经验检索 / planner 调用 / 预览缓存 / plan_ready 事件 /
        // A1 auto_confirm 发车——编排全部收敛到 execute_plan_chain（P3/D1：
        // 与 agent 工具 board_issue plan、autopilot auto_plan、项目
        // auto_start 四入口同一实现）。这里只负责丢后台 + 记日志（SSE 错误
        // 事件的发布在链内完成，不重复发）。
        let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
        let hub = ctx.state.event_hub.clone();
        let env = std::sync::Arc::new(PlanChainEnv {
            store: store.clone(),
            cluster: ctx.state.cluster.clone(),
            agent_loop,
            issue: issue.clone(),
            actor: actor.clone(),
            home: ctx.home.clone().map(std::path::PathBuf::from),
            hub: Some(hub),
        });
        let cfg = plan_chain_cfg(env.home.as_deref());
        spawn_plan_chain(env, plan_id.clone(), 1, cfg);
        Ok(Some(
            serde_json::json!({ "status": "planning", "plan_id": plan_id }),
        ))
    }
}

/// `issue.plan`（未编译 cluster）：planner 产物的派发语义（required_* 匹配、
/// 依赖补派波）全依赖集群。
#[cfg(not(feature = "cluster"))]
pub(super) async fn issue_plan(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.plan 需要集群支持（cluster feature 未编译）".to_string())
}

/// 二段确认：预览 → 落库（parent/required_*/origin=planner）→ 依赖边
/// （批内序号 → 真实 id）→ 依赖闸派发波（无依赖的立即派，其余留 backlog
/// 等补派触发器）→ 父单联动。单张派发失败不回滚整批（落库已成事实），
/// 系统评论留痕继续。
///
/// pub（全自动流转 P3/D1）：WSAPI `issue.plan` confirm 与 agent 工具 /
/// autopilot auto_plan / 项目 auto_start 共用（单一真相源）。
#[cfg(feature = "cluster")]
pub fn confirm_plan(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    parent: &nemesis_board::Issue,
    subs: Vec<nemesis_board::PlannedSubIssue>,
    actor: &Actor,
    downgrades: &[(usize, String)],
) -> Result<serde_json::Value, String> {
    // 1) 落库（保持批内顺序；id_by_idx 供依赖映射）。required_profession
    //    随拆解落库（M2）；D9 末轮宽和降级明细逐条落被降级子单的系统评论
    //    「required_profession=`…` 非法，已降级无职能」（审计可回查）。
    //    降级子单**强制置空职能**再落库（防御性单一真相源：清空语义在
    //    confirm_plan 定案，不依赖上游 parse_plan_lenient 是否已剥）。
    let mut id_by_idx: Vec<i64> = Vec::with_capacity(subs.len());
    for (idx, sub) in subs.iter().enumerate() {
        let ac = sub.acceptance_criteria.trim();
        let role = sub.required_role.trim();
        let downgraded = downgrades.iter().any(|(di, _)| *di == idx);
        let prof = if downgraded {
            ""
        } else {
            sub.required_profession.trim()
        };
        // required_backend 已在 parse_plan 归一（严格臂校验写回/宽和臂非法
        // 置空），此处只做空白防波；不参与职能降级（backend 闸与职能正交）。
        let backend = sub.required_backend.trim();
        let created = store.create_issue(nemesis_board::NewIssue {
            title: sub.title.clone(),
            description: sub.description.clone(),
            acceptance_criteria: (!ac.is_empty()).then(|| ac.to_string()),
            parent_issue_id: Some(parent.id),
            required_role: (!role.is_empty()).then(|| role.to_string()),
            required_tags: sub.required_tags.clone(),
            required_profession: (!prof.is_empty()).then(|| prof.to_string()),
            required_backend: (!backend.is_empty()).then(|| backend.to_string()),
            required_capabilities: sub.required_capabilities.clone(),
            // 子单归属父单所在项目（F2 首派联动读子单的 project_id 推进
            // 项目 active→in_progress；缺继承则联动早退、项目永远 active）。
            project_id: parent.project_id,
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "planner".to_string(),
                origin_id: parent.number.clone(),
                origin_meta: Default::default(),
            }),
            creator: actor.clone(),
            ..nemesis_board::NewIssue::default()
        })?;
        if downgraded && let Some((_, original)) = downgrades.iter().find(|(di, _)| *di == idx) {
            let _ = store.add_comment(nemesis_board::NewComment {
                issue_id: created.id,
                author: nemesis_board::Actor::system("board"),
                content: format!(
                    "⚠ required_profession=`{original}` 非法，已降级无职能（planner 末轮宽和）"
                ),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
        }
        id_by_idx.push(created.id);
    }

    // 2) 依赖边（批内序号 → 真实 id；越界 parse_plan 已挡，防御再钳一次）。
    for (idx, sub) in subs.iter().enumerate() {
        let deps: Vec<i64> = sub
            .depends_on
            .iter()
            .filter(|&&d| d < id_by_idx.len() && d != idx)
            .map(|&d| id_by_idx[d])
            .collect();
        store.set_issue_dependencies(id_by_idx[idx], &deps)?;
    }

    // C 里程碑 1（看板项目档案 goal P2）：拆解落库 → docs/plan.md +
    // timeline。档案写入失败不阻塞发车（writer 内部 WARN+审计，goal C 纪律）；
    // 存量项目（无 directory）静默跳过。
    nemesis_board::archive_writer::write_plan_milestone(store, parent, &subs);

    // 3) 派发波：依赖闸放行的立即派，其余留 backlog 等补派触发器。
    let mut dispatched = 0usize;
    let mut deferred: Vec<i64> = Vec::new();
    for &cid in &id_by_idx {
        match dispatch_subissue_auto(store, cluster, cid, actor, true) {
            Ok(Some(_)) => dispatched += 1,
            Ok(None) => deferred.push(cid),
            Err(e) => {
                tracing::warn!("[Board] 子单 {cid} 自动派发失败：{e}");
                let _ = store.add_comment(nemesis_board::NewComment {
                    issue_id: cid,
                    author: nemesis_board::Actor::system("board"),
                    content: format!("⛔ 自动派发失败：{e}"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    }

    // 4) 父单联动（首派 → in_progress 已在 dispatch_subissue_auto 内处理）。
    let _ = sync_parent_status(store, parent.id, actor);

    // 5) F5 件③：父单锚点下放防御性后置检查——提示词纪律（件②）LLM
    //    不保证遵守：父单 AC 含 `[CHECK] re:` 锚点但没有任何子单继承 →
    //    系统评论提醒人工补（不阻塞发车；评论就是持久记录，详情弹窗可见）。
    if nemesis_board::has_content_regex_anchor(parent.acceptance_criteria.as_deref().unwrap_or(""))
        && !subs
            .iter()
            .any(|s| nemesis_board::has_content_regex_anchor(&s.acceptance_criteria))
    {
        let _ = store.add_comment(nemesis_board::NewComment {
            issue_id: parent.id,
            author: nemesis_board::Actor::system("board"),
            content: "⚠ 锚点下放检查：父单验收标准含 [CHECK] re: 锚点，但拆解出的子单均未继承任何 re: 锚点——父单收口时这些锚点没有交付文本可核验，验收将失败。请给至少一个子单补上可独立核验的锚点行，或修正父单验收标准。".to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        tracing::warn!(
            "[Board] 父单 {} AC 含 re: 锚点但子单均未继承（已落评论提醒人工）",
            parent.id
        );
    }

    Ok(serde_json::json!({
        "created": id_by_idx,
        "dispatched": dispatched,
        "deferred": deferred,
        "issue": issue_to_view(store, &store.get_issue(parent.id)?)?,
    }))
}
// ---------------------------------------------------------------------------
// autopilot（W2 P4 定时派活）：模板建单 + 可选派发 + cron 触发簿记
// ---------------------------------------------------------------------------

/// autopilot 模板 → NewIssue：标题 `{date}` 占位符替换为本地日期
/// （YYYY-MM-DD）；origin 记 autopilot（run 历史 =
/// `list_issues_by_origin("autopilot", id)`）；target 非空时预指派 worker。
pub(super) fn autopilot_new_issue(ap: &nemesis_board::Autopilot, actor: &Actor) -> NewIssue {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut ni = NewIssue {
        title: ap.title.replace("{date}", &date),
        description: ap.description.clone(),
        // F-U5-1：验收标准透传——定时任务也要能全自动验收。空 = 不填，
        // 由 store.create_issue 的 ac_gen 创建时自动生成（NB-125 单一真相点，
        // 模板/规则作者不填也会得到自动推导的验收基准）；**不存在**旧语义
        // 「留空 = 保守转人工」——ac_gen 落地后留空不再是安全阀，规则作者
        // 想强制人工评判应显式写不可自动核验的验收标准或在收口时人工干预。
        acceptance_criteria: ap
            .acceptance_criteria
            .clone()
            .filter(|s| !s.trim().is_empty()),
        priority: ap.priority,
        project_id: ap.project_id,
        creator: actor.clone(),
        origin: Some(nemesis_board::models::TaskOrigin {
            origin_type: "autopilot".to_string(),
            origin_id: ap.id.to_string(),
            origin_meta: Default::default(),
        }),
        ..NewIssue::default()
    };
    let target = ap.target.trim();
    if !target.is_empty() {
        ni.assignee = Some(AssignmentType::Worker);
        ni.assignee_id = Some(target.to_string());
    }
    ni
}

// ---------------------------------------------------------------------------
// L23（三期 2026-11-03）：autopilot 父会话绑定
// 规则可选绑定发起 session_key：fire 时父会话存活 → 建单 description 注入
// 父会话溯源行 + 尾窗摘录（planner/worker 派发/评审三条既有链路读
// description 自动可见，零链路改动）；父会话 clear/删除 → 规则自动暂停 +
// 评论留痕（不孤儿运行）。注入的是会话账本（chat_log）里已过安全 8 层的
// 内容——与普通消息同源，不开旁路；工具面/tier 供给闸照常生效（建单不
// 改变任何 agent 装配）。不绑定 = 现行行为零变化。
// ---------------------------------------------------------------------------

/// L23 父会话存活判定（单一判据：chat_log 账本 total > 0）。clear 是截空
/// （文件在、0 行）→ total=0；删除（硬删/回收站移走）→ 文件消失 → total=0。
/// 两态都不存活——绑定规则 fire 时一律裁决为「父会话已不存在」。
fn parent_session_alive(session_key: &str) -> bool {
    let (_, total, _, _) = nemesis_agent::chat_log::read_chat_log(session_key, 1, None);
    total > 0
}

/// L23 父会话上下文块：尾窗 40 行 → 倒序筛可投影行（user/assistant、
/// 跳空内容与投影行）→ 最多 10 条、每条 600 字 → join 后总截 4000 字。
/// 与前端历史气泡同一投影判定（`is_projected_chat_row` 单一真相源）。
fn build_parent_context_block(session_key: &str) -> String {
    const WINDOW: usize = 40;
    const MAX_MESSAGES: usize = 10;
    const PER_MSG_CHARS: usize = 600;
    const TOTAL_CHARS: usize = 4000;
    let (rows, _, _, _) = nemesis_agent::chat_log::read_chat_log(session_key, WINDOW, None);
    let mut picked: Vec<(String, String)> = Vec::new();
    for row in rows.iter().rev() {
        if picked.len() >= MAX_MESSAGES {
            break;
        }
        let role = row.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = row.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if !nemesis_agent::chat_log::is_projected_chat_row(role, content) {
            continue;
        }
        picked.push((
            role.to_string(),
            nemesis_utils::truncate(content, PER_MSG_CHARS),
        ));
    }
    picked.reverse(); // 摘录保持时间顺序（旧 → 新）
    let joined = picked
        .iter()
        .map(|(role, content)| format!("[{role}] {content}"))
        .collect::<Vec<_>>()
        .join("\n");
    nemesis_utils::truncate(&joined, TOTAL_CHARS)
}

/// L23 暂停绑定规则：enabled=false 落库（store 是真相源；gateway 的 cron
/// tick 先读规则 disabled 短路，不误触发；job 侧 enabled 旗标启动同步时
/// 对齐——不 patch cron job，最终一致）+ ⏸ 系统评论落最近一次 run 的单
/// （从未运行无评论面 = WARN 日志即留痕面）。
fn pause_bound_autopilot(
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
    reason: &str,
) -> Result<(), String> {
    use nemesis_board::models::{AutopilotPatch, CommentType, NewComment};
    store.update_autopilot(
        ap.id,
        &AutopilotPatch {
            enabled: Some(false),
            ..Default::default()
        },
    )?;
    let content = format!(
        "⏸ 父会话 `{}` 已不存在（{}），本规则已自动暂停（不孤儿运行）；重新启用前请先改绑有效会话",
        ap.session_key.as_deref().unwrap_or(""),
        reason
    );
    match store.list_issues_by_origin("autopilot", &ap.id.to_string(), 1) {
        Ok(issues) => {
            if let Some(latest) = issues.first() {
                let _ = store.add_comment(NewComment {
                    issue_id: latest.id,
                    author: Actor::system("board"),
                    content,
                    parent_id: None,
                    ctype: CommentType::System,
                });
            } else {
                tracing::warn!(
                    "[Board] autopilot #{}「{}」绑定父会话已消失并自动暂停（{}）；规则从未运行，无评论面，留痕仅此处",
                    ap.id,
                    ap.name,
                    reason
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "[Board] autopilot #{}「{}」暂停留痕评论失败: {e}（暂停本身已生效）",
                ap.id,
                ap.name
            );
        }
    }
    Ok(())
}

/// L23 fire 时父会话裁决（单一入口，两个 fire_autopilot 变体共用）：
/// 未绑定 → `Ok(None)`（现行行为零变化）；绑定且父会话存活 →
/// `Ok(Some(上下文块))`；绑定但父会话已消失 → 先暂停（enabled=false +
/// 评论留痕）再返回 `Err`——fire 中止、不建单、gateway/WSAPI 把 Err 包成
/// 「触发失败：…已自动暂停」诚实可见。
fn resolve_autopilot_parent(
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
) -> Result<Option<String>, String> {
    let Some(key) = ap
        .session_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    if parent_session_alive(key) {
        let mut block = format!("## 父会话上下文（autopilot 绑定继承）\n\n- 父会话: {key}\n");
        let excerpt = build_parent_context_block(key);
        if !excerpt.trim().is_empty() {
            block.push_str("\n### 会话尾窗摘录\n\n");
            block.push_str(&excerpt);
            block.push('\n');
        }
        return Ok(Some(block));
    }
    pause_bound_autopilot(store, ap, "clear 或删除")?;
    Err(format!(
        "绑定的父会话 `{key}` 已不存在（clear 或删除），规则已自动暂停（不孤儿运行）"
    ))
}

/// L23 主动级联：父会话被删除/clear 时（sessions.rs delete/clear 路径调用），
/// 暂停所有绑定该会话的启用规则。返回被暂停的规则 id 列表（调用方用于
/// 汇报/留痕）。幂等：不启用或未绑定的规则不受影响。
pub fn pause_autopilots_bound_to(
    store: &Arc<BoardStore>,
    session_key: &str,
    reason: &str,
) -> Vec<i64> {
    let mut paused = Vec::new();
    let Ok(rules) = store.list_autopilots() else {
        return paused;
    };
    for ap in rules {
        if !ap.enabled {
            continue;
        }
        let bound = ap
            .session_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            == Some(session_key);
        if !bound {
            continue;
        }
        match pause_bound_autopilot(store, &ap, reason) {
            Ok(()) => paused.push(ap.id),
            Err(e) => {
                tracing::warn!(
                    "[Board] L23 级联暂停 autopilot #{}「{}」失败: {e}（其余规则照常）",
                    ap.id,
                    ap.name
                );
            }
        }
    }
    paused
}

/// 把 L23 上下文块注入 NewIssue 的 description（既有描述在前、块在后；
/// 既有描述为空时块即描述）。返回注入后的 NewIssue。
fn inject_parent_context(mut ni: NewIssue, block: String) -> NewIssue {
    if ni.description.trim().is_empty() {
        ni.description = block;
    } else {
        ni.description = format!("{}\n\n{}", ni.description, block);
    }
    ni
}

/// 全自动流转 D2：autopilot auto_plan 触发 plan 链所需的运行时上下文。
/// cron 路径的 moderator 槽在 gateway 装配期晚填（OnceLock 模式，照
/// autopilot_cluster_slot 同款）；WSAPI 路径 agent_loop 现成，包一个即抛
/// 槽传入。`hub` cron 路径传 None（web server 尚未装配，诚实降级：无 SSE
/// 推送，评论/落库/发车链路完整）。`cluster` 供拆解后的派发波使用——
/// planner 产物无预指派，`dispatch_subissue_auto` 走匹配器选节点，没有
/// 集群引用时诚实降级（子单留 todo + ⛔ 评论，单节点模式即此形态）。
#[cfg(feature = "cluster")]
pub struct AutoPlanContext {
    /// planner 的 moderator AgentLoop 槽（空槽 = 未就绪，诚实降级见
    /// `fire_autopilot` 内注释）。
    pub moderator_slot: Arc<std::sync::OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
    /// home 目录（A1 auto_confirm 旗标现读）。
    pub home: std::path::PathBuf,
    /// SSE 事件 hub（None = 不推事件）。
    pub hub: Option<Arc<crate::events::EventHub>>,
    /// 集群引用（拆解后派发波的匹配器选节点依赖它；None = 派发诚实失败落
    /// 系统评论，与单节点模式语义一致）。
    pub cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
}

/// autopilot 触发核心（W2 P4；WSAPI `autopilot.run` 与 gateway 的 cron
/// on_job 共用，单一真相源）：按模板建 issue → target 非空时派发 →
/// 记 last_run_at。`cluster` 传 `None` 且 target 非空 → 建单前拒绝
/// （不留半成品）。
///
/// 全自动流转 D2：`auto_plan: Option<&AutoPlanContext>` —— `Some` 且规则
/// `auto_plan=true` 且未预指派 target → 建单后异步触发 plan 链（moderator
/// 槽空时 WARN + 系统评论诚实降级，不炸 cron 回调）。`None`（CLI 手跑等
/// 无运行时上下文的入口）且规则开了 auto_plan → 结果 JSON 标注 skipped。
#[cfg(feature = "cluster")]
pub fn fire_autopilot(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    ap: &nemesis_board::Autopilot,
    actor: &Actor,
    auto_plan: Option<&AutoPlanContext>,
) -> Result<serde_json::Value, String> {
    // EST-03（复核 2026-09-16）：入口前置闸——estop 中 autopilot 不建单。
    // 此前闸只在下游（dispatch_issue_core / execute_plan_chain 派发点），
    // estop 中到点的规则仍先建出单再在派发/拆解处失败，留下孤儿单 + 失败
    // 评论噪音。前置拒绝：cron 下一轮到点自动重试（estop_frozen_error
    // 文案已声明该语义）。非 cluster 版 fire_autopilot 只做纯建单（target
    // 必须空、auto_plan 降级），无 agent 活动链，不设闸。
    if board_estop_engaged() {
        return Err(estop_frozen_error());
    }
    let target = ap.target.trim().to_string();
    if !target.is_empty() && cluster.is_none() {
        return Err(format!(
            "autopilot「{}」配置了派发目标 {target}，但集群未运行，无法派发",
            ap.name
        ));
    }
    // L23：父会话裁决（绑定且父会话消失 → 已自动暂停 + Err，不建单）。
    let parent_block = resolve_autopilot_parent(store, ap)?;
    let mut new_issue = autopilot_new_issue(ap, actor);
    if let Some(block) = parent_block {
        new_issue = inject_parent_context(new_issue, block);
    }
    let issue = store.create_issue(new_issue)?;
    let dispatch = if target.is_empty() {
        None
    } else {
        Some(
            dispatch_issue_core(store, cluster, issue.id, &target, actor, None)
                .map_err(|e| format!("issue #{} 已创建但派发失败: {e}", issue.number))?,
        )
    };
    // D2：无显式 assignee（target 空）+ 规则开 auto_plan → 自动拆解。
    let auto_plan_out = if target.is_empty() && ap.auto_plan {
        fire_autopilot_plan_chain(store, issue.id, actor, auto_plan)
    } else {
        serde_json::json!(null)
    };
    store.mark_autopilot_run(ap.id)?;
    Ok(serde_json::json!({
        "ran": true,
        "issue_id": issue.id,
        "issue_number": issue.number,
        "dispatch": dispatch,
        "auto_plan": auto_plan_out,
    }))
}

/// 全自动流转 P3/F1：项目自动启动——建父单（title=项目名、origin=project、
/// acceptance_criteria 透传、project_id 绑定）并后台触发 plan 链（与 WSAPI
/// `issue.plan` 同一 [`execute_plan_chain`]，D2/F1 共源）。返回创建的父单。
/// agent 未运行时诚实降级：父单照建 + 系统评论注明未拆解（项目已建成是
/// 事实，不因 planner 缺席回滚），不报错。
#[cfg(feature = "cluster")]
pub(super) fn spawn_project_auto_start(
    store: &Arc<BoardStore>,
    ctx: &RequestContext,
    actor: &Actor,
    project_name: &str,
    project_description: &str,
    acceptance_criteria: Option<String>,
    project_id: i64,
) -> Result<nemesis_board::Issue, String> {
    let issue = store.create_issue(NewIssue {
        title: project_name.to_string(),
        description: project_description.to_string(),
        acceptance_criteria: acceptance_criteria.filter(|s| !s.trim().is_empty()),
        project_id: Some(project_id),
        origin: Some(nemesis_board::models::TaskOrigin {
            origin_type: "project".to_string(),
            origin_id: project_id.to_string(),
            origin_meta: Default::default(),
        }),
        creator: actor.clone(),
        ..NewIssue::default()
    })?;
    let agent_loop = match ctx.state.agent_loop.read().clone() {
        Some(l) => l,
        None => {
            tracing::warn!(
                "[Board] 项目自动启动 issue={}：agent 未运行，仅建父单不拆解",
                issue.id
            );
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: "🤖 项目自动启动已建父单，但 agent 未运行，未自动拆解（可在看板手动拆解）"
                    .to_string(),
                parent_id: None,
                ctype: CommentType::System,
            });
            return Ok(issue);
        }
    };
    let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
    let env = std::sync::Arc::new(PlanChainEnv {
        store: store.clone(),
        cluster: ctx.state.cluster.clone(),
        agent_loop,
        issue: issue.clone(),
        actor: actor.clone(),
        home: ctx.home.clone().map(std::path::PathBuf::from),
        hub: Some(ctx.state.event_hub.clone()),
    });
    let cfg = plan_chain_cfg(env.home.as_deref());
    spawn_plan_chain(env, plan_id, 1, cfg);
    Ok(issue)
}

/// D2 内核：给刚建的 autopilot 单触发 plan 链（后台 spawn，不阻塞触发方）。
/// `auto_plan=None`（入口无运行时上下文）→ 返回 skipped 说明；槽空 →
/// WARN + 系统评论（moderator 未装配是可恢复状态——agent 重启间隙 cron
/// 恰好到点不该炸，下一轮规则触发自然恢复）。
#[cfg(feature = "cluster")]
fn fire_autopilot_plan_chain(
    store: &Arc<BoardStore>,
    issue_id: i64,
    actor: &Actor,
    auto_plan: Option<&AutoPlanContext>,
) -> serde_json::Value {
    use nemesis_board::models::{CommentType, NewComment};
    let Some(ap_ctx) = auto_plan else {
        return serde_json::json!({
            "status": "skipped",
            "reason": "auto_plan=true 但本入口无 moderator/事件上下文（CLI 手跑），未触发拆解",
        });
    };
    let Some(agent_loop) = ap_ctx.moderator_slot.get().cloned() else {
        tracing::warn!(
            "[Board] autopilot auto_plan issue={issue_id}：moderator loop 未就绪（agent 未装配完成），本轮跳过拆解",
        );
        let _ = store.add_comment(NewComment {
            issue_id,
            author: Actor::system("board"),
            content: "🤖 autopilot auto_plan 已开启，但 moderator agent 未就绪，本轮未自动拆解（下轮触发自动恢复）".to_string(),
            parent_id: None,
            ctype: CommentType::System,
        });
        return serde_json::json!({
            "status": "skipped",
            "reason": "moderator loop 未就绪",
        });
    };
    let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
    let issue_for_chain = match store.get_issue(issue_id) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("[Board] autopilot auto_plan issue={issue_id}：读取失败 {e}");
            return serde_json::json!({ "status": "failed", "reason": e });
        }
    };
    let env = std::sync::Arc::new(PlanChainEnv {
        store: store.clone(),
        cluster: ap_ctx.cluster.clone(), // 匹配器选节点靠它；None=派发诚实降级
        agent_loop,
        issue: issue_for_chain,
        actor: actor.clone(),
        home: Some(ap_ctx.home.clone()),
        hub: ap_ctx.hub.clone(),
    });
    let cfg = plan_chain_cfg(env.home.as_deref());
    spawn_plan_chain(env, plan_id.clone(), 1, cfg);
    serde_json::json!({ "status": "planning", "plan_id": plan_id })
}

/// 非 cluster 编译：autopilot 只支持 target 为空的周期建单。auto_plan=true
/// 的规则诚实降级：系统评论注明拆解未触发（需要集群编译），不静默吞。
#[cfg(not(feature = "cluster"))]
pub fn fire_autopilot(
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
    actor: &Actor,
) -> Result<serde_json::Value, String> {
    use nemesis_board::models::{CommentType, NewComment};
    if !ap.target.trim().is_empty() {
        return Err(format!(
            "autopilot「{}」配置了派发目标，但 cluster feature 未编译，无法派发",
            ap.name
        ));
    }
    // L23：父会话裁决（绑定且父会话消失 → 已自动暂停 + Err，不建单）。
    let parent_block = resolve_autopilot_parent(store, ap)?;
    let mut new_issue = autopilot_new_issue(ap, actor);
    if let Some(block) = parent_block {
        new_issue = inject_parent_context(new_issue, block);
    }
    let issue = store.create_issue(new_issue)?;
    let auto_plan_skipped = ap.auto_plan;
    if auto_plan_skipped {
        let _ = store.add_comment(NewComment {
            issue_id: issue.id,
            author: Actor::system("board"),
            content: "🤖 autopilot auto_plan 已开启，但本节点未编译集群支持，无法自动拆解"
                .to_string(),
            parent_id: None,
            ctype: CommentType::System,
        });
    }
    store.mark_autopilot_run(ap.id)?;
    Ok(serde_json::json!({
        "ran": true,
        "issue_id": issue.id,
        "issue_number": issue.number,
        "dispatch": Option::<serde_json::Value>::None,
        "auto_plan": if auto_plan_skipped {
            serde_json::json!({
                "status": "skipped",
                "reason": "cluster feature 未编译",
            })
        } else {
            serde_json::json!(null)
        },
    }))
}

/// autopilot 的 cron job 即时登记/跟随更新（W2 P4）。cron 服务未注入
/// （单测/极简构建）时跳过——gateway 启动同步会以 store 为真相源补注册。
/// 已登记过（cron_job_id 命中且 job 还在）→ patch 跟随 store；否则新登记
/// 并回存 job id（`board-ap:{autopilot_id}` 名字约定，启动同步据此清孤儿）。
pub(super) fn arm_autopilot_job(
    ctx: &RequestContext,
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
) -> Result<(), String> {
    let Some(cron) = ctx.state.cron.as_ref() else {
        return Ok(());
    };
    arm_autopilot_job_with(cron, store, ap)
}

/// autopilot cron 表达式校验单点（语法 + 语义，BUG-20261025-18）：
/// `validate_schedule` 只验解析；解析通过但永不匹配的表达式（如 `0 0 30 2 *`
/// ——2 月没有 30 日）会算不出任何下一次触发，若放行挂载就是 enabled 恒不
/// 触发的静默僵尸——BUG-20261024-11 的不变量在 autopilot 面同样生效。
/// `enabled=false` 只验语法（W1b：停用规则不排程，启用时重算再拦）。
/// WSAPI create/update、CLI create/update 与挂载防线共用这一份。
pub fn validate_autopilot_schedule(expr: &str, enabled: bool) -> Result<(), String> {
    nemesis_cron::CronService::validate_schedule(expr)
        .map_err(|e| format!("cron 表达式无效: {e}"))?;
    if enabled && !nemesis_cron::cron_expr_can_fire(expr, chrono::Local::now().timestamp_millis()) {
        return Err(format!(
            "cron 表达式 `{expr}` 解析通过但永不匹配（算不出任何下一次触发），\
             启用状态挂载即静默僵尸，拒绝——请检查日/月字段组合（如 2 月 30 日）"
        ));
    }
    Ok(())
}

/// arm 的核心（不依赖 RequestContext）：gateway 启动同步
/// （`sync_autopilot_jobs`）与 WSAPI handler 共用这一份构造逻辑——
/// 单一真相源，别在 gateway 里复制 CronSchedule/patch 语义。
fn arm_autopilot_job_with(
    cron: &Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
) -> Result<(), String> {
    let schedule = nemesis_cron::CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some(ap.cron.clone()),
        tz: None,
    };
    // 挂载防线（BUG-20261025-18）：入口校验之外的第二道——存量库里的
    // 死排程规则（手工改库/旧版本落库）在同步/挂载时 loud 拒绝，而不是
    // 经 add_job_internal（绕过 add_job_ext 创建闸）挂成静默僵尸。
    if ap.enabled
        && !nemesis_cron::cron_expr_can_fire(&ap.cron, chrono::Local::now().timestamp_millis())
    {
        return Err(format!(
            "autopilot #{} 启用中但 cron `{}` 永不匹配（死排程），拒绝挂载",
            ap.id, ap.cron
        ));
    }
    let svc = cron
        .lock()
        .map_err(|_| "cron service lock poisoned".to_string())?;
    if let Some(job_id) = ap.cron_job_id.as_deref()
        && svc.get_job(job_id).is_some()
    {
        svc.patch_job(
            job_id,
            &nemesis_cron::CronJobPatch {
                schedule: Some(schedule),
                enabled: Some(ap.enabled),
                ..Default::default()
            },
        )?;
        return Ok(());
    }
    // add_job_ext 返回随机 id（不支持指定 id），注册后回存映射。
    // add_job_internal：`board-ap:{id}` 是保留命名空间（用户面创建被
    // nemesis-cron loud 拒绝），内部装配走旁路。
    let job = svc.add_job_internal(
        &format!("board-ap:{}", ap.id),
        schedule,
        "",
        false,
        None,
        None,
        None,
        None,
        ap.enabled,
    )?;
    store.set_autopilot_cron_job(ap.id, Some(&job.id))
}

/// autopilot 删除时顺手摘掉 cron job（best-effort：失败只记日志，不阻断
/// 删除——启动同步的孤儿清理兜底）。
pub(super) fn disarm_autopilot_job(ctx: &RequestContext, ap: &nemesis_board::Autopilot) {
    if let (Some(cron), Some(job_id)) = (ctx.state.cron.as_ref(), ap.cron_job_id.as_deref()) {
        match cron.lock() {
            Ok(svc) => match svc.remove_job(job_id) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!("[Board] autopilot cron job not found: {job_id}");
                }
                Err(e) => {
                    tracing::warn!("[Board] autopilot cron job {job_id} remove failed: {e}");
                }
            },
            Err(_) => tracing::warn!(
                "[Board] cron service lock poisoned; autopilot job {job_id} left behind"
            ),
        }
    }
}

/// W2 P4: autopilot cron job 启动同步（gateway 在 cron.start 前调用）。
/// store 是唯一真相源，把 cron 服务对齐到 store：
///   1. 删孤儿：cron 里 `board-ap:*` 命名空间的 job 在 store 侧已无对应
///      登记（规则被删/改时 backfill 失败留下的幽灵 job）→ 移除，防到点
///      触发；只动 `board-ap:{i64}` 形态的作业（BUG-20261024-10 起按全
///      形态判定，非整数形态撞名不误删），不碰用户自己的 cron job。
///   2. 补登记：规则缺 job（上次登记时 cron 未注入、进程崩溃丢内存态）→
///      走 arm 重新登记并回存 job id。
///   3. 跟随：已登记的 job schedule/enabled 跟随 store（与 arm 同一逻辑，
///      幂等）。
///      返回重新登记的规则数（gateway 记日志用）。
pub fn sync_autopilot_jobs(
    cron: &Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    store: &Arc<BoardStore>,
) -> Result<usize, String> {
    let aps = store.list_autopilots()?;
    let valid_job_ids: std::collections::HashSet<&str> = aps
        .iter()
        .filter_map(|a| a.cron_job_id.as_deref())
        .collect();
    // 1) 孤儿清理。锁只在扫描段持有，remove 后即释放，不与 2)3) 嵌套。
    {
        let svc = cron
            .lock()
            .map_err(|_| "cron service lock poisoned".to_string())?;
        for job in svc.list_jobs(true) {
            // 孤儿判据：名字是 autopilot 作业形态（`board-ap:{i64}` 后缀可
            // 解析，与 Autopilot.id 同型）且 id 不在任何规则的注册映射里。
            // BUG-20261024-10：旧判据只看前缀——用户撞名作业（随机 id）会被
            // 当孤儿静默删除；保留命名空间拒绝创建后新撞名不可能，这里再按
            // 形态收紧，存量非整数形态撞名作业不再被清理误伤。
            if autopilot_id_from_job_name(&job.name).is_some()
                && !valid_job_ids.contains(job.id.as_str())
            {
                tracing::info!(
                    "[Board] autopilot sync: removing orphan cron job {} ({})",
                    job.id,
                    job.name
                );
                svc.remove_job(&job.id)
                    .map_err(|e| {
                        tracing::warn!("[Board] orphan cron job {} remove failed: {e}", job.id)
                    })
                    .ok();
            }
        }
    }
    // 2)+3) 逐条 arm（内部自判 patch 跟随 or 新登记；均幂等）。
    let mut rearmed = 0;
    for ap in &aps {
        let registered = match ap.cron_job_id.as_deref() {
            Some(jid) => {
                let svc = cron
                    .lock()
                    .map_err(|_| "cron service lock poisoned".to_string())?;
                svc.get_job(jid).is_some()
            }
            None => false,
        };
        // per-rule 隔离（BUG-20261025-18）：单条规则挂载失败（如死排程被
        // 防线拒绝）只 warn 跳过——启动同步是批量兜底面，直接 `?` 会因一
        // 条坏规则中断整批，其余好规则全部漏挂。
        if let Err(e) = arm_autopilot_job_with(cron, store, ap) {
            tracing::warn!(
                "[Board] autopilot sync: 规则 #{} ({}) 挂载失败，跳过: {e}",
                ap.id,
                ap.name
            );
            continue;
        }
        if !registered {
            rearmed += 1;
        }
    }
    Ok(rearmed)
}

/// 解析 autopilot cron 作业名 `board-ap:{i64}` → 规则 id（None = 非该形态）。
/// 名字形态的单一真相源：孤儿清理（[`sync_autopilot_jobs`]）与分派解析
/// （nemesisbot `resolve_autopilot_job`）都按这里判定，两侧口径不分叉。
pub fn autopilot_id_from_job_name(job_name: &str) -> Option<i64> {
    job_name.strip_prefix("board-ap:")?.parse().ok()
}
