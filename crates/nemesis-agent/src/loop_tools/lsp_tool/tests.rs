//! Tests for the `lsp` agent tool (L1 / U19). Registration semantics are
//! tested purely via `registration_plan` (acceptance ②); execute-side
//! validation errors are exercised without spawning any server (they fire
//! before the spawn path in `LspManager::query`).

use super::*;
use crate::context::RequestContext;

fn test_ctx() -> RequestContext {
    RequestContext {
        channel: "web".to_string(),
        chat_id: "chat".to_string(),
        user: "u".to_string(),
        session_key: "agent:test/session".to_string(),
        correlation_id: None,
        async_callback: None,
    }
}

/// Acceptance ②: the registration policy is exactly "config opted in AND
/// at least one language server found" — every other combination must NOT
/// register the tool.
#[test]
fn registration_plan_matrix() {
    assert!(!LspTool::registration_plan(false, 0), "disabled + none");
    assert!(!LspTool::registration_plan(false, 3), "disabled + servers");
    assert!(!LspTool::registration_plan(true, 0), "enabled + NO server");
    assert!(LspTool::registration_plan(true, 1), "enabled + one server");
    assert!(
        LspTool::registration_plan(true, 5),
        "enabled + many servers"
    );
}

#[test]
fn schema_documents_all_ops_with_new_name() {
    let t = LspTool::new(None, None);
    let p = t.parameters();
    let required: Vec<&str> = p["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for field in ["op", "path", "line", "character"] {
        assert!(
            required.contains(&field),
            "schema must require {field}: {p}"
        );
        assert!(
            p["properties"][field].is_object(),
            "schema must document {field}"
        );
    }
    // C7：new_name 是 rename 专用可选参数（不在 required——其余 op 不用）。
    assert!(
        p["properties"]["new_name"].is_object(),
        "schema must document new_name: {p}"
    );
    assert!(!required.contains(&"new_name"));
    let ops: Vec<&str> = p["properties"]["op"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        ops,
        vec![
            "definition",
            "references",
            "implementation",
            "hover",
            "rename",
            "code_action"
        ]
    );
    // 0-based convention must be stated — models default to 1-based.
    assert!(p.to_string().contains("0-based"));
    assert!(t.description().contains("语义"));
    // C7：描述必须如实暴露 rename 的安全语义（不再是纯只读工具）。
    assert!(t.description().contains("rename"));
    assert!(t.description().contains("安全审批"));
}

