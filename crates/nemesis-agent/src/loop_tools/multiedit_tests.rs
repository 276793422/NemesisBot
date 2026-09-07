//! A7：multiedit 批量编辑单元测试。
//!
//! 覆盖：args 解析（缺字段/空数组/坏 JSON）/ **原子性**（任一失败整批不
//! 落盘 + 逐条状态回灌）/ 同文件多条编辑按序累积 / A4 级联命中标注 /
//! replace_all 条目 / 缺文件中止 / 无差异诚实标注 / preview_all 多点预检
//! （去重保序、坏 args 空、Modify kind）/ 边界形态（with_boundary 拦工作
//! 区外路径整批中止）/ 基线注册接线。全用绝对路径（不碰进程 cwd——并行
//! 测试全局状态竞争家族）。

use super::{MultiEditTool, WorkspaceBoundary, register_default_tools};
use crate::context::RequestContext;
use crate::r#loop::{FileChangeKind, Tool};

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

fn multiedit_args(edits: serde_json::Value) -> String {
    serde_json::json!({ "edits": edits }).to_string()
}

/// 绝对路径字符串（Windows 下 to_string_lossy 带反斜杠，PathBuf::from 原样
/// 接受；工具不走 shell，无转义问题）。
fn p(dir: &std::path::Path, name: &str) -> String {
    dir.join(name).to_string_lossy().to_string()
}

#[tokio::test]
async fn parse_rejects_empty_and_missing_fields() {
    let tool = MultiEditTool::default();
    // 空 edits 数组
    let err = tool
        .execute(&multiedit_args(serde_json::json!([])), &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("non-empty"), "err: {err}");
    // 缺 edits 字段
    let err = tool
        .execute(&serde_json::json!({}).to_string(), &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("'edits'"), "err: {err}");
    // 条目缺 old_text
    let err = tool
        .execute(
            &multiedit_args(serde_json::json!([{"path": "a.txt", "new_text": "x"}])),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("old_text"), "err: {err}");
    // 坏 JSON（截断）
    let err = tool.execute("{\"edits\": [", &ctx()).await.unwrap_err();
    assert!(err.contains("Invalid JSON"), "err: {err}");
}

#[tokio::test]
async fn atomic_success_edits_three_files_with_diffs() {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [
        ("a.txt", "hello world\n"),
        ("b.txt", "foo bar\n"),
        ("c.txt", "line one\nline two\n"),
    ] {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    let tool = MultiEditTool::default();
    let out = tool
        .execute(
            &multiedit_args(serde_json::json!([
                {"path": p(dir.path(), "a.txt"), "old_text": "world", "new_text": "rust"},
                {"path": p(dir.path(), "b.txt"), "old_text": "bar", "new_text": "baz"},
                {"path": p(dir.path(), "c.txt"), "old_text": "one", "new_text": "1"}
            ])),
            &ctx(),
        )
        .await
        .unwrap();

    assert!(
        out.contains("Multiedit complete: 3 edit(s) across 3 file(s)"),
        "out: {out}"
    );
    assert!(out.contains("1 edit(s)"), "out: {out}");
    assert!(out.contains("```diff"), "out: {out}");
    // 磁盘逐字验证
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hello rust\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "foo baz\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("c.txt")).unwrap(),
        "line 1\nline two\n"
    );
}

#[tokio::test]
async fn atomic_rollback_leaves_disk_untouched() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ok.txt"), "alpha beta\n").unwrap();
    // old_text 在该文件出现两次（多命中歧义路径）
    std::fs::write(dir.path().join("bad.txt"), "gamma\ngamma\n").unwrap();
    let tool = MultiEditTool::default();
    let err = tool
        .execute(
            &multiedit_args(serde_json::json!([
                {"path": p(dir.path(), "ok.txt"), "old_text": "alpha", "new_text": "ALPHA"},
                {"path": p(dir.path(), "bad.txt"), "old_text": "gamma", "new_text": "GAMMA"}
            ])),
            &ctx(),
        )
        .await
        .unwrap_err();

    // 首个失败详情 + 逐条状态
    assert!(err.contains("multiedit aborted at edit 2/2"), "err: {err}");
    assert!(err.contains("No files were modified"), "err: {err}");
    assert!(err.contains("1. "), "status list missing: {err}");
    assert!(err.contains("ok"), "status ok entry missing: {err}");
    assert!(err.contains("FAILED"), "status FAILED entry missing: {err}");
    assert!(err.contains("appears 2 times"), "err: {err}");
    // 磁盘零变化（原子性核心断言：先成功的编辑也不落盘）
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ok.txt")).unwrap(),
        "alpha beta\n",
        "successful earlier edit must NOT hit disk on batch abort"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("bad.txt")).unwrap(),
        "gamma\ngamma\n"
    );
}

#[tokio::test]
async fn same_file_edits_apply_cumulatively() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "one two three\n").unwrap();
    let tool = MultiEditTool::default();
    let out = tool
        .execute(
            &multiedit_args(serde_json::json!([
                {"path": p(dir.path(), "f.txt"), "old_text": "one", "new_text": "1"},
                {"path": p(dir.path(), "f.txt"), "old_text": "1 two", "new_text": "1+2"}
            ])),
            &ctx(),
        )
        .await
        .unwrap();

    // 第二条看到第一条的结果（"1 two" 只有在累积后才存在）
    assert!(out.contains("2 edit(s) across 1 file(s)"), "out: {out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        "1+2 three\n"
    );
}

