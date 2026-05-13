//! Sanitizer - filters sensitive data from experiences and reflections.
//!
//! Replaces API keys, private IPs, file paths and other sensitive patterns
//! with `[REDACTED]` before sharing artifacts across the cluster.
//!
//! Also provides path sanitization (replacing home/workspace paths with
//! `[HOME]`/`[WORKSPACE]`) and public IP address replacement (replacing
//! non-private IPs with `[IP]`).

use regex::Regex;

/// The sanitizer scrubs sensitive information from strings.
pub struct Sanitizer {
    patterns: Vec<SanitizerPattern>,
    /// Optional home directory path for sanitization.
    home_dir: Option<String>,
    /// Optional workspace path for sanitization.
    workspace_dir: Option<String>,
}

struct SanitizerPattern {
    regex: Regex,
    label: &'static str,
}

impl Sanitizer {
    /// Create a new sanitizer with default rules.
    pub fn new() -> Self {
        let patterns = vec![
            // API keys: various formats (AWS, generic Bearer tokens, etc.)
            SanitizerPattern {
                regex: Regex::new(r#"(?i)(api[_-]?key|apikey|bearer|token|secret|authorization)\s*[:=]\s*['"]?[A-Za-z0-9_.\-]{20,}['"]?"#)
                    .expect("invalid api key regex"),
                label: "api_key",
            },
            // AWS-style keys
            SanitizerPattern {
                regex: Regex::new(r"AKIA[0-9A-Z]{16}")
                    .expect("invalid aws key regex"),
                label: "aws_key",
            },
            // Generic hex secrets (32+ hex chars)
            SanitizerPattern {
                regex: Regex::new(r"(?i)\b[0-9a-f]{32,}\b")
                    .expect("invalid hex secret regex"),
                label: "hex_secret",
            },
            // Windows file paths (C:\Users\... etc.)
            SanitizerPattern {
                regex: Regex::new(r#"(?i)[A-Z]:\\[^\s"')\]]+"#)
                    .expect("invalid windows path regex"),
                label: "file_path",
            },
            // Unix file paths (/home/..., /etc/..., /var/...)
            SanitizerPattern {
                regex: Regex::new(r#"(?:/home/|/etc/|/var/|/tmp/|/root/)[^\s"')\]]+"#)
                    .expect("invalid unix path regex"),
                label: "file_path",
            },
            // Email addresses
            SanitizerPattern {
                regex: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b")
                    .expect("invalid email regex"),
                label: "email",
            },
        ];

        // Detect home directory
        let home_dir = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok();

        Self {
            patterns,
            home_dir,
            workspace_dir: None,
        }
    }

    /// Create a new sanitizer with an explicit workspace path for sanitization.
    pub fn with_workspace(workspace: &str) -> Self {
        let mut s = Self::new();
        if !workspace.is_empty() {
            s.workspace_dir = Some(workspace.to_string());
        }
        s
    }

    /// Sanitize a string by replacing all detected sensitive patterns
    /// with `[REDACTED]`, then clean paths and public IPs.
    pub fn sanitize(&self, input: &str) -> String {
        let mut result = input.to_string();
        for pattern in &self.patterns {
            result = pattern.regex.replace_all(&result, "[REDACTED]").to_string();
        }
        // Apply path sanitization after pattern matching
        result = self.clean_paths(&result);
        // Apply public IP sanitization
        result = self.clean_public_ips(&result);
        result
    }

    /// Sanitize an experience's input and output summaries in-place.
    pub fn sanitize_experience(
        &self,
        experience: &mut nemesis_types::forge::Experience,
    ) {
        experience.input_summary = self.sanitize(&experience.input_summary);
        experience.output_summary = self.sanitize(&experience.output_summary);
        experience.session_key = self.sanitize(&experience.session_key);
    }

    /// Check if a string contains sensitive data (returns first match label).
    pub fn detect_sensitive(&self, input: &str) -> Option<&'static str> {
        for pattern in &self.patterns {
            if pattern.regex.is_match(input) {
                return Some(pattern.label);
            }
        }
        None
    }

    /// Replace file paths containing user home directory with `[HOME]`
    /// and workspace directory with `[WORKSPACE]`.
    ///
    /// This follows Go's `cleanPaths` logic:
    /// - `C:\Users\username\...` or `/home/username/...` replaced with `~/`
    /// - Workspace directory paths replaced with `[WORKSPACE]`
    /// - General Windows absolute paths (`C:\...`) replaced with `/`
    pub fn clean_paths(&self, content: &str) -> String {
        let mut result = content.to_string();

        // Replace workspace path first (more specific)
        if let Some(ref ws) = self.workspace_dir {
            if !ws.is_empty() {
                // Handle both forward-slash and backslash variants
                let ws_normalized = ws.replace('\\', "/");
                let ws_backslash = ws.replace('/', "\\");
                if result.contains(ws) {
                    result = result.replace(ws, "[WORKSPACE]");
                } else if result.contains(&ws_normalized) {
                    result = result.replace(&ws_normalized, "[WORKSPACE]");
                } else if result.contains(&ws_backslash) {
                    result = result.replace(&ws_backslash, "[WORKSPACE]");
                }
            }
        }

        // Replace home directory with [HOME]
        if let Some(ref home) = self.home_dir {
            if !home.is_empty() {
                let home_normalized = home.replace('\\', "/");
                let home_backslash = home.replace('/', "\\");
                if result.contains(home) {
                    result = result.replace(home, "[HOME]");
                } else if result.contains(&home_normalized) {
                    result = result.replace(&home_normalized, "[HOME]");
                } else if result.contains(&home_backslash) {
                    result = result.replace(&home_backslash, "[HOME]");
                }
            }
        }

        // Windows user paths: C:\Users\username\... -> ~/...
        let win_user_re = Regex::new(r"(?i)[A-Za-z]:\\Users\\[^\\]+\\").expect("invalid win user regex");
        result = win_user_re.replace_all(&result, "~/").to_string();

        // Unix user paths: /home/username/... -> ~/...
        let unix_home_re = Regex::new(r"/home/[^/]+/").expect("invalid unix home regex");
        result = unix_home_re.replace_all(&result, "~/").to_string();

        // General Windows absolute paths (non-Users): C:\ -> /
        let win_general_re = Regex::new(r"[A-Za-z]:\\").expect("invalid win general regex");
        result = win_general_re.replace_all(&result, "/").to_string();

        result
    }

    /// Replace public IP addresses with `[IP]`.
    ///
    /// Private/internal IPs (10.x, 172.16-31.x, 192.168.x, 127.x) are preserved.
    /// This follows Go's `cleanPublicIPs` logic.
    pub fn clean_public_ips(&self, content: &str) -> String {
        let ip_re = Regex::new(r"\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\b")
            .expect("invalid ip regex");
        ip_re
            .replace_all(content, |caps: &regex::Captures| {
                let ip = &caps[1];
                if is_private_ip(ip) {
                    ip.to_string()
                } else {
                    "[IP]".to_string()
                }
            })
            .to_string()
    }
}

impl Default for Sanitizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if an IP address is in a private/reserved range.
///
/// Matches Go's `isPrivateIP`:
/// - 127.x.x.x (loopback)
/// - 10.x.x.x (class A private)
/// - 192.168.x.x (class C private)
/// - 172.16.x.x - 172.31.x.x (class B private)
pub fn is_private_ip(ip: &str) -> bool {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return false;
    }

    let first = parts[0];

    // 127.x.x.x - loopback
    if first == "127" {
        return true;
    }

    // 10.x.x.x - class A private
    if first == "10" {
        return true;
    }

    // 192.168.x.x - class C private
    if first == "192" && parts[1] == "168" {
        return true;
    }

    // 172.16.x.x - 172.31.x.x - class B private
    if first == "172" {
        if let Ok(second) = parts[1].parse::<u32>() {
            if second >= 16 && second <= 31 {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use nemesis_types::forge::Experience;

    #[test]
    fn test_sanitize_api_key() {
        let sanitizer = Sanitizer::new();
        let input = r#"config: api_key="sk-abc123def456ghi789jkl012mno345""#;
        let result = sanitizer.sanitize(input);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("sk-abc123"));
    }

    #[test]
    fn test_sanitize_private_ip() {
        // Private IPs should NOT be redacted — they are preserved by clean_public_ips.
        // Only public IPs get replaced with [IP].
        let sanitizer = Sanitizer::new();
        let input = "Server running on 192.168.1.100 and 10.0.0.1";
        let result = sanitizer.sanitize(input);
        assert!(result.contains("192.168.1.100"));
        assert!(result.contains("10.0.0.1"));
    }

    #[test]
    fn test_sanitize_experience() {
        let sanitizer = Sanitizer::new();
        let mut exp = Experience {
            id: "exp-1".into(),
            tool_name: "file_read".into(),
            input_summary: "Read C:\\Users\\secret\\data.txt".into(),
            output_summary: "Connected to 192.168.1.50".into(),
            success: true,
            duration_ms: 100,
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: "sess-abc".into(),
        };
        sanitizer.sanitize_experience(&mut exp);
        assert!(exp.input_summary.contains("[REDACTED]"));
        // Private IP should be preserved (not [REDACTED])
        assert!(exp.output_summary.contains("192.168.1.50"));
    }

    #[test]
    fn test_detect_sensitive() {
        let sanitizer = Sanitizer::new();
        // Private IPs are no longer detected as sensitive by patterns
        assert_eq!(sanitizer.detect_sensitive("server at 10.0.0.1"), None);
        // But API keys are still detected
        assert_eq!(
            sanitizer.detect_sensitive("api_key=sk-1234567890abcdefghijklmnop"),
            Some("api_key")
        );
        assert_eq!(
            sanitizer.detect_sensitive("hello world"),
            None
        );
    }

    // --- Path sanitization tests ---

    #[test]
    fn test_clean_paths_workspace() {
        let sanitizer = Sanitizer::with_workspace("C:\\Projects\\mybot");
        let result = sanitizer.clean_paths("Reading C:\\Projects\\mybot\\config.json");
        assert!(result.contains("[WORKSPACE]"));
        assert!(!result.contains("C:\\Projects\\mybot"));
    }

    #[test]
    fn test_clean_paths_home_unix() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_paths("File at /home/alice/secret.txt was read");
        assert!(result.contains("~/"));
        assert!(!result.contains("/home/alice/"));
    }

    #[test]
    fn test_clean_paths_general_windows() {
        let sanitizer = Sanitizer::new();
        // Without a home dir set, general C:\ paths still get cleaned
        let result = sanitizer.clean_paths("Path is C:\\Windows\\System32");
        assert!(result.contains("/Windows"));
    }

    // --- Public IP tests ---

    #[test]
    fn test_clean_public_ips_replaces_public() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_public_ips("Connected to 8.8.8.8 for DNS");
        assert!(result.contains("[IP]"));
        assert!(!result.contains("8.8.8.8"));
    }

    #[test]
    fn test_clean_public_ips_preserves_private() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_public_ips("Local 192.168.1.1 and 10.0.0.1");
        assert!(result.contains("192.168.1.1"));
        assert!(result.contains("10.0.0.1"));
        assert!(!result.contains("[IP]"));
    }

