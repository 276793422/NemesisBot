//! Shared utility functions.

use uuid::Uuid;

/// Generate a new unique ID.
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Generate a correlation ID for RPC messages.
pub fn correlation_id(channel: &str, chat_id: &str) -> String {
    format!("{}:{}", channel, chat_id)
}

/// Format an RPC response with correlation ID prefix.
pub fn format_rpc_response(correlation_id: &str, content: &str) -> String {
    format!("[rpc:{}] {}", correlation_id, content)
}

/// Extract correlation ID from RPC response prefix.
pub fn extract_rpc_correlation_id(content: &str) -> Option<(&str, &str)> {
    if content.starts_with("[rpc:") {
        let end = content.find(']')?;
        let id = &content[5..end];
        let rest = content.get(end + 1..)?.trim_start();
        Some((id, rest))
    } else {
        None
    }
}

/// Get current ISO 8601 timestamp.
pub fn now_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Returns the largest byte index `i` such that `i <= max_len` and `s.is_char_boundary(i)`.
///
/// Safe to use for prefix slicing: `&s[..floor_char_boundary(s, n)]`
pub fn floor_char_boundary(s: &str, max_len: usize) -> usize {
    if max_len >= s.len() {
        return s.len();
    }
    let mut i = max_len;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Returns the smallest byte index `i` such that `i >= min_start` and `s.is_char_boundary(i)`.
///
/// Safe to use for suffix slicing: `&s[ceil_char_boundary(s, start)..]`
pub fn ceil_char_boundary(s: &str, min_start: usize) -> usize {
    if min_start == 0 || min_start >= s.len() {
        return min_start.min(s.len());
    }
    let mut i = min_start;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Truncate string to max byte length with ellipsis, UTF-8 safe.
///
/// Finds the largest char boundary not exceeding `max_len` before slicing.
/// If the string fits within `max_len` bytes, returns it unchanged.
/// Otherwise returns the longest prefix that fits (with room for "...").
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    // Need at least 3 bytes for "..."
    let budget = max_len.saturating_sub(3);
    if budget == 0 {
        return "...".to_string();
    }
    // Find the largest char boundary <= budget.
    let boundary = s
        .char_indices()
        .take_while(|(idx, ch)| *idx + ch.len_utf8() <= budget)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    if boundary == 0 {
        "...".to_string()
    } else {
        format!("{}...", &s[..boundary])
    }
}

#[cfg(test)]
mod tests;
