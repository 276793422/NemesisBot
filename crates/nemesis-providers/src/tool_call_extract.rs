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
mod tests;
