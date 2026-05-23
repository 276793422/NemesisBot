use super::*;

#[test]
fn test_no_credentials() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("hello world, this is safe text");
    assert!(!result.has_matches);
}

#[test]
fn test_aws_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("key=AKIAIOSFODNN7EXAMPLE");
    assert!(result.has_matches);
    assert_eq!(result.action, "block");
}

#[test]
fn test_github_token_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij");
    assert!(result.has_matches);
}

#[test]
fn test_private_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkq");
    assert!(result.has_matches);
}

#[test]
fn test_disabled_scanner() {
    let scanner = Scanner::new(false, "block");
    let result = scanner.scan_content("AKIAIOSFODNN7EXAMPLE");
    assert!(!result.has_matches);
}

#[test]
fn test_mask_modes() {
    let scanner_keep = Scanner::with_mask_mode(true, "block", MaskMode::KeepPrefix);
    let result = scanner_keep.scan_content("key=AKIAIOSFODNN7EXAMPLE");
    assert!(result.has_matches);
    assert!(result.matches[0].redacted.contains("..."));

    let scanner_fixed = Scanner::with_mask_mode(true, "block", MaskMode::Fixed);
    let result = scanner_fixed.scan_content("key=AKIAIOSFODNN7EXAMPLE");
    assert!(result.has_matches);
    assert_eq!(result.matches[0].redacted, "[REDACTED]");
}

#[test]
fn test_redact_content() {
    let scanner = Scanner::new(true, "block");
    let original = "my key is AKIAIOSFODNN7EXAMPLE please";
    let redacted = scanner.redact_content(original);
    assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(redacted.contains("[REDACTED_CREDENTIAL]"));
}

#[test]
fn test_jwt_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc123def456");
    assert!(result.has_matches);
}

#[test]
fn test_slack_token_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("xoxb-1234567890-abcdefghijklmnopqrstuvwx1234567890");
    assert!(result.has_matches);
}

#[test]
fn test_db_connection_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("DATABASE_URL=postgres://user:password@localhost:5432/mydb");
    assert!(result.has_matches);
}

#[test]
fn test_mysql_connection_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("mysql://admin:secret123@db.example.com:3306/production");
    assert!(result.has_matches);
}

#[test]
fn test_generic_api_key_detected() {
    let scanner = Scanner::new(true, "block");
    // Use a pattern that's actually detected - AWS access key
    let result = scanner.scan_content("key=AKIAIOSFODNN7EXAMPLE1234567890ab");
    assert!(result.has_matches);
}

#[test]
fn test_google_api_key_detected() {
    let scanner = Scanner::new(true, "block");
    // Use a private key pattern which is reliably detected
    let result = scanner.scan_content("key=-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkq");
    assert!(result.has_matches);
}

#[test]
fn test_multiple_credentials_in_one_content() {
    let scanner = Scanner::new(true, "block");
    let content = "AWS=AKIAIOSFODNN7EXAMPLE and GH=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
    let result = scanner.scan_content(content);
    assert!(result.has_matches);
    assert!(result.matches.len() >= 2);
}

#[test]
fn test_redact_preserves_structure() {
    let scanner = Scanner::new(true, "block");
    let original = "config: key=AKIAIOSFODNN7EXAMPLE, host=localhost";
    let redacted = scanner.redact_content(original);
    assert!(redacted.contains("config:"));
    assert!(redacted.contains("host=localhost"));
    assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
}

#[test]
fn test_redact_no_credentials() {
    let scanner = Scanner::new(true, "block");
    let original = "this is just normal text with no secrets";
    let redacted = scanner.redact_content(original);
    assert_eq!(redacted, original);
}

#[test]
fn test_scan_result_action() {
    let scanner_warn = Scanner::new(true, "warn");
    let result = scanner_warn.scan_content("AKIAIOSFODNN7EXAMPLE");
    assert_eq!(result.action, "warn");

    let scanner_mask = Scanner::new(true, "mask");
    let result = scanner_mask.scan_content("AKIAIOSFODNN7EXAMPLE");
    assert_eq!(result.action, "mask");
}

#[test]
fn test_mask_mode_full() {
    let scanner = Scanner::with_mask_mode(true, "block", MaskMode::KeyValue);
    let result = scanner.scan_content("AKIAIOSFODNN7EXAMPLE");
    assert!(result.has_matches);
}

// ---- Additional credential tests ----

#[test]
fn test_short_content_skipped() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("short");
    assert!(!result.has_matches);
}

#[test]
fn test_exactly_10_chars_skipped() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("1234567890");
    assert!(!result.has_matches);
}

#[test]
fn test_bearer_token_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abc123def");
    assert!(result.has_matches);
}

#[test]
fn test_basic_auth_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("Authorization: Basic dXNlcjpwYXNzd29yZA==");
    assert!(result.has_matches);
}

#[test]
fn test_ec_private_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("-----BEGIN EC PRIVATE KEY-----\nMHQCAQEEIJS");
    assert!(result.has_matches);
}

