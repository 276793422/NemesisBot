//! Tests for the Codex delegation tool (I4 / U13 other half).

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
fn test_codex_tool_schema() {
    let t = CodexTool::new("C:/fake/codex.exe".into(), None);
    let p = t.parameters();
    assert!(p["properties"]["prompt"].is_object());
    assert_eq!(p["required"][0], "prompt");
    assert!(t.description().contains("Codex"));
}

#[tokio::test]
async fn test_codex_tool_missing_prompt_fails() {
    let t = CodexTool::new("C:/fake/codex.exe".into(), None);
    let err = t.execute(r#"{}"#, &test_ctx()).await.unwrap_err();
    assert!(err.contains("prompt"));
}

/// Timeout path: a sleeping fake CLI produces a STRUCTURED error (never a
/// panic, never a hang) — same method as the H7 claude_code test.
#[tokio::test]
async fn test_codex_tool_timeout_structured_error() {
    let dir = tempfile::tempdir().unwrap();
    let script = if cfg!(windows) {
        let p = dir.path().join("fake_codex.bat");
        std::fs::write(&p, "@echo off\r\nping -n 3 127.0.0.1 > nul\r\n").unwrap();
        p
    } else {
        let p = dir.path().join("fake_codex.sh");
        std::fs::write(&p, "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    };
    let t = CodexTool::new(script.to_string_lossy().to_string(), Some(1));
    let run_dir = tempfile::tempdir().unwrap();
    let ctx = RequestContext {
        session_key: run_dir.path().join("session").to_string_lossy().to_string(),
        ..test_ctx()
    };
    let res = t.execute(r#"{"prompt":"do it"}"#, &ctx).await;
    let text = match res {
        Ok(t) => t,
        Err(e) => e,
    };
    assert!(text.contains("Error:"), "structured: {text}");
    assert!(text.contains("timed out"), "timeout named: {text}");
}
