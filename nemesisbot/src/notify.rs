//! 项 C（2026-10-22）— 出站通知记账 + 失败重试 sweeper。
//!
//! 真相源 = tasks.db `deliveries.notify_state` + `notify_outbox` 表；
//! 本模块只做三件事：
//! - **记账钩子**（[`notify_accounting`]，接线进
//!   `ChannelManager::set_notify_hook`）：每次真实投递尝试后推进
//!   notify_state——Ok → `channel_accepted`（+ 清重试债）；Err → 失败入箱
//!   （守卫：仅 `pending` 义务行入箱）。
//! - **重试 sweeper**（[`sweep_once`] + [`spawn_sweeper`]）：到期行重投
//!   （复用 `dispatch_outbound`——钩子自动记账，无双重计数：UPSERT 按
//!   delivery_key 幂等）；死信闸（age 7 天 / attempts 1000，CD6 同款）。
//! - **惰性接线**（[`install_notify_db`] / [`NOTIFY_DB`]）：db 槽由
//!   post_agent 在交付账本装配点注入；sweeper 每 tick 惰性读槽，槽空 =
//!   no-op（headless/测试形态零开销）。
//!
//! 纪律：钩子/sweeper 内部错误只 warn 留痕，绝不上抛炸投递主链路
//! （记账是辅助台账；与 cluster.rs record_delivery 同款「留痕不阻断」）。

use std::sync::Arc;

use nemesis_channels::manager::ChannelManager;
use nemesis_taskstore::TaskDb;
use nemesis_taskstore::deliveries::{notify_backoff_secs, notify_dead_lettered};
use nemesis_types::channel::OutboundMessage;

/// 全局通知 db 槽（post_agent 装配时注入一次；sweeper 每 tick 惰性读）。
static NOTIFY_DB: std::sync::OnceLock<Arc<TaskDb>> = std::sync::OnceLock::new();

/// 注入通知账本 db（幂等：首装生效，重复注入 debug 留痕——agent 重启
/// 重建走同一 `Arc<TaskDb>`，值相同属预期）。
pub(crate) fn install_notify_db(db: Arc<TaskDb>) {
    if NOTIFY_DB.set(db).is_err() {
        tracing::debug!("[Notify] notify db already installed (same Arc expected)");
    }
}

/// 记账钩子体：消息携带 `meta.task_id`（只有集群续行最终回复携带）且
/// db 已接线才记账；其余消息零开销直通。
pub(crate) fn notify_accounting(
    msg: &OutboundMessage,
    result: &Result<(), nemesis_types::error::NemesisError>,
) {
    let Some(task_id) = msg.meta.task_id.as_deref() else {
        return;
    };
    let Some(db) = NOTIFY_DB.get() else {
        tracing::debug!("[Notify] db not installed, skip accounting for task {task_id}");
        return;
    };
    match result {
        Ok(()) => {
            // 通道接受：recorded → delivered + notify channel_accepted。
            // forward-only 闸让迟到接受（已 confirmed/rejected）自然 no-op。
            match db.mark_delivery_channel_accepted(task_id) {
                Ok(true) => {
                    tracing::debug!("[Notify] delivery accepted: {task_id}");
                }
                Ok(false) => {
                    tracing::debug!("[Notify] delivery accept no-op (forward-only): {task_id}");
                }
                Err(e) => {
                    tracing::warn!("[Notify] mark accepted failed for {task_id}: {e}");
                }
            }
            // 成功任意处清重试债（箱里无行 = 无操作）。
            if let Err(e) = db.notify_remove_by_delivery(task_id) {
                tracing::warn!("[Notify] clear retry debt failed for {task_id}: {e}");
            }
        }
        Err(e) => {
            // 失败入箱——先读守卫：仅 pending 义务行产生重试债（已接受/
            // 已确认/无义务/旧账不重试）。
            let state = match db.delivery_by_key(task_id) {
                Ok(Some(rec)) => Some(rec.notify_state),
                Ok(None) => None,
                Err(err) => {
                    tracing::warn!("[Notify] delivery lookup failed for {task_id}: {err}");
                    None
                }
            };
            if state.as_deref() != Some("pending") {
                tracing::debug!(
                    "[Notify] delivery {task_id} notify_state={state:?} not pending, skip retry enqueue"
                );
                return;
            }
            let message = serde_json::to_string(msg).unwrap_or_else(|_| String::new());
            if message.is_empty() {
                tracing::warn!(
                    "[Notify] outbound message serialize failed, drop retry for {task_id}"
                );
                return;
            }
            if let Err(err) = db.notify_enqueue_failure(task_id, &message, &e.to_string()) {
                tracing::warn!("[Notify] enqueue retry failed for {task_id}: {err}");
            }
        }
    }
}

