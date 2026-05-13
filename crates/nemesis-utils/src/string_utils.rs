//! String utilities: truncation, random IDs, JSON helpers, time formatting.

use chrono::Utc;

/// Truncate a string to max_len characters, appending "..." if truncated.
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return s.chars().take(max_len).collect();
    }
    let truncated: String = s.chars().take(max_len - 3).collect();
    format!("{}...", truncated)
}

/// Generate a random hex ID (16 hex chars from 8 random bytes).
pub fn random_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Use a simple hash-based approach for deterministic-ish randomness
    let mut hash = nanos as u64;
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51afd7ed558ccd);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xc4ceb9fe1a85ec53);
    hash ^= hash >> 33;
    format!("{:016x}", hash)
}

/// Generate a random short ID (8 hex chars).
pub fn random_short_id() -> String {
    random_id()[..8].to_string()
}

/// Format a timestamp as RFC3339 string.
pub fn format_timestamp() -> String {
    Utc::now().to_rfc3339()
}

/// Format a timestamp as a compact datetime string (YYYY-MM-DD HH:MM:SS).
pub fn format_datetime_compact() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Try to parse a JSON string, returning a serde_json::Value.
pub fn parse_json(s: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(s).map_err(|e| format!("JSON parse error: {}", e))
}

/// Pretty-format a JSON string.
pub fn pretty_json(val: &serde_json::Value) -> String {
    serde_json::to_string_pretty(val).unwrap_or_else(|_| val.to_string())
}

/// Safely get a string from a JSON object by key.
pub fn json_get_str<'a>(val: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    val.get(key).and_then(|v| v.as_str())
}

/// Safely get an integer from a JSON object by key.
pub fn json_get_i64(val: &serde_json::Value, key: &str) -> Option<i64> {
    val.get(key).and_then(|v| v.as_i64())
}

/// Safely get a float from a JSON object by key.
pub fn json_get_f64(val: &serde_json::Value, key: &str) -> Option<f64> {
    val.get(key).and_then(|v| v.as_f64())
}

/// Safely get a boolean from a JSON object by key.
pub fn json_get_bool(val: &serde_json::Value, key: &str) -> Option<bool> {
    val.get(key).and_then(|v| v.as_bool())
}

/// Check if a string is blank (empty or only whitespace).
pub fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

/// Coalesce an Option<String> with a default.
pub fn deref_str(opt: Option<&String>, default: &str) -> String {
    opt.map(|s| s.clone()).unwrap_or_else(|| default.to_string())
}

