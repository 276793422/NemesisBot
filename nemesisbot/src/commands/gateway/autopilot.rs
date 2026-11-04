// ---------------------------------------------------------------------------
// W2 P4: board autopilot cron 触发 + 派发超时 sweep
// ---------------------------------------------------------------------------

// 唯一消费者 sweep_dispatch_timeouts 为 all(board,cluster) 门——随门收放，
// 免 board-only 裁剪构建 unused import。
#[cfg(all(feature = "board", feature = "cluster"))]
use tracing::warn;

/// 解析 `board-ap:{id}` → (store, autopilot)；store 不可用或 id 非法时报错。
#[cfg(feature = "board")]
fn resolve_autopilot_job<'a>(
    job_name: &str,
    board_store: Option<&'a std::sync::Arc<nemesis_board::BoardStore>>,
) -> Result<
    (
        &'a std::sync::Arc<nemesis_board::BoardStore>,
        nemesis_board::Autopilot,
    ),
    String,
> {
    let ap_id: i64 = nemesis_web::handlers::board::autopilot_id_from_job_name(job_name)
        .ok_or_else(|| format!("autopilot job 名无法解析: {job_name}"))?;
    let store = board_store.ok_or("board service not available (autopilot)")?;
    let ap = store
        .get_autopilot(ap_id)
        .map_err(|e| format!("autopilot #{ap_id} 加载失败: {e}"))?;
    Ok((store, ap))
}

/// cron on_job 的 board autopilot 分支（job 名 `board-ap:{id}`）：按规则
/// 模板建单（target 非空时派发）并落 last_run_at；返回值进 job run 历史。
/// disabled 规则到点跳过（启停是用户意图，不算故障）。
/// 全自动流转 D2：auto_plan 规则经 moderator 槽自动拆解（槽晚填 OnceLock；
/// hub 传 None——cron 装配早于 web server，诚实降级无 SSE 推送）。
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) fn fire_board_autopilot(
    job_name: &str,
    board_store: Option<&std::sync::Arc<nemesis_board::BoardStore>>,
    cluster: Option<&std::sync::Arc<nemesis_cluster::cluster::Cluster>>,
    auto_plan: Option<&nemesis_web::handlers::board::AutoPlanContext>,
) -> Result<String, String> {
    let (store, ap) = resolve_autopilot_job(job_name, board_store)?;
    if !ap.enabled {
        return Ok(format!("autopilot「{}」已停用，跳过", ap.name));
    }
    let out = nemesis_web::handlers::board::fire_autopilot(
        store,
        cluster,
        &ap,
        &nemesis_board::Actor::system("autopilot"),
        auto_plan,
    )
    .map_err(|e| format!("autopilot「{}」触发失败: {e}", ap.name))?;
    Ok(format!("autopilot「{}」已触发: {out}", ap.name))
}

/// 非 cluster 编译变体：只有本地建单能力（target 非空的规则由 fire_autopilot
/// 建单前拒绝，语义与 cluster 版一致）。
#[cfg(all(feature = "board", not(feature = "cluster")))]
pub(crate) fn fire_board_autopilot(
    job_name: &str,
    board_store: Option<&std::sync::Arc<nemesis_board::BoardStore>>,
) -> Result<String, String> {
    let (store, ap) = resolve_autopilot_job(job_name, board_store)?;
    if !ap.enabled {
        return Ok(format!("autopilot「{}」已停用，跳过", ap.name));
    }
    let out = nemesis_web::handlers::board::fire_autopilot(
        store,
        &ap,
        &nemesis_board::Actor::system("autopilot"),
    )
    .map_err(|e| format!("autopilot「{}」触发失败: {e}", ap.name))?;
    Ok(format!("autopilot「{}」已触发: {out}", ap.name))
}

