//! Tests for the Codex delegation tool (I4 / U13 other half; T5 sandbox tier
//! + shared-layer refactor).

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
fn test_codex_tool_schema() {
    let t = CodexTool::new("C:/fake/codex.exe".into(), None, None);
    let p = t.parameters();
    assert!(p["properties"]["prompt"].is_object());
    assert_eq!(p["required"][0], "prompt");
    assert!(t.description().contains("Codex"));
}

/// T5: the sandbox tier is a FIXED config tier — it must NOT appear in the
/// model-facing schema.
#[test]
fn test_codex_sandbox_not_in_schema() {
    let t = CodexTool::new("C:/fake/codex.exe".into(), None, Some("workspace_write"));
    let schema = t.parameters().to_string();
    assert!(
        !schema.contains("sandbox"),
        "sandbox must not be model-selectable: {schema}"
    );
}

/// T5: tier normalization — None → default read_only; explicit values pass
/// through; unknown → default.
#[test]
fn test_codex_sandbox_normalization() {
    assert_eq!(CodexTool::new("C:/fake".into(), None, None).sandbox(), "read_only");
    assert_eq!(
        CodexTool::new("C:/fake".into(), None, Some("workspace_write")).sandbox(),
        "workspace_write"
    );
    assert_eq!(
        CodexTool::new("C:/fake".into(), None, Some("danger_full_access")).sandbox(),
        "danger_full_access"
    );
    assert_eq!(CodexTool::new("C:/fake".into(), None, Some("nope")).sandbox(), "read_only");
}

#[tokio::test]
async fn test_codex_missing_prompt_fails() {
    let t = CodexTool::new("C:/fake/codex.exe".into(), None, None);
    let err = t.execute(r#"{}"#, &test_ctx()).await.unwrap_err();
    assert!(err.contains("prompt"));
}

/// Timeout path: a sleeping fake CLI produces a STRUCTURED error (never a
/// panic, never a hang) — exercises the shared run_cli_delegation timeout
/// arm, same method as the H7 claude_code test.
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
    let t = CodexTool::new(script.to_string_lossy().to_string(), Some(1), None);
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

/// T5: the spawned CLI actually receives the kebab-case `--sandbox <tier>`
/// and the implied `--ask-for-approval never` — default read_only, and an
/// explicitly configured tier.
#[tokio::test]
async fn test_codex_sandbox_flags_passed_to_cli() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeArgEchoCli::new(dir.path(), "fake_codex2");

    // Default (unset) tier.
    let t = CodexTool::new(fake.cli_path(), Some(5), None);
    let out = t.execute(r#"{"prompt":"hello task"}"#, &test_ctx()).await;
    assert!(out.is_ok(), "{:?}", out);
    let got = fake.received_args();
    assert!(got.contains("--sandbox read-only"), "default tier kebab: {got}");
    assert!(
        got.contains("--ask-for-approval never"),
        "non-interactive approval: {got}"
    );
    assert!(got.contains("exec"), "exec subcommand: {got}");
    assert!(got.contains("hello task"), "prompt forwarded: {got}");

    // Explicit tier: snake_case config → kebab-case CLI value.
    let _ = std::fs::remove_file(&fake.marker);
    let t2 = CodexTool::new(fake.cli_path(), Some(5), Some("workspace_write"));
    t2.execute(r#"{"prompt":"write it"}"#, &test_ctx())
        .await
        .unwrap();
    let got2 = fake.received_args();
    assert!(
        got2.contains("--sandbox workspace-write"),
        "explicit tier kebab: {got2}"
    );
}