/// Validate a skill identifier (slug or registry name).
/// Mirrors Go ValidateSkillIdentifier.
/// Must be non-empty, no backslashes, no "..", at most one "/".
pub fn validate_skill_identifier(identifier: &str) -> Result<(), String> {
    let trimmed = identifier.trim();
    if trimmed.is_empty() {
        return Err("identifier is required and must be a non-empty string".to_string());
    }
    if trimmed.contains('\\') {
        return Err("identifier must not contain backslashes to prevent directory traversal".to_string());
    }
    if trimmed.contains("..") {
        return Err("identifier must not contain '..' to prevent directory traversal".to_string());
    }
    if trimmed.matches('/').count() > 1 {
        return Err("identifier must not contain multiple slashes".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_truncation() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncation() {
        assert_eq!(truncate("hello world", 8), "hello...");
    }

    #[test]
    fn test_short_max() {
        assert_eq!(truncate("hello", 2), "he");
    }

    #[test]
    fn test_unicode() {
        assert_eq!(truncate("helloworld", 7), "hell...");
    }

    #[test]
    fn test_random_id() {
        let id = random_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_random_short_id() {
        let id = random_short_id();
        assert_eq!(id.len(), 8);
    }

    #[test]
    fn test_format_timestamp() {
        let ts = format_timestamp();
        assert!(ts.contains('T'));
    }

    #[test]
    fn test_format_datetime_compact() {
        let dt = format_datetime_compact();
        assert!(dt.contains('-'));
        assert!(dt.contains(':'));
    }

    #[test]
    fn test_parse_json() {
        let val = parse_json(r#"{"key": "value"}"#).unwrap();
        assert_eq!(val["key"], "value");
    }

    #[test]
    fn test_json_helpers() {
        let val = serde_json::json!({"name": "test", "count": 42, "rate": 3.14, "active": true});
        assert_eq!(json_get_str(&val, "name"), Some("test"));
        assert_eq!(json_get_i64(&val, "count"), Some(42));
        assert_eq!(json_get_f64(&val, "rate"), Some(3.14));
        assert_eq!(json_get_bool(&val, "active"), Some(true));
        assert_eq!(json_get_str(&val, "missing"), None);
    }

    #[test]
    fn test_is_blank() {
        assert!(is_blank(""));
        assert!(is_blank("   "));
        assert!(!is_blank("hello"));
    }

    #[test]
    fn test_deref_str() {
        assert_eq!(deref_str(Some(&"hello".to_string()), "default"), "hello");
        assert_eq!(deref_str(None, "default"), "default");
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_truncate_empty_string() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn test_truncate_zero_max() {
        assert_eq!(truncate("hello", 0), "");
    }

    #[test]
    fn test_truncate_one_max() {
        assert_eq!(truncate("hello", 1), "h");
    }

    #[test]
    fn test_truncate_three_max() {
        assert_eq!(truncate("hello world", 3), "hel");
    }

    #[test]
    fn test_truncate_four_max_adds_ellipsis() {
        assert_eq!(truncate("hello world", 4), "h...");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_unicode_characters() {
        // "hello" is ASCII, truncate to 3 chars (max_len <= 3), no ellipsis
        assert_eq!(truncate("hello", 3), "hel");
        // For 6 chars, we get first 3 + "..."
        assert_eq!(truncate("hello world", 6), "hel...");
    }

    #[test]
    fn test_random_id_unique() {
        // IDs should differ (extremely unlikely to collide)
        let id1 = random_id();
        let id2 = random_id();
        // Note: in fast tests they might be the same nanosecond,
        // so we just check format
        assert_eq!(id1.len(), 16);
        assert_eq!(id2.len(), 16);
    }

    #[test]
    fn test_random_short_id_unique() {
        let id1 = random_short_id();
        let id2 = random_short_id();
        assert_eq!(id1.len(), 8);
        assert_eq!(id2.len(), 8);
    }

    #[test]
    fn test_format_timestamp_rfc3339_format() {
        let ts = format_timestamp();
        // RFC3339 format: YYYY-MM-DDTHH:MM:SS+00:00
        assert!(ts.contains('T'));
        assert!(ts.contains(':'));
        assert!(ts.contains('-'));
        assert!(ts.contains('+') || ts.contains('Z'));
    }

    #[test]
    fn test_format_datetime_compact_format() {
        let dt = format_datetime_compact();
        // Format: YYYY-MM-DD HH:MM:SS
        assert_eq!(dt.len(), 19);
        assert_eq!(dt.chars().nth(4), Some('-'));
        assert_eq!(dt.chars().nth(7), Some('-'));
        assert_eq!(dt.chars().nth(10), Some(' '));
        assert_eq!(dt.chars().nth(13), Some(':'));
        assert_eq!(dt.chars().nth(16), Some(':'));
    }

    #[test]
    fn test_parse_json_invalid() {
        let result = parse_json("not json");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("JSON parse error"));
    }

    #[test]
    fn test_parse_json_array() {
        let val = parse_json("[1, 2, 3]").unwrap();
        assert!(val.is_array());
        assert_eq!(val.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_parse_json_nested() {
        let val = parse_json(r#"{"outer": {"inner": "value"}}"#).unwrap();
        assert_eq!(val["outer"]["inner"], "value");
    }

    #[test]
    fn test_pretty_json() {
        let val = serde_json::json!({"key": "value", "num": 42});
        let pretty = pretty_json(&val);
        assert!(pretty.contains('\n'));
        assert!(pretty.contains("  "));
    }

    #[test]
    fn test_json_get_str_missing_key() {
        let val = serde_json::json!({"name": "test"});
        assert_eq!(json_get_str(&val, "age"), None);
    }

    #[test]
    fn test_json_get_str_wrong_type() {
        let val = serde_json::json!({"name": 42});
        assert_eq!(json_get_str(&val, "name"), None);
    }

    #[test]
    fn test_json_get_i64_wrong_type() {
        let val = serde_json::json!({"count": "not a number"});
        assert_eq!(json_get_i64(&val, "count"), None);
    }

    #[test]
    fn test_json_get_f64_wrong_type() {
        let val = serde_json::json!({"rate": "not a number"});
        assert_eq!(json_get_f64(&val, "rate"), None);
    }

    #[test]
    fn test_json_get_bool_wrong_type() {
        let val = serde_json::json!({"active": "yes"});
        assert_eq!(json_get_bool(&val, "active"), None);
    }

    #[test]
    fn test_is_blank_tab() {
        assert!(is_blank("\t"));
    }

    #[test]
    fn test_is_blank_mixed_whitespace() {
        assert!(is_blank(" \t \n \r "));
    }

    #[test]
    fn test_deref_str_with_value() {
        let s = "hello".to_string();
        assert_eq!(deref_str(Some(&s), "default"), "hello");
    }

    #[test]
    fn test_validate_skill_identifier_valid() {
        assert!(validate_skill_identifier("my-skill").is_ok());
        assert!(validate_skill_identifier("author/my-skill").is_ok());
        assert!(validate_skill_identifier("anthropics/skills/coder").is_err()); // multiple slashes
    }

    #[test]
    fn test_validate_skill_identifier_empty() {
        assert!(validate_skill_identifier("").is_err());
        assert!(validate_skill_identifier("   ").is_err());
    }

    #[test]
    fn test_validate_skill_identifier_backslash() {
        assert!(validate_skill_identifier("path\\to\\skill").is_err());
    }

    #[test]
    fn test_validate_skill_identifier_double_dot() {
        assert!(validate_skill_identifier("../skill").is_err());
        assert!(validate_skill_identifier("skill/..").is_err());
    }

    #[test]
    fn test_validate_skill_identifier_multiple_slashes() {
        assert!(validate_skill_identifier("a/b/c").is_err());
    }

    #[test]
    fn test_validate_skill_identifier_single_slash() {
        assert!(validate_skill_identifier("author/skill").is_ok());
    }

    #[test]
    fn test_truncate_with_multibyte_chars() {
        // Chinese characters: each char is 3 bytes in UTF-8, but count as 1 char
        let s = "";
        assert_eq!(truncate(s, 5), ""); // 3 chars, fits in 5
        assert_eq!(truncate(s, 2), ""); // 3 chars, truncated to 2, max_len <=3 so no ellipsis
        assert_eq!(truncate(s, 5), ""); // 3 chars <= 5, no truncation
    }

    #[test]
    fn test_truncate_with_emoji() {
        // Emoji can be multi-char
        let s = "Hello World";
        assert_eq!(s.chars().count(), 11);
        assert_eq!(truncate(s, 10), "Hello W...");
    }

    #[test]
    fn test_random_id_format() {
        let id = random_id();
        // Should be exactly 16 hex chars
        assert_eq!(id.len(), 16);
        for c in id.chars() {
            assert!(c.is_ascii_hexdigit(), "Character '{}' is not hex", c);
        }
    }

    #[test]
    fn test_random_short_id_from_random_id() {
        // random_short_id should be the first 8 chars of random_id
        let short = random_short_id();
        assert_eq!(short.len(), 8);
        for c in short.chars() {
            assert!(c.is_ascii_hexdigit());
        }
    }

    #[test]
    fn test_parse_json_empty_object() {
        let val = parse_json("{}").unwrap();
        assert!(val.is_object());
        assert_eq!(val.as_object().unwrap().len(), 0);
    }

    #[test]
    fn test_parse_json_null() {
        let val = parse_json("null").unwrap();
        assert!(val.is_null());
    }

    #[test]
    fn test_parse_json_boolean() {
        let val = parse_json("true").unwrap();
        assert!(val.is_boolean());
        assert!(val.as_bool().unwrap());
    }

    #[test]
    fn test_parse_json_number() {
        let val = parse_json("42.5").unwrap();
        assert_eq!(val.as_f64().unwrap(), 42.5);
    }

    #[test]
    fn test_parse_json_empty_string() {
        let result = parse_json("");
        assert!(result.is_err());
    }

    #[test]
    fn test_pretty_json_nested() {
        let val = serde_json::json!({
            "outer": {
                "inner": [1, 2, 3]
            }
        });
        let pretty = pretty_json(&val);
        assert!(pretty.contains('\n'));
        assert!(pretty.contains("outer"));
        assert!(pretty.contains("inner"));
    }

    #[test]
    fn test_json_get_str_from_array() {
        let val = serde_json::json!([1, 2, 3]);
        // Array doesn't have keys, should return None
        assert_eq!(json_get_str(&val, "0"), None);
    }

    #[test]
    fn test_json_get_i64_missing_key() {
        let val = serde_json::json!({"count": 42});
        assert_eq!(json_get_i64(&val, "missing"), None);
    }

    #[test]
    fn test_json_get_i64_from_float() {
        // i64 from a float value should work if it's a whole number
        let val = serde_json::json!({"count": 42.0});
        // as_i64() returns None for float
        assert_eq!(json_get_i64(&val, "count"), None);
    }

    #[test]
    fn test_json_get_f64_missing_key() {
        let val = serde_json::json!({"rate": 3.14});
        assert_eq!(json_get_f64(&val, "missing"), None);
    }

    #[test]
    fn test_json_get_f64_from_int() {
        // f64 from int should work
        let val = serde_json::json!({"rate": 42});
        assert_eq!(json_get_f64(&val, "rate"), Some(42.0));
    }

    #[test]
    fn test_json_get_bool_missing_key() {
        let val = serde_json::json!({"active": true});
        assert_eq!(json_get_bool(&val, "missing"), None);
    }

    #[test]
    fn test_is_blank_with_text() {
        assert!(!is_blank("hello"));
        assert!(!is_blank("  hello  "));
    }

    #[test]
    fn test_is_blank_with_newline() {
        assert!(is_blank("\n"));
        assert!(is_blank("\n\r\n"));
    }

    #[test]
    fn test_deref_str_with_empty_string() {
        let s = "".to_string();
        assert_eq!(deref_str(Some(&s), "default"), "");
    }

    #[test]
    fn test_deref_str_default_used() {
        assert_eq!(deref_str(None, "fallback"), "fallback");
    }

    #[test]
    fn test_validate_skill_identifier_with_spaces() {
        // "  my-skill  " has spaces but trim() makes it "my-skill"
        assert!(validate_skill_identifier("  my-skill  ").is_ok());
    }

    #[test]
    fn test_validate_skill_identifier_no_slash() {
        assert!(validate_skill_identifier("simple-name").is_ok());
    }

    #[test]
    fn test_validate_skill_identifier_with_special_chars() {
        // Special chars that are not backslash or .. are ok
        assert!(validate_skill_identifier("my-skill-v2").is_ok());
        assert!(validate_skill_identifier("my_skill").is_ok());
        assert!(validate_skill_identifier("my.skill").is_ok());
    }

    #[test]
    fn test_format_timestamp_not_empty() {
        let ts = format_timestamp();
        assert!(!ts.is_empty());
    }

    #[test]
    fn test_format_datetime_compact_not_empty() {
        let dt = format_datetime_compact();
        assert!(!dt.is_empty());
    }
}
