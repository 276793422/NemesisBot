//! String utilities: truncation, random IDs, JSON helpers, time formatting.

use chrono::Local;

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
    Local::now().to_rfc3339()
}

/// Format a timestamp as a compact datetime string (YYYY-MM-DD HH:MM:SS).
pub fn format_datetime_compact() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
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
mod tests;
