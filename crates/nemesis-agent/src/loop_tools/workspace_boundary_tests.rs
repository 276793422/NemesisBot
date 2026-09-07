//! A5（2026-09-04 devtool-upgrade 阶段 2）测试：文件工具工作区边界。
//!
//! 覆盖目标：
//! - `validate_workspace_path` 校验矩阵：界内绝对/相对/深层新文件、界外
//!   绝对、`..` 穿越逃逸、restrict=false 全放行、平台归一化（Windows 大小
//!   写不敏感 / Unix symlink 界内可达 + symlink 逃逸拒绝）。
//! - 工具级带界形态：write_file 拒外放内、覆盖回执 + diff 块、新建无
//!   覆盖标记、同内容覆盖无 diff 块；edit_file / append_file 拒外。
//! - 无界默认形态向后兼容锚：`::default()` 照旧可写界外（基线注册 / 既有
//!   测试不变）。
//! - `register_shared_tools` 注入接线：带界 config 注册出的 write_file
//!   在注册表里就是带界形态（拒绝界外写入）。

use super::Tool;
use crate::context::RequestContext;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::WorkspaceBoundary;
use super::validate_workspace_path;

fn ctx() -> RequestContext {
    RequestContext {
        channel: "web".to_string(),
        chat_id: "chat".to_string(),
        user: "u".to_string(),
        session_key: "agent:test/ws_boundary".to_string(),
        correlation_id: None,
        async_callback: None,
    }
}

