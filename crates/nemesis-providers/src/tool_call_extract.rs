//! Extract tool calls from text LLM responses (CLI-based providers).

use crate::types::{FunctionCall, ToolCall, ToolDefinition};

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
                arguments,
            }
        })
        .collect()
}

/// 白名单过滤提取（同款不变量）：提取器对
/// `{"tool_calls"` 文本零校验，文档性 JSON（协议示例、配置样例等）会被劫持
/// 成幻影工具调用 + finish 强改。恢复调用的不变量是「可执行」：名字必须
/// ∈ 本请求的 tools 白名单（不在白名单的工具无论如何执行不了）；全滤空 =
/// 不恢复，宁可不修复不可误修复。CLI lane 的文本信封是唯一工具调用通道
/// （无 wire 级结构化字段可对照），过滤只拦名字不可执行的伪调用，不碰机制。
pub fn extract_tool_calls_in_whitelist(
    text: &str,
    allowed_tools: &[ToolDefinition],
) -> Vec<ToolCall> {
    extract_tool_calls_from_text(text)
        .into_iter()
        .filter(|tc| {
            tc.function
                .as_ref()
                .is_some_and(|f| allowed_tools.iter().any(|t| t.function.name == f.name))
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

    format!("{}{}", &text[..start], &text[end..])
        .trim()
        .to_string()
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

    for (i, &ch) in bytes.iter().enumerate().skip(pos) {
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
mod cov_tests;
#[cfg(test)]
mod tests;
