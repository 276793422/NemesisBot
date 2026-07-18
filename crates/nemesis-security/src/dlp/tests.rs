use super::*;

#[test]
fn test_clean_text() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Hello, this is a normal text.");
    assert!(!result.has_matches);
}

#[test]
fn test_credit_card_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Card: 4111111111111111");
    assert!(result.has_matches);
}

#[test]
fn test_email_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Contact: user@example.com for info");
    assert!(result.has_matches);
}

#[test]
fn test_ssn_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("SSN: 123-45-6789");
    assert!(result.has_matches);
}

#[test]
fn test_disabled() {
    let engine = DlpEngine::new(false, "block");
    let result = engine.scan_text("SSN: 123-45-6789");
    assert!(!result.has_matches);
}

#[test]
fn test_add_remove_rule() {
    let engine = DlpEngine::new(true, "block");
    engine.add_rule(DlpRule {
        name: "custom_secret".to_string(),
        category: "custom".to_string(),
        pattern: r"CUSTOM_SECRET_\d+".to_string(),
        enabled: true,
        action: "block".to_string(),
        confidence: DlpConfidence::Medium,
    }).unwrap();

    let result = engine.scan_text("Found CUSTOM_SECRET_12345 in text");
    assert!(result.has_matches);
    assert!(result.matches.iter().any(|m| m.rule_name == "custom_secret"));

    assert!(engine.remove_rule("custom_secret"));
    let result = engine.scan_text("Found CUSTOM_SECRET_12345 in text");
    assert!(!result.matches.iter().any(|m| m.rule_name == "custom_secret"));
}

#[test]
fn test_get_rule_names() {
    let engine = DlpEngine::new(true, "block");
    let names = engine.get_rule_names();
    assert!(names.len() >= 25);
}

#[test]
fn test_redact_content() {
    let engine = DlpEngine::new(true, "block");
    let original = "Email: user@example.com and SSN: 123-45-6789";
    let redacted = engine.redact_content(original);
    assert!(!redacted.contains("user@example.com"));
    assert!(!redacted.contains("123-45-6789"));
    assert!(redacted.contains("[REDACTED]"));
}

#[test]
fn test_update_config() {
    let mut engine = DlpEngine::new(true, "block");
    engine.update_config(Some(false), None);
    let result = engine.scan_text("SSN: 123-45-6789");
    assert!(!result.has_matches);
}

#[test]
fn test_discover_card() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Card: 6011111111111117");
    assert!(result.has_matches);
}

#[test]
fn test_ipv6_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("IP: 2001:0db8:85a3:0000:0000:8a2e:0370:7334");
    assert!(result.has_matches);
}

#[test]
fn test_phone_number_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Phone: (555) 123-4567");
    assert!(result.has_matches);
}

#[test]
fn test_multiple_sensitive_data() {
    let engine = DlpEngine::new(true, "block");
    let text = "Email: user@example.com, SSN: 123-45-6789, Card: 4111111111111111";
    let result = engine.scan_text(text);
    assert!(result.has_matches);
    assert!(result.matches.len() >= 2);
}

#[test]
fn test_redact_preserves_safe_text() {
    let engine = DlpEngine::new(true, "block");
    let original = "Hello world, this is safe text.";
    let redacted = engine.redact_content(original);
    assert_eq!(redacted, original);
}

#[test]
fn test_update_config_action() {
    let mut engine = DlpEngine::new(true, "block");
    engine.update_config(None, Some("warn".to_string()));
    let result = engine.scan_text("SSN: 123-45-6789");
    assert!(result.has_matches);
    assert_eq!(result.action, "warn");
}

#[test]
fn test_add_invalid_rule() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.add_rule(DlpRule {
        name: "bad_rule".to_string(),
        category: "custom".to_string(),
        pattern: "[invalid(regex".to_string(),
        enabled: true,
        action: "block".to_string(),
        confidence: DlpConfidence::Medium,
    });
    assert!(result.is_err());
}

#[test]
fn test_remove_nonexistent_rule() {
    let engine = DlpEngine::new(true, "block");
    assert!(!engine.remove_rule("nonexistent_rule"));
}

#[test]
fn test_disabled_rule_not_matched() {
    let engine = DlpEngine::new(true, "block");
    engine.add_rule(DlpRule {
        name: "disabled_test".to_string(),
        category: "custom".to_string(),
        pattern: r"DISABLED_PATTERN_\d+".to_string(),
        enabled: false,
        action: "block".to_string(),
        confidence: DlpConfidence::Medium,
    }).unwrap();

    let result = engine.scan_text("Found DISABLED_PATTERN_12345");
    assert!(!result.matches.iter().any(|m| m.rule_name == "disabled_test"));
}

