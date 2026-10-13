//! §2 免打扰运行时态单测（2026-10-11 无人值守计划 Part 2）。

use super::ApprovalAutoApproveState;

#[test]
fn default_off_and_flip_hot() {
    let s = ApprovalAutoApproveState::new(false);
    assert!(!s.enabled(), "缺省关（默认 false pin——现行为零变化）");
    assert_eq!(s.count(), 0);
    s.set_enabled(true);
    assert!(s.enabled(), "热切即时生效");
    s.set_enabled(false);
    assert!(!s.enabled());
}

#[test]
fn count_accumulates_and_shared_arc_observes() {
    let s = std::sync::Arc::new(ApprovalAutoApproveState::new(true));
    let observer = s.clone();
    s.bump();
    s.bump();
    s.bump();
    assert_eq!(
        observer.count(),
        3,
        "同一 Arc 双端可见（auditor 写 / handler 读）"
    );
    assert!(observer.enabled());
}
