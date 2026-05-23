//! Pattern matching utilities for security rules.
//!
//! Provides wildcard/glob matching for file paths, command patterns, and domain patterns.
//! Matches the Go implementation in `module/security/matcher.go`.

use regex::Regex;

/// Check if a target matches a pattern with wildcard support.
///
/// Supported wildcards:
/// - `*` matches any sequence within a single directory level (e.g., `*.key`, `D:/123/*.key`)
/// - `**` matches any sequence across multiple directory levels (e.g., `D:/123/**.key`)
/// - No wildcard: exact match (e.g., `/etc/passwd`)
///
/// Special case: patterns without a directory separator and containing wildcards
/// (e.g., `*.key`) match globally across all directories.
///
/// # Examples
/// ```
/// use nemesis_security::matcher::match_pattern;
/// assert!(match_pattern("*.key", "/home/user/test.key"));
/// assert!(match_pattern("D:/123/*.key", "D:/123/test.key"));
/// assert!(match_pattern("/etc/passwd", "/etc/passwd"));
/// ```
pub fn match_pattern(pattern: &str, target: &str) -> bool {
    // Normalize path separators to /
    let pattern = normalize_path(pattern);
    let target = normalize_path(target);

    // If no wildcards, do exact match
    if !pattern.contains('*') {
        return pattern == target;
    }

    // Special case: if pattern has no directory separator and has wildcards,
    // it's a global pattern - prepend ** to match across all directories.
    if !pattern.contains('/') {
        return do_match(&format!("**{}", pattern), &target);
    }

    do_match(&pattern, &target)
}

/// Normalize path separators to forward slash.
fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Convert a wildcard pattern to a regex and match against the target.
fn do_match(pattern: &str, target: &str) -> bool {
    let regex_pattern = wildcard_to_regex(pattern);
    match Regex::new(&regex_pattern) {
        Ok(re) => re.is_match(target),
        Err(_) => false,
    }
}

/// Convert a wildcard pattern to a regex pattern.
///
/// Supports:
/// - `*` matches any sequence except `/` (single directory level)
/// - `**` matches any sequence including `/` (multiple directory levels)
fn wildcard_to_regex(pattern: &str) -> String {
    let mut regex = String::with_capacity(pattern.len() * 2);
    regex.push('^');

    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Check for **
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            regex.push_str(".*");
            i += 2;
        } else if chars[i] == '*' {
            // Single * matches any sequence except /
            regex.push_str("[^/]*");
            i += 1;
        } else if matches!(
            chars[i],
            '^' | '$' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            // Escape special regex characters
            regex.push('\\');
            regex.push(chars[i]);
            i += 1;
        } else {
            regex.push(chars[i]);
            i += 1;
        }
    }

    regex.push('$');
    regex
}

/// Check if a command matches a pattern.
///
/// Supports `*` wildcard for command arguments. Unlike path matching,
/// `*` in command patterns matches any characters including spaces.
///
/// # Examples
/// ```
/// use nemesis_security::matcher::match_command_pattern;
/// assert!(match_command_pattern("git *", "git status"));
/// assert!(match_command_pattern("rm -rf *", "rm -rf /tmp/test"));
/// assert!(match_command_pattern("*sudo*", "sudo apt-get install"));
/// ```
pub fn match_command_pattern(pattern: &str, command: &str) -> bool {
    // For commands, * matches any characters including spaces.
    // Use a placeholder to preserve wildcards through quoting.
    const WILDCARD_PLACEHOLDER: &str = "\x00WILDCARD\x00";

    let escaped = pattern.replace('*', WILDCARD_PLACEHOLDER);
    let quoted = regex::escape(&escaped);
    let regex_body = quoted.replace(WILDCARD_PLACEHOLDER, ".*");
    let regex_pattern = format!("^{}$", regex_body);

    match Regex::new(&regex_pattern) {
        Ok(re) => re.is_match(command),
        Err(_) => false,
    }
}

/// Check if a domain matches a pattern.
///
/// # Examples
/// ```
/// use nemesis_security::matcher::match_domain_pattern;
/// assert!(match_domain_pattern("*.github.com", "api.github.com"));
/// assert!(match_domain_pattern("github.com", "github.com"));
/// assert!(!match_domain_pattern("*.github.com", "github.com"));
/// ```
pub fn match_domain_pattern(pattern: &str, domain: &str) -> bool {
    let domain = domain.to_lowercase();
    let pattern = pattern.to_lowercase();

    // No wildcard - exact match
    if !pattern.contains('*') {
        return domain == pattern;
    }

    // Use placeholders to preserve wildcards and dots through escaping
    const WILDCARD_PLACEHOLDER: &str = "\x00WILDCARD\x00";
    const DOT_PLACEHOLDER: &str = "\x00LITERALDOT\x00";

    // Step 1: Replace wildcards with placeholder
    let p = pattern.replace('*', WILDCARD_PLACEHOLDER);
    // Step 2: Replace literal dots with placeholder
    let p = p.replace('.', DOT_PLACEHOLDER);
    // Step 3: Escape remaining special characters
    let p = regex::escape(&p);
    // Step 4: Replace placeholders with actual regex patterns
    // For domains, * should match only a single subdomain level (anything except dot)
    let p = p.replace(WILDCARD_PLACEHOLDER, "[^.]*");
    let p = p.replace(DOT_PLACEHOLDER, "\\.");

    let regex_pattern = format!("^{}$", p);
    match Regex::new(&regex_pattern) {
        Ok(re) => re.is_match(&domain),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