#[test]
fn test_visa_card_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Card: 4222222222222222");
    assert!(result.has_matches);
}

#[test]
fn test_mastercard_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Card: 5555555555554444");
    assert!(result.has_matches);
}

// ---- Additional DLP tests ----

#[test]
fn test_amex_card_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Card: 378282246310005");
    assert!(result.has_matches);
}

#[test]
fn test_aws_access_key_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("aws_access_key=AKIAIOSFODNN7EXAMPLE");
    assert!(result.has_matches);
}

#[test]
fn test_google_api_key_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("api_key=AIzaSyA1234567890abcdefghijklmnopqrstuv");
    assert!(result.has_matches);
}

#[test]
fn test_rsa_private_key_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA");
    assert!(result.has_matches);
}

#[test]
fn test_openssh_private_key_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAA");
    assert!(result.has_matches);
}

#[test]
fn test_pkcs8_encrypted_key_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIE6TAbBgkqhkiG9w0B");
    assert!(result.has_matches);
}

#[test]
fn test_jwt_token_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc123def456");
    assert!(result.has_matches);
}

#[test]
fn test_github_token_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij123456");
    assert!(result.has_matches);
}

#[test]
fn test_china_id_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("ID: 110101199001011234");
    assert!(result.has_matches);
}

#[test]
fn test_private_ip_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Server: 192.168.1.100");
    assert!(result.has_matches);
}

#[test]
fn test_10_network_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Internal: 10.0.0.1");
    assert!(result.has_matches);
}

#[test]
fn test_172_network_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Host: 172.16.0.50");
    assert!(result.has_matches);
}

#[test]
fn test_password_assignment_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text(r#"password = "SuperSecret123!""#);
    assert!(result.has_matches);
}

#[test]
fn test_authorization_header_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.sig");
    assert!(result.has_matches);
}

#[test]
fn test_database_connection_string_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("postgres://user:password@localhost:5432/mydb");
    assert!(result.has_matches);
}

#[test]
fn test_stripe_key_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("sk_test_abcdefghijklmnopqrstuvwxyz123456");
    assert!(result.has_matches);
}

#[test]
fn test_slack_token_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("xoxb-1234567890-123456789012-abcdefghijklmnopqrstuvwx1234");
    assert!(result.has_matches);
}

#[test]
fn test_dlp_match_severity_mapping() {
    let engine = DlpEngine::new(true, "block");
    // Credit card should be High severity
    let result = engine.scan_text("Card: 4111111111111111");
    assert!(result.has_matches);
    assert_eq!(result.matches[0].severity, DlpSeverity::High);
}

#[test]
fn test_dlp_credential_severity() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("aws_secret_access_key = ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789012");
    if result.has_matches {
        // Credential matches should be Critical
        assert!(result.matches.iter().any(|m| m.severity == DlpSeverity::Critical));
    }
}

#[test]
fn test_partial_mask_short() {
    assert_eq!(partial_mask("abc"), "[REDACTED]");
}

#[test]
fn test_partial_mask_long() {
    let masked = partial_mask("1234567890ABCDEF");
    assert!(masked.starts_with("12"));
    assert!(masked.contains("****"));
    assert!(masked.ends_with("EF"));
}

#[test]
fn test_partial_mask_exact_boundary() {
    let masked = partial_mask("abcd");
    assert_eq!(masked, "[REDACTED]");
}

#[test]
fn test_partial_mask_multibyte_no_panic() {
    // Chinese matched text must not panic (old byte-slice &s[..2] would) and
    // should keep first 2 + last 2 chars, redacting the middle.
    let masked = partial_mask("中文密码符"); // 5 chars
    assert!(masked.contains("****"));
    assert!(masked.starts_with("中文"));
    assert!(masked.ends_with("码符"));
    assert!(!masked.contains("密"), "middle char must be redacted");
    // Short multibyte → fully redacted (no panic).
    assert_eq!(partial_mask("京"), "[REDACTED]");
}

#[test]
fn test_category_to_severity_mappings() {
    assert_eq!(category_to_severity("credential"), DlpSeverity::Critical);
    assert_eq!(category_to_severity("secret_key"), DlpSeverity::Critical);
    assert_eq!(category_to_severity("api_key"), DlpSeverity::Critical);
    assert_eq!(category_to_severity("pii"), DlpSeverity::High);
    assert_eq!(category_to_severity("financial"), DlpSeverity::High);
    assert_eq!(category_to_severity("credit_card"), DlpSeverity::High);
    assert_eq!(category_to_severity("network"), DlpSeverity::Medium);
    assert_eq!(category_to_severity("contact"), DlpSeverity::Medium);
    assert_eq!(category_to_severity("email"), DlpSeverity::Medium);
    assert_eq!(category_to_severity("phone"), DlpSeverity::Medium);
    assert_eq!(category_to_severity("ip"), DlpSeverity::Medium);
    assert_eq!(category_to_severity("unknown_category"), DlpSeverity::Low);
}

