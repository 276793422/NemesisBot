//! dossier 导出单元测试（临时 store + 临时目录，零网络零 LLM）。

use super::*;
use crate::assignment::Actor;
use crate::models::{CommentType, NewComment, NewIssue};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-dossiertest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn temp_store(name: &str) -> (BoardStore, PathBuf) {
    let dir = temp_dir(name);
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    (store, dir)
}

fn admin() -> Actor {
    Actor::admin("tester")
}

fn new_issue(title: &str) -> NewIssue {
    NewIssue {
        title: title.to_string(),
        creator: admin(),
        ..NewIssue::default()
    }
}

/// 无账闭包（usage.csv 只剩表头，诚实缺行）。
const NO_USAGE: fn(&str) -> Option<DossierUsage> = |_| None;

#[test]
fn test_issue_tree_export_structure_and_notes() {
    let (store, dir) = temp_store("tree");
    let ws = temp_dir("ws");
    let parent = store.create_issue(new_issue("父任务")).expect("create");
    let child = store
        .create_issue(NewIssue {
            parent_issue_id: Some(parent.id),
            ..new_issue("子任务")
        })
        .expect("create child");
    store
        .add_comment(NewComment {
            issue_id: parent.id,
            author: admin(),
            content: "第一手评论".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .expect("comment");
    store
        .add_activity(parent.id, &admin(), "status_changed", Some("todo→done"))
        .expect("activity");
    store
        .insert_dispatch("task-abc", child.id, "node-w", &admin())
        .expect("dispatch");

    let out_root = ws.join("logs").join("dossiers");
    let outcome =
        export_issue_tree(&store, &parent.number, &ws, &out_root, &NO_USAGE).expect("export");

    assert_eq!(
        outcome.issue_numbers,
        vec![parent.number.clone(), child.number.clone()]
    );
    // 执行档案未落地（无项目档案、无收件箱）→ 诚实注记。
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("执行档案未落地") && n.contains("task-abc")),
        "notes: {:?}",
        outcome.notes
    );

    let root = outcome.root;
    assert!(root.join("README.md").is_file());
    assert!(root.join("timeline.md").is_file());
    assert!(root.join("usage.csv").is_file());
    for f in ["activities.jsonl", "comments.jsonl", "dispatches.jsonl"] {
        assert!(root.join("audit").join(f).is_file(), "missing {f}");
    }
    let comments = std::fs::read_to_string(root.join("audit/comments.jsonl")).unwrap();
    assert!(comments.contains("第一手评论"));
    let dispatches = std::fs::read_to_string(root.join("audit/dispatches.jsonl")).unwrap();
    assert!(dispatches.contains("task-abc") && dispatches.contains("node-w"));
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    assert!(readme.contains("执行档案未落地"));
    // 无账 = usage.csv 只有表头。
    let csv = std::fs::read_to_string(root.join("usage.csv")).unwrap();
    assert_eq!(csv.lines().count(), 1);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_project_export_records_inbox_usage() {
    let (store, dir) = temp_store("proj");
    let ws = temp_dir("ws");
    let archive = temp_dir("archive");
    let project = store
        .create_project(
            "星舰",
            "desc",
            None,
            "🚀",
            "验收",
            Some(archive.to_str().unwrap()),
        )
        .expect("project");
    let a = store
        .create_issue(NewIssue {
            project_id: Some(project.id),
            ..new_issue("任务A")
        })
        .expect("a");
    let b = store
        .create_issue(NewIssue {
            project_id: Some(project.id),
            ..new_issue("任务B")
        })
        .expect("b");
    store
        .insert_dispatch("task-a1", a.id, "node-w", &admin())
        .expect("dispatch");
    store
        .insert_dispatch("task-b2", b.id, "node-x", &admin())
        .expect("dispatch2");

    // 项目档案：records/<A>/execution/<ts>/log.md + timeline.jsonl 事件。
    let rec = archive
        .join("records")
        .join(&a.number)
        .join("execution")
        .join("20260929_100000");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(rec.join("00.request.md"), "逐轮原文").unwrap();
    std::fs::write(
        archive.join("timeline.jsonl"),
        format!(
            "{{\"issue\":\"{}\",\"ts\":\"2026-09-29T10:00:00+08:00\",\"kind\":\"dispatch\",\"summary\":\"派出\"}}\n",
            a.number
        ),
    )
    .unwrap();

    // 收件箱：task-b2 未安置档案（诚实可见形态）。
    let inbox = ws.join("cluster").join("inbox").join("task-b2");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("01.AI.Request.raw.json"), "{}").unwrap();

    // 用量闭包：task-a1 有账，task-b2 无账（缺行诚实）。
    let usage_of = |tid: &str| {
        (tid == "task-a1").then_some(DossierUsage {
            input_tokens: 100,
            output_tokens: 20,
        })
    };

    let out_root = ws.join("logs").join("dossiers");
    let outcome = export_project(&store, project.id, &ws, &out_root, &usage_of).expect("export");
    assert_eq!(outcome.issue_numbers.len(), 2);
    assert!(outcome.notes.is_empty(), "notes: {:?}", outcome.notes);

    let root = outcome.root;
    // records 拷贝到位。
    assert!(
        root.join("records")
            .join(&a.number)
            .join("execution")
            .join("20260929_100000")
            .join("00.request.md")
            .is_file()
    );
    // inbox-unplaced 收进档案。
    assert!(
        root.join("inbox-unplaced")
            .join("task-b2")
            .join("01.AI.Request.raw.json")
            .is_file()
    );
    // usage.csv：task-a1 有账（120），task-b2 无账缺行。
    let csv = std::fs::read_to_string(root.join("usage.csv")).unwrap();
    assert!(csv.contains("task-a1,node-w,100,20,120"), "csv: {csv}");
    assert!(!csv.contains("task-b2"));
    // timeline.md 含项目档案事件 + README 含项目名。
    let tl = std::fs::read_to_string(root.join("timeline.md")).unwrap();
    assert!(tl.contains("派出"), "timeline: {tl}");
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    assert!(readme.contains("星舰"));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&archive);
}