#[tokio::test]
async fn cascade_level_reported_and_replace_all_works() {
    let dir = tempfile::tempdir().unwrap();
    // 行尾空白差异 → A4 line-trimmed 级联命中（确定性模糊 fixture）。
    std::fs::write(dir.path().join("c.txt"), "alpha   \nbeta\n").unwrap();
    std::fs::write(dir.path().join("r.txt"), "dup X dup X\n").unwrap();
    let tool = MultiEditTool::default();
    let out = tool
        .execute(
            &multiedit_args(serde_json::json!([
                {"path": p(dir.path(), "c.txt"), "old_text": "alpha\nbeta", "new_text": "ALPHA\nbeta"},
                {"path": p(dir.path(), "r.txt"), "old_text": "dup", "new_text": "DUP", "replace_all": true}
            ])),
            &ctx(),
        )
        .await
        .unwrap();

    assert!(
        out.contains("matched via"),
        "cascade level must be reported: {out}"
    );
    // replace_all：两处都换
    assert_eq!(
        std::fs::read_to_string(dir.path().join("r.txt")).unwrap(),
        "DUP X DUP X\n"
    );
}

#[tokio::test]
async fn missing_file_aborts_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("exists.txt"), "data\n").unwrap();
    let tool = MultiEditTool::default();
    let err = tool
        .execute(
            &multiedit_args(serde_json::json!([
                {"path": p(dir.path(), "ghost.txt"), "old_text": "a", "new_text": "b"}
            ])),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("File not found"), "err: {err}");
    assert!(err.contains("No files were modified"), "err: {err}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("exists.txt")).unwrap(),
        "data\n"
    );
}

#[tokio::test]
async fn no_change_edits_reported_honestly() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("same.txt"), "abc\n").unwrap();
    let tool = MultiEditTool::default();
    let out = tool
        .execute(
            &multiedit_args(serde_json::json!([
                {"path": p(dir.path(), "same.txt"), "old_text": "abc", "new_text": "abc"}
            ])),
            &ctx(),
        )
        .await
        .unwrap();
    // 替换为相同内容 = 应用成功但无差异 → 诚实标注 (no change)，磁盘不动
    assert!(out.contains("(no change)"), "out: {out}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("same.txt")).unwrap(),
        "abc\n"
    );
}

// ---------------------------------------------------------------------------
// preview_all（checkpoint 多点预检）
// ---------------------------------------------------------------------------

#[test]
fn preview_all_lists_unique_modify_changes_in_order() {
    let tool = MultiEditTool::default();
    let changes = tool.preview_all(&multiedit_args(serde_json::json!([
        {"path": "a.txt", "old_text": "1", "new_text": "2"},
        {"path": "b.txt", "old_text": "3", "new_text": "4"},
        {"path": "a.txt", "old_text": "5", "new_text": "6"}
    ])));
    assert_eq!(changes.len(), 2, "same-file edits dedupe to one snapshot");
    assert_eq!(changes[0].path, "a.txt");
    assert_eq!(changes[1].path, "b.txt");
    assert!(changes.iter().all(|c| c.kind == FileChangeKind::Modify));
}

#[test]
fn preview_all_empty_on_bad_args() {
    let tool = MultiEditTool::default();
    assert!(tool.preview_all("not json").is_empty());
    assert!(tool.preview_all("{\"edits\": []}").is_empty());
}

// ---------------------------------------------------------------------------
// 边界形态（with_boundary）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn boundary_rejects_outside_path_and_aborts_batch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("in.txt"), "inside\n").unwrap();
    std::fs::write(dir.path().join("out.txt"), "outside\n").unwrap();

    let tool = MultiEditTool::with_boundary(std::sync::Arc::new(WorkspaceBoundary {
        root: root.clone(),
        restrict: true,
    }));
    let err = tool
        .execute(
            &multiedit_args(serde_json::json!([
                {"path": "in.txt", "old_text": "inside", "new_text": "IN"},
                {"path": "../out.txt", "old_text": "outside", "new_text": "OUT"}
            ])),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("multiedit aborted"), "err: {err}");
    assert!(err.contains("No files were modified"), "err: {err}");
    // 盒内文件也没写（整批原子）
    assert_eq!(
        std::fs::read_to_string(root.join("in.txt")).unwrap(),
        "inside\n"
    );
}

// ---------------------------------------------------------------------------
// 注册面 + Tool trait 接线
// ---------------------------------------------------------------------------

#[test]
fn registered_by_default_and_preview_all_wired() {
    let tools = register_default_tools();
    assert!(
        tools.contains_key("multiedit"),
        "multiedit must be in baseline registry"
    );
    // dyn Tool 对象上 preview_all 覆盖生效（checkpoint 调用形态）。
    let tool = tools.get("multiedit").unwrap();
    let changes = tool.preview_all(&multiedit_args(serde_json::json!([
        {"path": "x.txt", "old_text": "a", "new_text": "b"}
    ])));
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "x.txt");
    assert_eq!(changes[0].kind, FileChangeKind::Modify);
    // 单点 preview 诚实 None（无从选代表文件）。
    assert!(
        tool.preview(&multiedit_args(serde_json::json!([
            {"path": "x.txt", "old_text": "a", "new_text": "b"}
        ])))
        .is_none()
    );
}

#[test]
fn tool_schema_shape() {
    let tool = MultiEditTool::default();
    let params = tool.parameters();
    assert_eq!(params["properties"]["edits"]["type"], "array");
    assert_eq!(
        params["properties"]["edits"]["items"]["required"],
        serde_json::json!(["path", "old_text", "new_text"])
    );
    assert_eq!(params["required"], serde_json::json!(["edits"]));
}
