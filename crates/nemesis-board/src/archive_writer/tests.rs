//! archive_writer 里程碑写入测试（看板项目档案 goal P2/C）。

use super::*;
use crate::models::NewIssue;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// 唯一临时目录（crate 无 tempfile 依赖，与 store/tests.rs 同款）。
fn temp_root(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-awtest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn temp_store(name: &str) -> BoardStore {
    let dir = temp_root(name);
    BoardStore::open(&dir.join("board.db"), "NB").expect("open store")
}

/// 建一个绑定了档案目录（含脚手架）的项目 + 一个项目内 issue。
fn fixture(store: &BoardStore, name: &str) -> (i64, Issue, PathBuf) {
    let root = temp_root(name);
    crate::archive::ensure_scaffold(&root, 0, name, "active").expect("scaffold");
    let project = store
        .create_project(name, "", None, "", "", Some(root.to_str().unwrap()))
        .unwrap();
    let issue = store
        .create_issue(NewIssue {
            title: format!("任务{name}"),
            description: "说明".to_string(),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    (project.id, issue, root)
}

#[test]
fn plan_milestone_writes_docs_plan_md_with_dependencies() {
    let store = temp_store("plan");
    let (_pid, issue, root) = fixture(&store, "planproj");
    let subs = vec![
        PlannedSubIssue {
            title: "子任务零".into(),
            description: "第一步".into(),
            required_role: String::new(),
            required_tags: vec!["rust".into()],
            acceptance_criteria: "[CHECK] file:out/a.md exists".into(),
            depends_on: vec![],
        },
        PlannedSubIssue {
            title: "子任务一".into(),
            description: String::new(),
            required_role: "coder".into(),
            required_tags: vec![],
            acceptance_criteria: String::new(),
            depends_on: vec![0],
        },
    ];
    write_plan_milestone(&store, &issue, &subs);
    let md = std::fs::read_to_string(root.join("docs").join("plan.md")).unwrap();
    assert!(md.contains("# 拆解计划"));
    assert!(md.contains("子任务零") && md.contains("子任务一"));
    assert!(md.contains("依赖：子0"), "依赖边渲染：{md}");
    assert!(md.contains("rust"), "标签渲染");
    assert!(md.contains("coder"), "角色渲染");
    assert!(md.contains("[CHECK] file:out/a.md exists"), "锚点行透传");
    // timeline 有 plan 事件。
    let tl = std::fs::read_to_string(root.join("timeline.jsonl")).unwrap();
    assert!(tl.contains("\"kind\":\"plan\""), "timeline plan 事件: {tl}");
}

#[test]
fn dispatch_and_delivery_and_review_milestones_land_in_records() {
    let store = temp_store("mile");
    let (_pid, issue, root) = fixture(&store, "mileproj");

    write_dispatch_milestone(&store, &issue, "node-b", "task-42", None);
    let rec = root.join("records").join(&issue.number);
    let disp = std::fs::read_to_string(rec.join("dispatch.md")).unwrap();
    assert!(disp.contains("node-b"));
    assert!(disp.contains("task-42"));
    assert!(disp.contains("P4 起填充"), "P2 阶段基线标记留空占位");
    assert!(disp.contains(issue.acceptance_criteria.as_deref().unwrap_or("")));

    write_delivery_milestone(&store, &issue, "node-b", true, "完成交付");
    let del = std::fs::read_to_string(rec.join("delivery.md")).unwrap();
    assert!(del.contains("✅ 成功") && del.contains("完成交付"));

    // 失败交付同样入档（零信息丢失）。
    write_delivery_milestone(&store, &issue, "node-b", false, "锚点未过");
    let del = std::fs::read_to_string(rec.join("delivery.md")).unwrap();
    assert!(del.contains("⛔ 失败") && del.contains("锚点未过"));

    write_review_milestone(
        &store,
        &issue,
        "node-a",
        "auto_accept",
        "PASS",
        &serde_json::json!({ "decision": "auto_accept", "verdict": "PASS" }),
    );
    let rev = std::fs::read_to_string(
        root.join("docs")
            .join("review")
            .join(format!("{}.md", issue.number)),
    )
    .unwrap();
    assert!(rev.contains("auto_accept") && rev.contains("PASS"));
    assert!(rev.contains("\"decision\""), "details JSON 原样投影");

    let tl = std::fs::read_to_string(root.join("timeline.jsonl")).unwrap();
    for kind in ["\"dispatch\"", "\"delivery\"", "\"review\""] {
        assert!(tl.contains(kind), "timeline 缺 {kind}: {tl}");
    }
}

#[test]
fn milestones_silently_skip_legacy_projects_without_directory() {
    let store = temp_store("legacy");
    let project = store
        .create_project("存量", "", None, "", "", None)
        .unwrap();
    let issue = store
        .create_issue(NewIssue {
            title: "旧任务".into(),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    // 存量项目（无 directory）：四个里程碑全跳过，不炸不写。
    write_plan_milestone(&store, &issue, &[]);
    write_dispatch_milestone(&store, &issue, "node-b", "t", None);
    write_delivery_milestone(&store, &issue, "node-b", true, "x");
    write_review_milestone(
        &store,
        &issue,
        "n",
        "auto_accept",
        "PASS",
        &serde_json::json!({}),
    );
}

#[test]
fn write_failure_is_visible_not_blocking() {
    let store = temp_store("fail");
    let (_pid, issue, root) = fixture(&store, "failproj");
    // 破坏脚手架：docs 替换成同名文件 → 写 plan.md 必败。
    std::fs::remove_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs"), b"not a dir").unwrap();
    write_plan_milestone(&store, &issue, &[]); // 必须不 panic、不阻塞
    // 诚实可见：审计留痕 archive_write_failed。
    let acts = store
        .list_recent_activity(50, Some(ACTIVITY_ARCHIVE_WRITE_FAILED))
        .unwrap();
    assert!(
        acts.iter().any(|a| a.activity.issue_id == issue.id),
        "写失败必须留审计"
    );
    // timeline 同样缺脚手架保护：不 panic。
    std::fs::remove_file(root.join("timeline.jsonl")).unwrap();
    timeline(&store, &root, &issue, "x", "y");
}

#[test]
fn sync_manifest_projects_status_into_project_json() {
    let store = temp_store("sync");
    let (pid, _issue, root) = fixture(&store, "syncproj");
    store
        .update_project(
            pid,
            &crate::models::ProjectPatch {
                status: Some("in_progress".into()),
                ..Default::default()
            },
        )
        .unwrap();
    sync_project_manifest(&store, pid);
    let m = crate::archive::read_manifest(&root).unwrap();
    assert_eq!(m.status, "in_progress");
    assert_eq!(m.name, "syncproj");
    assert_eq!(
        m.project_id, pid,
        "sync 必须回填真实 project_id（脚手架先建时 id=0）"
    );
    // 存量项目 / 不存在的项目：静默跳过。
    sync_project_manifest(&store, 999_999);
}
