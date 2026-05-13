//! Agent ID normalization.

pub const DEFAULT_AGENT_ID: &str = "main";
pub const DEFAULT_ACCOUNT_ID: &str = "default";
pub const MAX_AGENT_ID_LENGTH: usize = 64;

/// Normalize an agent ID to [a-z0-9][a-z0-9_-]{0,63}.
pub fn normalize_agent_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return DEFAULT_AGENT_ID.to_string();
    }
    let lower = trimmed.to_lowercase();
    if is_valid_id(&lower) {
        return lower;
    }
    let result: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let result = result.trim_matches(|c: char| c == '-').to_string();
    let result = if result.len() > MAX_AGENT_ID_LENGTH {
        &result[..MAX_AGENT_ID_LENGTH]
    } else {
        &result
    };
    let result = result.trim_matches(|c: char| c == '-').to_string();
    if result.is_empty() {
        DEFAULT_AGENT_ID.to_string()
    } else {
        result.to_string()
    }
}

/// Normalize an account ID.
pub fn normalize_account_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return DEFAULT_ACCOUNT_ID.to_string();
    }
    let lower = trimmed.to_lowercase();
    if is_valid_id(&lower) {
        return lower;
    }
    let result: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let result = result.trim_matches(|c: char| c == '-').to_string();
    if result.is_empty() {
        DEFAULT_ACCOUNT_ID.to_string()
    } else {
        result
    }
}

fn is_valid_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_AGENT_ID_LENGTH {
        return false;
    }
    let first = id.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_empty() {
        assert_eq!(normalize_agent_id(""), DEFAULT_AGENT_ID);
    }

    #[test]
    fn test_normalize_clean() {
        assert_eq!(normalize_agent_id("my-agent"), "my-agent");
    }

    #[test]
    fn test_normalize_uppercase() {
        assert_eq!(normalize_agent_id("MyAgent"), "myagent");
    }

    #[test]
    fn test_normalize_special_chars() {
        let result = normalize_agent_id("my agent!");
        assert!(!result.contains(' '));
        assert!(!result.contains('!'));
    }

    #[test]
    fn test_normalize_account_id() {
        assert_eq!(normalize_account_id(""), DEFAULT_ACCOUNT_ID);
        assert_eq!(normalize_account_id("MyAccount"), "myaccount");
    }

    #[test]
    fn test_normalize_agent_id_long_string() {
        let long_id = "a".repeat(100);
        let result = normalize_agent_id(&long_id);
        assert_eq!(result.len(), MAX_AGENT_ID_LENGTH);
        assert!(result.chars().all(|c| c == 'a'));
    }

    #[test]
    fn test_normalize_agent_id_only_special_chars() {
        let result = normalize_agent_id("!@#$%^&*()");
        // All special chars replaced with '-', then trimmed, leaving empty -> DEFAULT
        assert_eq!(result, DEFAULT_AGENT_ID);
    }

    #[test]
    fn test_normalize_agent_id_mixed_special() {
        let result = normalize_agent_id("my agent!!!");
        assert!(!result.contains(' '));
        assert!(!result.contains('!'));
        assert!(result.contains('-'));
    }

    #[test]
    fn test_normalize_agent_id_leading_special() {
        let result = normalize_agent_id("!myagent");
        // Leading '!' becomes '-', then trimmed
        assert!(!result.starts_with('-'));
        assert_eq!(result, "myagent");
    }

    #[test]
    fn test_normalize_agent_id_underscore_and_hyphen() {
        assert_eq!(normalize_agent_id("my_agent-1"), "my_agent-1");
    }

    #[test]
    fn test_normalize_agent_id_unicode() {
        let result = normalize_agent_id("agent\u{4e2d}\u{6587}");
        // Chinese characters become '-', then trimmed from ends -> "agent"
        assert!(!result.contains('\u{4e2d}'));
        assert!(!result.contains('\u{6587}'));
        // Result is valid ASCII
        assert!(result.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn test_normalize_agent_id_unicode_between_ascii() {
        let result = normalize_agent_id("a\u{4e2d}b");
        // Unicode char in the middle becomes '-', so "a-b"
        assert_eq!(result, "a-b");
    }

    #[test]
    fn test_normalize_account_id_special_chars() {
        let result = normalize_account_id("my account!");
        assert!(!result.contains(' '));
        assert!(!result.contains('!'));
    }

    #[test]
    fn test_normalize_account_id_only_special_chars() {
        let result = normalize_account_id("!@#");
        assert_eq!(result, DEFAULT_ACCOUNT_ID);
    }

    #[test]
    fn test_normalize_account_id_uppercase() {
        assert_eq!(normalize_account_id("MyAccount"), "myaccount");
    }

    #[test]
    fn test_normalize_account_id_with_underscore() {
        assert_eq!(normalize_account_id("my_account"), "my_account");
    }

    #[test]
    fn test_normalize_agent_id_whitespace_only() {
        assert_eq!(normalize_agent_id("   "), DEFAULT_AGENT_ID);
    }

    #[test]
    fn test_normalize_account_id_whitespace_only() {
        assert_eq!(normalize_account_id("   "), DEFAULT_ACCOUNT_ID);
    }

    #[test]
    fn test_is_valid_id_empty() {
        assert!(!is_valid_id(""));
    }

    #[test]
    fn test_is_valid_id_too_long() {
        let long = "a".repeat(MAX_AGENT_ID_LENGTH + 1);
        assert!(!is_valid_id(&long));
    }

    #[test]
    fn test_is_valid_id_starts_with_hyphen() {
        assert!(!is_valid_id("-agent"));
    }

    #[test]
    fn test_is_valid_id_valid() {
        assert!(is_valid_id("my-agent_1"));
    }

    #[test]
    fn test_is_valid_id_at_max_length() {
        let id = "a".repeat(MAX_AGENT_ID_LENGTH);
        assert!(is_valid_id(&id));
    }
}