#[test]
fn test_missing_archive_dir_note_and_error_paths() {
    let (store, dir) = temp_store("gap");
    let ws = temp_dir("ws");
    let project = store
        .create_project("孤儿", "", None, "x", "", Some("Z:/definitely/not/exist"))
        .expect("project");
    let _a = store
        .create_issue(NewIssue {
            project_id: Some(project.id),
            ..new_issue("A")
        })
        .expect("a");
    let outcome =
        export_project(&store, project.id, &ws, &ws.join("out"), &NO_USAGE).expect("export");
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("项目未绑定档案目录")),
        "notes: {:?}",
        outcome.notes
    );

    // 编号不存在 → Err；空项目 → Err。
    assert!(export_issue_tree(&store, "NB-99999", &ws, &ws.join("out"), &NO_USAGE).is_err());
    let empty = store
        .create_project("空项目", "", None, "x", "", None)
        .unwrap();
    assert!(export_project(&store, empty.id, &ws, &ws.join("out"), &NO_USAGE).is_err());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn test_tree_export_includes_grandchildren() {
    let (store, dir) = temp_store("depth");
    let ws = temp_dir("ws");
    let p = store.create_issue(new_issue("P")).unwrap();
    let c = store
        .create_issue(NewIssue {
            parent_issue_id: Some(p.id),
            ..new_issue("C")
        })
        .unwrap();
    let g = store
        .create_issue(NewIssue {
            parent_issue_id: Some(c.id),
            ..new_issue("G")
        })
        .unwrap();
    let outcome = export_issue_tree(&store, &p.number, &ws, &ws.join("out"), &NO_USAGE).unwrap();
    assert_eq!(
        outcome.issue_numbers,
        vec![p.number.clone(), c.number.clone(), g.number.clone()]
    );
    let readme = std::fs::read_to_string(outcome.root.join("README.md")).unwrap();
    assert!(readme.contains("G"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ws);
}
