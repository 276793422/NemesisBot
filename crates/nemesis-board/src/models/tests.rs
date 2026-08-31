//! 模型序列化 / 枚举解析测试。

use super::*;

#[test]
fn test_issue_status_serde_snake_case() {
    assert_eq!(
        serde_json::to_string(&IssueStatus::InProgress).unwrap(),
        "\"in_progress\""
    );
    assert_eq!(
        serde_json::to_string(&IssueStatus::Backlog).unwrap(),
        "\"backlog\""
    );
    let de: IssueStatus = serde_json::from_str("\"in_review\"").unwrap();
    assert_eq!(de, IssueStatus::InReview);
}

#[test]
fn test_issue_status_str_roundtrip() {
    let all = [
        IssueStatus::Backlog,
        IssueStatus::Todo,
        IssueStatus::InProgress,
        IssueStatus::InReview,
        IssueStatus::Done,
        IssueStatus::Blocked,
        IssueStatus::Cancelled,
    ];
    for s in all {
        assert_eq!(IssueStatus::from_str(s.as_str()), Some(s), "{s}");
        assert_eq!(s.to_string(), s.as_str());
    }
    assert_eq!(IssueStatus::from_str("unknown"), None);
}

#[test]
fn test_comment_type_serde_and_str() {
    assert_eq!(
        serde_json::to_string(&CommentType::StatusChange).unwrap(),
        "\"status_change\""
    );
    let de: CommentType = serde_json::from_str("\"system\"").unwrap();
    assert_eq!(de, CommentType::System);
    assert_eq!(CommentType::from_str("comment"), Some(CommentType::Comment));
    assert_eq!(CommentType::from_str("bogus"), None);
}

#[test]
fn test_input_defaults() {
    let ni = NewIssue::default();
    assert_eq!(ni.priority, priority::MEDIUM);
    assert_eq!(ni.creator.kind, "admin");
    assert!(ni.title.is_empty());
    assert!(ni.assignee.is_none());

    let filter = IssueFilter::default();
    assert!(filter.status.is_none());
    assert!(filter.query.is_none());

    let patch = IssuePatch::default();
    assert!(patch.title.is_none());
    assert!(patch.priority.is_none());
    assert!(patch.parent_issue_id.is_none());
}

#[test]
fn test_issue_serde_roundtrip() {
    let issue = Issue {
        id: 7,
        number: "NB-7".into(),
        title: "标题".into(),
        description: String::new(),
        status: IssueStatus::InProgress,
        priority: priority::HIGH,
        assignee: Some(crate::assignment::AssignmentType::Worker),
        assignee_id: Some("node-b".into()),
        creator: crate::assignment::Actor::admin("admin"),
        parent_issue_id: None,
        project_id: Some(1),
        due_date: Some(1_800_000_000),
        position: 7,
        acceptance_criteria: Some("- [ ] 全绿".into()),
        origin: Some(TaskOrigin {
            origin_type: "autopilot".into(),
            origin_id: "cron-1".into(),
        }),
        created_at: 1_756_000_000,
        updated_at: 1_756_500_000,
    };
    let json = serde_json::to_string(&issue).unwrap();
    let back: Issue = serde_json::from_str(&json).unwrap();
    assert_eq!(back.number, "NB-7");
    assert_eq!(back.status, IssueStatus::InProgress);
    assert_eq!(back.origin.as_ref().unwrap().origin_type, "autopilot");
}
