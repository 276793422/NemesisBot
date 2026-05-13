//! Extract tool calls from text LLM responses (CLI-based providers).

use crate::types::{FunctionCall, ToolCall};

/// Extract tool calls from text that contains a `{"tool_calls":[...]}` JSON block.
/// Both ClaudeCliProvider and CodexCliProvider use this to extract tool calls
/// that the model outputs in its response text.
pub fn extract_tool_calls_from_text(text: &str) -> Vec<ToolCall> {
    let start = match text.find("{\"tool_calls\"") {
        Some(s) => s,
        None => return vec![],
    };

    let end = match find_matching_brace(text, start) {
        Some(e) => e,
        None => return vec![],
    };

    let json_str = &text[start..end];

    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[serde(rename = "tool_calls")]
        tool_calls: Vec<RawToolCall>,
    }

    #[derive(serde::Deserialize)]
    struct RawToolCall {
        id: String,
        #[serde(rename = "type", default)]
        call_type: String,
        function: RawFunction,
    }

    #[derive(serde::Deserialize)]
    struct RawFunction {
        name: String,
        arguments: String,
    }

    let wrapper: Wrapper = match serde_json::from_str(json_str) {
        Ok(w) => w,
        Err(_) => return vec![],
    };

    wrapper
        .tool_calls
        .into_iter()
        .map(|tc| {
            let arguments: Option<std::collections::HashMap<String, serde_json::Value>> =
                serde_json::from_str(&tc.function.arguments).ok();

            ToolCall {
                id: tc.id,
                call_type: Some(tc.call_type),
                function: Some(FunctionCall {
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments,
                }),
                name: Some(tc.function.name),
                arguments: arguments,
            }
        })
        .collect()
}

/// Strip tool call JSON from response text, leaving only the content portion.
pub fn strip_tool_calls_from_text(text: &str) -> String {
    let start = match text.find("{\"tool_calls\"") {
        Some(s) => s,
        None => return text.to_string(),
    };

    let end = match find_matching_brace(text, start) {
        Some(e) => e,
        None => return text.to_string(),
    };

    format!("{}{}", &text[..start], &text[end..]).trim().to_string()
}

/// Find the index after the closing brace matching the opening brace at `pos`.
/// Returns `None` if no matching brace is found.
pub fn find_matching_brace(text: &str, pos: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if pos >= bytes.len() || bytes[pos] != b'{' {
        return None;
    }

    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for i in pos..bytes.len() {
        let ch = bytes[i];

        if escape_next {
            escape_next = false;
            continue;
        }

        if ch == b'\\' && in_string {
            escape_next = true;
            continue;
        }

        if ch == b'"' {
            in_string = !in_string;
            continue;
        }

        if in_string {
            continue;
        }

        match ch {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_matching_brace_simple() {
        let text = r#"{"key": "value"}"#;
        assert_eq!(find_matching_brace(text, 0), Some(16));
    }

    #[test]
    fn test_find_matching_brace_nested() {
        let text = r#"{"a": {"b": 1}}"#;
        assert_eq!(find_matching_brace(text, 0), Some(15));
    }

    #[test]
    fn test_find_matching_brace_with_braces_in_string() {
        let text = r#"{"args": "{\"path\": \"/tmp\"}"}"#;
        assert_eq!(find_matching_brace(text, 0), Some(32));
    }

    #[test]
    fn test_find_matching_brace_no_match() {
        let text = r#"{"key": "value"#;
        assert_eq!(find_matching_brace(text, 0), None);
    }

    #[test]
    fn test_extract_tool_calls() {
        let text = r#"Here is the result: {"tool_calls":[{"id":"call_123","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"/tmp/test\"}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_123");
        assert_eq!(
            calls[0].function.as_ref().unwrap().name,
            "read_file"
        );
    }

    #[test]
    fn test_extract_tool_calls_none() {
        let text = "No tool calls here, just text.";
        let calls = extract_tool_calls_from_text(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_multiple() {
        let text = r#"{"tool_calls":[{"id":"c1","type":"function","function":{"name":"tool1","arguments":"{}"}},{"id":"c2","type":"function","function":{"name":"tool2","arguments":"{}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "c1");
        assert_eq!(calls[1].id, "c2");
    }

    #[test]
    fn test_strip_tool_calls() {
        let text = r#"Some text {"tool_calls":[{"id":"c1","type":"function","function":{"name":"t","arguments":"{}"}}]} trailing"#;
        let stripped = strip_tool_calls_from_text(text);
        assert_eq!(stripped, "Some text  trailing");
    }

    #[test]
    fn test_strip_tool_calls_none() {
        let text = "No tool calls here.";
        assert_eq!(strip_tool_calls_from_text(text), text);
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_find_matching_brace_empty_object() {
        assert_eq!(find_matching_brace("{}", 0), Some(2));
    }

    #[test]
    fn test_find_matching_brace_at_wrong_position() {
        assert_eq!(find_matching_brace("abc{def}", 0), None);
    }

    #[test]
    fn test_find_matching_brace_out_of_bounds() {
        assert_eq!(find_matching_brace("{}", 5), None);
    }

    #[test]
    fn test_find_matching_brace_deeply_nested() {
        let text = r#"{"a": {"b": {"c": 1}}}"#;
        assert_eq!(find_matching_brace(text, 0), Some(text.len()));
    }

    #[test]
    fn test_find_matching_brace_array() {
        let text = r#"{"arr": [1, 2, 3]}"#;
        assert_eq!(find_matching_brace(text, 0), Some(text.len()));
    }

    #[test]
    fn test_find_matching_brace_with_escaped_quotes() {
        let text = r#"{"msg": "say \"hello\""}"#;
        assert_eq!(find_matching_brace(text, 0), Some(text.len()));
    }

    #[test]
    fn test_extract_tool_calls_with_arguments_object() {
        let text = r#"{"tool_calls":[{"id":"call_1","type":"function","function":{"name":"shell","arguments":"{\"command\":\"ls\"}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.as_ref().unwrap().name, "shell");
        // Arguments should be parsed
        assert!(calls[0].arguments.is_some());
        assert_eq!(calls[0].arguments.as_ref().unwrap().get("command").unwrap(), "ls");
    }

    #[test]
    fn test_extract_tool_calls_invalid_json() {
        // Malformed JSON after the tool_calls prefix
        let text = r#"{"tool_calls": not valid json}"#;
        let calls = extract_tool_calls_from_text(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_with_leading_text() {
        let text = r#"I need to read a file. {"tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "c1");
    }

    #[test]
    fn test_strip_tool_calls_with_leading_trailing() {
        let text = r#"Here is my plan. {"tool_calls":[{"id":"c1","type":"function","function":{"name":"t","arguments":"{}"}}]} Done!"#;
        let stripped = strip_tool_calls_from_text(text);
        assert!(stripped.starts_with("Here is my plan."));
        assert!(stripped.ends_with("Done!"));
        assert!(!stripped.contains("tool_calls"));
    }

    #[test]
    fn test_strip_tool_calls_unmatched_brace() {
        let text = r#"Text {"tool_calls":[{"id":"c1"}] incomplete"#;
        // No matching brace, should return original
        assert_eq!(strip_tool_calls_from_text(text), text);
    }

    #[test]
    fn test_tool_call_fields() {
        let text = r#"{"tool_calls":[{"id":"call_abc","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"/tmp/test\"}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 1);
        let tc = &calls[0];
        assert_eq!(tc.id, "call_abc");
        assert_eq!(tc.call_type, Some("function".to_string()));
        assert_eq!(tc.name, Some("write_file".to_string()));
        assert!(tc.arguments.is_some());
    }
}
