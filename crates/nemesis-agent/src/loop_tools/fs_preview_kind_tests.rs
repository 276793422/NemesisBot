//! （R3123 / BUG-20261127-039）写族工具 preview kind 的路径解析同源回归。
//!
//! 根因：`WriteFileTool::preview` / `EditFileTool::preview`（append 分支）
//! 对**裸 args 路径**做 `Path::exists()` 判 Create/Modify，而 execute 经
//! `validate_workspace_path` 把相对路径解析到工作区落点。gateway 进程 CWD
//! ≠ 工作区 → 相对路径指向的工作区已有文件被系统性误报 Create → checkpoint
//! 快照按 Create 语义不读基线内容（content=None）→ rewind 落删除臂整删
//! 文件而非还原（JSON 形态无影子 tree，前向内容不可恢复）。
//!
//! 覆盖：
//! - 工具级：write_file 相对路径 已存在=Modify / 不存在=Create；
//!   绝对路径界内 已存在=Modify（旧行为保持）；界外路径（围栏必拒）=None。
//! - edit_file append 分支：相对路径 已存在=Modify / 不存在=Create；
//!   action=edit 恒 Modify。
//! - checkpoint 级闭环锁：相对路径覆盖写的基线（Modify + 现盘内容）被
//!   快照捕获，restore_code_scoped 还原**上一版内容**而非删除文件。

use super::Tool;
use super::fs::{EditFileTool, WriteFileTool};
use crate::checkpoint::CheckpointStore;
use crate::loop_tools::FileChangeKind;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::WorkspaceBoundary;

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_lt_prevkind_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn boundary(root: &Path) -> WorkspaceBoundary {
    WorkspaceBoundary {
        root: root.to_path_buf(),
        restrict: true,
        extra_roots: Vec::new(),
        editor_gate: None,
        goals_guard: None,
        protect_log_tree: false,
    }
}

#[test]
fn preview_write_rel_existing_is_modify() {
    let root = temp_root("mod");
    fs::write(root.join("a.txt"), "v1").unwrap();
    let tool = WriteFileTool::with_boundary(Arc::new(boundary(&root)));
    let fc = tool
        .preview(r#"{"path":"a.txt","content":"v2"}"#)
        .expect("in-root write must preview");
    assert!(
        matches!(fc.kind, FileChangeKind::Modify),
        "rel path to existing workspace file must preview Modify, got {:?}",
        fc.kind
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn preview_write_rel_missing_is_create() {
    let root = temp_root("new");
    let tool = WriteFileTool::with_boundary(Arc::new(boundary(&root)));
    let fc = tool
        .preview(r#"{"path":"new.txt","content":"v1"}"#)
        .expect("in-root write must preview");
    assert!(matches!(fc.kind, FileChangeKind::Create));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn preview_write_abs_existing_is_modify() {
    let root = temp_root("abs");
    let abs = root.join("a.txt");
    fs::write(&abs, "v1").unwrap();
    let tool = WriteFileTool::with_boundary(Arc::new(boundary(&root)));
    // JSON 里反斜杠是转义字符——测试用正斜杠形态（Windows API 同接受）。
    let args = format!(
        r#"{{"path":"{}","content":"v2"}}"#,
        abs.display().to_string().replace('\\', "/")
    );
    let fc = tool.preview(&args).expect("in-root write must preview");
    assert!(matches!(fc.kind, FileChangeKind::Modify));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn preview_write_outside_ws_returns_none() {
    let root = temp_root("out");
    let outside = std::env::temp_dir().join(format!(
        "nemesis_lt_prevkind_out_{}.txt",
        std::process::id()
    ));
    fs::write(&outside, "x").unwrap();
    let tool = WriteFileTool::with_boundary(Arc::new(boundary(&root)));
    let args = format!(r#"{{"path":"{}","content":"y"}}"#, outside.display());
    assert!(
        tool.preview(&args).is_none(),
        "fence-doomed out-of-root write must not preview a baseline"
    );
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_file(&outside);
}

#[test]
fn preview_edit_append_rel_existing_is_modify() {
    let root = temp_root("appmod");
    fs::write(root.join("a.txt"), "v1").unwrap();
    let tool = EditFileTool::with_boundary(Arc::new(boundary(&root)));
    let fc = tool
        .preview(r#"{"path":"a.txt","action":"append","content":"!"}"#)
        .expect("append must preview");
    assert!(matches!(fc.kind, FileChangeKind::Modify));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn preview_edit_append_rel_missing_is_create() {
    let root = temp_root("appnew");
    let tool = EditFileTool::with_boundary(Arc::new(boundary(&root)));
    let fc = tool
        .preview(r#"{"path":"b.txt","action":"append","content":"!"}"#)
        .expect("append must preview");
    assert!(matches!(fc.kind, FileChangeKind::Create));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn preview_edit_action_edit_is_modify() {
    let root = temp_root("edit");
    fs::write(root.join("a.txt"), "old").unwrap();
    let tool = EditFileTool::with_boundary(Arc::new(boundary(&root)));
    let fc = tool
        .preview(r#"{"path":"a.txt","old_text":"old","new_text":"new"}"#)
        .expect("edit must preview");
    assert!(matches!(fc.kind, FileChangeKind::Modify));
    let _ = fs::remove_dir_all(&root);
}

/// checkpoint 级闭环锁（数据破坏面的端到端钉死）：相对路径覆盖写必须
/// 捕获基线内容，rewind 还原**上一版**而非删除。旧代码此处 fc2.kind=Create
/// → 基线 content=None → restore 删除臂 → 文件丢失。
#[tokio::test]
async fn rewind_restores_rel_path_overwrite_baseline() {
    let root = temp_root("rt");
    let store = CheckpointStore::new(None, root.clone());
    let sess = "agent:prevkind/ws";
    let tool = WriteFileTool::with_boundary(Arc::new(boundary(&root)));

    // turn1：新建（Create，基线=不存在）。
    store.begin_for_session(1, "p1", Some(sess));
    let fc1 = tool
        .preview(r#"{"path":"a.txt","content":"v1"}"#)
        .expect("preview turn1");
    assert!(matches!(fc1.kind, FileChangeKind::Create));
    // 快照先于 execute（dispatch preview 阶段时序）。
    store.snapshot_for_session(&fc1, Some(sess)).await;
    fs::write(root.join("a.txt"), "v1").unwrap();
    store.seal_turn(1);

    // turn2：覆盖写（Modify，基线=现盘 v1）。
    store.begin_for_session(2, "p2", Some(sess));
    let fc2 = tool
        .preview(r#"{"path":"a.txt","content":"v2"}"#)
        .expect("preview turn2");
    assert!(
        matches!(fc2.kind, FileChangeKind::Modify),
        "overwrite of existing file must preview Modify (BUG-20261127-039)"
    );
    store.snapshot_for_session(&fc2, Some(sess)).await;
    fs::write(root.join("a.txt"), "v2").unwrap();
    store.seal_turn(2);

    // rewind：只移除 turn2 → 基线 v1 必须被**写回**（旧代码：文件被删）。
    let report = store.restore_code_scoped(2, Some(sess)).await;
    assert!(
        report.failures.is_empty(),
        "restore must succeed, failures={:?}",
        report.failures
    );
    assert!(
        report.written.iter().any(|p| p.ends_with("a.txt")),
        "baseline must be restored via write arm, written={:?} deleted={:?}",
        report.written,
        report.deleted
    );
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "v1",
        "rewind over overwrite turn must restore pre-turn content v1"
    );
    let _ = fs::remove_dir_all(&root);
}
