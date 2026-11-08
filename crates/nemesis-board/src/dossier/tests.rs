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
            None,
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
        .create_project(
            "孤儿",
            "",
            None,
            "x",
            "",
            Some("Z:/definitely/not/exist"),
            None,
        )
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
        .create_project("空项目", "", None, "x", "", None, None)
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

/// U-C 单一真相源断言：timeline.md 与 `timeline::collect_issue_timeline`
/// 同源——聚合出的每个事件摘要都能在导出的 markdown 里找到。
#[test]
fn test_timeline_md_shares_aggregation_source() {
    let (store, dir) = temp_store("ucsrc");
    let ws = temp_dir("ws");
    let issue = store.create_issue(new_issue("同源")).unwrap();
    store
        .add_activity(
            issue.id,
            &admin(),
            "auto_decide",
            Some(r#"{"verdict":"FAIL"}"#),
        )
        .unwrap();
    store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: admin(),
            content: "交付已上传".into(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();
    store
        .insert_dispatch("task-uc", issue.id, "node-w", &admin())
        .unwrap();

    let outcome =
        export_issue_tree(&store, &issue.number, &ws, &ws.join("out"), &NO_USAGE).unwrap();
    let md = std::fs::read_to_string(outcome.root.join("timeline.md")).unwrap();

    let events = crate::timeline::collect_issue_timeline(&store, issue.id, Vec::new());
    assert!(events.len() >= 3, "events: {events:?}");
    for ev in &events {
        let needle = match ev.source {
            // 评论行渲染的是摘要（首行）；派发行含 task id；活动行含 action。
            "comment" => ev.summary.clone(),
            "dispatch" => ev.detail.clone().unwrap_or_default(),
            _ => ev.kind.clone(),
        };
        assert!(
            !needle.is_empty() && md.contains(&needle),
            "source={} needle={needle:?} 未出现在 timeline.md",
            ev.source
        );
    }
    // 图标形态保留（评论 💬 / 派发 📤）。
    assert!(md.contains("💬[delivery]"));
    assert!(md.contains("📤 派发 → node-w"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ws);
}

/// 读侧（三轮审计）：收件箱补收按落地侧同源消毒名探测——
/// 异形 task_id（含 `..`）不得逃出导出树，消毒名目录正常补收、落位核对
/// 不再误报未落地。
#[test]
fn test_inbox_unplaced_hostile_task_id_sanitized() {
    let (store, dir) = temp_store("t8");
    let ws = temp_dir("ws");
    let issue = store.create_issue(new_issue("补收")).expect("create");
    let hostile = "../t8escape";
    store
        .insert_dispatch(hostile, issue.id, "node-w", &admin())
        .expect("dispatch");

    // 收件箱里放一个消毒名落地目录（transfer 落地侧的实际形态）。
    let inbox = ws.join("cluster").join("inbox");
    let safe = nemesis_utils::sanitize::sanitize_path_segment(hostile);
    std::fs::create_dir_all(inbox.join(&safe).join("files")).unwrap();
    std::fs::write(inbox.join(&safe).join("files").join("a.md"), b"x").unwrap();

    let out_root = ws.join("logs").join("dossiers");
    let outcome =
        export_issue_tree(&store, &issue.number, &ws, &out_root, &NO_USAGE).expect("export");

    // 补收成功：消毒名目录拷进导出树 inbox-unplaced/。
    let unplaced = outcome.root.join("inbox-unplaced");
    assert!(
        unplaced.join(&safe).join("files").join("a.md").is_file(),
        "消毒名目录必须被补收"
    );
    // 无逃逸：旧形态裸拼会落 root/inbox-unplaced/../t8escape = root/t8escape。
    assert!(!outcome.root.join("t8escape").exists(), "不得逃出导出树");

    // 落位核对（unplaced 探针同源消毒）不再误报未落地。
    assert!(
        !outcome.notes.iter().any(|n| n.contains("执行档案未落地")),
        "notes: {:?}",
        outcome.notes
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ws);
}

/// usage.csv 字段消毒：worker_id 是用户自设自由文本
/// （cluster init --name / node.update_identity，无字符集校验），含逗号/
/// 引号的名字此前裸拼错位移列。现在走 nemesis_utils 单一真相源消毒
/// （RFC 4180 引号包裹 + 内部引号翻倍）；普通名字逐字节直通由
/// test_project_export_records_inbox_usage 的 `task-a1,node-w,100,20,120`
/// 断言钉住（无谓引号即红）。
#[test]
fn test_usage_csv_escapes_hostile_worker_id() {
    let (store, dir) = temp_store("csv");
    let ws = temp_dir("ws");
    let issue = store.create_issue(new_issue("账")).unwrap();
    let hostile = "bad,\"quote\" name";
    store
        .insert_dispatch("task-c1", issue.id, hostile, &admin())
        .unwrap();
    let usage_of = |tid: &str| {
        (tid == "task-c1").then_some(DossierUsage {
            input_tokens: 100,
            output_tokens: 20,
        })
    };
    let outcome =
        export_issue_tree(&store, &issue.number, &ws, &ws.join("out"), &usage_of).unwrap();
    let csv = std::fs::read_to_string(outcome.root.join("usage.csv")).unwrap();
    // 恰两行（表头 + 一行数据）：引号字段不得把行拆碎。
    assert_eq!(csv.lines().count(), 2, "csv: {csv}");
    assert_eq!(
        csv.lines().nth(1).unwrap(),
        "task-c1,\"bad,\"\"quote\"\" name\",100,20,120"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ws);
}

/// dossier 导出的目录递归必须跳过目录符号链接——
/// 链接环（junction/symlink 指回父目录自身）在旧实现（`p.is_dir()`
/// 跟随链接）下整子树以「loop\loop\...」假路径重复拷贝几十层，直到
/// OS 路径长度上限才收敛（导出目录被垃圾嵌套拷贝污染）。
/// 链接创建免提权降级链：symlink_dir（开发者模式/管理员）→
/// junction（mklink /J，任何用户）。
#[cfg(windows)]
fn make_dir_link(target: &std::path::Path, link: &std::path::Path) {
    use std::os::windows::fs::symlink_dir;
    if symlink_dir(target, link).is_ok() {
        return;
    }
    let out = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("spawn cmd mklink");
    assert!(
        out.status.success(),
        "创建目录链接失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
fn make_dir_link(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn copy_dir_and_walk_skip_symlink_cycles() {
    let dir = temp_dir("symcycle");
    let src = dir.join("records");
    let sub = src.join("issue").join("execution");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("x.md"), "data").unwrap();

    // 链接环：records/loop → records 自身
    make_dir_link(&src, &src.join("loop"));

    // 旧实现：跟随环病态重复递归（红证 = dst/loop 在位——整子树被
    // 重复拷贝进假路径）。
    let dst = dir.join("out");
    super::copy_dir_sorted(&src, &dst).unwrap();
    assert!(
        dst.join("issue").join("execution").join("x.md").is_file(),
        "真实子树照常拷贝"
    );
    assert!(!dst.join("loop").exists(), "目录链接必须跳过");
    // walk_has_dir 同环下有限返回
    assert!(super::walk_has_dir(&src, "execution"));
    assert!(!super::walk_has_dir(&src, "nonexistent"));

    let _ = std::fs::remove_dir_all(&dir);
}
