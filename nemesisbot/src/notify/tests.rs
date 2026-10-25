//! notify sweep 可投递性裁决测试（外置：`#[cfg(test)] mod tests;` 约定）。
//!
//! BUG-20261025-48 回归锁：sweep 重投前必须复查交付状态——义务已终或交付
//! 已拒绝的箱行清箱停车，拒绝语义不被投递回泼。

use nemesis_taskstore::deliveries::DeliveryRecord;

use super::{SweepAction, sweep_deliverability};

fn rec(status: &str, notify_state: &str) -> Result<Option<DeliveryRecord>, String> {
    Ok(Some(DeliveryRecord {
        id: "d1".into(),
        run_id: Some("run-1".into()),
        kind: "result".into(),
        idempotency_key: "run-1:branch".into(),
        status: status.into(),
        payload: serde_json::json!({}),
        created_at: "2026-10-25T00:00:00+08:00".into(),
        delivered_at: None,
        confirmed_at: None,
        rejected_at: None,
        reject_reason: None,
        notify_state: notify_state.into(),
        notify_attempts: 1,
        notify_last_error: Some("send failed".into()),
        notify_next_retry_at: Some("2026-10-25T00:00:15+08:00".into()),
    }))
}

#[test]
fn r48_pending_recorded_dispatches() {
    assert!(matches!(
        sweep_deliverability(&rec("recorded", "pending")),
        SweepAction::Dispatch
    ));
}

#[test]
fn r48_rejected_delivery_drops_row_even_when_notify_pending() {
    // 核心缺陷形态：reject 终止交付，notify_state 仍留 pending（reject 不改
    // 义务段位）——裁决必须看 status，拒绝的交付内容不得重投。
    assert!(matches!(
        sweep_deliverability(&rec("rejected", "pending")),
        SweepAction::DropRow
    ));
}

#[test]
fn r48_terminal_notify_states_drop_row() {
    for state in ["channel_accepted", "confirmed", "none", "legacy", "dead"] {
        assert!(
            matches!(
                sweep_deliverability(&rec("recorded", state)),
                SweepAction::DropRow
            ),
            "notify_state={state} must not re-dispatch"
        );
    }
}

#[test]
fn r48_missing_delivery_drops_row_and_lookup_error_skips_tick() {
    assert!(matches!(
        sweep_deliverability(&Ok(None)),
        SweepAction::DropRow
    ));
    assert!(matches!(
        sweep_deliverability(&Err("db busy".to_string())),
        SweepAction::SkipTick
    ));
}
