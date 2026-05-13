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
mod tests {
    use super::*;

    // ---- match_pattern tests ----

    #[test]
    fn test_match_pattern_exact() {
        assert!(match_pattern("/etc/passwd", "/etc/passwd"));
        assert!(!match_pattern("/etc/passwd", "/etc/shadow"));
    }

    #[test]
    fn test_match_pattern_single_star() {
        assert!(match_pattern("D:/123/*.key", "D:/123/test.key"));
        assert!(!match_pattern("D:/123/*.key", "D:/123/sub/test.key"));
    }

    #[test]
    fn test_match_pattern_double_star() {
        assert!(match_pattern("D:/123/**.key", "D:/123/test.key"));
        assert!(match_pattern("D:/123/**.key", "D:/123/sub/test.key"));
        assert!(match_pattern("D:/123/**.key", "D:/123/a/b/c/test.key"));
    }

    #[test]
    fn test_match_pattern_global_wildcard() {
        // Pattern without separator is global
        assert!(match_pattern("*.key", "/home/user/test.key"));
        assert!(match_pattern("*.key", "test.key"));
        assert!(match_pattern("*.key", "C:/Users/test.key"));
    }

    #[test]
    fn test_match_pattern_backslash_normalization() {
        assert!(match_pattern("D:\\123\\*.key", "D:/123/test.key"));
        assert!(match_pattern("D:/123/*.key", "D:\\123\\test.key"));
    }

    #[test]
    fn test_match_pattern_star_no_match() {
        assert!(!match_pattern("D:/123/*.txt", "D:/123/test.key"));
    }

    // ---- match_command_pattern tests ----

    #[test]
    fn test_match_command_pattern_wildcard() {
        assert!(match_command_pattern("git *", "git status"));
        assert!(match_command_pattern("git *", "git commit -m 'msg'"));
        assert!(match_command_pattern("rm -rf *", "rm -rf /tmp/test"));
    }

    #[test]
    fn test_match_command_pattern_surrounding_wildcard() {
        assert!(match_command_pattern("*sudo*", "sudo apt-get install"));
        assert!(match_command_pattern("*sudo*", "run sudo vim"));
    }

    #[test]
    fn test_match_command_pattern_exact() {
        assert!(match_command_pattern("ls", "ls"));
        assert!(!match_command_pattern("ls", "ls -la"));
    }

    #[test]
    fn test_match_command_pattern_special_chars() {
        assert!(match_command_pattern("cat *.txt", "cat file.txt"));
        assert!(match_command_pattern("echo hello", "echo hello"));
    }

    // ---- match_domain_pattern tests ----

    #[test]
    fn test_match_domain_exact() {
        assert!(match_domain_pattern("github.com", "github.com"));
        assert!(!match_domain_pattern("github.com", "api.github.com"));
    }

    #[test]
    fn test_match_domain_wildcard() {
        assert!(match_domain_pattern("*.github.com", "api.github.com"));
        assert!(!match_domain_pattern("*.github.com", "raw.githubusercontent.com"));
        assert!(!match_domain_pattern("*.github.com", "github.com"));
    }

    #[test]
    fn test_match_domain_case_insensitive() {
        assert!(match_domain_pattern("*.GitHub.COM", "API.GITHUB.COM"));
        assert!(match_domain_pattern("*.github.com", "Api.GitHub.Com"));
    }

    #[test]
    fn test_match_domain_wildcard_single_level() {
        // * should match only a single subdomain level
        assert!(match_domain_pattern("*.example.com", "api.example.com"));
        assert!(!match_domain_pattern("*.example.com", "a.b.example.com"));
    }

    #[test]
    fn test_match_domain_openai() {
        assert!(match_domain_pattern("*.openai.com", "api.openai.com"));
        assert!(!match_domain_pattern("*.openai.com", "openai.com"));
    }

    // --- Benchmark-style throughput tests ---

    #[test]
    fn test_match_pattern_throughput() {
        let patterns = [
            "/etc/passwd",
            "D:/123/*.key",
            "D:/123/**.key",
            "*.txt",
            "/home/*/config",
        ];
        let inputs = [
            "/etc/passwd",
            "D:/123/test.key",
            "D:/123/sub/deep/test.key",
            "/home/user/config",
            "/var/log/test.txt",
        ];

        let start = std::time::Instant::now();
        let iterations = 100;
        for _ in 0..iterations {
            for pattern in &patterns {
                for input in &inputs {
                    let _ = match_pattern(pattern, input);
                }
            }
        }
        let elapsed = start.elapsed();
        // Warm-up run - just verify it completes reasonably fast
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "Pattern matching too slow: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_match_command_pattern_throughput() {
        let patterns = [
            "git *",
            "rm -rf *",
            "*sudo*",
            "cat *.txt",
            "ls",
        ];
        let commands = [
            "git status",
            "rm -rf /tmp/test",
            "sudo apt-get install",
            "cat file.txt",
            "ls",
            "ls -la",
        ];

        let start = std::time::Instant::now();
        let iterations = 100;
        for _ in 0..iterations {
            for pattern in &patterns {
                for cmd in &commands {
                    let _ = match_command_pattern(pattern, cmd);
                }
            }
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "Command pattern matching too slow: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_match_domain_pattern_throughput() {
        let patterns = [
            "github.com",
            "*.github.com",
            "*.openai.com",
            "example.com",
            "*.example.com",
        ];
        let domains = [
            "github.com",
            "api.github.com",
            "raw.githubusercontent.com",
            "api.openai.com",
            "sub.example.com",
            "a.b.example.com",
        ];

        let start = std::time::Instant::now();
        let iterations = 100;
        for _ in 0..iterations {
            for pattern in &patterns {
                for domain in &domains {
                    let _ = match_domain_pattern(pattern, domain);
                }
            }
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "Domain pattern matching too slow: {:?}",
            elapsed
        );
    }
}
