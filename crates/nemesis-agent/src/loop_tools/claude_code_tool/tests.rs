//! Tests for the Claude Code delegation tool (H7 / U13 half; T5 permission
//! tier + shared-layer refactor).

use super::*;
use crate::context::RequestContext;
use crate::loop_tools::cli_delegation::tests::FakeArgEchoCli;

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

#[test]
fn test_claude_code_tool_schema() {
    let t = ClaudeCodeTool::new("C:/fake/claude.exe".into(), None, None);
    let p = t.parameters();
    assert!(p["properties"]["prompt"].is_object());
    assert_eq!(p["required"][0], "prompt");
    assert!(t.description().contains("Claude Code"));
}

/// T5: the permission mode is a FIXED config tier — it must NOT appear in the
/// model-facing schema.
#[test]
fn test_claude_code_permission_mode_not_in_schema() {
    let t = ClaudeCodeTool::new("C:/fake/claude.exe".into(), None, Some("plan"));
    let schema = t.parameters().to_string();
    assert!(
        !schema.contains("permission"),
        "permission mode must not be model-selectable: {schema}"
    );
}

/// T5: tier normalization — None → default acceptEdits; explicit camelCase
/// values pass through; legacy snake_case maps over; unknown → default.
/// （值集与 claude CLI 2.1.240 `--help` 实测对齐——V4 真机修正。）
#[test]
fn test_claude_code_permission_mode_normalization() {
    assert_eq!(
        ClaudeCodeTool::new("C:/fake".into(), None, None).permission_mode(),
        "acceptEdits"
    );
    assert_eq!(
        ClaudeCodeTool::new("C:/fake".into(), None, Some("plan")).permission_mode(),
        "plan"
    );
    assert_eq!(
        ClaudeCodeTool::new("C:/fake".into(), None, Some("bypassPermissions")).permission_mode(),
        "bypassPermissions"
    );
    // Legacy snake_case（T5 时代的错误值集）→ camelCase 映射。
    assert_eq!(
        ClaudeCodeTool::new("C:/fake".into(), None, Some("accept_edits")).permission_mode(),
        "acceptEdits"
    );
    assert_eq!(
        ClaudeCodeTool::new("C:/fake".into(), None, Some("bypass_permissions")).permission_mode(),
        "bypassPermissions"
    );
    assert_eq!(
        ClaudeCodeTool::new("C:/fake".into(), None, Some("nope")).permission_mode(),
        "acceptEdits"
    );
}

#[tokio::test]
async fn test_claude_code_missing_prompt_fails() {
    let t = ClaudeCodeTool::new("C:/fake/claude.exe".into(), None, None);
    let ctx = test_ctx();
    let err = t.execute(r#"{}"#, &ctx).await.unwrap_err();
    assert!(err.contains("prompt"));
    let err2 = t
        .execute(r#"{"prompt":"   "}"#, &ctx)
        .await
        .unwrap_err();
    assert!(err2.contains("empty"));
}

/// Goal-required: a slow fake CLI hits the timeout and produces a STRUCTURED
/// error, not a panic. Exercises the shared run_cli_delegation timeout arm.
#[tokio::test]
async fn test_claude_code_tool_timeout_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let script = if cfg!(windows) {
        let p = dir.path().join("fake_claude.bat");
        std::fs::write(&p, "@echo off\r\nping -n 3 127.0.0.1 > nul\r\n").unwrap();
        p
    } else {
        let p = dir.path().join("fake_claude.sh");
        std::fs::write(&p, "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    };
    let t = ClaudeCodeTool::new(script.to_string_lossy().to_string(), Some(1), None);
    let ctx = test_ctx();
    // The timeout surfaces as Result::Err carrying a STRUCTURED message
    // (never a panic, never a hang). Both channels are acceptable shape.
    let res = t.execute(r#"{"prompt":"do it"}"#, &ctx).await;
    let text = match res {
        Ok(t) => t,
        Err(e) => e,
    };
    assert!(text.contains("Error:"), "structured error: {text}");
    assert!(text.contains("timed out"), "mentions timeout: {text}");
}

/// T5: the spawned CLI actually receives `--permission-mode <tier>` —
/// default acceptEdits, and an explicitly configured tier. Values are the
/// CLI's own camelCase set (V4 真机对齐：snake_case 会被真实 CLI 拒收)。
#[tokio::test]
async fn test_claude_code_permission_mode_flag_passed_to_cli() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeArgEchoCli::new(dir.path(), "fake_claude2");

    // Default (unset) tier.
    let t = ClaudeCodeTool::new(fake.cli_path(), Some(5), None);
    let out = t.execute(r#"{"prompt":"hello task"}"#, &test_ctx()).await;
    assert!(out.is_ok(), "{:?}", out);
    let got = fake.received_args();
    assert!(got.contains("--permission-mode"), "flag present: {got}");
    assert!(got.contains("acceptEdits"), "default tier: {got}");
    assert!(got.contains("--print"), "print mode: {got}");
    assert!(got.contains("hello task"), "prompt forwarded: {got}");

    // Explicit tier.
    let _ = std::fs::remove_file(&fake.marker);
    let t2 = ClaudeCodeTool::new(fake.cli_path(), Some(5), Some("plan"));
    t2.execute(r#"{"prompt":"plan it"}"#, &test_ctx())
        .await
        .unwrap();
    let got2 = fake.received_args();
    assert!(got2.contains("--permission-mode plan"), "explicit tier: {got2}");
}

/// Goal-required: enabled=false (or CLI absent) ⇒ build_tools output does
/// NOT contain claude_code. Uses the shared registry builder.
#[test]
fn test_claude_code_tool_not_registered_without_cli() {
    use crate::loop_tools::SharedToolConfig;
    // Default config: flag false → not registered regardless of CLI state.
    let tools = crate::loop_tools::register_shared_tools(&SharedToolConfig::default());
    assert!(
        !tools.contains_key("claude_code"),
        "default (disabled) config must not register claude_code"
    );

    // Enabled but the probe happens at registration: on this test host the
    // CLI is (almost certainly) absent in the sandboxed test env; if a real
    // claude IS on PATH this branch registers — assert only the
    // disabled-path invariant above (the enabled-path presence depends on
    // the host, which is exactly the graceful-degradation contract).
    let mut cfg = SharedToolConfig::default();
    cfg.claude_code_tool_enabled = true;
    let tools2 = crate::loop_tools::register_shared_tools(&cfg);
    if crate::loop_tools::claude_code_tool::find_claude_cli().is_none() {
        assert!(!tools2.contains_key("claude_code"));
    }
    // (When the CLI exists, presence is expected — no assert either way.)
}
