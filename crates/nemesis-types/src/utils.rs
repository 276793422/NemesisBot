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
mod tests {
    use super::*;

    #[test]
    fn test_format_rpc_response() {
        let result = format_rpc_response("abc123", "hello world");
        assert_eq!(result, "[rpc:abc123] hello world");
    }

    #[test]
    fn test_extract_rpc_correlation_id() {
        let (id, content) = extract_rpc_correlation_id("[rpc:abc123] hello world").unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_extract_rpc_correlation_id_no_prefix() {
        assert!(extract_rpc_correlation_id("no prefix").is_none());
    }

    #[test]
    fn test_new_id_unique() {
        let id1 = new_id();
        let id2 = new_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        assert_eq!(truncate("hello world", 8), "hello...");
    }

    #[test]
    fn test_truncate_exactly_max_len() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_max_len_zero() {
        assert_eq!(truncate("hello", 0), "...");
    }

    #[test]
    fn test_truncate_max_len_one() {
        assert_eq!(truncate("hello", 1), "...");
    }

    #[test]
    fn test_truncate_max_len_four() {
        assert_eq!(truncate("hello", 4), "h...");
    }

    // --- UTF-8 safety tests ---

    #[test]
    fn test_truncate_chinese_at_boundary() {
        // "你好世界ABCD" = 4×3 (CJK) + 4×1 (ASCII) = 16 bytes
        // truncate to 10: budget = 10-3 = 7 bytes
        // "你好" = 6 bytes fits, "世" = 3 bytes → 6+3=9 > 7, stops at "你好"
        let s = "你好世界ABCD";
        let result = truncate(s, 10);
        assert!(result.starts_with("你好"), "got: {}", result);
        assert!(result.ends_with("..."), "got: {}", result);
        assert!(result.is_char_boundary(result.len()));
    }

    #[test]
    fn test_truncate_exact_char_boundary() {
        // "你好" = 6 bytes, truncate to 6 → no truncation
        assert_eq!(truncate("你好", 6), "你好");
    }

    #[test]
    fn test_truncate_single_multibyte_char() {
        // "你好" = 6 bytes, truncate to 5 → cannot fit any char in budget=2
        assert_eq!(truncate("你好", 5), "...");
    }

    #[test]
    fn test_truncate_emoji() {
        // "Hello 🌍 World" — 🌍 is 4 bytes
        let s = "Hello 🌍 World";
        let result = truncate(s, 10);
        // "Hello " = 6 bytes, budget = 10-3 = 7, "Hello " fits, "🌍" needs 4 more = 10 > 7
        assert!(result.starts_with("Hello "), "got: {}", result);
        assert!(result.ends_with("..."), "got: {}", result);
    }

    #[test]
    fn test_correlation_id() {
        let id = correlation_id("rpc", "chat123");
        assert_eq!(id, "rpc:chat123");
    }

    #[test]
    fn test_correlation_id_empty_parts() {
        let id = correlation_id("", "");
        assert_eq!(id, ":");
    }

    #[test]
    fn test_correlation_id_with_colons() {
        let id = correlation_id("rpc", "host:8080:sess1");
        assert_eq!(id, "rpc:host:8080:sess1");
    }

    // --- extract_rpc_correlation_id edge cases ---

    #[test]
    fn test_extract_rpc_correlation_id_empty_after_bracket() {
        // Content is "[rpc:abc123]" with nothing after ] — rest should be empty
        // "[rpc:abc123]" has len=12, ']' at index 11, get(12..) => Some("")
        let (id, rest) = extract_rpc_correlation_id("[rpc:abc123]").unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(rest, "");
    }

    #[test]
    fn test_extract_rpc_correlation_id_empty_id() {
        // ID between [rpc: and ] is empty
        let (id, rest) = extract_rpc_correlation_id("[rpc:] hello").unwrap();
        assert_eq!(id, "");
        assert_eq!(rest, "hello");
    }

    #[test]
    fn test_extract_rpc_correlation_id_with_spaces() {
        let (id, rest) = extract_rpc_correlation_id("[rpc:abc123]   hello world").unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(rest, "hello world"); // trim_start removes leading spaces
    }

    #[test]
    fn test_extract_rpc_correlation_id_nested_brackets() {
        // First ']' after "[rpc:" is found
        let (id, rest) = extract_rpc_correlation_id("[rpc:abc]123] content").unwrap();
        assert_eq!(id, "abc");
        assert_eq!(rest, "123] content");
    }

    #[test]
    fn test_extract_rpc_correlation_id_no_closing_bracket() {
        // "[rpc:abc123" has no ']' => find returns None
        assert!(extract_rpc_correlation_id("[rpc:abc123").is_none());
    }

    #[test]
    fn test_extract_rpc_correlation_id_bracket_at_end_is_same_as_empty_after() {
        // This is the same as test_extract_rpc_correlation_id_empty_after_bracket,
        // just confirming the behavior with explicit assertion
        let result = extract_rpc_correlation_id("[rpc:abc123]");
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_rpc_correlation_id_prefix_not_rpc() {
        // Starts with [ but not [rpc:
        assert!(extract_rpc_correlation_id("[other:abc] content").is_none());
    }

    #[test]
    fn test_extract_rpc_correlation_id_empty_string() {
        assert!(extract_rpc_correlation_id("").is_none());
    }

    #[test]
    fn test_extract_rpc_correlation_id_partial_prefix() {
        assert!(extract_rpc_correlation_id("[rpc").is_none());
        assert!(extract_rpc_correlation_id("[rp").is_none());
    }

    // --- truncate edge cases ---

    #[test]
    fn test_truncate_max_len_two() {
        // budget = 2-3 = 0 → "..."
        assert_eq!(truncate("hello", 2), "...");
    }

    #[test]
    fn test_truncate_max_len_three() {
        // budget = 3-3 = 0 → "..."
        assert_eq!(truncate("hello", 3), "...");
    }

    #[test]
    fn test_truncate_empty_string() {
        assert_eq!(truncate("", 0), "");
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn test_truncate_unicode_fits() {
        let s = "héllo";
        assert_eq!(truncate(s, 10), "héllo");
    }

    #[test]
    fn test_truncate_long_string() {
        let s = "a".repeat(1000);
        let result = truncate(&s, 50);
        assert_eq!(result.len(), 50); // 47 chars + "..." = 50
        assert!(result.ends_with("..."));
    }

    // --- new_id ---

    #[test]
    fn test_new_id_format() {
        let id = new_id();
        // UUID v4 format: xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|&c| c == '-').count(), 4);
        // Version nibble should be 4
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert!(parts[2].starts_with('4')); // version 4
    }

    #[test]
    fn test_new_id_multiple_unique() {
        let ids: Vec<String> = (0..100).map(|_| new_id()).collect();
        let unique: std::collections::HashSet<String> = ids.iter().cloned().collect();
        assert_eq!(unique.len(), 100); // All unique
    }

    // --- now_timestamp ---

    #[test]
    fn test_now_timestamp_format() {
        let ts = now_timestamp();
        // RFC 3339 format should contain 'T' and either 'Z' or '+' for timezone
        assert!(ts.contains('T'));
        // Should be parseable by chrono
        let parsed = chrono::DateTime::parse_from_rfc3339(&ts);
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_now_timestamp_monotonic() {
        let t1 = now_timestamp();
        let t2 = now_timestamp();
        // t2 should be >= t1 (both are UTC)
        assert!(t2 >= t1);
    }

    // --- format_rpc_response ---

    #[test]
    fn test_format_rpc_response_empty_content() {
        assert_eq!(format_rpc_response("id1", ""), "[rpc:id1] ");
    }

    #[test]
    fn test_format_rpc_response_empty_id() {
        assert_eq!(format_rpc_response("", "hello"), "[rpc:] hello");
    }

    #[test]
    fn test_format_rpc_response_roundtrip() {
        let id = "test-correlation-123";
        let content = "This is the response content";
        let formatted = format_rpc_response(id, content);
        let (extracted_id, extracted_content) = extract_rpc_correlation_id(&formatted).unwrap();
        assert_eq!(extracted_id, id);
        assert_eq!(extracted_content, content);
    }

    // --- floor_char_boundary ---

    #[test]
    fn test_floor_char_boundary_ascii() {
        assert_eq!(floor_char_boundary("hello", 3), 3);
        assert_eq!(floor_char_boundary("hello", 5), 5);
        assert_eq!(floor_char_boundary("hello", 10), 5);
    }

    #[test]
    fn test_floor_char_boundary_multibyte() {
        // "你好世界" = 12 bytes (3 bytes each)
        let s = "你好世界";
        assert_eq!(floor_char_boundary(s, 5), 3); // 5 -> 3 (first char boundary)
        assert_eq!(floor_char_boundary(s, 4), 3); // 4 -> 3
        assert_eq!(floor_char_boundary(s, 6), 6); // exact boundary
        assert_eq!(floor_char_boundary(s, 0), 0);
    }

    #[test]
    fn test_floor_char_boundary_empty() {
        assert_eq!(floor_char_boundary("", 0), 0);
        assert_eq!(floor_char_boundary("", 5), 0);
    }

    // --- ceil_char_boundary ---

    #[test]
    fn test_ceil_char_boundary_ascii() {
        assert_eq!(ceil_char_boundary("hello", 2), 2);
        assert_eq!(ceil_char_boundary("hello", 5), 5);
        assert_eq!(ceil_char_boundary("hello", 0), 0);
    }

    #[test]
    fn test_ceil_char_boundary_multibyte() {
        // "你好世界" = 12 bytes
        let s = "你好世界";
        assert_eq!(ceil_char_boundary(s, 4), 6); // 4 -> 6 (next char boundary)
        assert_eq!(ceil_char_boundary(s, 5), 6); // 5 -> 6
        assert_eq!(ceil_char_boundary(s, 6), 6); // exact boundary
        assert_eq!(ceil_char_boundary(s, 0), 0);
    }

    #[test]
    fn test_ceil_char_boundary_empty() {
        assert_eq!(ceil_char_boundary("", 0), 0);
    }
}