#[test]
fn test_scan_tool_output_reduces_severity() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_tool_output("exec", "Card: 4111111111111111");
    if result.has_matches {
        // Critical should be downgraded to High in tool output
        assert!(result.matches.iter().all(|m| m.severity != DlpSeverity::Critical));
    }
}

#[test]
fn test_scan_tool_output_clean() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_tool_output("exec", "Hello world, this is clean output");
    assert!(!result.has_matches);
}

#[test]
fn test_dlp_enabled_rules_filter() {
    let config = DlpConfig {
        enabled: true,
        action: "block".to_string(),
        custom_rules: vec![],
        enabled_rules: vec!["email".to_string()],
        max_content_length: 0,
        low_confidence_action: "log".to_string(),
    };
    let engine = DlpEngine::with_config(config);
    // Only email rule should fire
    let result = engine.scan_text("SSN: 123-45-6789 and email: test@example.com");
    // SSN should NOT be detected since only "email" is enabled
    assert!(result.matches.iter().any(|m| m.rule_name == "email"));
    assert!(!result.matches.iter().any(|m| m.rule_name == "us_ssn"));
}

#[test]
fn test_dlp_max_content_length() {
    let config = DlpConfig {
        enabled: true,
        action: "block".to_string(),
        custom_rules: vec![],
        enabled_rules: vec![],
        max_content_length: 20,
        low_confidence_action: "log".to_string(),
    };
    let engine = DlpEngine::with_config(config);
    // Content longer than max_content_length should be truncated
    let long_text = format!("SSN: 123-45-6789 and then more padding text here that goes beyond limit");
    let result = engine.scan_text(&long_text);
    // The SSN pattern is in the first 20 chars so should still be detected
    assert!(result.has_matches);
}

#[test]
fn test_scan_content_alias() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_content("Email: test@example.com");
    assert!(result.has_matches);
}

#[test]
fn test_is_enabled_set_enabled() {
    let mut engine = DlpEngine::new(true, "block");
    assert!(engine.is_enabled());
    engine.set_enabled(false);
    assert!(!engine.is_enabled());
    engine.set_enabled(true);
    assert!(engine.is_enabled());
}

#[test]
fn test_total_rule_count() {
    let engine = DlpEngine::new(true, "block");
    let count = engine.total_rule_count();
    assert!(count >= 25, "expected >= 25 rules, got {}", count);
}

#[test]
fn test_enabled_rule_count_no_filter() {
    let engine = DlpEngine::new(true, "block");
    let count = engine.enabled_rule_count();
    assert!(count >= 25);
}

#[test]
fn test_enabled_rule_count_with_filter() {
    let config = DlpConfig {
        enabled: true,
        action: "block".to_string(),
        custom_rules: vec![],
        enabled_rules: vec!["email".to_string(), "us_ssn".to_string()],
        max_content_length: 0,
        low_confidence_action: "log".to_string(),
    };
    let engine = DlpEngine::with_config(config);
    assert_eq!(engine.enabled_rule_count(), 2);
}

#[test]
fn test_add_duplicate_dynamic_rule() {
    let engine = DlpEngine::new(true, "block");
    engine.add_rule(DlpRule {
        name: "dup_rule".to_string(),
        category: "custom".to_string(),
        pattern: r"PATTERN_\d+".to_string(),
        enabled: true,
        action: "block".to_string(),
        confidence: DlpConfidence::Medium,
    }).unwrap();
    // Adding again should succeed (appended, not replaced)
    engine.add_rule(DlpRule {
        name: "dup_rule".to_string(),
        category: "custom".to_string(),
        pattern: r"OTHER_\d+".to_string(),
        enabled: true,
        action: "block".to_string(),
        confidence: DlpConfidence::Medium,
    }).unwrap();
    // Both patterns should match
    let result = engine.scan_text("PATTERN_123 and OTHER_456");
    assert!(result.matches.iter().filter(|m| m.rule_name == "dup_rule").count() >= 2);
}

#[test]
fn test_dlp_result_action_reflects_config() {
    let engine = DlpEngine::new(true, "warn");
    let result = engine.scan_text("test@example.com");
    assert_eq!(result.action, "warn");
}

#[test]
fn test_dlp_match_start_position() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("Hello test@example.com world");
    assert!(result.has_matches);
    // Email should start at position 6
    let email_match = result.matches.iter().find(|m| m.rule_name == "email");
    assert!(email_match.is_some());
    assert_eq!(email_match.unwrap().start_position, 6);
}

