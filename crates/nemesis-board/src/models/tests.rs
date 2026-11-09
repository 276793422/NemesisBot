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
    assert!(!ni.milestone);

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
            origin_meta: Default::default(),
        }),
        required_role: Some("worker".into()),
        required_tags: vec!["rust".into()],
        required_profession: Some("dev:cpp".into()),
        required_backend: Some("vm".into()),
        required_capabilities: vec!["chrome".into()],
        requires_evidence: true,
        hidden: false,
        quality: "experience".into(),
        unattended: Some(false),
        milestone: true,
        created_at: 1_756_000_000,
        updated_at: 1_756_500_000,
    };
    let json = serde_json::to_string(&issue).unwrap();
    let back: Issue = serde_json::from_str(&json).unwrap();
    assert_eq!(back.number, "NB-7");
    assert_eq!(back.status, IssueStatus::InProgress);
    assert_eq!(back.origin.as_ref().unwrap().origin_type, "autopilot");
    assert!(back.milestone);
}

// is_experience_quality：trim + 大小写不敏感（读取宽容 = quality 字段
// 声明的既定语义）。单一真相源——规格闸/milestone 归一/原型机器闸三
// 消费点同口径；此前的裸 `== "experience"`（原型闸）对 `"Experience"`
// 档静默落空（同族解析漂移，BUG-20261109-010 横向修复）。
#[test]
fn is_experience_quality_trim_and_case_insensitive() {
    let mk = |quality: &str| -> Issue {
        Issue {
            id: 1,
            number: "NB-1".into(),
            title: "t".into(),
            description: String::new(),
            status: IssueStatus::Todo,
            priority: priority::MEDIUM,
            assignee: None,
            assignee_id: None,
            creator: crate::assignment::Actor::admin("admin"),
            parent_issue_id: None,
            project_id: None,
            due_date: None,
            position: 0,
            acceptance_criteria: None,
            origin: None,
            required_role: None,
            required_tags: Vec::new(),
            required_profession: None,
            required_backend: None,
            required_capabilities: Vec::new(),
            requires_evidence: false,
            hidden: false,
            quality: quality.to_string(),
            unattended: None,
            milestone: false,
            created_at: 0,
            updated_at: 0,
        }
    };
    assert!(mk("experience").is_experience_quality());
    assert!(mk("Experience").is_experience_quality());
    assert!(mk("EXPERIENCE").is_experience_quality());
    assert!(mk("  experience  ").is_experience_quality());
    assert!(!mk("").is_experience_quality(), "空串 = 标准档");
    assert!(!mk("standard").is_experience_quality());
    assert!(!mk("experiences").is_experience_quality());
}

// ---------------------------------------------------------------------------
// Wave4 覆盖批次：is_terminal / allowed_targets 全臂。
// ---------------------------------------------------------------------------

#[test]
fn is_terminal_only_done_and_cancelled() {
    assert!(IssueStatus::Done.is_terminal());
    assert!(IssueStatus::Cancelled.is_terminal());
    assert!(!IssueStatus::Backlog.is_terminal());
    assert!(!IssueStatus::Todo.is_terminal());
    assert!(!IssueStatus::InProgress.is_terminal());
    assert!(!IssueStatus::InReview.is_terminal());
    assert!(!IssueStatus::Blocked.is_terminal());
}

#[test]
fn allowed_targets_covers_every_status() {
    assert_eq!(
        IssueStatus::Backlog.allowed_targets(),
        "todo/in_progress/done/blocked/cancelled"
    );
    assert_eq!(
        IssueStatus::Todo.allowed_targets(),
        "in_progress/done/blocked/cancelled"
    );
    assert_eq!(
        IssueStatus::InProgress.allowed_targets(),
        "in_review/done/blocked/cancelled"
    );
    assert_eq!(
        IssueStatus::InReview.allowed_targets(),
        "in_progress/done/blocked/cancelled"
    );
    assert_eq!(
        IssueStatus::Blocked.allowed_targets(),
        "todo/in_progress/cancelled"
    );
    assert_eq!(IssueStatus::Done.allowed_targets(), "（终态）");
    assert_eq!(
        IssueStatus::Cancelled.allowed_targets(),
        "backlog（issue.reopen）"
    );
}
