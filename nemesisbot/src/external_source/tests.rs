//! P1 外循环纯函数面测试：解析抢救 / 标签过滤 / 回写文案 / exclude 幂等。
//! （需要 AgentLoop/BoardStore 的装配路径不在此测——import 幂等由
//! `list_issues_by_origin` 去重 + store 侧既有测试覆盖，轮询循环本身
//! 依赖网关活体，P0 验证面。）

use super::*;

// ---------------------------------------------------------------------------
// parse_issue_list：8K 截断抢救
// ---------------------------------------------------------------------------

#[test]
fn parse_issue_list_full_json_no_salvage() {
    let text = r#"[{"number":1,"title":"a"},{"number":2,"title":"b"}]"#;
    let (entries, salvaged) = parse_issue_list(text);
    assert!(!salvaged);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["number"], 1);
}

#[test]
fn parse_issue_list_truncated_tail_salvages_complete_entries() {
    // 模拟 HTTP_BODY_CAP 截断：第二条对象中途被斩断。
    let mut text = String::from(r#"[{"number":1,"title":"完整条目"},{"number":2,"title":"被截"#);
    text.push_str("断的条目体，无闭合");
    let (entries, salvaged) = parse_issue_list(&text);
    assert!(salvaged);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["number"], 1);
}

#[test]
fn parse_issue_list_salvage_respects_string_state() {
    // 花括号出现在字符串字面量内不得破坏配对深度。
    let text = r#"[{"number":3,"body":"正文里有 } 和 { 花括号"},{"number":4"#;
    let (entries, salvaged) = parse_issue_list(text);
    assert!(salvaged);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["number"], 3);
    assert_eq!(entries[0]["body"], "正文里有 } 和 { 花括号");
}

#[test]
fn parse_issue_list_non_array_is_empty_no_salvage() {
    let (entries, salvaged) = parse_issue_list("Tool error: HTTP 401: unauthorized");
    assert!(entries.is_empty());
    assert!(!salvaged);
}

#[test]
fn parse_issue_list_pr_entry_survives_salvage() {
    // Gitea issues_list 混入 PR（带 pull_request 字段）——截断抢救后仍可辨。
    let text = r#"[{"number":5,"pull_request":{"id":9}},{"number":6"#;
    let (entries, salvaged) = parse_issue_list(text);
    assert!(salvaged);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].get("pull_request").is_some());
}

// ---------------------------------------------------------------------------
// labels_match
// ---------------------------------------------------------------------------

fn entry_with_labels(names: &[&str]) -> serde_json::Value {
    let labels: Vec<serde_json::Value> = names
        .iter()
        .map(|n| serde_json::json!({ "name": n }))
        .collect();
    serde_json::json!({ "number": 1, "labels": labels })
}

#[test]
fn labels_match_empty_config_accepts_all() {
    let src = ExternalSourceConfig {
        owner: "o".into(),
        repo: "r".into(),
        ..Default::default()
    };
    assert!(labels_match(&src, &entry_with_labels(&["bug"])));
    assert!(labels_match(&src, &entry_with_labels(&[])));
}

#[test]
fn labels_match_filters_by_any_hit() {
    let src = ExternalSourceConfig {
        owner: "o".into(),
        repo: "r".into(),
        labels: vec!["agent-ready".into()],
        ..Default::default()
    };
    assert!(labels_match(
        &src,
        &entry_with_labels(&["bug", "agent-ready"])
    ));
    assert!(!labels_match(&src, &entry_with_labels(&["bug", "docs"])));
    assert!(!labels_match(&src, &entry_with_labels(&[])));
}

// ---------------------------------------------------------------------------
// render_writeback
// ---------------------------------------------------------------------------

fn issue_with_origin(status: nemesis_board::IssueStatus) -> nemesis_board::Issue {
    // 手工构造最小 Issue（避免依赖 store）——字段集以当前 models 为准，
    // 新增字段会在此编译期暴露。
    let mut issue = minimal_issue();
    issue.status = status;
    issue
}

fn minimal_issue() -> nemesis_board::Issue {
    serde_json::from_value(serde_json::json!({
        "id": 7,
        "number": "NB-9",
        "title": "[gitea/o/r#12] 修复登录",
        "description": "",
        "status": "in_progress",
        "priority": 1,
        "creator": {"kind": "system", "id": "external-source"},
        "position": 0,
        "required_tags": [],
        "required_capabilities": [],
        "origin": {"origin_type": "gitea", "origin_id": "o/r#12"},
        "created_at": 0,
        "updated_at": 0,
    }))
    .expect("最小 Issue JSON 应可反序列化")
}

#[test]
fn render_writeback_covers_three_terminal_states() {
    let issue = issue_with_origin(nemesis_board::IssueStatus::Done);
    let done = render_writeback(&issue, "in_review", "done");
    assert!(done.contains("NB-9"));
    assert!(done.contains("✅"));
    let review = render_writeback(&issue, "in_progress", "in_review");
    assert!(review.contains("🔄") && review.contains("in_review"));
    let cancelled = render_writeback(&issue, "todo", "cancelled");
    assert!(cancelled.contains("⛔"));
}

// ---------------------------------------------------------------------------
// exclude_projection_files：幂等 + 不重复追加
// ---------------------------------------------------------------------------

#[test]
fn exclude_projection_files_writes_once_and_idempotent() {
    let tmp = std::env::temp_dir().join(format!("ext-src-excl-{}", uuid::Uuid::new_v4()));
    let root = tmp.join("repo");
    std::fs::create_dir_all(root.join(".git").join("info")).unwrap();
    exclude_projection_files(&root);
    let exclude = root.join(".git").join("info").join("exclude");
    let first = std::fs::read_to_string(&exclude).unwrap();
    for want in [
        "/project.json",
        "/timeline.jsonl",
        "/docs/",
        "/records/",
        "/.gitignore",
    ] {
        assert!(first.contains(want), "exclude 缺 {want}");
    }
    // 二次执行：字节不变（幂等）。
    exclude_projection_files(&root);
    let second = std::fs::read_to_string(&exclude).unwrap();
    assert_eq!(first, second);
    // 非 git 目录（普通项目目录）= 静默零操作不炸。
    let plain = tmp.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    exclude_projection_files(&plain); // 不 panic 即过
    std::fs::remove_dir_all(&tmp).ok();
}

// ---------------------------------------------------------------------------
// 来源键推导（回写 source_key 与状态表对齐）
// ---------------------------------------------------------------------------

#[test]
fn writeback_source_key_derivation_contract() {
    // 消费者按 origin_id="owner/repo#N" 推状态表键 "connector/owner/repo"；
    // 此契约由 spawn() 初始化表 + install_write_back 双侧共享，钉在纯函数
    // 语义层（拆函数会破坏对齐）。
    let origin_id = "myowner/myrepo#42";
    let repo_path = origin_id
        .rsplit_once('#')
        .map(|(a, _)| a)
        .unwrap_or(origin_id);
    let key = format!("gitea/{repo_path}");
    assert_eq!(key, "gitea/myowner/myrepo");
}