#[test]
fn test_ib_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("IBAN: GB29NWBK60161331926819");
    assert!(result.has_matches);
}

// === L1: confidence-aware blocking ===

#[test]
fn test_low_confidence_phone_does_not_block() {
    // The url-collector bug: a Chinese ICP filing number (11010802047360) read
    // as a phone. phone_international is Low-confidence → detect but don't block.
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("京公网安备11010802047360号");
    assert!(r.has_matches, "phone should still match");
    assert!(!r.should_block, "low-confidence phone must not block");
    assert!(r.matches.iter().all(|m| m.confidence == DlpConfidence::Low));
}

#[test]
fn test_low_confidence_ip_version_does_not_block() {
    // ip_address_public matches a software version "2.4.1.8" → Low → no block.
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("version 2.4.1.8 released");
    assert!(r.matches.iter().any(|m| m.rule_name == "ip_address_public"));
    assert!(!r.should_block);
}

#[test]
fn test_high_confidence_private_key_blocks() {
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA");
    assert!(r.should_block);
    assert!(r.matches.iter().any(|m| m.confidence == DlpConfidence::High));
}

#[test]
fn test_medium_confidence_ssn_blocks() {
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("SSN: 123-45-6789");
    assert!(r.should_block);
    assert!(r.matches.iter().any(|m| m.confidence == DlpConfidence::Medium));
}

// === L2: checksum validation (demote fakes to Low) ===

#[test]
fn test_credit_card_luhn_demotes_fake_card() {
    // 4111111111111112: 16 digits, starts with 4 (visa shape) but Luhn-invalid.
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("Card: 4111111111111112");
    let visa = r.matches.iter().find(|m| m.rule_name == "visa");
    assert!(visa.is_some(), "visa pattern should match");
    assert_eq!(visa.unwrap().confidence, DlpConfidence::Low, "Luhn-invalid → demoted");
    assert!(!r.should_block);
}

#[test]
fn test_credit_card_luhn_keeps_valid_card() {
    // 4111111111111111: Luhn-valid → stays Medium → blocks.
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("Card: 4111111111111111");
    let visa = r.matches.iter().find(|m| m.rule_name == "visa");
    assert!(visa.is_some());
    assert_ne!(visa.unwrap().confidence, DlpConfidence::Low);
    assert!(r.should_block);
}

#[test]
fn test_china_id_valid_checksum_keeps_medium() {
    // 110101199001011237: first-17-digit weighted sum % 11 = 5 → check char '7',
    // matches last digit → valid checksum → stays Medium.
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("ID: 110101199001011237");
    let cid = r.matches.iter().find(|m| m.rule_name == "china_id");
    assert!(cid.is_some());
    assert_ne!(cid.unwrap().confidence, DlpConfidence::Low);
}

#[test]
fn test_china_id_invalid_checksum_demotes_low() {
    // 110101199001011234: valid 18-digit shape, but checksum should be '7' not
    // '4' → demoted to Low (pattern matches, not a real ID).
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("ID: 110101199001011234");
    let cid = r.matches.iter().find(|m| m.rule_name == "china_id");
    assert!(cid.is_some());
    assert_eq!(cid.unwrap().confidence, DlpConfidence::Low);
}

#[test]
fn test_low_confidence_action_config_block_makes_low_block() {
    // When low_confidence_action="block", Low matches DO block (user opt-in).
    let engine = DlpEngine::with_config(DlpConfig {
        enabled: true,
        action: "block".to_string(),
        custom_rules: vec![],
        enabled_rules: vec![],
        max_content_length: 0,
        low_confidence_action: "block".to_string(),
    });
    let r = engine.scan_text("京公网安备11010802047360号");
    assert!(r.should_block, "low_confidence_action=block must block Low matches");
}

#[test]
fn test_scan_text_max_length_multibyte_no_panic() {
    // "京" is 3 bytes; max_content_length=4 lands mid-second-京 (bytes 3-5), a
    // non-char-boundary. Naive `&text[..4]` would panic; floor_char_boundary
    // floors to 3 (end of first 京), so the AKIA key (byte 9+) is truncated
    // away and not matched. Regression guard for the pre-existing slice bug.
    let config = DlpConfig {
        enabled: true,
        action: "block".to_string(),
        custom_rules: vec![],
        enabled_rules: vec![],
        max_content_length: 4,
        low_confidence_action: "log".to_string(),
    };
    let engine = DlpEngine::with_config(config);
    let result = engine.scan_text("京京京AKIAIOSFODNN7EXAMPLE"); // must not panic
    assert!(!result.has_matches, "content truncated before the key → no match");
}
