
use super::*;

// ---- opt_u64 ----

#[test]
fn opt_u64_returns_default_when_missing() {
    let data = Some(serde_json::json!({"limit": 5}));
    assert_eq!(opt_u64(&data, "offset", 99), 99);
}

#[test]
fn opt_u64_returns_value_when_present() {
    let data = Some(serde_json::json!({"limit": 5}));
    assert_eq!(opt_u64(&data, "limit", 99), 5);
}

#[test]
fn opt_u64_returns_default_when_none_data() {
    assert_eq!(opt_u64(&None, "limit", 42), 42);
}

#[test]
fn opt_u64_returns_default_when_non_number() {
    let data = Some(serde_json::json!({"limit": "many"}));
    assert_eq!(opt_u64(&data, "limit", 7), 7);
}

// ---- file_type_name ----

#[test]
fn file_type_name_strips_first_segment() {
    // "01.AI.Request.raw.json" → "AI.Request.raw.json"
    assert_eq!(
        file_type_name("01.AI.Request.raw.json"),
        "AI.Request.raw.json"
    );
}

#[test]
fn file_type_name_returns_whole_when_no_dot() {
    assert_eq!(file_type_name("README"), "README");
}

// ---- sanitize_session_key ----

#[test]
fn sanitize_session_key_replaces_path_and_special_chars() {
    let cleaned = sanitize_session_key("web://user/chat|file?x");
    assert!(!cleaned.contains(':'));
    assert!(!cleaned.contains('/'));
    assert!(!cleaned.contains('?'));
    assert!(!cleaned.contains('|'));
    assert_eq!(cleaned, "web___user_chat_file_x");
}

#[test]
fn sanitize_session_key_passes_through_plain_text() {
    assert_eq!(sanitize_session_key("plain-key_123"), "plain-key_123");
}

// ---- parse_duration_seconds ----

#[test]
fn parse_duration_seconds_whole_seconds() {
    assert_eq!(parse_duration_seconds("30s"), Some(30000));
}

#[test]
fn parse_duration_seconds_fractional() {
    assert_eq!(parse_duration_seconds("0.022s"), Some(22));
}

#[test]
fn parse_duration_seconds_zero() {
    assert_eq!(parse_duration_seconds("0s"), Some(0));
}

#[test]
fn parse_duration_seconds_rejects_non_numeric_unit() {
    // "5m" → not a valid f64 → None (this helper only handles plain seconds)
    assert_eq!(parse_duration_seconds("5m"), None);
}

#[test]
fn parse_duration_seconds_rejects_negative() {
    assert_eq!(parse_duration_seconds("-5s"), None);
}

#[test]
fn parse_duration_seconds_rejects_garbage() {
    assert_eq!(parse_duration_seconds("abc"), None);
}

// ---- stringify_message_content ----

#[test]
fn stringify_message_content_none_is_empty() {
    assert_eq!(stringify_message_content(None), "");
}

#[test]
fn stringify_message_content_plain_string() {
    assert_eq!(
        stringify_message_content(Some(&serde_json::json!("hello world"))),
        "hello world"
    );
}

#[test]
fn stringify_message_content_array_of_text_parts() {
    let content = serde_json::json!([
        { "text": "line1" },
        { "text": "line2" },
    ]);
    assert_eq!(stringify_message_content(Some(&content)), "line1\nline2");
}

#[test]
fn stringify_message_content_array_of_content_parts() {
    let content = serde_json::json!([{ "content": "alt" }]);
    assert_eq!(stringify_message_content(Some(&content)), "alt");
}

#[test]
fn stringify_message_content_other_value_stringified() {
    let content = serde_json::json!(42);
    assert_eq!(stringify_message_content(Some(&content)), "42");
}

// ---- compute_audit_hash ----

#[test]
fn compute_audit_hash_is_deterministic() {
    let ev = nemesis_security::integrity::AuditEvent {
        id: "e1".into(),
        timestamp: "2026-06-29T00:00:00Z".into(),
        operation: "file_write".into(),
        tool_name: "filesystem".into(),
        user: "alice".into(),
        source: "web".into(),
        target: "/tmp/f".into(),
        decision: "allow".into(),
        reason: "ok".into(),
        hash: String::new(),
        prev_hash: "genesis".into(),
        sign: None,
    };
    let h1 = compute_audit_hash(&ev);
    let h2 = compute_audit_hash(&ev);
    assert_eq!(h1, h2);
    assert!(!h1.is_empty());
}

#[test]
fn compute_audit_hash_changes_with_field() {
    let base = || nemesis_security::integrity::AuditEvent {
        id: "e1".into(),
        timestamp: "t".into(),
        operation: "op".into(),
        tool_name: "tn".into(),
        user: "u".into(),
        source: "s".into(),
        target: "tgt".into(),
        decision: "allow".into(),
        reason: "r".into(),
        hash: String::new(),
        prev_hash: "p".into(),
        sign: None,
    };
    let mut ev = base();
    let h1 = compute_audit_hash(&ev);
    ev.operation = "file_delete".into();
    let h2 = compute_audit_hash(&ev);
    assert_ne!(h1, h2);
}

// ---- parse_local_tool_results ----

#[test]
fn parse_local_tool_results_empty() {
    assert!(parse_local_tool_results("").is_empty());
    assert!(parse_local_tool_results("no operations here").is_empty());
}

#[test]
fn parse_local_tool_results_success_with_args_and_duration() {
    let md = "\
## Operation 1: Tool Execution

**Name**: read_file
**Status**: success

### Arguments
{\"path\": \"/tmp/a\"}

### Result
hello contents

### Duration 0.022s

---
";
    let out = parse_local_tool_results(md);
    assert_eq!(out.len(), 1);
    let op = &out[0];
    assert_eq!(op["name"], "read_file");
    assert_eq!(op["callId"], "read_file");
    assert_eq!(op["result"]["status"], "success");
    assert_eq!(op["result"]["output"], "hello contents\n\n");
    assert_eq!(op["args"]["path"], "/tmp/a");
    assert_eq!(op["duration_ms"], 22);
}

#[test]
fn parse_local_tool_results_error_branch() {
    let md = "\
## Operation 2: Tool Execution

**Name**: write_file
**Status**: error

### Error
permission denied

### Duration 0.005s
";
    let out = parse_local_tool_results(md);
    assert_eq!(out.len(), 1);
    let op = &out[0];
    assert_eq!(op["name"], "write_file");
    assert_eq!(op["result"]["status"], "error");
    assert_eq!(op["result"]["error"], "permission denied\n\n");
    // No `output` key on the error branch.
    assert!(op["result"].get("output").is_none());
}

#[test]
fn parse_local_tool_results_args_not_json_falls_back_to_string() {
    let md = "\
## Operation 1: Tool Execution

**Name**: shell
**Status**: success

### Arguments
not a json blob

### Result
done
";
    let out = parse_local_tool_results(md);
    assert_eq!(out.len(), 1);
    // Non-JSON args become a JSON string value.
    assert_eq!(out[0]["args"], "not a json blob");
}

#[test]
fn parse_local_tool_results_multiple_operations_flush() {
    let md = "\
## Operation 1: Tool Execution

**Name**: a
**Status**: success

## Operation 2: Tool Execution

**Name**: b
**Status**: success
";
    let out = parse_local_tool_results(md);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["name"], "a");
    assert_eq!(out[1]["name"], "b");
}
