// archive_writer.rs 覆盖率补充测试（交付失败档案 235 / timeline 写入失败
// 的诚实留痕 79-85 / records 建目录失败留痕 176-178 / sync_project_manifest
// 三个早退臂 319/323-324/330）。
//
// 平台豁免：52/58/60-61（honest_failure 里 add_activity 也失败的二次降级
// warn 臂）需要 store 数据库同时故障，单进程内无确定性触发形态。

use super::*;
use crate::models::NewIssue;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

fn temp_root(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nmb-aw-cov-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn temp_store(name: &str) -> BoardStore {
    let dir = temp_root(name);
    BoardStore::open(&dir.join("board.db"), "NB").expect("open store")
}

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

fn has_archive_failure_activity(store: &BoardStore, issue_id: i64) -> bool {
    store
        .list_activity(issue_id)
        .unwrap()
        .iter()
        .any(|a| a.action == ACTIVITY_ARCHIVE_WRITE_FAILED)
}

/// 失败交付同样落档案（235 的 ok=false 分支内容）。
#[test]
fn delivery_milestone_records_failed_delivery() {
    let store = temp_store("delivery-fail");
    let (_pid, issue, root) = fixture(&store, "dfail");

    write_delivery_milestone(&store, &issue, "node-worker", false, "执行超时");
    let record = root.join("records").join(&issue.number).join("delivery.md");
    let body = std::fs::read_to_string(record).unwrap();
    assert!(body.contains("失败"), "{body}");
    assert!(body.contains("执行超时"), "{body}");
    assert!(!has_archive_failure_activity(&store, issue.id));

    let _ = std::fs::remove_dir_all(&root);
}

/// timeline.jsonl 只读 → append 失败 → 诚实留痕（79-85 + honest_failure
/// 主体），里程碑文档本体不受影响。
#[test]
fn timeline_write_failure_leaves_audit_activity() {
    let store = temp_store("timeline-ro");
    let (_pid, issue, root) = fixture(&store, "tro");
    let timeline = root.join("timeline.jsonl");
    let mut perms = std::fs::metadata(&timeline).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&timeline, perms).unwrap();

    write_plan_milestone(&store, &issue, &[]);
    assert!(
        has_archive_failure_activity(&store, issue.id),
        "timeline 失败必须留痕"
    );
    assert!(root.join("docs/plan.md").exists(), "里程碑文档本体照常落盘");

    // 解除只读再清理（Windows 上只读文件阻断 remove_dir_all）。
    let mut perms = std::fs::metadata(&timeline).unwrap().permissions();
    perms.set_readonly(false);
    std::fs::set_permissions(&timeline, perms).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

/// records 被同名的普通文件占位 → 建目录失败 → 留痕（176-178）。
#[test]
fn records_dir_blocked_by_file_leaves_audit_activity() {
    let store = temp_store("records-blocked");
    let (_pid, issue, root) = fixture(&store, "rblock");
    // records/ 目录脚手架已存在；占位其下的单据子目录（NB-x 为普通文件）
    // 使 create_dir_all(records/NB-x) 失败。
    std::fs::write(root.join("records").join(&issue.number), b"occupied").unwrap();

    write_dispatch_milestone(&store, &issue, "node-worker", "task-1", None);
    assert!(
        has_archive_failure_activity(&store, issue.id),
        "records 建目录失败必须留痕"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// sync_project_manifest 三个早退臂：项目不存在（319）/ 无目录（323-324）/
/// 目录无 manifest（330）——全部静默无副作用。
#[test]
fn sync_manifest_early_returns_are_silent() {
    let store = temp_store("sync-early");
    // 项目不存在。
    sync_project_manifest(&store, 999_999);

    // 无目录项目。
    let no_dir = store
        .create_project("无目录", "", None, "", "", None)
        .unwrap();
    sync_project_manifest(&store, no_dir.id);

    // 有目录但无 manifest（空目录）。
    let empty = temp_root("sync-empty");
    std::fs::create_dir_all(&empty).unwrap();
    let with_empty = store
        .create_project("空目录", "", None, "", "", Some(empty.to_str().unwrap()))
        .unwrap();
    sync_project_manifest(&store, with_empty.id);

    let _ = std::fs::remove_dir_all(&empty);
}

// ===========================================================================
// wave6 追加：脚手架缺失四形态（79-85/169-170/224-225/283-284）、records
// 建目录失败（229-235）、三个里程碑的 fs::write 失败臂（198/256/304）、
// honest_failure 审计留痕二次失败的降级 warn（52，幽灵 issue id 触发 FK）、
// sync_project_manifest 的 project.json 刷新失败（330，只读文件）。
// ===========================================================================

use crate::models::Issue;

fn audit_rows(store: &BoardStore) -> usize {
    store
        .list_recent_activity(50, Some(ACTIVITY_ARCHIVE_WRITE_FAILED))
        .unwrap()
        .len()
}

fn strip_scaffold(root: &Path) {
    std::fs::remove_file(root.join("project.json")).unwrap();
    std::fs::remove_file(root.join("timeline.jsonl")).unwrap();
}

/// write_plan_milestone：脚手架缺失 → 诚实失败 + 审计留痕（79-85）。
#[test]
fn w6_plan_scaffold_missing_honest_failure() {
    let store = temp_store("w6aw-plan");
    let (_pid, issue, root) = fixture(&store, "w6plan");
    strip_scaffold(&root);
    write_plan_milestone(&store, &issue, &[]);
    assert_eq!(audit_rows(&store), 1, "必须留 archive_write_failed 审计");
    let _ = std::fs::remove_dir_all(&root);
}

/// dispatch/delivery/review 三里程碑：脚手架缺失 → 诚实失败（169-170 /
/// 224-225 / 283-284）。
#[test]
fn w6_dispatch_delivery_review_scaffold_missing_honest_failure() {
    let store = temp_store("w6aw-scaf");
    let (_pid, issue, root) = fixture(&store, "w6scaf");
    strip_scaffold(&root);
    write_dispatch_milestone(&store, &issue, "node-b", "t", None);
    write_delivery_milestone(&store, &issue, "node-b", true, "x");
    write_review_milestone(
        &store,
        &issue,
        "node-a",
        "auto_accept",
        "PASS",
        &serde_json::json!({}),
    );
    assert_eq!(audit_rows(&store), 3, "三条审计各留一次");
    let _ = std::fs::remove_dir_all(&root);
}

/// honest_failure 的审计留痕也失败 → 二次降级为日志，不 panic（52）。
/// 幽灵 issue id（库中不存在）触发 activity_log 外键违例。
#[test]
fn w6_honest_failure_audit_also_fails_degrades_to_log() {
    let store = temp_store("w6aw-ghost");
    let (_pid, issue, root) = fixture(&store, "w6ghost");
    strip_scaffold(&root);
    let mut ghost = issue.clone();
    ghost.id = 999_999; // 库里不存在 → add_activity FK 违例
    write_plan_milestone(&store, &ghost, &[]);
    assert_eq!(audit_rows(&store), 0, "审计留痕本身失败，库中无痕");
    let _ = std::fs::remove_dir_all(&root);
}

/// write_delivery_milestone：records 为同名文件 → create_dir_all 失败
///（229-235）。
#[test]
fn w6_delivery_records_create_failure() {
    let store = temp_store("w6aw-rec");
    let (_pid, issue, root) = fixture(&store, "w6rec");
    // 脚手架自带 records/ 目录——先移除再换成同名文件。
    std::fs::remove_dir_all(root.join("records")).unwrap();
    std::fs::write(root.join("records"), b"not a dir").unwrap();
    write_delivery_milestone(&store, &issue, "node-b", true, "x");
    assert_eq!(audit_rows(&store), 1);
    let _ = std::fs::remove_dir_all(&root);
}

/// write_dispatch_milestone：dispatch.md 被同名目录占位 → fs::write 失败
///（198）。
#[test]
fn w6_dispatch_write_failure_honest_failure() {
    let store = temp_store("w6aw-dw");
    let (_pid, issue, root) = fixture(&store, "w6dw");
    std::fs::create_dir_all(root.join("records").join(&issue.number).join("dispatch.md")).unwrap();
    write_dispatch_milestone(&store, &issue, "node-b", "t", None);
    assert_eq!(audit_rows(&store), 1);
    let _ = std::fs::remove_dir_all(&root);
}

/// write_delivery_milestone：delivery.md 被同名目录占位 → fs::write 失败
///（256）。
#[test]
fn w6_delivery_write_failure_honest_failure() {
    let store = temp_store("w6aw-dl");
    let (_pid, issue, root) = fixture(&store, "w6dl");
    std::fs::create_dir_all(root.join("records").join(&issue.number).join("delivery.md")).unwrap();
    write_delivery_milestone(&store, &issue, "node-b", true, "x");
    assert_eq!(audit_rows(&store), 1);
    let _ = std::fs::remove_dir_all(&root);
}

/// write_review_milestone：docs/review 为同名文件 → fs::write 失败（304）。
#[test]
fn w6_review_write_failure_honest_failure() {
    let store = temp_store("w6aw-rw");
    let (_pid, issue, root) = fixture(&store, "w6rw");
    // 脚手架自带 docs/review/ 目录——先移除再换成同名文件。
    std::fs::remove_dir_all(root.join("docs").join("review")).unwrap();
    std::fs::write(root.join("docs").join("review"), b"not a dir").unwrap();
    write_review_milestone(
        &store,
        &issue,
        "node-a",
        "auto_accept",
        "PASS",
        &serde_json::json!({}),
    );
    assert_eq!(audit_rows(&store), 1);
    let _ = std::fs::remove_dir_all(&root);
}

/// sync_project_manifest：project.json 只读 → write_manifest 失败走 warn
///（330），不 panic 不上抛。
#[test]
fn w6_sync_manifest_write_failure_is_nonblocking() {
    let store = temp_store("w6aw-sync");
    let (pid, _issue, root) = fixture(&store, "w6syncro");
    let manifest_path = root.join("project.json");
    let mut perms = std::fs::metadata(&manifest_path).unwrap().permissions();
    perms.set_readonly(true); // 跨平台只读位
    std::fs::set_permissions(&manifest_path, perms).unwrap();
    sync_project_manifest(&store, pid); // 必须不 panic
    let _ = std::fs::remove_dir_all(&root);
}