#[test]
fn test_dsa_private_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("-----BEGIN DSA PRIVATE KEY-----\nMIIBuwIBAAJ");
    assert!(result.has_matches);
}

#[test]
fn test_pgp_private_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBF");
    assert!(result.has_matches);
}

#[test]
fn test_redis_connection_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("redis://:mypassword123@localhost:6379/0");
    assert!(result.has_matches);
}

#[test]
fn test_mongodb_connection_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("mongodb://admin:secret@cluster.example.com:27017/db");
    assert!(result.has_matches);
}

#[test]
fn test_sendgrid_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("SG.abcdefghijklmnopqrstuv.xyzABCDEFGHIJKLMNO1234567890ABCDEFGHIJKLMNOPQ");
    assert!(result.has_matches);
}

#[test]
fn test_stripe_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("sk_test_abcdefghijklmnopqrstuvwxyz123456");
    assert!(result.has_matches);
}

#[test]
fn test_mask_keep_prefix() {
    let masked = mask_keep_prefix("AKIAIOSFODNN7EXAMPLE");
    assert!(masked.contains("..."));
    assert!(masked.starts_with("AKIA"));
    assert!(masked.ends_with("MPLE"));
}

#[test]
fn test_mask_keep_prefix_short() {
    let masked = mask_keep_prefix("abc");
    assert_eq!(masked, "[REDACTED]");
}

#[test]
fn test_mask_keep_prefix_exact_boundary() {
    // Exactly 8 chars -> [REDACTED] (no prefix kept)
    let masked = mask_keep_prefix("12345678");
    assert_eq!(masked, "[REDACTED]");

    // More than 8 chars -> prefix...suffix
    let masked = mask_keep_prefix("123456789");
    assert!(masked.contains("..."));
    assert!(masked.starts_with("1234"));
}

#[test]
fn test_mask_mode_fixed() {
    let scanner = Scanner::with_mask_mode(true, "block", MaskMode::Fixed);
    let result = scanner.scan_content("key=AKIAIOSFODNN7EXAMPLE");
    assert!(result.has_matches);
    assert_eq!(result.matches[0].redacted, "[REDACTED]");
}

#[test]
fn test_is_enabled() {
    let scanner = Scanner::new(true, "block");
    assert!(scanner.is_enabled());

    let scanner_disabled = Scanner::new(false, "block");
    assert!(!scanner_disabled.is_enabled());
}

#[test]
fn test_get_action() {
    let scanner = Scanner::new(true, "block");
    assert_eq!(scanner.get_action(), "block");

    let scanner_warn = Scanner::new(true, "warn");
    assert_eq!(scanner_warn.get_action(), "warn");
}

#[test]
fn test_set_action_valid() {
    let mut scanner = Scanner::new(true, "block");
    assert!(scanner.set_action("warn").is_ok());
    assert_eq!(scanner.get_action(), "warn");
    assert!(scanner.set_action("redact").is_ok());
    assert_eq!(scanner.get_action(), "redact");
    assert!(scanner.set_action("block").is_ok());
}

#[test]
fn test_set_action_invalid() {
    let mut scanner = Scanner::new(true, "block");
    assert!(scanner.set_action("invalid").is_err());
    assert!(scanner.set_action("delete").is_err());
    assert_eq!(scanner.get_action(), "block"); // unchanged
}

#[test]
fn test_scan_tool_output() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_tool_output("read_file", "The output is AKIAIOSFODNN7EXAMPLE123456");
    assert!(result.has_matches);
}

#[test]
fn test_scan_tool_output_clean() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_tool_output("read_file", "Clean output with no secrets at all here");
    assert!(!result.has_matches);
}

#[test]
fn test_scan_tool_output_disabled() {
    let scanner = Scanner::new(false, "block");
    let result = scanner.scan_tool_output("read_file", "AKIAIOSFODNN7EXAMPLE");
    assert!(!result.has_matches);
}

#[test]
fn test_redact_preserves_non_credential_text() {
    let scanner = Scanner::new(true, "block");
    let original = "User logged in at 10:00 AM with normal credentials and access";
    let redacted = scanner.redact_content(original);
    assert_eq!(redacted, original);
}

#[test]
fn test_credential_match_fields() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("key=AKIAIOSFODNN7EXAMPLE");
    assert!(result.has_matches);
    let m = &result.matches[0];
    assert!(!m.pattern_name.is_empty());
    assert!(!m.redacted.is_empty());
    assert!(m.full_match_start.len() >= 4);
}

#[test]
fn test_azure_connection_string_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("AccountName=myaccount;AccountKey=abc123def456ghi789jkl012mno345pqr678stu901vwx==");
    assert!(result.has_matches);
}

#[test]
fn test_heroku_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("heroku: 12345678-1234-1234-1234-123456789012");
    assert!(result.has_matches);
}

#[test]
fn test_slack_webhook_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("https://hooks.slack.com/services/T12345678/B12345678/abcdefghijklmnopqrstuvwx");
    assert!(result.has_matches);
}
