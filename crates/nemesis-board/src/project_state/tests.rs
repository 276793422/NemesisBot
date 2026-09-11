use super::*;
use crate::models::ProjectStatus;

#[test]
fn same_state_is_never_a_transition() {
    for s in [
        ProjectStatus::Active,
        ProjectStatus::InProgress,
        ProjectStatus::Completed,
        ProjectStatus::Archived,
    ] {
        assert!(!can_transition(s, s));
        assert!(validate_transition(s, s).is_err());
    }
}

#[test]
fn legal_transitions_exhaustive() {
    // 主链 + 归档旁路 + 重开 + F3 预留回退。
    let legal = [
        (ProjectStatus::Active, ProjectStatus::InProgress),
        (ProjectStatus::Active, ProjectStatus::Archived),
        (ProjectStatus::InProgress, ProjectStatus::Completed),
        (ProjectStatus::InProgress, ProjectStatus::Archived),
        (ProjectStatus::Completed, ProjectStatus::Archived),
        (ProjectStatus::Completed, ProjectStatus::InProgress),
        (ProjectStatus::Archived, ProjectStatus::Active),
    ];
    for (from, to) in legal {
        assert!(can_transition(from, to), "{from:?} → {to:?} 应合法");
        assert!(validate_transition(from, to).is_ok());
    }
}

#[test]
fn illegal_transitions_exhaustive() {
    // 全笛卡尔积减去合法集与同态，其余全部非法。
    let all = [
        ProjectStatus::Active,
        ProjectStatus::InProgress,
        ProjectStatus::Completed,
        ProjectStatus::Archived,
    ];
    let legal: Vec<(ProjectStatus, ProjectStatus)> = vec![
        (ProjectStatus::Active, ProjectStatus::InProgress),
        (ProjectStatus::Active, ProjectStatus::Archived),
        (ProjectStatus::InProgress, ProjectStatus::Completed),
        (ProjectStatus::InProgress, ProjectStatus::Archived),
        (ProjectStatus::Completed, ProjectStatus::Archived),
        (ProjectStatus::Completed, ProjectStatus::InProgress),
        (ProjectStatus::Archived, ProjectStatus::Active),
    ];
    for &from in &all {
        for &to in &all {
            if from == to || legal.contains(&(from, to)) {
                continue;
            }
            assert!(!can_transition(from, to), "{from:?} → {to:?} 应非法");
            assert!(validate_transition(from, to).is_err());
        }
    }
}

#[test]
fn error_messages_are_actionable() {
    let err = validate_transition(ProjectStatus::Archived, ProjectStatus::Completed).unwrap_err();
    assert!(err.contains("archived"), "报错应含当前状态：{err}");
    assert!(err.contains("completed"), "报错应含目标状态：{err}");
    assert!(
        err.contains(ProjectStatus::Archived.allowed_targets()),
        "报错应含合法目标集提示：{err}"
    );
    let same = validate_transition(ProjectStatus::Active, ProjectStatus::Active).unwrap_err();
    assert!(same.contains("已处于 active"), "同态报错文案：{same}");
}

#[test]
fn status_str_roundtrip() {
    for s in [
        ProjectStatus::Active,
        ProjectStatus::InProgress,
        ProjectStatus::Completed,
        ProjectStatus::Archived,
    ] {
        assert_eq!(ProjectStatus::from_str(s.as_str()), Some(s));
        assert_eq!(s.to_string(), s.as_str());
    }
    // serde 词形与 as_str 一致（落库/前端同形）。
    let json = serde_json::to_string(&ProjectStatus::InProgress).unwrap();
    assert_eq!(json, "\"in_progress\"");
}
