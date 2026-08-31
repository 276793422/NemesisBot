//! 指派类型 / Actor 测试。

use super::*;

#[test]
fn test_assignment_type_str_roundtrip() {
    assert_eq!(AssignmentType::ManagerSelf.as_str(), "manager_self");
    assert_eq!(AssignmentType::Worker.as_str(), "worker");
    assert_eq!(
        AssignmentType::from_str("manager_self"),
        Some(AssignmentType::ManagerSelf)
    );
    assert_eq!(
        AssignmentType::from_str("worker"),
        Some(AssignmentType::Worker)
    );
    assert_eq!(AssignmentType::from_str("admin"), None);
    assert_eq!(AssignmentType::Worker.to_string(), "worker");
}

#[test]
fn test_assignment_type_serde_snake_case() {
    assert_eq!(
        serde_json::to_string(&AssignmentType::ManagerSelf).unwrap(),
        "\"manager_self\""
    );
    let de: AssignmentType = serde_json::from_str("\"worker\"").unwrap();
    assert_eq!(de, AssignmentType::Worker);
}

#[test]
fn test_actor_constructors() {
    assert_eq!(Actor::admin("alice"), Actor::new("admin", "alice"));
    assert_eq!(Actor::agent("mgr-1").kind, "agent");
    assert_eq!(Actor::system("autopilot").kind, "system");
    assert_ne!(Actor::admin("a"), Actor::agent("a"));
}