fn ps(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_lt_wsb_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn boundary(root: &Path, restrict: bool) -> WorkspaceBoundary {
    WorkspaceBoundary {
        root: root.to_path_buf(),
        restrict,
    }
}

// ---------------------------------------------------------------------------
// validate_workspace_path 校验矩阵
// ---------------------------------------------------------------------------

#[test]
fn boundary_inside_absolute_ok() {
    let root = temp_dir("in");
    let target = root.join("file.txt");
    let out = validate_workspace_path(&ps(&target), &boundary(&root, true)).unwrap();
    assert_eq!(out, target);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn boundary_outside_absolute_denied() {
    let root = temp_dir("out");
    let other = temp_dir("other_root");
    let target = other.join("secret.txt");
    let err = validate_workspace_path(&ps(&target), &boundary(&root, true)).unwrap_err();
    assert!(err.contains("access denied"), "{err}");
    assert!(err.contains("outside the workspace"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn boundary_relative_joined_under_root() {
    let root = temp_dir("rel");
    let out = validate_workspace_path("sub/dir/file.txt", &boundary(&root, true)).unwrap();
    assert_eq!(out, root.join("sub/dir/file.txt"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn boundary_deep_new_file_inside_ok() {
    // 写目标尚不存在（write_file 语义：建目录再写）：最长存在祖先解析
    // 必须把它判为界内，而不是 ENOENT 误拒。
    let root = temp_dir("deep");
    let target = root.join("a/b/c/new.txt");
    assert!(validate_workspace_path(&ps(&target), &boundary(&root, true)).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn boundary_traversal_escape_denied() {
    let root = temp_dir("trav");
    let target = root.join("..").join("outside.txt");
    let err = validate_workspace_path(&ps(&target), &boundary(&root, true)).unwrap_err();
    assert!(err.contains("access denied"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn boundary_restrict_false_allows_outside() {
    // restrict=false：不设界（管理面显式放开），界外绝对路径原样放行。
    let root = temp_dir("norestrict");
    let other = temp_dir("other_root2");
    let target = other.join("ok.txt");
    let out = validate_workspace_path(&ps(&target), &boundary(&root, false)).unwrap();
    assert_eq!(out, target);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

#[cfg(windows)]
#[test]
fn boundary_case_insensitive_on_windows() {
    // Windows 路径大小写不敏感：大写形态的界内路径必须命中
    // （canonicalize_for_compare 归一化，裸字符串前缀比较会误拒）。
    let root = temp_dir("case");
    let target = root.join("file.txt").to_string_lossy().to_uppercase();
    assert!(validate_workspace_path(&target, &boundary(&root, true)).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn boundary_symlink_inside_ok_and_escape_denied() {
    use std::os::unix::fs::symlink;
    let root = temp_dir("sym");
    let inside = root.join("real");
    std::fs::create_dir_all(&inside).unwrap();
    symlink(&inside, root.join("link")).unwrap();
    let outside = temp_dir("sym_out");
    symlink(&outside, root.join("escape")).unwrap();

    // symlink 指向界内 → 可达。
    let t1 = root.join("link").join("f.txt");
    assert!(validate_workspace_path(&ps(&t1), &boundary(&root, true)).is_ok());
    // symlink 指向界外 → 解析后拒绝（不是被表面路径骗过）。
    let t2 = root.join("escape").join("x.txt");
    let err = validate_workspace_path(&ps(&t2), &boundary(&root, true)).unwrap_err();
    assert!(err.contains("access denied"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// 工具级带界形态
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_file_with_boundary_denies_outside() {
    let root = temp_dir("wf_out");
    let other = temp_dir("wf_other");
    let target = other.join("evil.txt");
    let args = serde_json::json!({"path": ps(&target), "content": "hi"}).to_string();
    let tool = super::WriteFileTool::with_boundary(Arc::new(boundary(&root, true)));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("access denied"), "{err}");
    assert!(!target.exists(), "界外文件不得被创建");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

#[tokio::test]
async fn write_file_with_boundary_allows_inside() {
    let root = temp_dir("wf_in");
    let target = root.join("sub").join("ok.txt");
    let args = serde_json::json!({"path": ps(&target), "content": "body"}).to_string();
    let tool = super::WriteFileTool::with_boundary(Arc::new(boundary(&root, true)));
    let out = tool.execute(&args, &ctx()).await.unwrap();
    assert!(!out.contains("overwrote"), "新建文件不应有覆盖标记: {out}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "body");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn write_file_overwrite_reports_diff() {
    let root = temp_dir("wf_diff");
    let target = root.join("existed.txt");
    std::fs::write(&target, "old line\n").unwrap();
    let args = serde_json::json!({"path": ps(&target), "content": "new line\n"}).to_string();
    let tool = super::WriteFileTool::with_boundary(Arc::new(boundary(&root, true)));
    let out = tool.execute(&args, &ctx()).await.unwrap();
    assert!(out.contains("(overwrote existing)"), "{out}");
    assert!(out.contains("```diff"), "{out}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new line\n");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn write_file_identical_overwrite_no_diff_block() {
    let root = temp_dir("wf_same");
    let target = root.join("same.txt");
    std::fs::write(&target, "stable\n").unwrap();
    let args = serde_json::json!({"path": ps(&target), "content": "stable\n"}).to_string();
    let tool = super::WriteFileTool::with_boundary(Arc::new(boundary(&root, true)));
    let out = tool.execute(&args, &ctx()).await.unwrap();
    assert!(out.contains("(overwrote existing)"), "{out}");
    assert!(
        !out.contains("```diff"),
        "无变化时不应输出空 diff 块: {out}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn edit_file_with_boundary_denies_outside() {
    let root = temp_dir("ef_out");
    let other = temp_dir("ef_other");
    let target = other.join("t.txt");
    std::fs::write(&target, "alpha\n").unwrap();
    let args = serde_json::json!({"path": ps(&target), "old_text": "alpha", "new_text": "beta"})
        .to_string();
    let tool = super::EditFileTool::with_boundary(Arc::new(boundary(&root, true)));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("access denied"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "alpha\n",
        "界外文件不得被改"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

#[tokio::test]
async fn append_file_with_boundary_denies_outside() {
    let root = temp_dir("af_out");
    let other = temp_dir("af_other");
    let target = other.join("t.txt");
    std::fs::write(&target, "keep\n").unwrap();
    let args = serde_json::json!({"path": ps(&target), "content": "more\n"}).to_string();
    let tool = super::AppendFileTool::with_boundary(Arc::new(boundary(&root, true)));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("access denied"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "keep\n",
        "界外文件不得被追加"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}

#[tokio::test]
async fn default_tools_without_boundary_keep_absolute_access() {
    // 向后兼容锚：无界默认形态（register_default_tools / 既有测试 / 老调用
    // 方）照旧可写任意路径——A5 只在生产装配点注入边界。
    let other = temp_dir("wf_default");
    let target = other.join("legacy.txt");
    let args = serde_json::json!({"path": ps(&target), "content": "x"}).to_string();
    let out = super::WriteFileTool::default().execute(&args, &ctx()).await;
    assert!(out.is_ok(), "{out:?}");
    let _ = std::fs::remove_dir_all(&other);
}

// ---------------------------------------------------------------------------
// register_shared_tools 注入接线
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_shared_tools_inserts_bounded_write_file() {
    let root = temp_dir("reg");
    let other = temp_dir("reg_other");
    let cfg = super::SharedToolConfig {
        workspace_boundary: Some(Arc::new(boundary(&root, true))),
        ..Default::default()
    };
    let tools = super::register_shared_tools(&cfg);
    let tool = tools.get("write_file").expect("write_file 必须注册");
    // 界外 → 注册表里的带界形态拒绝。
    let outside = serde_json::json!({"path": ps(&other.join("x.txt")), "content": "x"}).to_string();
    let err = tool.execute(&outside, &ctx()).await.unwrap_err();
    assert!(err.contains("access denied"), "{err}");
    // 界内 → 放行落盘。
    let inside = serde_json::json!({"path": ps(&root.join("y.txt")), "content": "y"}).to_string();
    tool.execute(&inside, &ctx()).await.unwrap();
    assert_eq!(std::fs::read_to_string(root.join("y.txt")).unwrap(), "y");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}
