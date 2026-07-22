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
