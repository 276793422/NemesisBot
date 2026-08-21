//! Tests for the Claude Code delegation tool (H7 / U13 half).

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

#[test]
fn test_claude_code_tool_schema() {
    let t = ClaudeCodeTool::new("C:/fake/claude.exe".into(), None);
    let p = t.parameters();
    assert!(p["properties"]["prompt"].is_object());
    assert_eq!(p["required"][0], "prompt");
    assert!(t.description().contains("Claude Code"));
}

#[tokio::test]
async fn test_claude_code_missing_prompt_fails() {
    let t = ClaudeCodeTool::new("C:/fake/claude.exe".into(), None);
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
/// error, not a panic. Uses a python one-liner sleep as the "CLI".
#[tokio::test]
async fn test_claude_code_tool_timeout_returns_error() {
    let fake_cli = if cfg!(windows) {
        // python sleeps 30s; tool timeout is 1s.
        "python"
    } else {
        "python3"
    };
    // Build the tool with the fake CLI directly (bypassing find_claude_cli).
    let t = ClaudeCodeTool::new(fake_cli.to_string(), Some(1));

    // Monkey-run: our execute always passes --print..., which python won't
    // understand. Instead, verify the timeout path with a wrapper: use
    // cmd/powershell that sleeps. Simplest cross-platform: use python via a
    // tiny script file as the "cli".
    let dir = tempfile::tempdir().unwrap();
    let script = if cfg!(windows) {
        let p = dir.path().join("fake_claude.bat");
        std::fs::write(&p, "@echo off\r\nping -n 30 127.0.0.1 > nul\r\n").unwrap();
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
    let t = ClaudeCodeTool::new(script.to_string_lossy().to_string(), Some(1));
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