/// Unknown op is rejected with the valid set spelled out — no server
/// needed (validation happens before any spawn).
#[tokio::test]
async fn execute_rejects_unknown_op_listing_valid() {
    let t = LspTool::new(None, None);
    let err = t
        .execute(
            r#"{"op":"goto","path":"/x/a.rs","line":0,"character":0}"#,
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Invalid 'op'"), "{err}");
    for valid in [
        "definition",
        "references",
        "implementation",
        "hover",
        "rename",
        "code_action",
    ] {
        assert!(err.contains(valid), "error should list {valid}: {err}");
    }
}

/// C7：rename 缺 new_name 在任何服务器交互之前被点名（new_name 提取先于
/// manager 前置检查）。
#[tokio::test]
async fn execute_rename_without_new_name_errors() {
    let t = LspTool::new(None, None);
    let err = t
        .execute(
            r#"{"op":"rename","path":"/x/a.rs","line":0,"character":0}"#,
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("'new_name'"), "{err}");
}

/// Missing file: clear error, no server spawned (the is_file check runs
/// before the PATH probe in `LspManager::query`).
#[tokio::test]
async fn execute_rejects_missing_file() {
    let t = LspTool::new(None, None);
    let missing = if cfg!(windows) {
        "Z:/definitely/missing/a.rs"
    } else {
        "/definitely/missing/a.rs"
    };
    let err = t
        .execute(
            &format!(r#"{{"op":"definition","path":{missing:?},"line":0,"character":0}}"#),
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("file does not exist"), "{err}");
}

/// Unsupported file types list the supported languages (actionable error,
/// not a bare panic).
#[tokio::test]
async fn execute_rejects_unsupported_file_type() {
    let t = LspTool::new(None, None);
    let missing = if cfg!(windows) {
        "Z:/definitely/missing/a.md"
    } else {
        "/definitely/missing/a.md"
    };
    let err = t
        .execute(
            &format!(r#"{{"op":"hover","path":{missing:?},"line":0,"character":0}}"#),
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("unsupported file type"), "{err}");
    assert!(err.contains("rust"), "{err}");
}

/// Non-integer line/character are rejected (as_u64 misses bools/strings).
#[tokio::test]
async fn execute_rejects_non_integer_positions() {
    let t = LspTool::new(None, None);
    let err = t
        .execute(
            r#"{"op":"hover","path":"/x/a.rs","line":"zero","character":0}"#,
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("line"), "{err}");
}

/// Malformed JSON args surface the parse error (model sees its own typo).
#[tokio::test]
async fn execute_rejects_invalid_json_args() {
    let t = LspTool::new(None, None);
    let err = t.execute("{not json", &test_ctx()).await.unwrap_err();
    assert!(err.contains("Invalid arguments"), "{err}");
}

/// Missing required fields are named individually (op / path).
#[tokio::test]
async fn execute_rejects_missing_op_and_missing_path() {
    let t = LspTool::new(None, None);
    let err = t
        .execute(r#"{"path":"/x/a.rs","line":0,"character":0}"#, &test_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("Missing 'op'"), "{err}");

    let err = t
        .execute(r#"{"op":"hover","line":0,"character":0}"#, &test_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("Missing 'path'"), "{err}");
}

/// Positions beyond u32 are rejected explicitly instead of truncating
/// silently (a wrapped 2^32+offset would query the wrong line).
#[tokio::test]
async fn execute_rejects_position_overflow() {
    let t = LspTool::new(None, None);
    let err = t
        .execute(
            r#"{"op":"hover","path":"/x/a.rs","line":4294967296,"character":0}"#,
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("'line' out of range"), "{err}");

    let err = t
        .execute(
            r#"{"op":"hover","path":"/x/a.rs","line":0,"character":99999999999}"#,
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("'character' out of range"), "{err}");
}

// ---------------------------------------------------------------------------
// C5 (2026-09-04 devtool-upgrade 阶段 1): 外部 manager 单例接线
// ---------------------------------------------------------------------------

/// with_manager 必须持外部传入的同一个 Arc（gateway shutdown_all 才能收尸
/// LspTool 会话——单例的意义就在 ptr_eq）。
#[test]
fn with_manager_shares_the_singleton() {
    let mgr = std::sync::Arc::new(nemesis_lsp::LspManager::new(None, None));
    let t = LspTool::with_manager(std::sync::Arc::clone(&mgr));
    assert!(
        std::sync::Arc::ptr_eq(t.manager(), &mgr),
        "LspTool must hold the externally provided manager Arc"
    );
}

/// 默认构造（兜底路径）自建 manager——与 C5 前行为一致，不碰全局状态。
#[test]
fn new_builds_its_own_manager() {
    let t = LspTool::new(None, None);
    let standalone = std::sync::Arc::new(nemesis_lsp::LspManager::new(None, None));
    assert!(
        !std::sync::Arc::ptr_eq(t.manager(), &standalone),
        "LspTool::new is the self-built fallback, not a global singleton"
    );
}

/// 注册侧：未启用时（默认路径）即使给了 manager 也不注册——enabled 闸先于
/// manager 消费，且不触发 PATH 探测（机器无关，CI 稳定）。
#[test]
fn register_disabled_ignores_injected_manager() {
    use crate::loop_tools::SharedToolConfig;
    let mgr = std::sync::Arc::new(nemesis_lsp::LspManager::new(None, None));
    let cfg = SharedToolConfig {
        lsp_tool_enabled: false,
        lsp_manager: Some(mgr),
        ..Default::default()
    };
    let tools = crate::loop_tools::register_shared_tools(&cfg);
    assert!(
        !tools.contains_key("lsp"),
        "disabled config must not register lsp even with an injected manager"
    );
}

/// SharedToolConfig Default：lsp_manager 缺省 None（独立 runner 兜底路径）。
#[test]
fn shared_tool_config_default_has_no_manager() {
    assert!(
        crate::loop_tools::SharedToolConfig::default()
            .lsp_manager
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// C7 (2026-09-06 devtool-upgrade 阶段 6): rename 两阶段安全闸（gate_and_write）
// ---------------------------------------------------------------------------

/// 两文件 fixture：gate_and_write 的输入 + 磁盘断言用旧内容。
fn c7_two_files() -> (tempfile::TempDir, Vec<nemesis_lsp::AppliedFileEdit>) {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.rs");
    let b = dir.path().join("b.rs");
    std::fs::write(&a, "old a\n").unwrap();
    std::fs::write(&b, "old b\n").unwrap();
    let files = vec![
        nemesis_lsp::AppliedFileEdit {
            path: a.to_string_lossy().into_owned(),
            new_text: "new a\n".to_string(),
            edit_count: 1,
        },
        nemesis_lsp::AppliedFileEdit {
            path: b.to_string_lossy().into_owned(),
            new_text: "new b\n".to_string(),
            edit_count: 2,
        },
    ];
    (dir, files)
}

/// 无闸（插件未注入）：直接落盘——gate 缺席≠gate 通过，与无闸普通写工具
/// 同语义；返回值如实列出已写文件。
#[test]
fn c7_gate_and_write_none_plugin_writes_all() {
    let (_dir, files) = c7_two_files();
    let written = gate_and_write(&files, None, "web").unwrap();
    assert_eq!(written.len(), 2);
    assert_eq!(std::fs::read_to_string(&files[0].path).unwrap(), "new a\n");
    assert_eq!(std::fs::read_to_string(&files[1].path).unwrap(), "new b\n");
}

/// allow-all 插件：合成 write_file 调用过完整 8 层管线后放行 → 两文件都写。
/// multi_thread flavor：execute 的放行路径走审计链 block_on（pipeline.rs），
/// current_thread runtime 下会 panic——生产调用点（execute_rename）运行在
/// gateway 的 multi-thread runtime 上，同构。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c7_gate_and_write_allow_all_plugin_writes_all() {
    let audit_dir = tempfile::tempdir().unwrap();
    let plugin = allow_all_plugin(&audit_dir.path().join("audit.log"));
    let (_dir, files) = c7_two_files();
    let written = gate_and_write(&files, Some(&plugin), "web").unwrap();
    assert_eq!(written.len(), 2);
    assert_eq!(std::fs::read_to_string(&files[0].path).unwrap(), "new a\n");
    assert_eq!(std::fs::read_to_string(&files[1].path).unwrap(), "new b\n");
}

/// deny 规则命中任一文件 → **一个字节都不落盘**（两阶段：gate 全部先于
/// write 全部——半改名状态是破碎代码）。错误带 SECURITY BLOCKED 前缀 +
/// 明示未修改。
#[cfg(feature = "security")]
#[tokio::test]
async fn c7_gate_and_write_deny_blocks_everything() {
    use nemesis_security::types::SecurityRule;
    let audit_dir = tempfile::tempdir().unwrap();
    let plugin = {
        use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
        SecurityPlugin::new(SecurityPluginConfig {
            enabled: true,
            default_action: "allow".to_string(),
            audit_chain_enabled: true,
            audit_chain_path: Some(
                audit_dir
                    .path()
                    .join("audit.log")
                    .to_string_lossy()
                    .into_owned(),
            ),
            file_rules: vec![SecurityRule {
                pattern: "*.rs".to_string(),
                action: "deny".to_string(),
                comment: "C7 test: deny rs writes".to_string(),
            }],
            ..Default::default()
        })
    };
    let (_dir, files) = c7_two_files();
    let err = gate_and_write(&files, Some(&plugin), "web").unwrap_err();
    assert!(err.contains("SECURITY BLOCKED"), "{err}");
    assert!(err.contains("no files were modified"), "{err}");
    assert_eq!(std::fs::read_to_string(&files[0].path).unwrap(), "old a\n");
    assert_eq!(std::fs::read_to_string(&files[1].path).unwrap(), "old b\n");
}

#[cfg(feature = "security")]
fn allow_all_plugin(audit_path: &std::path::Path) -> nemesis_security::pipeline::SecurityPlugin {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        default_action: "allow".to_string(),
        audit_chain_enabled: true,
        audit_chain_path: Some(audit_path.to_string_lossy().into_owned()),
        ..Default::default()
    })
}
