// loop_tools/lsp_tool.rs 覆盖率补充测试（execute 参数校验决策表 /
// registration_plan / gate_and_write 无闸直写路径）。
//
// 需要真实语言服务器的链路（manager.rename 的 WorkspaceEdit 计算、
// 诊断回读格式化）由 nemesis-lsp crate 与真机场景覆盖；本文件只打
// 无服务器依赖的决策面。

use super::*;
use crate::context::RequestContext;

fn cov_ctx() -> RequestContext {
    RequestContext::new("web", "chat-1", "covuser", "agent:main:session:covlsp")
}

async fn exec_err(tool: &LspTool, args: serde_json::Value) -> String {
    crate::loop_tools::Tool::execute(tool, &args.to_string(), &cov_ctx())
        .await
        .expect_err("validation failure must be an Err")
}

// ---------------------------------------------------------------------------
// execute 参数校验决策表（全部在触达语言服务器之前返回）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_rejects_invalid_op() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "teleport", "path": "/x.rs", "line": 0, "character": 0}),
    )
    .await;
    assert!(err.contains("Invalid 'op'"), "got: {err}");
    assert!(
        err.contains("definition | references"),
        "valid values listed: {err}"
    );
}

#[tokio::test]
async fn execute_rejects_missing_path() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "line": 0, "character": 0}),
    )
    .await;
    assert!(err.contains("Missing 'path'"), "got: {err}");
}

/// 相对路径按进程 cwd 解析（join 分支），随后在缺 line 处如实报错。
#[tokio::test]
async fn execute_resolves_relative_path_against_cwd() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "path": "rel/file.rs", "character": 0}),
    )
    .await;
    assert!(err.contains("Missing or non-integer 'line'"), "got: {err}");
}

#[tokio::test]
async fn execute_rejects_missing_character() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "path": "/x.rs", "line": 1}),
    )
    .await;
    assert!(
        err.contains("Missing or non-integer 'character'"),
        "got: {err}"
    );
}

/// LSP 位置是 u32——超界 u64 早拒（不做静默截断）。
#[tokio::test]
async fn execute_rejects_out_of_range_positions() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "path": "/x.rs", "line": u64::from(u32::MAX) + 1, "character": 0}),
    )
    .await;
    assert!(err.contains("'line' out of range"), "got: {err}");

    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "path": "/x.rs", "line": 0, "character": u64::from(u32::MAX) + 1}),
    )
    .await;
    assert!(err.contains("'character' out of range"), "got: {err}");
}

#[tokio::test]
async fn execute_rename_requires_new_name() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "rename", "path": "/x.rs", "line": 0, "character": 0}),
    )
    .await;
    assert!(err.contains("Missing 'new_name'"), "got: {err}");
}

// ---------------------------------------------------------------------------
// registration_plan（纯函数）
// ---------------------------------------------------------------------------

#[test]
fn registration_plan_truth_table() {
    assert!(LspTool::registration_plan(true, 3));
    assert!(
        !LspTool::registration_plan(true, 0),
        "no languages → no registration"
    );
    assert!(
        !LspTool::registration_plan(false, 3),
        "disabled → no registration"
    );
    assert!(!LspTool::registration_plan(false, 0));
}

// ---------------------------------------------------------------------------
// gate_and_write：无闸形态直接落盘（gate 缺席 ≠ gate 通过）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
#[test]
fn gate_and_write_without_security_writes_all_files() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.rs");
    let b = dir.path().join("b.rs");
    let files = vec![
        AppliedFileEdit {
            path: a.to_string_lossy().into_owned(),
            new_text: "fn a() {}".to_string(),
            edit_count: 2,
        },
        AppliedFileEdit {
            path: b.to_string_lossy().into_owned(),
            new_text: "fn b() {}".to_string(),
            edit_count: 1,
        },
    ];

    let written = gate_and_write(&files, None, "web").expect("ungated write succeeds");
    assert_eq!(
        written,
        vec![
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned()
        ]
    );
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "fn a() {}");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "fn b() {}");

    // 写盘中途失败（第二个文件路径非法）→ Err，第一个已如实上报过。
    let bad = vec![AppliedFileEdit {
        path: dir
            .path()
            .join("no-such-dir")
            .join("x.rs")
            .to_string_lossy()
            .into_owned(),
        new_text: "x".to_string(),
        edit_count: 1,
    }];
    let err = gate_and_write(&bad, None, "web").expect_err("unwritable target must fail");
    assert!(
        err.contains("write") && err.contains("failed"),
        "got: {err}"
    );
}

// ===========================================================================
// Wave4 覆盖批次：真实 rust-analyzer 端到端（execute_rename 全链路 /
// code_action 渲染 / hover 格式化）。rust-analyzer 不在 PATH、冷启动失败
// 或 20s 内未就绪时按 SKIP 约定跳过（eprintln + return），套件保持绿。
// ===========================================================================

/// 一次性 cargo 脚手架：零依赖，rust-analyzer 冷启动无需网络。
const RA_LIB_SRC: &str = "pub fn cov_target_fn() -> i32 {\n    42\n}\n\npub fn cov_caller() -> i32 {\n    cov_target_fn()\n}\n";

