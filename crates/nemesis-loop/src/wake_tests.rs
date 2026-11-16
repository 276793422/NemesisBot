//! wake 纯逻辑单测：唤醒计算（delay/cron/互斥）、预算三维、sweep 裁决
//! （到期唤醒 / cron 重算 / estop 冻结 / 预算停车 / cron 失效停车）。

use crate::state::{LoopState, LoopStatus};
use crate::wake::{budget_exhausted, compute_next_wake, sweep_loop, SweepOutcome, WAKE_MARKER};
use chrono::Timelike;

fn active(goal: &str) -> LoopState {
    LoopState::new(
        "L1".to_string(),
        goal.to_string(),
        "agent:main:session:a".to_string(),
        0,
    )
}

// ── compute_next_wake ──────────────────────────────────────────────

#[test]
fn compute_delay_one_shot() {
    let (w, cron) = compute_next_wake(10_000, Some(30), None).unwrap();
    assert_eq!(w, Some(40_000));
    assert_eq!(cron, None);
}

#[test]
fn compute_delay_zero_rejected() {
    assert!(compute_next_wake(0, Some(0), None).is_err());
}

#[test]
fn compute_cron_local_semantics() {
    // 本地时区 "0 9 * * *"：从 2026-11-16 08:00 本地起算 → 下次 09:00 本地。
    let local = chrono::Local::now();
    let base = local
        .date_naive()
        .and_hms_opt(8, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .unwrap()
        .timestamp_millis();
    let (w, cron) = compute_next_wake(base, None, Some("0 9 * * *")).unwrap();
    assert_eq!(cron, Some("0 9 * * *".to_string()));
    let next = w.unwrap();
    assert!(next > base, "下次唤醒必须晚于基准");
    // 与基准同日 09:00（08:00 起算的下一次发生在当天）。
    let dt = chrono::DateTime::from_timestamp_millis(next)
        .unwrap()
        .with_timezone(&chrono::Local);
    assert_eq!(dt.hour(), 9);
    assert_eq!(dt.minute(), 0);
}

#[test]
fn compute_invalid_cron_rejected() {
    assert!(compute_next_wake(0, None, Some("not a cron")).is_err());
}

#[test]
fn compute_mutual_exclusion_and_requirement() {
    assert!(compute_next_wake(0, Some(1), Some("0 9 * * *")).is_err());
    assert!(compute_next_wake(0, None, None).is_err());
}

// ── budget_exhausted ───────────────────────────────────────────────

#[test]
fn budget_zero_means_unlimited() {
    let mut s = active("g");
    s.spent.turns = 10_000;
    s.spent.tokens = 10_000;
    assert_eq!(budget_exhausted(&s, 10_000_000), None);
}

#[test]
fn budget_turns_dimension() {
    let mut s = active("g");
    s.budget.max_turns_total = 3;
    s.spent.turns = 2;
    assert_eq!(budget_exhausted(&s, 1000), None);
    s.spent.turns = 3;
    assert_eq!(budget_exhausted(&s, 1000), Some("max_turns_total"));
}

#[test]
fn budget_wall_clock_from_started_at() {
    let mut s = active("g");
    s.budget.wall_clock_budget_secs = 100;
    // started_at=0；99999ms 未满 100s。
    assert_eq!(budget_exhausted(&s, 99_999), None);
    assert_eq!(
        budget_exhausted(&s, 100_000),
        Some("wall_clock_budget_secs")
    );
}

#[test]
fn budget_tokens_dimension() {
    let mut s = active("g");
    s.budget.max_tokens = 500;
    s.spent.tokens = 499;
    assert_eq!(budget_exhausted(&s, 0), None);
    s.spent.tokens = 500;
    assert_eq!(budget_exhausted(&s, 0), Some("max_tokens"));
}

// ── sweep_loop ─────────────────────────────────────────────────────

#[test]
fn sweep_wakes_when_due_one_shot_clears_schedule() {
    let mut s = active("LOOPBENCH:GOAL");
    s.next_wake_ms = Some(5000);
    let out = sweep_loop(&mut s, 5001, false).unwrap();
    let SweepOutcome::Wake { message } = out else {
        panic!("expected Wake");
    };
    assert!(message.contains(WAKE_MARKER));
    assert!(message.contains("L1"));
    assert!(message.contains("LOOPBENCH:GOAL"));
    assert!(message.contains("继续你的目标"));
    assert_eq!(s.spent.turns, 1);
    assert_eq!(s.next_wake_ms, None, "一次性排程唤醒后清空");
    // 未到期不醒。
    let mut s2 = active("g");
    s2.next_wake_ms = Some(5000);
    assert!(sweep_loop(&mut s2, 4999, false).is_none());
    // 无排程不醒。
    let mut s3 = active("g");
    assert!(sweep_loop(&mut s3, 999_999, false).is_none());
}

#[test]
fn sweep_wake_message_carries_progress_tail() {
    let mut s = active("g");
    s.push_progress(1, "完成步骤A");
    s.push_progress(2, "完成步骤B");
    s.next_wake_ms = Some(10);
    let SweepOutcome::Wake { message } = sweep_loop(&mut s, 11, false).unwrap() else {
        panic!();
    };
    assert!(message.contains("完成步骤A"));
    assert!(message.contains("完成步骤B"));
    assert!(message.contains("loop_wakeup"));
    assert!(message.contains("loop_finish"));
}

#[test]
fn sweep_cron_recomputes_next_wake() {
    let mut s = active("g");
    s.cron = Some("* * * * *".to_string()); // 每分钟
    s.next_wake_ms = Some(30_000);
    let out = sweep_loop(&mut s, 30_001, false).unwrap();
    assert!(matches!(out, SweepOutcome::Wake { .. }));
    assert_eq!(s.spent.turns, 1);
    assert!(s.next_wake_ms.is_some(), "cron 形态唤醒后必须重排下次");
    assert!(s.next_wake_ms.unwrap() > 30_001);
    assert_eq!(s.status, LoopStatus::Active);
}

#[test]
fn sweep_estop_freezes_everything() {
    let mut s = active("g");
    s.next_wake_ms = Some(1000);
    s.budget.max_turns_total = 1;
    s.spent.turns = 1; // 预算同样已耗尽
                       // 冻结期：到期唤醒与预算停车都零动作。
    assert!(sweep_loop(&mut s, 2000, true).is_none());
    assert_eq!(s.status, LoopStatus::Active);
    assert_eq!(s.spent.turns, 1, "冻结不偷改状态");
    // release 后：预算闸先于唤醒 → 停车。
    let out = sweep_loop(&mut s, 2001, false).unwrap();
    assert!(matches!(out, SweepOutcome::Park { .. }));
    assert_eq!(s.status, LoopStatus::Parked);
}

#[test]
fn sweep_parks_on_budget_before_wake() {
    let mut s = active("g");
    s.budget.max_turns_total = 1;
    s.spent.turns = 1;
    s.next_wake_ms = Some(1000);
    let out = sweep_loop(&mut s, 1001, false).unwrap();
    let SweepOutcome::Park { dim } = out else {
        panic!("expected Park");
    };
    assert_eq!(dim, "max_turns_total");
    assert_eq!(s.status, LoopStatus::Parked);
    assert_eq!(s.next_wake_ms, None);
    assert!(s.cron.is_none());
    let last = s.progress_log.last().unwrap();
    assert!(last.text.contains("[parked]"));
    assert!(last.text.contains("max_turns_total"));
}

#[test]
fn sweep_budget_park_applies_even_without_due_wake() {
    // 状态诚实优先：预算耗尽即使未排程也停车（下一个 sweep 必然收口）。
    let mut s = active("g");
    s.budget.wall_clock_budget_secs = 10;
    assert!(s.next_wake_ms.is_none());
    let out = sweep_loop(&mut s, 20_000, false).unwrap();
    assert!(matches!(out, SweepOutcome::Park { .. }));
}

#[test]
fn sweep_skips_non_active_status() {
    for status in [LoopStatus::Paused, LoopStatus::Parked, LoopStatus::Done] {
        let mut s = active("g");
        s.status = status;
        s.next_wake_ms = Some(1);
        assert!(
            sweep_loop(&mut s, 1000, false).is_none(),
            "{status:?} 不被唤醒"
        );
        assert_eq!(s.status, status);
    }
}

#[test]
fn sweep_parks_when_cron_became_invalid() {
    let mut s = active("g");
    s.cron = Some("garbage".to_string());
    s.next_wake_ms = Some(1000);
    let out = sweep_loop(&mut s, 1001, false).unwrap();
    let SweepOutcome::Park { dim } = out else {
        panic!();
    };
    assert_eq!(dim, "cron_invalid");
    assert_eq!(s.status, LoopStatus::Parked);
}

#[test]
fn wake_after_restart_picks_up_past_due() {
    // durable 语义（G5 同款单测面）：重启后 sweep 从盘上状态恢复——
    // next_wake 已过期照样醒（不要求新鲜排程）。
    let dir = tempfile::tempdir().unwrap();
    let mut s = active("重启续跑");
    s.next_wake_ms = Some(1000);
    crate::store::save_loop(dir.path(), &s).unwrap();
    let mut back = crate::store::load_loop(dir.path(), "L1").unwrap().unwrap();
    let SweepOutcome::Wake { message } = sweep_loop(&mut back, 999_999, false).unwrap() else {
        panic!();
    };
    assert!(message.contains("重启续跑"));
}
