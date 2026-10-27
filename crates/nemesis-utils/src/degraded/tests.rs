//! degraded 注册表测试（M2 批次一）。共享进程级单例——注册表是全局
//! static，cargo 默认并行跑测试会互踩，用模块级 TEST_LOCK 串行
//! （background_registry TEST_LOCK 同款纪律），每用例再 reset 清场。

use super::*;
use parking_lot::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn report_then_take_pending_drains_once() {
    let _g = TEST_LOCK.lock();
    reset_for_tests();
    report("chat_log", "open C:\\x: denied".to_string());
    let pending = take_pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, "chat_log");
    assert!(pending[0].detail.contains("denied"));
    assert_eq!(pending[0].count, 1);
    // 已取走：再次 take 为空。
    assert!(take_pending().is_empty());
    // 条目仍在快照里（/ready 面）。
    let snap = snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].count, 1);
}

#[test]
fn debounce_window_suppresses_repeat_alerts_but_counts() {
    let _g = TEST_LOCK.lock();
    reset_for_tests();
    report("turn_wal", "write fail a".to_string());
    // 去抖窗口内的后续失败：计数/detail 更新，但不置 pending。
    report("turn_wal", "write fail b".to_string());
    report("turn_wal", "write fail c".to_string());
    let pending = take_pending();
    assert_eq!(pending.len(), 1, "去抖窗口内只告警一次");
    assert_eq!(pending[0].count, 3, "计数照常累计");
    assert_eq!(pending[0].detail, "write fail c", "detail 反映最新现场");
    // 快照同样反映最新计数。
    assert_eq!(snapshot()[0].count, 3);
}

#[test]
fn kinds_are_independent_and_sorted() {
    let _g = TEST_LOCK.lock();
    reset_for_tests();
    report("turn_wal", "w".to_string());
    report("chat_log", "c".to_string());
    report("checkpoint_tree", "t".to_string());
    let pending = take_pending();
    let kinds: Vec<&str> = pending.iter().map(|d| d.kind.as_str()).collect();
    assert_eq!(kinds, vec!["chat_log", "checkpoint_tree", "turn_wal"]);
}

#[test]
fn timestamps_are_epoch_ordered() {
    let _g = TEST_LOCK.lock();
    reset_for_tests();
    report("chat_log", "x".to_string());
    let snap = snapshot();
    assert!(snap[0].first_at_ms <= snap[0].last_at_ms);
    assert!(snap[0].first_at_ms > 1_700_000_000_000, "epoch 毫秒量级");
}
