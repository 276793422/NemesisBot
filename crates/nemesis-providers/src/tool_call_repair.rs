//! Tool-call repair — recover tool calls from non-standard model outputs.
//!
//! Some models (notably deepseek-v4-flash) intermittently emit tool calls as
//! plain text in the `content` field instead of the standard `tool_calls`
//! structure. This module detects and parses those non-standard formats back
//! into standard `ToolCall`s, so the agent loop still executes them.
//!
//! Supported formats (tried in priority order):
//! 1. **DSML** — deepseek's internal markup: `<｜｜DSML｜｜invoke name="X">...`
//!    (the bars are U+FF5C fullwidth vertical bar)
//! 2. **JSON text** — a fenced ```json [...] ``` block or a bare JSON array/object
//!    whose entries have `name` + `arguments`
//! 3. **XML-style** — `<invoke name="X"><parameter name="Y">val</parameter></invoke>`
//!    (Anthropic-style)
//!
//! Standard `tool_calls` already present in the response are NOT touched — the
//! caller only invokes [`repair_tool_calls`] when the standard field is empty.

use regex::Regex;
use std::sync::OnceLock;

use crate::types::{FunctionCall, ToolCall};

/// Parse non-standard tool calls embedded in `content` into standard ToolCalls.
///
/// Returns an empty vec when no known format is detected.
pub fn repair_tool_calls(content: &str) -> Vec<ToolCall> {
    // Each parser yields (name, arguments_json_string) pairs.
    let mut calls: Vec<(String, String)> = Vec::new();

    // 1. DSML (deepseek internal markup).
    calls.extend(parse_dsml(content));

    // 2. JSON text (fenced block or bare array/object).
    if calls.is_empty() {
        calls.extend(parse_json_text(content));
    }

    // 3. XML-style <invoke>.
    if calls.is_empty() {
        calls.extend(parse_xml_invoke(content));
    }

    // Build ToolCalls with generated ids.
    calls
        .into_iter()
        .enumerate()
        .map(|(i, (name, arguments))| ToolCall {
            id: format!("repair_{i}"),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall { name, arguments }),
            name: None,
            arguments: None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// DSML — deepseek internal markup. The separator is ｜ (U+FF5C fullwidth bar),
// written literally here to avoid regex unicode-escape ambiguity.

fn dsml_invoke_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // The separator is ｜｜ (U+FF5C × 2). Build the pattern via format! with
        // an explicit \u{ff5c} so the regex receives the right Unicode scalar
        // regardless of how the bar renders in the source file.
        let b = "\u{ff5c}\u{ff5c}";
        let pattern =
            format!(r#"(?s)<{b}DSML{b}invoke\s+name="([^"]+)">(.*?)</{b}DSML{b}invoke>"#);
        Regex::new(&pattern).expect("dsml invoke regex")
    })
}

fn dsml_param_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let b = "\u{ff5c}\u{ff5c}";
        let pattern = format!(
            r#"(?s)<{b}DSML{b}parameter\s+name="([^"]+)"[^>]*>(.*?)</{b}DSML{b}parameter>"#
        );
        Regex::new(&pattern).expect("dsml param regex")
    })
}

fn parse_dsml(content: &str) -> Vec<(String, String)> {
    let invoke_re = dsml_invoke_re();
    let param_re = dsml_param_re();
    invoke_re
        .captures_iter(content)
        .filter_map(|caps| {
            let name = caps.get(1)?.as_str().trim().to_string();
            let body = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            Some((name, collect_params(body, param_re)))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// JSON text — fenced ```json block or a bare JSON array/object with
// `name`/`arguments` entries.

fn parse_json_text(content: &str) -> Vec<(String, String)> {
    let candidates: Vec<String> = extract_fenced_json(content)
        .map(|f| vec![f])
        .unwrap_or_else(|| find_json_spans(content));

    for cand in candidates {
        // Try as array of calls.
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&cand) {
            let out: Vec<_> = arr.iter().filter_map(json_value_to_call).collect();
            if !out.is_empty() {
                return out;
            }
        }
        // Try as a single object.
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&cand) {
            if let Some(c) = json_value_to_call(&obj) {
                return vec![c];
            }
        }
    }
    Vec::new()
}

fn json_value_to_call(v: &serde_json::Value) -> Option<(String, String)> {
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .or_else(|| v.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()))?;
    let arguments = match v.get("arguments") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
        None => "{}".to_string(),
    };
    Some((name.to_string(), arguments))
}

