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
mod tests;
