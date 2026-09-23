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
    let ap_id: i64 = job_name
        .strip_prefix("board-ap:")
        .and_then(|s| s.parse().ok())
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
/// `fail_dispatch` 是竞态闸（只认 dispatched/running 态）：赢者补 ⛔ 系统评论 +
/// dispatch_failed 站内通知；输者（worker 恰好回报）不动，下一轮自然不再
/// 列出。MVP 策略 = abort + notify + 手动重派，不自动 retry/reassign——
/// 同一 issue 双 worker 并发执行的风险大于自动化的收益（开发日志有记）。
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) fn sweep_dispatch_timeouts(
    store: &std::sync::Arc<nemesis_board::BoardStore>,
    cluster: &std::sync::Arc<nemesis_cluster::cluster::Cluster>,
    timeout_secs: u64,
) {
    const OFFLINE_GRACE_SECS: u64 = 600;
    let records = match store.list_active_dispatches() {
        Ok(r) => r,
        Err(e) => {
            warn!("[Board][Sweep] list active dispatches failed: {e}");
            return;
        }
    };
    if records.is_empty() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
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
        let reason = if timed_out {
            format!("派发超时（{timeout_secs}s 无回报）")
        } else {
            format!("worker 离线（{age}s 无回报且节点已离线）")
        };
        match store.fail_dispatch(&record.task_id, &reason) {
            Ok(Some(rec)) => {
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
                    &reason,
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
            Ok(None) => { /* 输竞态（worker 恰好回报），下轮不再列出 */ }
            Err(e) => warn!(
                "[Board][Sweep] fail_dispatch (task {}): {e}",
                record.task_id
            ),
        }
    }
}