/// Extract the first fenced code block (```...```) content, if any.
fn extract_fenced_json(content: &str) -> Option<String> {
    let mut in_fence = false;
    let mut buf = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_fence {
                return Some(buf.clone());
            }
            in_fence = true;
            buf.clear();
        } else if in_fence {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    None
}

/// Best-effort: find balanced `[...]` / `{...}` spans that contain `"name"`.
fn find_json_spans(content: &str) -> Vec<String> {
    let bytes = content.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'[' || c == b'{' {
            let open = c;
            let close: u8 = if open == b'[' { b']' } else { b'}' };
            let (matched, end) = match_balanced(bytes, i, open, close);
            if matched {
                let span = &content[i..=end];
                if span.contains("\"name\"") {
                    spans.push(span.to_string());
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    spans
}

/// From `start` (an opening bracket), scan to its matching close, respecting
/// string literals and escapes. Returns (matched, close_index).
fn match_balanced(bytes: &[u8], start: usize, open: u8, close: u8) -> (bool, usize) {
    let mut depth = 1isize;
    let mut j = start + 1;
    let mut in_str = false;
    let mut esc = false;
    while j < bytes.len() {
        let ch = bytes[j];
        if in_str {
            if esc {
                esc = false;
            } else if ch == b'\\' {
                esc = true;
            } else if ch == b'"' {
                in_str = false;
            }
        } else {
            match ch {
                b'"' => in_str = true,
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return (true, j);
                    }
                }
                _ => {}
            }
        }
        j += 1;
    }
    (false, j)
}

// ---------------------------------------------------------------------------
// XML-style — `<invoke name="X"><parameter name="Y">val</parameter></invoke>`

fn xml_invoke_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<invoke\s+name="([^"]+)">(.*?)</invoke>"#).expect("xml invoke regex")
    })
}

fn xml_param_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<parameter\s+name="([^"]+)">(.*?)</parameter>"#).expect("xml param regex")
    })
}

fn parse_xml_invoke(content: &str) -> Vec<(String, String)> {
    let invoke_re = xml_invoke_re();
    let param_re = xml_param_re();
    invoke_re
        .captures_iter(content)
        .filter_map(|caps| {
            let name = caps.get(1)?.as_str().trim().to_string();
            let body = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            Some((name, collect_params(body, param_re)))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Shared helper: collect <parameter name="Y">val</...> into a JSON object string.

fn collect_params(body: &str, param_re: &Regex) -> String {
    let mut args = serde_json::Map::new();
    for pcaps in param_re.captures_iter(body) {
        let pname = pcaps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let pval = pcaps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        // Try to parse the value as JSON (number/bool/null/object), else string.
        let val: serde_json::Value =
            serde_json::from_str(pval).unwrap_or_else(|_| serde_json::Value::String(pval.to_string()));
        args.insert(pname.to_string(), val);
    }
    serde_json::to_string(&serde_json::Value::Object(args)).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ｜｜DSML｜｜ — the full deepseek DSML tag prefix
    // (U+FF5C × 2 + "DSML" + U+FF5C × 2). Tests build content with this so the
    // <{d}invoke ...> form yields the real <｜｜DSML｜｜invoke ...> shape.
    const D: &str = "\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}";

    #[test]
    fn test_dsml_single_invoke() {
        // Real deepseek-v4-flash output observed during /learn debugging.
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
        let content =
            "[{\"name\":\"a\",\"arguments\":{}},{\"name\":\"b\",\"arguments\":{\"k\":1}}]";
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
        let args: serde_json::Value = serde_json::from_str(
            &calls[0].function.as_ref().unwrap().arguments
        ).unwrap();
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
        let args: serde_json::Value = serde_json::from_str(
            &calls[0].function.as_ref().unwrap().arguments
        ).unwrap();
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
        let args: serde_json::Value = serde_json::from_str(
            &calls[0].function.as_ref().unwrap().arguments
        ).unwrap();
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
        assert!(calls.is_empty(), "should not false-positive on prose mentioning tool_calls");
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
}
