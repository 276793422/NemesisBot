use super::*;

// ｜｜DSML｜｜ — the full deepseek DSML tag prefix
// (U+FF5C × 2 + "DSML" + U+FF5C × 2). Tests build content with this so the
// <{d}invoke ...> form yields the real <｜｜DSML｜｜invoke ...> shape.
const D: &str = "\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}";

#[test]
fn test_dsml_single_invoke() {
    // DSML-shaped sample (from a /learn debugging session; the root cause was
    // later traced to a local config error — tools weren't being passed — not a
    // model defect).
    let content = format!(
        "<{d}tool_calls>\n<{d}invoke name=\"skills_info\">\n\
         <{d}parameter name=\"action\" string=\"true\">list</{d}parameter>\n\
         </{d}invoke>\n</{d}tool_calls>",
        d = D
    );
    let calls = repair_tool_calls(&content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "skills_info");
    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.as_ref().unwrap().arguments).unwrap();
    assert_eq!(args["action"], "list");
    assert_eq!(calls[0].id, "repair_0");
}

#[test]
fn test_dsml_numeric_and_bool_params() {
    let content = format!(
        "<{d}invoke name=\"exec\">\
         <{d}parameter name=\"count\" int=\"true\">42</{d}parameter>\
         <{d}parameter name=\"force\" bool=\"true\">true</{d}parameter>\
         </{d}invoke>",
        d = D
    );
    let calls = repair_tool_calls(&content);
    assert_eq!(calls.len(), 1);
    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.as_ref().unwrap().arguments).unwrap();
    assert_eq!(args["count"], 42);
    assert_eq!(args["force"], true);
}

#[test]
fn test_json_text_fenced_array() {
    let content = "I'll use the tool:\n\
        ```json\n\
        [{\"name\": \"read_file\", \"arguments\": {\"path\": \"/tmp/x\"}}]\n\
        ```\n\
        done.";
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "read_file");
    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.as_ref().unwrap().arguments).unwrap();
    assert_eq!(args["path"], "/tmp/x");
}

#[test]
fn test_json_text_bare_array_multiple() {
    let content = "[{\"name\":\"a\",\"arguments\":{}},{\"name\":\"b\",\"arguments\":{\"k\":1}}]";
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "a");
    assert_eq!(calls[1].function.as_ref().unwrap().name, "b");
}

#[test]
fn test_xml_invoke() {
    let content = "<function_calls>\n\
        <invoke name=\"edit_file\">\n\
        <parameter name=\"path\">src/main.rs</parameter>\n\
        <parameter name=\"old\">foo</parameter>\n\
        </invoke>\n\
        </function_calls>";
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "edit_file");
    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.as_ref().unwrap().arguments).unwrap();
    assert_eq!(args["path"], "src/main.rs");
    assert_eq!(args["old"], "foo");
}

#[test]
fn test_no_tool_call_in_plain_text() {
    // Normal prose must NOT be misread as a tool call.
    let content = "I'll read the file and then edit it. The result should be fine.";
    let calls = repair_tool_calls(content);
    assert!(calls.is_empty(), "plain text should yield no calls");
}

#[test]
fn test_empty_content() {
    assert!(repair_tool_calls("").is_empty());
}

#[test]
fn test_priority_dsml_over_json() {
    // If both somehow present, DSML wins (it's checked first).
    let content = format!(
        "<{d}invoke name=\"a\">\
         <{d}parameter name=\"x\">1</{d}parameter>\
         </{d}invoke>",
        d = D
    );
    let calls = repair_tool_calls(&content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "a");
}

// ---- Edge cases / branch coverage ----

#[test]
fn test_dsml_multiple_invokes() {
    let content = format!(
        "<{d}invoke name=\"read_file\">\
         <{d}parameter name=\"path\">a.rs</{d}parameter>\
         </{d}invoke>\
         <{d}invoke name=\"grep\">\
         <{d}parameter name=\"pattern\">TODO</{d}parameter>\
         </{d}invoke>",
        d = D
    );
    let calls = repair_tool_calls(&content);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "read_file");
    assert_eq!(calls[1].function.as_ref().unwrap().name, "grep");
}

#[test]
fn test_json_single_object_not_array() {
    let content = r#"I'll call: {"name": "exec", "arguments": {"cmd": "ls"}}"#;
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "exec");
    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.as_ref().unwrap().arguments).unwrap();
    assert_eq!(args["cmd"], "ls");
}

#[test]
fn test_json_with_function_wrapper() {
    // {"function": {"name": "X", "arguments": "..."}} style
    let content = r#"[{"function":{"name":"write_file","arguments":"{\"path\":\"x\"}"}}]"#;
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "write_file");
}

#[test]
fn test_json_nested_arguments() {
    let content = r#"[{"name":"edit","arguments":{"old":"a","new":"b","path":"x.rs"}}]"#;
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1);
    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.as_ref().unwrap().arguments).unwrap();
    assert_eq!(args["old"], "a");
    assert_eq!(args["new"], "b");
    assert_eq!(args["path"], "x.rs");
}

#[test]
fn test_xml_multiple_invokes() {
    let content = "<invoke name=\"a\"><parameter name=\"x\">1</parameter></invoke>\
                   <invoke name=\"b\"><parameter name=\"y\">2</parameter></invoke>";
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "a");
    assert_eq!(calls[1].function.as_ref().unwrap().name, "b");
}

#[test]
fn test_xml_invoke_with_no_params() {
    let content = "<invoke name=\"noop\"></invoke>";
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().name, "noop");
    let args: serde_json::Value =
        serde_json::from_str(&calls[0].function.as_ref().unwrap().arguments).unwrap();
    assert!(args.as_object().unwrap().is_empty());
}

#[test]
fn test_json_empty_arguments() {
    let content = r#"[{"name":"noop","arguments":{}}]"#;
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().arguments, "{}");
}

#[test]
fn test_json_missing_arguments_defaults_empty() {
    // No "arguments" key → should default to "{}".
    let content = r#"[{"name":"ping"}]"#;
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.as_ref().unwrap().arguments, "{}");
}

#[test]
fn test_standard_tool_calls_not_in_content_text() {
    // Even if content mentions "tool_calls" as text, it shouldn't be parsed
    // as a non-standard format if it doesn't match any detector pattern.
    let content = "I would use tool_calls but I don't have the right format";
    let calls = repair_tool_calls(content);
    assert!(
        calls.is_empty(),
        "should not false-positive on prose mentioning tool_calls"
    );
}

#[test]
fn test_whitespace_only_content() {
    assert!(repair_tool_calls("   \n\t  ").is_empty());
}

#[test]
fn test_generated_ids_are_unique() {
    let content = format!(
        "<{d}invoke name=\"a\"></{d}invoke>\
         <{d}invoke name=\"b\"></{d}invoke>\
         <{d}invoke name=\"c\"></{d}invoke>",
        d = D
    );
    let calls = repair_tool_calls(&content);
    let ids: Vec<&str> = calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["repair_0", "repair_1", "repair_2"]);
}

#[test]
fn test_call_type_always_function() {
    let content = format!("<{d}invoke name=\"x\"></{d}invoke>", d = D);
    let calls = repair_tool_calls(&content);
    assert_eq!(calls[0].call_type.as_ref().unwrap(), "function");
}
