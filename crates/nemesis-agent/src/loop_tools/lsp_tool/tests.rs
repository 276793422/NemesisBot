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
    assert!(LspTool::registration_plan(true, 5), "enabled + many servers");
}

#[test]
fn schema_requires_all_four_params_with_op_enum() {
    let t = LspTool::new(None, None);
    let p = t.parameters();
    let required: Vec<&str> = p["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for field in ["op", "path", "line", "character"] {
        assert!(required.contains(&field), "schema must require {field}: {p}");
        assert!(p["properties"][field].is_object(), "schema must document {field}");
    }
    let ops: Vec<&str> = p["properties"]["op"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(ops, vec!["definition", "references", "implementation", "hover"]);
    // 0-based convention must be stated — models default to 1-based.
    assert!(p.to_string().contains("0-based"));
    assert!(t.description().contains("语义"));
}

/// Unknown op is rejected with the valid set spelled out — no server
/// needed (validation happens before any spawn).
#[tokio::test]
async fn execute_rejects_unknown_op_listing_valid() {
    let t = LspTool::new(None, None);
    let err = t
        .execute(
            r#"{"op":"rename","path":"/x/a.rs","line":0,"character":0}"#,
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Invalid 'op'"), "{err}");
    for valid in ["definition", "references", "implementation", "hover"] {
        assert!(err.contains(valid), "error should list {valid}: {err}");
    }
}

/// Missing file: clear error, no server spawned (the is_file check runs
/// before the PATH probe in `LspManager::query`).
#[tokio::test]
async fn execute_rejects_missing_file() {
    let t = LspTool::new(None, None);
    let missing = if cfg!(windows) { "Z:/definitely/missing/a.rs" } else { "/definitely/missing/a.rs" };
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
    let missing = if cfg!(windows) { "Z:/definitely/missing/a.md" } else { "/definitely/missing/a.md" };
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
