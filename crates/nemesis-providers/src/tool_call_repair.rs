//! Tool-call repair — recover tool calls from non-standard model outputs.
//!
//! Some models (especially smaller local models) emit tool calls as plain text
//! in the `content` field instead of the standard `tool_calls` structure. This
//! module detects and parses those non-standard formats back into standard
//! `ToolCall`s, so the agent loop still executes them.
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
mod tests;
