//! state + store 单测（roundtrip / id 分配 / 坏文件容错 / 环形截断）。

use crate::state::{LoopState, LoopStatus, PROGRESS_LOG_CAP};
use crate::store::{alloc_id, list_loops, load_loop, loops_dir, remove_loop, save_loop};
use std::path::PathBuf;

fn st(id: &str, session: &str) -> LoopState {
    LoopState::new(
        id.to_string(),
        "测试目标".to_string(),
        session.to_string(),
        1000,
    )
}

#[test]
fn new_state_defaults_active_and_unscheduled() {
    let s = st("L1", "agent:main:session:a");
    assert_eq!(s.status, LoopStatus::Active);
    assert_eq!(s.next_wake_ms, None);
    assert_eq!(s.cron, None);
    assert_eq!(s.spent.turns, 0);
    assert!(s.finish_summary.is_none());
}

#[test]
fn push_progress_caps_at_limit_dropping_oldest() {
    let mut s = st("L1", "k");
    for i in 0..(PROGRESS_LOG_CAP + 20) {
        s.push_progress(i as i64, format!("p{i}"));
    }
    assert_eq!(s.progress_log.len(), PROGRESS_LOG_CAP);
    // 最旧 20 条被丢：首条是 p20。
    assert_eq!(s.progress_log[0].text, "p20");
    assert_eq!(
        s.progress_log.last().unwrap().text,
        format!("p{}", PROGRESS_LOG_CAP + 19)
    );
}

#[test]
fn progress_tail_returns_last_n_in_order() {
    let mut s = st("L1", "k");
    for i in 0..5 {
        s.push_progress(i, format!("p{i}"));
    }
    // 原序（旧→新）取尾部 2 条——与完整日志同向，时间线自然阅读。
    let tail: Vec<String> = s
        .progress_tail(2)
        .into_iter()
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(tail, vec!["p3", "p4"]);
}

#[test]
fn save_load_roundtrip_preserves_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = st("L1", "agent:main:session:a");
    s.push_progress(100, "第一步完成");
    s.next_wake_ms = Some(9999);
    s.cron = Some("0 9 * * *".to_string());
    s.plan = "先做A再做B".to_string();
    save_loop(dir.path(), &s).unwrap();
    let back = load_loop(dir.path(), "L1").unwrap().unwrap();
    assert_eq!(back.goal, s.goal);
    assert_eq!(back.status, LoopStatus::Active);
    assert_eq!(back.progress_log.len(), 1);
    assert_eq!(back.next_wake_ms, Some(9999));
    assert_eq!(back.cron.as_deref(), Some("0 9 * * *"));
    assert_eq!(back.plan, "先做A再做B");
    // 缺文件 = None 非错误。
    assert!(load_loop(dir.path(), "L999").unwrap().is_none());
}

#[test]
fn list_loops_sorted_and_skips_corrupt_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = st("L1", "k1");
    a.goal = "AAA".to_string();
    let mut c = st("L3", "k3");
    c.goal = "CCC".to_string();
    save_loop(dir.path(), &a).unwrap();
    save_loop(dir.path(), &c).unwrap();
    // 坏文件混入（半写）。
    std::fs::write(dir.path().join("L2.json"), "{broken").unwrap();
    let all = list_loops(dir.path());
    assert_eq!(all.len(), 2, "坏文件必须跳过不炸整表");
    assert_eq!(all[0].id, "L1");
    assert_eq!(all[1].id, "L3");
}

#[test]
fn alloc_id_monotonic_and_ignores_non_loop_files() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(alloc_id(dir.path()), "L1");
    let mut s = st("L7", "k");
    s.goal = "g".to_string();
    save_loop(dir.path(), &s).unwrap();
    // 非 loop 形态文件（events.json 之类同名目录混入物）不参与 max。
    std::fs::write(dir.path().join("junk.json"), "{}").unwrap();
    assert_eq!(alloc_id(dir.path()), "L8");
}

#[test]
fn remove_loop_reports_existence() {
    let dir = tempfile::tempdir().unwrap();
    let s = st("L1", "k");
    save_loop(dir.path(), &s).unwrap();
    assert!(remove_loop(dir.path(), "L1").unwrap());
    assert!(!remove_loop(dir.path(), "L1").unwrap());
}

#[test]
fn loops_dir_layout() {
    assert_eq!(
        loops_dir(&PathBuf::from("/ws")),
        PathBuf::from("/ws").join("loops")
    );
}

#[test]
fn status_terminal_and_str_roundtrip() {
    assert!(LoopStatus::Done.is_terminal());
    assert!(LoopStatus::Failed.is_terminal());
    assert!(!LoopStatus::Active.is_terminal());
    assert!(!LoopStatus::Paused.is_terminal());
    assert!(!LoopStatus::Parked.is_terminal());
    for (s, name) in [
        (LoopStatus::Active, "active"),
        (LoopStatus::Paused, "paused"),
        (LoopStatus::Parked, "parked"),
        (LoopStatus::Done, "done"),
        (LoopStatus::Failed, "failed"),
    ] {
        assert_eq!(s.as_str(), name);
    }
}