/// 一轮派发超时清扫（W2 P4-①②，board 派发无人回报时的兜底）。逐条
/// dispatched 记录判定：
///   ① 派发超过 `timeout_secs` 未回报 → 超时失败；
///   ② worker 在注册表且明确离线、且距派发 ≥ 离线宽限期（600s，容忍抖动）
///      → 离线失败。peer 缺失不判——未发现的 worker 可能只是还没上线，
///      保守等超时。
/// `fail_dispatch_checked` 是竞态闸（state IN dispatched/running + generation
/// 等值，AOR 批三 5.2 双闸代际扩展）：赢者补 ⛔ 系统评论 + dispatch_failed
/// 站内通知；输者（worker 恰好回报/代际已变）不动，下一轮自然不再列出。
///
/// M5-2（2026-10-10 机制加固）：assisted 模式下清扫失败的 issue **回流验
/// 收决策链**——推进 in_progress → in_review 并把 issue_id 返回给调用方
/// spawn 标准评审 agent 定案。
///
/// AOR 批三 5.2（sweep 策略化）：`autonomous=true` 时失败判定不再一律落
/// 终态+回流，改走监察环自治链：
///   - 监察环预算（独立于 unlimited_mode 的硬防线）：派发轮次（派发行数
///     − 1）≥ `max_redispatch`（0=关自动重派）→ 🛑 冻结停车（评论+通知+
///     审计漏斗），**不回流评审链**（autonomous 前置 unlimited=true，评审
///     重派环会绕过本硬防线）；
///   - 冻结类失败分类（unknown_side_effect / token_budget）→ 🛑 冻结停车
///     （同形态）；
///   - 离线触发 / 环境类·能力类分类 → 换节点重派（加权 ranked 排除失败
///     worker），经 `dispatch_issue_core` 单一入口；
///   - 超时且瞬态类/无分类 → 同节点重派（现状预算逻辑语义）；
///   - 重派成功 → 🔁 审计评论 + record_auto_decide 单一审计漏斗，不回流
///     （等 worker 正常写回走标准评审）；无候选/重派失败 → 回落 assisted
///     骨架（回流评审链定夺）。
///
/// estop 不变量：急停生效中不做任何自治动作（整体回落 assisted）。
/// 返回值 = 需 spawn 评审的 issue_id 列表（assisted 路径 + 自治回落路径）。
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) fn sweep_dispatch_timeouts(
    deps: &crate::board_review::BoardReviewDeps,
    timeout_secs: u64,
    autonomous: bool,
    max_redispatch: u32,
) -> Vec<i64> {
    use crate::board_review::{
        FailClassPolicy, latest_fail_class, pick_sweep_switch_target, redispatch_policy,
    };

    /// 竞态赢者落定三件套（assisted 与自治共用的单一真相源）：G3 账本收账
    /// + ⛔ System 评论 + dispatch_failed 站内通知。
    fn sweep_fail_settle(
        store: &std::sync::Arc<nemesis_board::BoardStore>,
        rec: &nemesis_board::models::DispatchRecord,
        reason: &str,
    ) {
        nemesis_taskstore::ledger::settle_run(
            &nemesis_taskstore::ledger::board_dispatch_run_id(&rec.task_id),
            "failed",
            Some(reason),
        );
        let _ = store.add_comment(nemesis_board::NewComment {
            issue_id: rec.issue_id,
            author: nemesis_board::Actor::system("board"),
            content: format!(
                "⛔ {reason}，派发已标记失败（task {}）。可重新派发或取消任务。",
                rec.task_id
            ),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        if let Err(e) = store.notify_dispatch_event(
            rec.issue_id,
            nemesis_board::notification_kind::DISPATCH_FAILED,
            reason,
        ) {
            warn!(
                "[Board][Sweep] notify dispatch_failed (task {}): {e}",
                rec.task_id
            );
        }
        warn!(
            "[Board][Sweep] dispatch failed: task={} issue={} ({reason})",
            rec.task_id, rec.issue_id
        );
    }

    /// M5-2 回流（单一真相源）：in_progress → in_review；`true` = 调用方把
    /// issue_id push 进返回列表（spawn 评审）。非 in_progress（人工已动）
    /// 让位不回流；转移失败只 WARN。
    fn sweep_reflow_to_review(
        store: &std::sync::Arc<nemesis_board::BoardStore>,
        issue_id: i64,
    ) -> bool {
        match store.get_issue(issue_id) {
            Ok(issue) if issue.status == nemesis_board::IssueStatus::InProgress => {
                match store.transition_issue(
                    issue_id,
                    nemesis_board::IssueStatus::InReview,
                    &nemesis_board::Actor::system("board-sweep"),
                ) {
                    Ok(_) => true,
                    Err(e) => {
                        warn!(
                            "[Board][Sweep] issue {issue_id} 回流 in_review 失败（留人工处置）：{e}"
                        );
                        false
                    }
                }
            }
            Ok(_) => false, /* 非 in_progress（人工已动）→ 让位，不回流 */
            Err(e) => {
                warn!("[Board][Sweep] issue {issue_id} 状态读取失败：{e}");
                false
            }
        }
    }

    /// 自治冻结停车（AOR 批三 5.2）：🛑 System 评论 + dispatch_failed 通知 +
    /// in_review 冻结（**不回流评审链**——autonomous 前置 unlimited=true，
    /// 评审重派环会绕过监察环硬防线）+ record_auto_decide 审计漏斗留痕。
    fn sweep_freeze_park(
        deps: &crate::board_review::BoardReviewDeps,
        rec: &nemesis_board::models::DispatchRecord,
        reason: &str,
        note: &str,
    ) {
        let store = &deps.store;
        let _ = store.add_comment(nemesis_board::NewComment {
            issue_id: rec.issue_id,
            author: nemesis_board::Actor::system("board-sweep"),
            content: format!(
                "🛑 {reason}；{note}（task {}）。冻结转人工处置。",
                rec.task_id
            ),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        if let Err(e) = store.notify_dispatch_event(
            rec.issue_id,
            nemesis_board::notification_kind::DISPATCH_FAILED,
            &format!("{reason}；{note}"),
        ) {
            warn!("[Board][Sweep] notify freeze (task {}): {e}", rec.task_id);
        }
        let _ = sweep_reflow_to_review(store, rec.issue_id);
        crate::board_review::record_auto_decide(
            deps,
            rec.issue_id,
            "escalate_human",
            "fail",
            serde_json::json!({
                "source": "sweep",
                "reason": reason,
                "note": note,
                "task_id": rec.task_id,
            }),
        );
        warn!(
            "[Board][Sweep] issue {} 自治冻结停车：{reason}；{note}",
            rec.issue_id
        );
        // AOR 批三 5.3 触发点③：自治冻结停车 → 重规划起草（建议≠授权；
        // 本函数是同步 fn，spawn_replan_draft 内部有 runtime 在场闸——
        // 无 runtime 的直调方诚实跳过）。
        crate::board_replan::spawn_replan_draft(deps.clone(), rec.issue_id, "sweep_freeze");
    }

    const OFFLINE_GRACE_SECS: u64 = 600;
    let store = &deps.store;
    let cluster = &deps.cluster;
    let records = match store.list_active_dispatches() {
        Ok(r) => r,
        Err(e) => {
            warn!("[Board][Sweep] list active dispatches failed: {e}");
            return Vec::new();
        }
    };
    if records.is_empty() {
        return Vec::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // estop 不变量：急停生效中不做任何自治动作——整体回落 assisted 骨架
    // （评审链自会按 estop 停车语义处置）。
    let autonomous = autonomous && !deps.estop.is_engaged();
    let mut for_review: Vec<i64> = Vec::new();
    for record in records {
        let age = now.saturating_sub(record.dispatched_at.max(0) as u64);
        let offline = cluster
            .get_peer(&record.worker_id)
            .map(|p| !p.is_online())
            .unwrap_or(false);
        let timed_out = age >= timeout_secs;
        let offline_expired = offline && age >= OFFLINE_GRACE_SECS;
        if !timed_out && !offline_expired {
            continue;
        }
        // 离线优先定性（两者同真时离线语义更强：天然换节点类）。
        let offline_triggered = offline_expired;
        let reason = if offline_expired {
            format!("worker 离线（{age}s 无回报且节点已离线）")
        } else {
            format!("派发超时（{timeout_secs}s 无回报）")
        };
        // 竞态闸（双闸之一，批三 5.2 代际扩展）：赢者才能收尾/自治。
        let rec = match store.fail_dispatch_checked(&record.task_id, &reason, record.generation) {
            Ok(Some(rec)) => Some(rec),
            Ok(None) => None, /* 输竞态（worker 恰好回报/代际已变），下轮不再列出 */
            Err(e) => {
                warn!(
                    "[Board][Sweep] fail_dispatch_checked (task {}): {e}",
                    record.task_id
                );
                None
            }
        };
        let Some(rec) = rec else { continue };
        sweep_fail_settle(store, &rec, &reason);
        if !autonomous {
            // assisted（现状行为，零回归）：回流验收决策链。
            if sweep_reflow_to_review(store, rec.issue_id) {
                for_review.push(rec.issue_id);
            }
            continue;
        }

        // ---- 自治臂（AOR 批三 5.2）----
        // 监察环预算（独立于 unlimited_mode 的硬防线）：派发轮次 = 派发行数 − 1。
        let rounds = store
            .list_dispatches(rec.issue_id)
            .map(|v| v.len())
            .unwrap_or(1)
            .saturating_sub(1) as u64;
        if max_redispatch == 0 || rounds >= u64::from(max_redispatch) {
            sweep_freeze_park(
                deps,
                &rec,
                &reason,
                &format!("监察环重派预算耗尽（已 {rounds} 轮 ≥ 上限 {max_redispatch}）"),
            );
            continue;
        }
        // 失败分类 → 冻结类（unknown_side_effect / token_budget）不自动重派。
        let comments = store.list_comments(rec.issue_id).unwrap_or_default();
        let fc = latest_fail_class(&comments);
        if fc.map(redispatch_policy) == Some(FailClassPolicy::Freeze) {
            sweep_freeze_park(
                deps,
                &rec,
                &reason,
                &format!(
                    "失败分类 {} 判定冻结（副作用后果不明/预算语义），不自动重派",
                    fc.unwrap_or("?")
                ),
            );
            continue;
        }
        // 目标选取：离线触发 / 环境类·能力类分类 → 换节点（排除失败 worker）；
        // 瞬态类/无分类 → 同节点（现状预算逻辑语义）。
        let switch_first = offline_triggered
            || matches!(fc, Some(f) if redispatch_policy(f) != FailClassPolicy::TransientRetry);
        let issue = match store.get_issue(rec.issue_id) {
            Ok(i) => i,
            Err(e) => {
                warn!(
                    "[Board][Sweep] issue {} 读取失败（回落评审链）：{e}",
                    rec.issue_id
                );
                if sweep_reflow_to_review(store, rec.issue_id) {
                    for_review.push(rec.issue_id);
                }
                continue;
            }
        };
        let target = if switch_first {
            pick_sweep_switch_target(deps, &issue, &rec.worker_id)
        } else {
            Some(rec.worker_id.clone())
        };
        let Some(target) = target else {
            // 无可换节点：自治重派不可行 → 回落评审链（预算/同因熔断/转人工
            // 漏斗接管——「无可换节点转人工」语义由评审链兑现）。
            let _ = store.add_comment(nemesis_board::NewComment {
                issue_id: rec.issue_id,
                author: nemesis_board::Actor::system("board-sweep"),
                content: format!(
                    "🔁 {reason}；监察环无可用换节点候选（排除 {} 后空），回落验收评审链定夺（task {}）。",
                    rec.worker_id, rec.task_id
                ),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
            if sweep_reflow_to_review(store, rec.issue_id) {
                for_review.push(rec.issue_id);
            }
            continue;
        };
        // 自治重派走派发单一入口（EST-01/02 estop 闸 / F1 目标校验 / P1 拓扑
        // 闸全生效）；review_feedback 带失败原因（worker 侧可见）。
        match nemesis_web::handlers::board::dispatch_issue_core(
            store,
            Some(cluster),
            rec.issue_id,
            &target,
            &nemesis_board::Actor::system("board-sweep"),
            Some(&reason),
        ) {
            Ok(_) => {
                // 🔁 审计评论 + 单一审计漏斗（对齐评审重派臂：成功后留痕）。
                // 不回流评审链：重派已发车，等 worker 正常写回走标准评审。
                let _ = store.add_comment(nemesis_board::NewComment {
                    issue_id: rec.issue_id,
                    author: nemesis_board::Actor::system("board-sweep"),
                    content: format!(
                        "🔁 {reason}；监察环自动重派 → {target}（task {} 发起新派发）。",
                        rec.task_id
                    ),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
                crate::board_review::record_auto_decide(
                    deps,
                    rec.issue_id,
                    "redispatch",
                    "fail",
                    serde_json::json!({
                        "source": "sweep",
                        "trigger": if offline_triggered { "worker_offline" } else { "timeout" },
                        "target": target,
                        "task_id": rec.task_id,
                        "prior_rounds": rounds,
                        "fail_class": fc,
                    }),
                );
            }
            Err(e) => {
                // 重派失败：诚实留痕 + 回落评审链；estop 竞态窗口不回流
                // （评审链自会按 estop 停车，push 无行为差）。
                let _ = store.add_comment(nemesis_board::NewComment {
                    issue_id: rec.issue_id,
                    author: nemesis_board::Actor::system("board-sweep"),
                    content: format!(
                        "🔁 {reason}；监察环自动重派 → {target} 失败：{e}；回落验收评审链定夺（task {}）。",
                        rec.task_id
                    ),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
                let estop_now = deps.estop.is_engaged();
                if !estop_now && sweep_reflow_to_review(store, rec.issue_id) {
                    for_review.push(rec.issue_id);
                }
            }
        }
    }
    for_review
}

#[cfg(test)]
mod cov_tests;