/// 一轮到期重投。行数上限 16/tick（15s tick → 稳态 ≥1 条/秒的投递能力，
/// 死信/退避封顶后远超实际需求）。复用 `dispatch_outbound`——投递结果经
/// 钩子自动记账（成功清箱、失败 UPSERT 累计），sweeper 自身不重复计数。
pub(crate) async fn sweep_once(db: &Arc<TaskDb>, mgr: &Arc<ChannelManager>) {
    let now = chrono::Local::now().to_rfc3339();
    let due = match db.notify_due(&now, 16) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("[Notify] due query failed: {e}");
            return;
        }
    };
    for row in due {
        // 死信闸（先于投递）：age 7 天 / attempts 1000 → dead 停车。
        let age_secs = chrono::DateTime::parse_from_rfc3339(&row.created_at)
            .map(|t| chrono::Local::now().signed_duration_since(t).num_seconds())
            .unwrap_or(0);
        if notify_dead_lettered(age_secs, row.attempts) {
            tracing::warn!(
                delivery = %row.delivery_key,
                attempts = row.attempts,
                age_secs = age_secs,
                "[Notify] 重试转死信停车（age 7 天 / attempts 1000 死信闸；交付保持 pending 可见）"
            );
            if let Err(e) = db.mark_delivery_notify_dead(&row.delivery_key) {
                tracing::warn!("[Notify] mark dead failed for {}: {e}", row.delivery_key);
            }
            if let Err(e) = db.notify_remove(&row.id) {
                tracing::warn!("[Notify] dead row remove failed: {e}");
            }
            continue;
        }
        // 反序列化重投；损坏行诚实丢弃（payload 只出自本进程序列化，
        // 损坏 = 盘上异常，保留只会永久占位）。
        let msg: OutboundMessage = match serde_json::from_str(&row.message) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    delivery = %row.delivery_key,
                    "[Notify] outbox message corrupt, drop: {e}"
                );
                let _ = db.notify_remove(&row.id);
                continue;
            }
        };
        match mgr.dispatch_outbound(msg).await {
            // 成功：钩子已记账 + 清箱；防御性再清一次（幂等，钩子缺席
            // 场景兜底）。
            Ok(()) => {
                let _ = db.notify_remove(&row.id);
            }
            Err(e) => {
                // 失败：钩子已 UPSERT 累计（attempts+1、退避翻倍）；这里
                // 只留痕。attempts 达死信闸后下轮 sweep 前置闸收口。
                tracing::debug!(
                    delivery = %row.delivery_key,
                    attempts = row.attempts,
                    backoff_secs = notify_backoff_secs(row.attempts + 1),
                    "[Notify] retry dispatch failed, backoff: {e}"
                );
            }
        }
    }
}

/// spawn 重试 sweeper（15s tick；MissedTickBehavior::Delay——tick 慢不
/// 追赶堆积）。db 槽空 = tick no-op；进程退出随 runtime 一起终止。
pub(crate) fn spawn_sweeper(mgr: Arc<ChannelManager>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Some(db) = NOTIFY_DB.get() {
                sweep_once(db, &mgr).await;
            }
        }
    });
    tracing::info!("[Notify] delivery notify sweeper started (15s tick)");
}
