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
    assert!(!match_domain_pattern(
        "*.github.com",
        "raw.githubusercontent.com"
    ));
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
    let patterns = ["git *", "rm -rf *", "*sudo*", "cat *.txt", "ls"];
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

// ---- wildcard escape-arm coverage (regex metachars in pattern) ----

#[test]
fn test_match_pattern_escapes_regex_metachar() {
    // '+' falls into the escape arm of wildcard_to_regex: literal '+' in the
    // pattern matches a literal '+' in the target and nothing else.
    assert!(match_pattern("file+.tmp", "file+.tmp"));
    assert!(!match_pattern("file+.tmp", "fileXtmp"));
    // other metachars from the escape set
    assert!(match_pattern("a(b)", "a(b)"));
    assert!(match_pattern("x{2}", "x{2}"));
    assert!(match_pattern("c^d$e", "c^d$e"));
}

// ---- BUG-20261116-037：url_host / match_network_rule ----
// 网络族工具 target 是完整 URL，域名词法（match_domain_pattern）对整串
// URL 锚定恒败——出厂默认 `*.github.com` allow 是死规则、deny fail-open。
// match_network_rule 先按 URL host 归位，整串兜底保 `**`-style 向后兼容。

#[test]
fn url_host_extracts_from_url_forms() {
    assert_eq!(
        url_host("https://api.github.com/v1/repos"),
        Some("api.github.com")
    );
    assert_eq!(url_host("http://evil.com/path?x=1#frag"), Some("evil.com"));
    assert_eq!(
        url_host("https://host.example:8443/a"),
        Some("host.example")
    );
    assert_eq!(
        url_host("https://user:pass@host.example/x"),
        Some("host.example")
    );
    assert_eq!(url_host("http://[::1]:8080/x"), Some("[::1]"));
    // 裸域名 / host:port 无 scheme 原样归位
    assert_eq!(url_host("api.github.com"), Some("api.github.com"));
    assert_eq!(url_host("localhost:8080"), Some("localhost"));
    // 空 target = None（调用方落整串臂，行为同旧）
    assert_eq!(url_host(""), None);
}

#[test]
fn network_rule_matches_url_targets_by_host() {
    // 出厂默认 allow 规则形态——修复前对 URL 恒败（死规则）
    assert!(match_network_rule(
        "*.github.com",
        "https://api.github.com/repos/x/y"
    ));
    assert!(match_network_rule(
        "*.openai.com",
        "https://api.openai.com/v1/chat"
    ));
    // deny 方向：URL 带路径也拦得住（修复前 fail-open）
    assert!(match_network_rule("evil.com", "http://evil.com/steal"));
    assert!(match_network_rule(
        "*.evil.com",
        "http://sub.evil.com/steal"
    ));
    // 单标签域名词法语义保持（文档契约：*.github.com 不命中裸 github.com）
    assert!(!match_network_rule("*.github.com", "https://github.com/x"));
    // 无关域名不命中
    assert!(!match_network_rule("*.github.com", "https://evil.com/path"));
    // 大小写：match_domain_pattern 内部双折
    assert!(match_network_rule(
        "*.github.com",
        "https://API.GitHub.Com/v1"
    ));
}

#[test]
fn network_rule_raw_fallback_keeps_url_style_patterns() {
    // `**`-style URL 通配规则（修复前唯一能命中 URL 的形态）向后兼容
    assert!(match_network_rule("**evil.com**", "http://evil.com/steal"));
    // 裸 `*` 全域规则照常（host 臂即命中）
    assert!(match_network_rule("*", "https://anything.example/x"));
    // 空 target：host 提不出，落整串臂——`*` 对空串行为同旧
    assert!(match_network_rule("*", ""));
    assert!(!match_network_rule("*.github.com", ""));
}
