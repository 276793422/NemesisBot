//! grep 显式 path 工作区边界回归（BUG-20261117-067）。
//!
//! 根因背景：GrepTool 只有 workspace 字符串、无边界闸——显式 `path`
//! 绝对路径不校验、相对 join 无包含检查。restrict=true（生产默认）下
//! read_file 拒绝的界外路径可被 grep 直接搜出匹配行内容（读族 fence
//! 结论分叉；同族缺口第一例 lsp_tool 同轮修复）。修复 = 显式 path
//! 统一走 resolve_tool_path（Read 族），与 read_file 同路径同结论。

use super::*;
use crate::context::RequestContext;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-grep-b", "u", "agent:convo-grep-b")
}

fn boundary_at(root: &std::path::Path, restrict: bool) -> std::sync::Arc<WorkspaceBoundary> {
    std::sync::Arc::new(WorkspaceBoundary {
        root: root.to_path_buf(),
        restrict,
        extra_roots: Vec::new(),
        editor_gate: None,
    })
}

/// restrict=true：绝对路径出界 → 入口拒绝（read_file 同路径已拒）。
#[tokio::test]
async fn grep_boundary_restrict_denies_absolute_outside() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "needle\n").unwrap();

    let tool = GrepTool::new(ws.path().to_string_lossy().to_string())
        .with_boundary(boundary_at(ws.path(), true));
    let err = tool
        .execute(
            &format!(
                r#"{{"pattern":"needle","path":{}}}"#,
                serde_json::to_string(outside.path().to_string_lossy().as_ref()).unwrap()
            ),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("outside the workspace"), "{err}");
}

/// restrict=true：相对路径 `..` 逃逸 → 入口拒绝。
#[tokio::test]
async fn grep_boundary_restrict_denies_relative_dotdot_escape() {
    let ws = tempfile::tempdir().unwrap();
    let tool = GrepTool::new(ws.path().to_string_lossy().to_string())
        .with_boundary(boundary_at(ws.path(), true));
    let err = tool
        .execute(r#"{"pattern":"needle","path":"sub/../../outside"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("outside the workspace"), "{err}");
}

/// restrict=true + 界内相对子目录：不误拒，正常搜出。
#[tokio::test]
async fn grep_boundary_restrict_allows_in_workspace_relative() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/a.rs"), "needle here\n").unwrap();

    let tool = GrepTool::new(ws.path().to_string_lossy().to_string())
        .with_boundary(boundary_at(ws.path(), true));
    let out = tool
        .execute(r#"{"pattern":"needle","path":"src"}"#, &ctx())
        .await
        .unwrap();
    assert!(out.contains("a.rs"), "界内相对子目录必须正常搜出: {out}");
}

/// restrict=false：界外路径不拦（旧行为保持，用户显式关闸）。
#[tokio::test]
async fn grep_boundary_unrestrict_keeps_outside_allowed() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("other.txt"), "needle\n").unwrap();

    let tool = GrepTool::new(ws.path().to_string_lossy().to_string())
        .with_boundary(boundary_at(ws.path(), false));
    let out = tool
        .execute(
            &format!(
                r#"{{"pattern":"needle","path":{}}}"#,
                serde_json::to_string(outside.path().to_string_lossy().as_ref()).unwrap()
            ),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(
        out.contains("other.txt"),
        "restrict=false 界外应可搜: {out}"
    );
}

/// 无边界形态（基线/测试注册）：旧行为逐字节保持——界外绝对路径可搜。
#[tokio::test]
async fn grep_no_boundary_legacy_outside_allowed() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("legacy.txt"), "needle\n").unwrap();

    let tool = GrepTool::new(ws.path().to_string_lossy().to_string());
    let out = tool
        .execute(
            &format!(
                r#"{{"pattern":"needle","path":{}}}"#,
                serde_json::to_string(outside.path().to_string_lossy().as_ref()).unwrap()
            ),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(
        out.contains("legacy.txt"),
        "无界基线形态旧行为必须保持: {out}"
    );
}