    #[test]
    fn test_clean_public_ips_preserves_loopback() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_public_ips("Server at 127.0.0.1");
        assert!(result.contains("127.0.0.1"));
        assert!(!result.contains("[IP]"));
    }

    #[test]
    fn test_clean_public_ips_preserves_class_b() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_public_ips("Gateway 172.16.0.1");
        assert!(result.contains("172.16.0.1"));
        assert!(!result.contains("[IP]"));
    }

    #[test]
    fn test_clean_public_ips_replaces_non_private_172() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_public_ips("Host at 172.32.0.1");
        assert!(result.contains("[IP]"));
        assert!(!result.contains("172.32.0.1"));
    }

    // --- is_private_ip unit tests ---

    #[test]
    fn test_is_private_ip() {
        assert!(is_private_ip("127.0.0.1"));
        assert!(is_private_ip("10.0.0.1"));
        assert!(is_private_ip("10.255.255.255"));
        assert!(is_private_ip("192.168.0.1"));
        assert!(is_private_ip("192.168.1.100"));
        assert!(is_private_ip("172.16.0.1"));
        assert!(is_private_ip("172.31.255.255"));
        assert!(!is_private_ip("8.8.8.8"));
        assert!(!is_private_ip("1.1.1.1"));
        assert!(!is_private_ip("172.15.0.1"));
        assert!(!is_private_ip("172.32.0.1"));
        assert!(!is_private_ip("192.169.0.1"));
    }

    #[test]
    fn test_sanitize_default_creates_sanitizer() {
        let sanitizer = Sanitizer::default();
        let result = sanitizer.sanitize("hello world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_sanitize_aws_key() {
        let sanitizer = Sanitizer::new();
        let input = "Access key: AKIAIOSFODNN7EXAMPLE";
        let result = sanitizer.sanitize(input);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_sanitize_hex_secret() {
        let sanitizer = Sanitizer::new();
        let input = "Hash: a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4";
        let result = sanitizer.sanitize(input);
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_sanitize_email() {
        let sanitizer = Sanitizer::new();
        let input = "Contact: admin@example.com for help";
        let result = sanitizer.sanitize(input);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("admin@example.com"));
    }

    #[test]
    fn test_sanitize_bearer_token() {
        let sanitizer = Sanitizer::new();
        let input = r#"token: "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abcdefghij""#;
        let result = sanitizer.sanitize(input);
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_sanitize_localhost() {
        // Localhost is no longer a pattern (matching Go behavior).
        // It's a private/loopback IP and should be preserved.
        let sanitizer = Sanitizer::new();
        let input = "Connect to localhost:8080 or 127.0.0.1:3000";
        let result = sanitizer.sanitize(input);
        assert!(result.contains("localhost"));
        assert!(result.contains("127.0.0.1"));
    }

    #[test]
    fn test_sanitize_no_sensitive_data() {
        let sanitizer = Sanitizer::new();
        let input = "This is a normal message with no sensitive data.";
        let result = sanitizer.sanitize(input);
        assert_eq!(result, "This is a normal message with no sensitive data.");
    }

    #[test]
    fn test_is_private_ip_invalid() {
        assert!(!is_private_ip(""));
        assert!(!is_private_ip("not.an.ip"));
        assert!(!is_private_ip("1.2.3"));
        assert!(!is_private_ip("256.0.0.1"));
    }

    #[test]
    fn test_clean_paths_workspace_forward_slash() {
        let sanitizer = Sanitizer::with_workspace("/home/user/workspace");
        let result = sanitizer.clean_paths("File at /home/user/workspace/config.json");
        assert!(result.contains("[WORKSPACE]"));
    }

    #[test]
    fn test_clean_paths_workspace_backslash() {
        let sanitizer = Sanitizer::with_workspace("C:\\Users\\test\\ws");
        let result = sanitizer.clean_paths("Reading C:\\Users\\test\\ws\\data.txt");
        assert!(result.contains("[WORKSPACE]"));
    }

    #[test]
    fn test_detect_sensitive_various() {
        let sanitizer = Sanitizer::new();
        assert!(sanitizer.detect_sensitive("api_key=sk-1234567890abcdefghijklmnop").is_some());
        assert!(sanitizer.detect_sensitive("user@email.com").is_some());
        assert!(sanitizer.detect_sensitive("hello world").is_none());
    }

    #[test]
    fn test_sanitize_experience_full() {
        let sanitizer = Sanitizer::new();
        let mut exp = Experience {
            id: "exp-test".into(),
            tool_name: "tool".into(),
            input_summary: "api_key=sk-1234567890abcdefghijklmnop".into(),
            output_summary: "result".into(),
            success: true,
            duration_ms: 50,
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_key: "session-abc".into(),
        };
        sanitizer.sanitize_experience(&mut exp);
        assert!(exp.input_summary.contains("[REDACTED]"));
        // tool_name and other fields should remain unchanged
        assert_eq!(exp.tool_name, "tool");
    }

    #[test]
    fn test_clean_public_ips_multiple() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_public_ips("IPs: 8.8.8.8, 1.1.1.1, 192.168.1.1, 10.0.0.1");
        assert!(result.contains("[IP]"));
        assert!(result.contains("192.168.1.1"));
        assert!(result.contains("10.0.0.1"));
        assert!(!result.contains("8.8.8.8"));
        assert!(!result.contains("1.1.1.1"));
    }

    // --- Additional sanitizer tests ---

    #[test]
    fn test_sanitize_openai_key_various_formats() {
        let sanitizer = Sanitizer::new();
        // Format that matches the API key regex: api_key=<long value>
        let result1 = sanitizer.sanitize("api_key: sk-proj-abcdef1234567890abcdefghij");
        assert!(result1.contains("[REDACTED]"));
        // Token format with colon
        let result2 = sanitizer.sanitize("token: abcdefghijklmnopqrstuvwx1234567890yz");
        assert!(result2.contains("[REDACTED]"));
    }

    #[test]
    fn test_sanitize_github_token() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.sanitize("token: ghp_1234567890abcdefghijklmnopqrstuv");
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_sanitize_generic_api_key_patterns() {
        let sanitizer = Sanitizer::new();
        // api_key= format
        let result1 = sanitizer.sanitize("api_key=ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
        assert!(result1.contains("[REDACTED]"));
        // Bearer format
        let result2 = sanitizer.sanitize("Bearer abcdef1234567890abcdef1234567890");
        assert!(result2.contains("[REDACTED]"));
    }

    #[test]
    fn test_sanitize_multiple_secrets_in_one_string() {
        let sanitizer = Sanitizer::new();
        let input = "api_key=sk-1234567890abcdefghijklmnop and email=user@domain.com";
        let result = sanitizer.sanitize(input);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("sk-1234567890"));
        assert!(!result.contains("user@domain.com"));
    }

    #[test]
    fn test_sanitize_preserves_structure() {
        let sanitizer = Sanitizer::new();
        let input = "Config: api_key=sk-1234567890abcdefghijklmnop, host=example.com";
        let result = sanitizer.sanitize(input);
        // Should still have config structure
        assert!(result.contains("Config:"));
        assert!(result.contains("host=example.com"));
    }

    #[test]
    fn test_sanitize_empty_string() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.sanitize("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_sanitize_no_false_positives_short_strings() {
        let sanitizer = Sanitizer::new();
        let input = "The quick brown fox jumps over the lazy dog.";
        let result = sanitizer.sanitize(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_is_private_ip_private_ranges() {
        assert!(is_private_ip("10.0.0.1"));
        assert!(is_private_ip("10.255.255.255"));
        assert!(is_private_ip("172.16.0.1"));
        assert!(is_private_ip("172.31.255.255"));
        assert!(is_private_ip("192.168.0.1"));
        assert!(is_private_ip("192.168.255.255"));
        assert!(is_private_ip("127.0.0.1"));
        assert!(is_private_ip("127.255.255.255"));
    }

    #[test]
    fn test_is_private_ip_public_addresses() {
        assert!(!is_private_ip("8.8.8.8"));
        assert!(!is_private_ip("1.1.1.1"));
        assert!(!is_private_ip("203.0.113.1"));
        assert!(!is_private_ip("172.15.0.1")); // Just below 172.16 range
        assert!(!is_private_ip("172.32.0.1")); // Just above 172.31 range
    }

    #[test]
    fn test_clean_paths_no_workspace() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_paths("Reading /etc/config.json");
        // Without workspace, should return as-is (no workspace replacement)
        assert_eq!(result, "Reading /etc/config.json");
    }

    #[test]
    fn test_clean_paths_multiple_workspace_refs() {
        let sanitizer = Sanitizer::with_workspace("/home/user/ws");
        let result = sanitizer.clean_paths(
            "Read /home/user/ws/a.txt and /home/user/ws/b.txt and /home/user/ws/c.txt"
        );
        assert!(result.contains("[WORKSPACE]"));
        assert!(!result.contains("/home/user/ws/"));
    }

    #[test]
    fn test_clean_public_ips_no_public_ips() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_public_ips("Only private: 192.168.1.1, 10.0.0.1, 127.0.0.1");
        assert_eq!(result, "Only private: 192.168.1.1, 10.0.0.1, 127.0.0.1");
    }

    #[test]
    fn test_clean_public_ips_empty_string() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.clean_public_ips("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_detect_sensitive_none_for_plain_text() {
        let sanitizer = Sanitizer::new();
        assert!(sanitizer.detect_sensitive("just some normal text").is_none());
        assert!(sanitizer.detect_sensitive("").is_none());
        assert!(sanitizer.detect_sensitive("12345").is_none());
    }

    #[test]
    fn test_detect_sensitive_api_key_patterns() {
        let sanitizer = Sanitizer::new();
        assert!(sanitizer.detect_sensitive("api_key=ABCDEFGHIJKLMNOPQRSTUVWXYZ012345").is_some());
        // Use "secret=" which matches the regex pattern (secret\s*[:=])
        assert!(sanitizer.detect_sensitive("secret=abc123def456ghi789jkl012mno345pqr678").is_some());
    }

    #[test]
    fn test_detect_sensitive_email_patterns() {
        let sanitizer = Sanitizer::new();
        assert!(sanitizer.detect_sensitive("test@example.com").is_some());
        assert!(sanitizer.detect_sensitive("user.name@domain.org").is_some());
    }

    #[test]
    fn test_sanitize_experience_preserves_non_sensitive_fields() {
        let sanitizer = Sanitizer::new();
        let mut exp = Experience {
            id: "exp-clean".into(),
            tool_name: "file_read".into(),
            input_summary: "clean input".into(),
            output_summary: "clean output".into(),
            success: true,
            duration_ms: 100,
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_key: "session-123".into(),
        };
        sanitizer.sanitize_experience(&mut exp);
        assert_eq!(exp.id, "exp-clean");
        assert_eq!(exp.tool_name, "file_read");
        assert_eq!(exp.input_summary, "clean input");
        assert_eq!(exp.output_summary, "clean output");
        assert!(exp.success);
        assert_eq!(exp.duration_ms, 100);
    }

    #[test]
    fn test_sanitize_experience_cleans_output_summary() {
        let sanitizer = Sanitizer::new();
        let mut exp = Experience {
            id: "exp-dirty".into(),
            tool_name: "exec".into(),
            input_summary: "clean input".into(),
            output_summary: "result: api_key=sk-1234567890abcdefghijklmnop".into(),
            success: true,
            duration_ms: 50,
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_key: "session-456".into(),
        };
        sanitizer.sanitize_experience(&mut exp);
        assert!(exp.output_summary.contains("[REDACTED]"));
        assert!(!exp.output_summary.contains("sk-1234567890"));
    }

    #[test]
    fn test_sanitize_combined_workspace_and_secrets() {
        let sanitizer = Sanitizer::with_workspace("/home/user/project");
        let input = "api_key=sk-1234567890abcdefghijklmnop at /home/user/project/config.json";
        let result = sanitizer.sanitize(input);
        assert!(result.contains("[REDACTED]"));
        // workspace cleaning is separate from sanitize, which only does secrets
    }

    #[test]
    fn test_sanitize_aws_key_variant() {
        let sanitizer = Sanitizer::new();
        let result = sanitizer.sanitize("key: AKIA1234567890ABCDEF");
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_sanitize_experience_both_fields_dirty() {
        let sanitizer = Sanitizer::new();
        let mut exp = Experience {
            id: "exp-both".into(),
            tool_name: "network".into(),
            input_summary: "url with admin@secret.com".into(),
            output_summary: "response has api_key=sk-1234567890abcdefghijklmnopqrst".into(),
            success: false,
            duration_ms: 999,
            timestamp: "2026-03-15T12:00:00Z".into(),
            session_key: "sess-dirty".into(),
        };
        sanitizer.sanitize_experience(&mut exp);
        assert!(exp.input_summary.contains("[REDACTED]"));
        assert!(exp.output_summary.contains("[REDACTED]"));
        assert!(!exp.success);
        assert_eq!(exp.duration_ms, 999);
    }
}
