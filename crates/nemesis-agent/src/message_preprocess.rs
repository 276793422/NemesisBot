//! Message preprocessing — @file reference expansion.
//!
//! Scans user message content for `@path` tokens, reads each existing file
//! relative to `base`, and inlines its content so the agent sees it without an
//! extra tool round-trip. Mirrors Reasonix's @-reference feature.

use std::path::Path;

fn at_file_re() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"@([^\s]+)").unwrap())
}

/// Expand `@path` references: for each @path that resolves to an existing file
/// under `base` (or is absolute and exists), inline its content in a `<file>`
/// block appended to the message. Non-existent paths are left untouched.
pub fn expand_at_files(content: &str, base: &Path) -> String {
    let re = at_file_re();
    let mut refs: Vec<String> = Vec::new();
    for cap in re.captures_iter(content) {
        let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let path_str = raw
            .trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | '!' | '?' | ')' | ']'));
        if path_str.is_empty() {
            continue;
        }
        let candidate = if Path::new(path_str).is_absolute() {
            Path::new(path_str).to_path_buf()
        } else {
            base.join(path_str)
        };
        if candidate.is_file() {
            if let Ok(body) = std::fs::read_to_string(&candidate) {
                let display = candidate
                    .strip_prefix(base)
                    .unwrap_or(&candidate)
                    .display();
                let truncated = if body.len() > 20000 {
                    format!(
                        "{}…\n(truncated, {} bytes total)",
                        &body[..20000.min(body.len())],
                        body.len()
                    )
                } else {
                    body
                };
                refs.push(format!("<file path=\"{}\">\n{}\n</file>", display, truncated));
            }
        }
    }
    if refs.is_empty() {
        content.to_string()
    } else {
        format!(
            "{}\n\n--- Referenced files ---\n{}",
            content,
            refs.join("\n\n")
        )
    }
}

#[cfg(test)]
mod tests;
