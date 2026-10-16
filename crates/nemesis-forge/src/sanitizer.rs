//! Sanitizer - filters sensitive data from experiences and reflections.
//!
//! Replaces API keys, private IPs, file paths and other sensitive patterns
//! with `[REDACTED]` before sharing artifacts across the cluster.
//!
//! Also provides path sanitization (replacing home/workspace paths with
//! `[HOME]`/`[WORKSPACE]`) and public IP address replacement (replacing
//! non-private IPs with `[IP]`).

use std::sync::LazyLock;

use regex::Regex;

// 编译期静态正则（2026-10-16 R18）：Sanitizer 此前在 new()/clean_paths()/
// clean_public_ips() 里各处现场 Regex::new——collector 每条经验构造一个
// Sanitizer，反思循环里 sanitize 逐条调用，10 个正则每次全量重编译。
// 正则全部是字面量常量，提为 LazyLock 单次编译全局复用。
static RE_API_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(api[_-]?key|apikey|bearer|token|secret|authorization)\s*[:=]\s*['"]?[A-Za-z0-9_.\-]{20,}['"]?"#)
        .expect("invalid api key regex")
});
static RE_AWS_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"AKIA[0-9A-Z]{16}").expect("invalid aws key regex"));
static RE_HEX_SECRET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[0-9a-f]{32,}\b").expect("invalid hex secret regex"));
static RE_WIN_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)[A-Z]:\\[^\s"')\]]+"#).expect("invalid windows path regex"));
static RE_UNIX_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:/home/|/etc/|/var/|/tmp/|/root/)[^\s"')\]]+"#)
        .expect("invalid unix path regex")
});
static RE_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").expect("invalid email regex")
});
static RE_WIN_USER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[A-Za-z]:\\Users\\[^\\]+\\").expect("invalid win user regex")
});
static RE_UNIX_HOME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/home/[^/]+/").expect("invalid unix home regex"));
static RE_WIN_GENERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z]:\\").expect("invalid win general regex"));
static RE_IP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})\b").expect("invalid ip regex")
});

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
                regex: RE_API_KEY.clone(),
                label: "api_key",
            },
            // AWS-style keys
            SanitizerPattern {
                regex: RE_AWS_KEY.clone(),
                label: "aws_key",
            },
            // Generic hex secrets (32+ hex chars)
            SanitizerPattern {
                regex: RE_HEX_SECRET.clone(),
                label: "hex_secret",
            },
            // Windows file paths (C:\Users\... etc.)
            SanitizerPattern {
                regex: RE_WIN_PATH.clone(),
                label: "file_path",
            },
            // Unix file paths (/home/..., /etc/..., /var/...)
            SanitizerPattern {
                regex: RE_UNIX_PATH.clone(),
                label: "file_path",
            },
            // Email addresses
            SanitizerPattern {
                regex: RE_EMAIL.clone(),
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
    pub fn sanitize_experience(&self, experience: &mut nemesis_types::forge::Experience) {
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
        if let Some(ref ws) = self.workspace_dir
            && !ws.is_empty()
        {
            // Handle both forward-slash and backslash variants.
            // (BUG #24, quality-hardening goal 冲刺 S8) 顺序替换全部拼写变体——
            // 原 else-if 链在内容混用分隔符拼写时只替换第一个命中的变体，
            // 另一种拼写的路径原样泄露。
            let ws_normalized = ws.replace('\\', "/");
            let ws_backslash = ws.replace('/', "\\");
            for v in [ws.as_str(), ws_normalized.as_str(), ws_backslash.as_str()] {
                if result.contains(v) {
                    result = result.replace(v, "[WORKSPACE]");
                }
            }
        }

        // Replace home directory with [HOME]
        if let Some(ref home) = self.home_dir
            && !home.is_empty()
        {
            let home_normalized = home.replace('\\', "/");
            let home_backslash = home.replace('/', "\\");
            for v in [
                home.as_str(),
                home_normalized.as_str(),
                home_backslash.as_str(),
            ] {
                if result.contains(v) {
                    result = result.replace(v, "[HOME]");
                }
            }
        }

        // Windows user paths: C:\Users\username\... -> ~/...
        result = RE_WIN_USER.replace_all(&result, "~/").to_string();

        // Unix user paths: /home/username/... -> ~/...
        result = RE_UNIX_HOME.replace_all(&result, "~/").to_string();

        // General Windows absolute paths (non-Users): C:\ -> /
        result = RE_WIN_GENERAL.replace_all(&result, "/").to_string();

        result
    }

    /// Replace public IP addresses with `[IP]`.
    ///
    /// Private/internal IPs (10.x, 172.16-31.x, 192.168.x, 127.x) are preserved.
    /// This follows Go's `cleanPublicIPs` logic.
    pub fn clean_public_ips(&self, content: &str) -> String {
        RE_IP
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
    if first == "172"
        && let Ok(second) = parts[1].parse::<u32>()
        && (16..=31).contains(&second)
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests;
