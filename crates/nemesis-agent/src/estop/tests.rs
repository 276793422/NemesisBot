use super::*;
use std::sync::Arc;

#[test]
fn starts_disengaged() {
    let e = EstopState::new();
    assert!(!e.is_engaged());
}

#[test]
fn trigger_release_roundtrip() {
    let e = EstopState::new();
    e.trigger();
    assert!(e.is_engaged());
    e.release();
    assert!(!e.is_engaged());
}

#[test]
fn trigger_is_idempotent() {
    let e = EstopState::new();
    e.trigger();
    e.trigger();
    assert!(e.is_engaged());
    e.release();
    assert!(!e.is_engaged());
}

#[test]
fn subscribe_sees_transitions() {
    let e = EstopState::new();
    let rx = e.subscribe();
    assert_eq!(*rx.borrow(), false);
    e.trigger();
    assert_eq!(*rx.borrow(), true);
    e.release();
    assert_eq!(*rx.borrow(), false);
}

#[test]
fn shared_via_arc_is_consistent() {
    // 模拟生产形态：web 层（trigger）和 agent loop（is_engaged）
    // 共享同一个 Arc<EstopState>。
    let e = Arc::new(EstopState::new());
    let e2 = e.clone();
    assert!(!e.is_engaged());
    e2.trigger();
    assert!(e.is_engaged());
}

#[test]
fn multiple_subscribers_all_see_change() {
    let e = EstopState::new();
    let rx1 = e.subscribe();
    let rx2 = e.subscribe();
    e.trigger();
    assert_eq!(*rx1.borrow(), true);
    assert_eq!(*rx2.borrow(), true);
    e.release();
    assert_eq!(*rx1.borrow(), false);
    assert_eq!(*rx2.borrow(), false);
}