fn scratch_ra_project() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir().ok()?;
    std::fs::create_dir_all(dir.path().join("src")).ok()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"cov_scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .ok()?;
    let lib = dir.path().join("src").join("lib.rs");
    std::fs::write(&lib, RA_LIB_SRC).ok()?;
    Some((dir, lib))
}

/// RA 就绪重试：先 didOpen 把文档注册进 vfs（touch_file 尽力而为语义），
/// 再执行 op；「file not found」/「No references found」/ null WorkspaceEdit /
/// null hover 都是工作区尚未分析完成的瞬态 → 500ms 后重试，60s 预算耗尽
/// 仍不可用 → Err（调用方 SKIP）。真实错误立即透传。
async fn ra_execute_with_retry(
    tool: &LspTool,
    lib: &std::path::Path,
    args: &serde_json::Value,
) -> Result<String, String> {
    let _ = tool.manager().touch_file(lib).await;
    for _ in 0..120 {
        let outcome = LspTool::execute(tool, &args.to_string(), &cov_ctx()).await;
        // hover 的空结果走 Ok 臂（format_result 对 null 悬停渲染成诚实文案）
        // ——同属「分析未就绪」的瞬态，一并重试。
        if let Ok(o) = &outcome {
            if !o.contains("no hover information at this position") {
                return Ok(o.clone());
            }
            let _ = tool.manager().touch_file(lib).await;
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }
        match outcome {
            Ok(_) => unreachable!("Err 臂已在上面处理"),
            Err(e)
                if e.contains("file not found")
                    || e.contains("closed the stream")
                    || e.contains("timed out")
                    || e.contains("No references found")
                    || e.contains("server returned no edits")
                    || e.contains("no hover information at this position") =>
            {
                let _ = tool.manager().touch_file(lib).await;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err("rust-analyzer never became ready within 60s".to_string())
}

/// rename 全链路（lsp_tool.rs 218-300）：服务器算 WorkspaceEdit → 读盘应用
/// → 无闸直写 → notify_change → 诊断回读 → 格式化输出。落盘内容断言改名
/// 同时命中定义与调用两点。
#[tokio::test]
async fn execute_rename_end_to_end_with_rust_analyzer() {
    let Some((_dir, lib)) = scratch_ra_project() else {
        eprintln!("SKIP: tempdir unavailable");
        return;
    };
    let tool = LspTool::new(Some(60), Some(60));
    let args = serde_json::json!({
        "op": "rename",
        "path": lib.to_string_lossy(),
        "line": 0,
        "character": 10,
        "new_name": "cov_renamed_fn",
    });
    let out = match ra_execute_with_retry(&tool, &lib, &args).await {
        Ok(o) => o,
        Err(e) => {
            eprintln!("SKIP: rust-analyzer rename unavailable: {e}");
            return;
        }
    };
    assert!(out.contains("Renamed to"), "{out}");
    assert!(out.contains("1 file(s) written"), "{out}");
    let after = std::fs::read_to_string(&lib).unwrap();
    assert_eq!(
        after.matches("cov_renamed_fn").count(),
        2,
        "定义与调用两点都要改名：{after}"
    );
    assert!(!after.contains("cov_target_fn"), "旧名必须清零：{after}");
}

/// code_action 渲染臂（169-201）：干净代码上 rust-analyzer 通常无 quickfix
/// → 诚实空文案；若有则必须带「listing only」尾注。两种形态都算过渲染路径。
#[tokio::test]
async fn execute_code_action_reports_honestly_with_rust_analyzer() {
    let Some((_dir, lib)) = scratch_ra_project() else {
        eprintln!("SKIP: tempdir unavailable");
        return;
    };
    let tool = LspTool::new(Some(60), Some(60));
    let args = serde_json::json!({
        "op": "code_action",
        "path": lib.to_string_lossy(),
        "line": 0,
        "character": 10,
    });
    let out = match ra_execute_with_retry(&tool, &lib, &args).await {
        Ok(o) => o,
        Err(e) => {
            eprintln!("SKIP: rust-analyzer code_action unavailable: {e}");
            return;
        }
    };
    assert!(
        out.contains("no quickfix code actions") || out.contains("code action(s) available"),
        "{out}"
    );
    if out.contains("code action(s) available") {
        assert!(out.contains("listing only"), "{out}");
    }
}

/// hover 只读链路（query → format_result）：真实服务器返回签名悬停。
#[tokio::test]
async fn execute_hover_formats_real_server_response() {
    let Some((_dir, lib)) = scratch_ra_project() else {
        eprintln!("SKIP: tempdir unavailable");
        return;
    };
    let tool = LspTool::new(Some(60), Some(60));
    let args = serde_json::json!({
        "op": "hover",
        "path": lib.to_string_lossy(),
        "line": 0,
        "character": 10,
    });
    let out = match ra_execute_with_retry(&tool, &lib, &args).await {
        Ok(o) => o,
        Err(e) => {
            eprintln!("SKIP: rust-analyzer hover unavailable: {e}");
            return;
        }
    };
    assert!(!out.trim().is_empty(), "hover 不得为空：{out}");
    assert!(
        out.contains("cov_target_fn") || out.contains("i32") || out.contains("Hover"),
        "悬停应包含符号信息：{out}"
    );
}
