//! 触点状态机 / 默认动作 / 审计事件测试。

use super::*;
use crate::models::{TouchpointKind, TouchpointStatus};

#[test]
fn pending_has_six_outbound_arms() {
    use TouchpointStatus as S;
    for to in [
        S::Confirmed,
        S::TimeoutDefaulted,
        S::VetoedLocal,
        S::VetoedSpec,
        S::Cancelled,
        S::ParkedSync,
    ] {
        assert!(can_transition(S::Pending, to), "pending → {to}");
    }
    // pending 无自转移。
    assert!(!can_transition(S::Pending, S::Pending));
}

#[test]
fn vetoed_states_rehang_or_park_or_cancel_only() {
    use TouchpointStatus as S;
    for from in [S::VetoedLocal, S::VetoedSpec] {
        assert!(
            can_transition(from, S::Pending),
            "{from} → pending（v2 重挂/复验回卡）"
        );
        assert!(
            can_transition(from, S::ParkedSync),
            "{from} → parked_sync（熔断）"
        );
        assert!(can_transition(from, S::Cancelled), "{from} → cancelled");
        // 否决态不能直接确认/超时/互相转移。
        assert!(
            !can_transition(from, S::Confirmed),
            "{from} → confirmed 必须先回 pending"
        );
        assert!(!can_transition(from, S::TimeoutDefaulted));
        assert!(!can_transition(from, S::VetoedLocal));
        assert!(!can_transition(from, S::VetoedSpec));
        assert!(!can_transition(from, from));
    }
    // 两种否决态互斥不互通。
    assert!(!can_transition(S::VetoedLocal, S::VetoedSpec));
    assert!(!can_transition(S::VetoedSpec, S::VetoedLocal));
}

#[test]
fn terminal_states_have_no_outbound() {
    use TouchpointStatus as S;
    let all = [
        S::Pending,
        S::Confirmed,
        S::TimeoutDefaulted,
        S::VetoedLocal,
        S::VetoedSpec,
        S::Cancelled,
        S::ParkedSync,
    ];
    for from in [
        S::Confirmed,
        S::TimeoutDefaulted,
        S::Cancelled,
        S::ParkedSync,
    ] {
        assert!(from.is_terminal(), "{from} 应判终态");
        for to in all {
            assert!(!can_transition(from, to), "终态 {from} → {to} 必须拒绝");
        }
    }
    // 非终态集合钉住：pending 与两种否决态可恢复。
    assert!(!S::Pending.is_terminal());
    assert!(!S::VetoedLocal.is_terminal());
    assert!(!S::VetoedSpec.is_terminal());
}

#[test]
fn default_action_per_kind() {
    assert_eq!(
        default_action_for(TouchpointKind::Spec),
        "proceed_with_draft"
    );
    assert_eq!(
        default_action_for(TouchpointKind::Prototype),
        "proceed_current_version"
    );
}

#[test]
fn audit_append_grows_array_and_preserves_history() {
    let e1 = TouchpointAuditEvent::new(1000, "system:web", "hang", "规格 v1 挂卡");
    let e2 = TouchpointAuditEvent::new(2000, "human:admin", "confirm", "");
    let json = append_audit_event("", e1).unwrap();
    assert_eq!(
        json,
        r#"[{"ts_ms":1000,"actor":"system:web","action":"hang","detail":"规格 v1 挂卡"}]"#
    );
    let json2 = append_audit_event(&json, e2.clone()).unwrap();
    let back: Vec<TouchpointAuditEvent> = serde_json::from_str(&json2).unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].action, "hang");
    assert_eq!(back[1].actor, "human:admin");
    // 同事件可重复追加（调用方各自构造，不共享所有权约束）。
    let json3 = append_audit_event(&json2, e2).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<TouchpointAuditEvent>>(&json3)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn audit_append_refuses_corrupted_array() {
    let e = TouchpointAuditEvent::new(1, "timeout-sweep", "timeout_default", "proceed_with_draft");
    let err = append_audit_event("not json", e.clone()).unwrap_err();
    assert!(err.contains("解析失败"), "{err}");
    // 空串视作空数组（新建行 audit 缺省 ''）。
    let json = append_audit_event("", e).unwrap();
    assert!(json.contains("timeout_default"));
}
