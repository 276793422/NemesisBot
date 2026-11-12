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
    engine
        .add_rule(DlpRule {
            name: "custom_secret".to_string(),
            category: "custom".to_string(),
            pattern: r"CUSTOM_SECRET_\d+".to_string(),
            enabled: true,
            action: "block".to_string(),
            confidence: DlpConfidence::Medium,
        })
        .unwrap();

    let result = engine.scan_text("Found CUSTOM_SECRET_12345 in text");
    assert!(result.has_matches);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.rule_name == "custom_secret")
    );

    assert!(engine.remove_rule("custom_secret"));
    let result = engine.scan_text("Found CUSTOM_SECRET_12345 in text");
    assert!(
        !result
            .matches
            .iter()
            .any(|m| m.rule_name == "custom_secret")
    );
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
    engine
        .add_rule(DlpRule {
            name: "disabled_test".to_string(),
            category: "custom".to_string(),
            pattern: r"DISABLED_PATTERN_\d+".to_string(),
            enabled: false,
            action: "block".to_string(),
            confidence: DlpConfidence::Medium,
        })
        .unwrap();

    let result = engine.scan_text("Found DISABLED_PATTERN_12345");
    assert!(
        !result
            .matches
            .iter()
            .any(|m| m.rule_name == "disabled_test")
    );
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
    let result = engine.scan_text("aws_access_key=AKIANCDC6VFQTFGRMBKW");
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
    let result = engine.scan_text("-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA7dGd4vWrbO8pRT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAo");
    assert!(result.has_matches);
}

#[test]
fn test_openssh_private_key_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG9wZW5zc2gtY2VydC12MDEtb3BlbnNzaC1rZX");
    assert!(result.has_matches);
}

#[test]
fn test_pkcs8_encrypted_key_detected() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIE6TAbBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDdktVWS0ceTWrbO8p");
    assert!(result.has_matches);
}

#[test]
fn test_jwt_token_detected() {
    let engine = DlpEngine::new(true, "block");
    let result =
        engine.scan_text("token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc123def456");
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
    let result =
        engine.scan_text("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.sig");
    assert!(result.has_matches);
}

#[test]
fn test_database_connection_string_detected() {
    let engine = DlpEngine::new(true, "block");
    // 2026-11-11 横扫批：教程 canonical 形态经 connection_not_placeholder
    // 降级 Low（只 log 不 block）——检测仍发生（has_matches 保持 true），
    // block 口径的豁免回归由 test_db_connection_tutorial_placeholders_log_not_block
    // 承担。
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
    let result = engine.scan_text(
        "aws_secret_access_key = ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789012",
    );
    if result.has_matches {
        // Credential matches should be Critical
        assert!(
            result
                .matches
                .iter()
                .any(|m| m.severity == DlpSeverity::Critical)
        );
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
        assert!(
            result
                .matches
                .iter()
                .all(|m| m.severity != DlpSeverity::Critical)
        );
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
    let long_text =
        "SSN: 123-45-6789 and then more padding text here that goes beyond limit".to_string();
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
    engine
        .add_rule(DlpRule {
            name: "dup_rule".to_string(),
            category: "custom".to_string(),
            pattern: r"PATTERN_\d+".to_string(),
            enabled: true,
            action: "block".to_string(),
            confidence: DlpConfidence::Medium,
        })
        .unwrap();
    // Adding again should succeed (appended, not replaced)
    engine
        .add_rule(DlpRule {
            name: "dup_rule".to_string(),
            category: "custom".to_string(),
            pattern: r"OTHER_\d+".to_string(),
            enabled: true,
            action: "block".to_string(),
            confidence: DlpConfidence::Medium,
        })
        .unwrap();
    // Both patterns should match
    let result = engine.scan_text("PATTERN_123 and OTHER_456");
    assert!(
        result
            .matches
            .iter()
            .filter(|m| m.rule_name == "dup_rule")
            .count()
            >= 2
    );
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

// ===: confidence-aware blocking ===

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
    let r = engine.scan_text("-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA7dGd4vWrbO8pRT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAo");
    assert!(r.should_block);
    assert!(
        r.matches
            .iter()
            .any(|m| m.confidence == DlpConfidence::High)
    );
}

#[test]
fn test_medium_confidence_ssn_blocks() {
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("SSN: 123-45-6789");
    assert!(r.should_block);
    assert!(
        r.matches
            .iter()
            .any(|m| m.confidence == DlpConfidence::Medium)
    );
}

// ===: checksum validation (demote fakes to Low) ===

#[test]
fn test_credit_card_luhn_demotes_fake_card() {
    // 4111111111111112: 16 digits, starts with 4 (visa shape) but Luhn-invalid.
    let engine = DlpEngine::new(true, "block");
    let r = engine.scan_text("Card: 4111111111111112");
    let visa = r.matches.iter().find(|m| m.rule_name == "visa");
    assert!(visa.is_some(), "visa pattern should match");
    assert_eq!(
        visa.unwrap().confidence,
        DlpConfidence::Low,
        "Luhn-invalid → demoted"
    );
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
    assert!(
        r.should_block,
        "low_confidence_action=block must block Low matches"
    );
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
    let result = engine.scan_text("京京京AKIANCDC6VFQTFGRMBKW"); // must not panic
    assert!(
        !result.has_matches,
        "content truncated before the key → no match"
    );
}

// ============================================================
// batch 3: dynamic-rule filter arms / redact / tool-output
// truncation / severity downgrade / defensive validator arms
// ============================================================

#[test]
fn test_scan_text_dynamic_rule_disabled_and_filtered_skipped() {
    let mut engine = DlpEngine::new(true, "block");

    // 禁用的动态规则被跳过（enabled continue 臂）
    engine
        .add_rule(DlpRule {
            name: "dyn-off".to_string(),
            category: "custom".to_string(),
            pattern: r"XYZMARKER\d+".to_string(),
            enabled: false,
            action: "log".to_string(),
            confidence: DlpConfidence::High,
        })
        .unwrap();
    let r = engine.scan_text("value XYZMARKER9 here");
    assert!(
        !r.matches.iter().any(|m| m.rule_name == "dyn-off"),
        "disabled rule must be skipped: {:?}",
        r.matches
    );

    // enabled_rules 过滤器不含该规则名 → 跳过（is_rule_enabled continue 臂）
    engine.config.enabled_rules = vec!["some-other-rule".to_string()];
    engine
        .add_rule(DlpRule {
            name: "dyn-filtered".to_string(),
            category: "custom".to_string(),
            pattern: r"XYZMARKER\d+".to_string(),
            enabled: true,
            action: "block".to_string(),
            confidence: DlpConfidence::High,
        })
        .unwrap();
    let r = engine.scan_text("value XYZMARKER9 here");
    assert!(
        !r.matches.iter().any(|m| m.rule_name == "dyn-filtered"),
        "rule not in enabled_rules must be skipped: {:?}",
        r.matches
    );

    // enabled_rules 包含该规则名 → 命中（动态循环的命中臂）
    engine.config.enabled_rules = vec!["dyn-filtered".to_string()];
    let r = engine.scan_text("value XYZMARKER9 here");
    assert!(
        r.matches.iter().any(|m| m.rule_name == "dyn-filtered"),
        "enabled dynamic rule must match: {:?}",
        r.matches
    );
}

#[test]
fn test_redact_content_disabled_returns_unchanged() {
    let engine = DlpEngine::new(false, "block");
    let text = "AKIANCDC6VFQTFGRMBKW";
    assert_eq!(engine.redact_content(text), text);
}

#[test]
fn test_redact_content_dynamic_rule_and_trailing_text() {
    let engine = DlpEngine::new(true, "block");
    engine
        .add_rule(DlpRule {
            name: "dyn-redact".to_string(),
            category: "custom".to_string(),
            pattern: r"TOPSECRETWORD".to_string(),
            enabled: true,
            action: "block".to_string(),
            confidence: DlpConfidence::High,
        })
        .unwrap();
    let out = engine.redact_content("prefix TOPSECRETWORD suffix");
    assert_eq!(out, "prefix [REDACTED] suffix", "{out}");
}

#[test]
fn test_redact_content_overlap_prefers_longer_span() {
    // 两条动态规则跨度重叠（abcd 与 cdex）→ 后开始的短跨度被跳过。
    let engine = DlpEngine::new(true, "block");
    engine
        .add_rule(DlpRule {
            name: "dyn-long".to_string(),
            category: "custom".to_string(),
            pattern: "abcd".to_string(),
            enabled: true,
            action: "log".to_string(),
            confidence: DlpConfidence::High,
        })
        .unwrap();
    engine
        .add_rule(DlpRule {
            name: "dyn-overlap".to_string(),
            category: "custom".to_string(),
            pattern: "cdex".to_string(),
            enabled: true,
            action: "log".to_string(),
            confidence: DlpConfidence::High,
        })
        .unwrap();
    let out = engine.redact_content("abcdex");
    assert_eq!(out, "[REDACTED]ex", "{out}");
}

#[test]
fn test_get_rule_names_includes_dynamic() {
    let engine = DlpEngine::new(true, "block");
    let before = engine.get_rule_names();
    engine
        .add_rule(DlpRule {
            name: "dyn-named".to_string(),
            category: "custom".to_string(),
            pattern: "zzq".to_string(),
            enabled: true,
            action: "log".to_string(),
            confidence: DlpConfidence::Medium,
        })
        .unwrap();
    let after = engine.get_rule_names();
    assert!(after.len() == before.len() + 1);
    assert!(after.contains(&"dyn-named".to_string()));
}

#[test]
fn test_scan_tool_output_truncates_beyond_5000_bytes() {
    let engine = DlpEngine::new(true, "block");
    // 命中点在 5000 字节之前 → 检出
    let near = format!("{}{}", "x".repeat(4900), "AKIANCDC6VFQTFGRMBKW");
    let r = engine.scan_tool_output("shell", &near);
    assert!(
        r.matches.iter().any(|m| m.rule_name == "aws_access_key"),
        "{:?}",
        r.matches
    );

    // 命中点在 5000 字节之后 → 被截掉，不检出
    let far = format!("{}{}", "x".repeat(5010), " AKIANCDC6VFQTFGRMBKW");
    let r2 = engine.scan_tool_output("shell", &far);
    assert!(
        !r2.matches.iter().any(|m| m.rule_name == "aws_access_key"),
        "{:?}",
        r2.matches
    );
}

#[test]
fn test_scan_tool_output_downgrades_critical_to_high() {
    let engine = DlpEngine::new(true, "block");
    // scan_text：credential 类别 → Critical
    let r = engine.scan_text("key AKIANCDC6VFQTFGRMBKW");
    assert!(
        r.matches
            .iter()
            .any(|m| m.severity == DlpSeverity::Critical && m.rule_name == "aws_access_key"),
        "{:?}",
        r.matches
    );
    // scan_tool_output：同一命中被降级为 High
    let r2 = engine.scan_tool_output("shell", "key AKIANCDC6VFQTFGRMBKW");
    assert!(
        r2.matches
            .iter()
            .any(|m| m.severity == DlpSeverity::High && m.rule_name == "aws_access_key"),
        "{:?}",
        r2.matches
    );
    assert!(
        !r2.matches
            .iter()
            .any(|m| m.severity == DlpSeverity::Critical),
        "no Critical should remain in tool output scan: {:?}",
        r2.matches
    );
}

#[test]
fn test_scan_args_extracts_array_elements() {
    let engine = DlpEngine::new(true, "block");
    let args = serde_json::json!(["AKIANCDC6VFQTFGRMBKW", {"nested": "AIzaABCDEFGHIJKLMNOPQRSTUVWXYZ012345678"}]);
    let r = engine.scan_tool_input("exec", &args);
    assert!(
        r.matches.iter().any(|m| m.rule_name == "aws_access_key"),
        "{:?}",
        r.matches
    );
    assert!(
        r.matches.iter().any(|m| m.rule_name == "google_api_key"),
        "{:?}",
        r.matches
    );
    // 纯标量走 _ 空串臂
    assert_eq!(extract_text(&serde_json::json!(42)), "");
    assert_eq!(extract_text(&serde_json::json!(true)), "");
    assert_eq!(extract_text(&serde_json::Value::Null), "");
}

#[test]
fn test_luhn_valid_defensive_arms() {
    // get(start..end) 非字符边界 → None → false
    assert!(!luhn_valid("aé中", 0, 4));
    // 数字字符 < 2 → false
    assert!(!luhn_valid("x", 0, 1));
    assert!(!luhn_valid("ab", 0, 2));
}

#[test]
fn test_china_id_valid_defensive_arms() {
    // 非字符边界 → None → false
    assert!(!china_id_valid("aé中", 0, 4));
    // 长度 != 18 → false
    assert!(!china_id_valid("12345", 0, 5));
    // 前 17 位含非数字 → to_digit None → false
    assert!(!china_id_valid("ABCDEFGHIJKLMNOPQR", 0, 18));
}

// ===== 会话凭据模板豁免（2026-11-10 B4 误拦修复）：完整合法
// `{{vault:<alias>}}` 引用不构成 DLP 命中；明文/混拼/畸形照旧。
// 均用默认 block 配置验收（Medium → block 不依赖宽松策略）。 =====

/// secret_password_assignment（Medium→block）对完整引用不命中。
#[test]
fn test_vault_template_reference_not_blocked() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("password = \"{{vault:agent/s1/pw}}\"");
    assert!(
        !result.should_block,
        "vault reference must not block: {:?}",
        result.matches
    );
    assert!(!result.has_matches);
}

/// 同位置明文秘密 → 照旧 block。
#[test]
fn test_vault_template_plaintext_still_blocks() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("password = \"hunter2secret\"");
    assert!(result.should_block, "plaintext secret must stay blocked");
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.rule_name == "secret_password_assignment"),
        "secret_password_assignment match expected: {:?}",
        result.matches
    );
}

/// 混拼尾巴（剔除模板后仍满足规则）→ 照旧 block。
#[test]
fn test_vault_template_mixed_tail_still_blocks() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("password = \"{{vault:agent/s1/pw}}plain_suffix\"");
    assert!(
        result.should_block,
        "mixed plaintext tail must stay blocked: {:?}",
        result.matches
    );
}

/// 动态规则臂同款豁免：引用形态不命中，明文命中。
#[test]
fn test_vault_template_dynamic_rule_exempt_and_block() {
    let engine = DlpEngine::new(true, "block");
    engine
        .add_rule(DlpRule {
            name: "custom_pw".to_string(),
            category: "custom".to_string(),
            pattern: r#"(?i)custom_pw\s*=\s*\S{8,}"#.to_string(),
            enabled: true,
            action: "block".to_string(),
            confidence: DlpConfidence::Medium,
        })
        .unwrap();

    // 完整引用：剔除模板后无剩余载荷 → 不命中。
    let ref_result = engine.scan_text("custom_pw = {{vault:agent/s1/pw}}");
    assert!(
        !ref_result
            .matches
            .iter()
            .any(|m| m.rule_name == "custom_pw"),
        "dynamic rule must exempt pure reference: {:?}",
        ref_result.matches
    );

    // 明文：照旧命中。
    let plain_result = engine.scan_text("custom_pw = hunter2secret_value");
    assert!(
        plain_result
            .matches
            .iter()
            .any(|m| m.rule_name == "custom_pw"),
        "dynamic rule must keep flagging plaintext"
    );
}

/// redact_content 豁免：纯模板引用原样返回（gateway 出站 DLP 的
/// 早退条件 = redact 后与原文相同——纯引用不再产生假审计）。
#[test]
fn test_redact_preserves_vault_template_reference() {
    let engine = DlpEngine::new(true, "block");
    let text = "password = \"{{vault:agent/s1/pw}}\"";
    let out = engine.redact_content(text);
    assert_eq!(out, text, "纯引用不得被打码: {out}");
}

/// 混合内容：真秘密 [REDACTED]、模板引用原样保留。
#[test]
fn test_redact_mixed_secret_and_template() {
    let engine = DlpEngine::new(true, "block");
    let text = "password = \"{{vault:agent/s1/pw}}\" pwd = hunter2secret_value";
    let out = engine.redact_content(text);
    assert!(out.contains("[REDACTED]"), "真秘密必须打码: {out}");
    assert!(
        out.contains("{{vault:agent/s1/pw}}"),
        "模板引用必须原样保留: {out}"
    );
    assert!(!out.contains("hunter2secret_value"), "明文必须消失: {out}");
}

// ===== 裸引用形态豁免（2026-11-10 裸形态批，用户拍板选 1）：裸
// `vault:<alias>` 是配置文件合法写法。DLP 独有面 = **无引号形态**——
// secret_password_assignment 的值引号可选（`['"]?`），裸引用无引号也
// 命中，模板批的豁免管不到这里。仍用默认 block 配置验收。 =====

/// secret_password_assignment 对裸引用不命中（带引号 + **无引号**两形态）。
#[test]
fn test_bare_vault_reference_not_blocked() {
    let engine = DlpEngine::new(true, "block");
    for content in [
        "password = \"vault:zope/main\"",
        "password = vault:zope/main",
        "passwd: vault:openai-main",
    ] {
        let result = engine.scan_text(content);
        assert!(
            !result.should_block,
            "bare vault reference {:?} must not block: {:?}",
            content, result.matches
        );
        assert!(!result.has_matches);
    }
}

/// 边界：剥离后非别名残余仍满足规则 → 照旧 block。
#[test]
fn test_bare_vault_reference_nonalias_residue_still_blocks() {
    let engine = DlpEngine::new(true, "block");
    let result = engine.scan_text("password = \"vault:abc$%^12345\"");
    assert!(
        result.should_block,
        "non-alias residue must stay blocked: {:?}",
        result.matches
    );
}

/// 动态规则臂裸引用同款豁免（引用剔除后无剩余载荷 → 不命中）。
#[test]
fn test_bare_vault_reference_dynamic_rule_exempt() {
    let engine = DlpEngine::new(true, "block");
    engine
        .add_rule(DlpRule {
            name: "custom_pw".to_string(),
            category: "custom".to_string(),
            pattern: r#"(?i)custom_pw\s*=\s*\S{8,}"#.to_string(),
            enabled: true,
            action: "block".to_string(),
            confidence: DlpConfidence::Medium,
        })
        .unwrap();
    let ref_result = engine.scan_text("custom_pw = vault:agent/s1/pw");
    assert!(
        !ref_result
            .matches
            .iter()
            .any(|m| m.rule_name == "custom_pw"),
        "dynamic rule must exempt bare reference: {:?}",
        ref_result.matches
    );
}

/// redact_content 裸引用豁免：原样保留；并存真秘密照旧打码。
#[test]
fn test_redact_preserves_bare_vault_reference() {
    let engine = DlpEngine::new(true, "block");
    let bare = "password = vault:zope/main";
    let out = engine.redact_content(bare);
    assert_eq!(out, bare, "裸引用不得被打码: {out}");

    let mixed = "password = vault:zope/main pwd = hunter2secret_value";
    let out_mixed = engine.redact_content(mixed);
    assert!(
        out_mixed.contains("[REDACTED]"),
        "真秘密必须打码: {out_mixed}"
    );
    assert!(
        out_mixed.contains("vault:zope/main"),
        "裸引用必须原样保留: {out_mixed}"
    );
}

// ===== keyed-assignment 值段形态闸（BUG-20261110-046）：值段无引号 =
// 代码表达式/标识符拷贝，降级 Low 不 block；引号字面量保持 Medium 照拦。
// 2026-11-10 实录：只读 Jira 脚本三处 `password = <函数调用>` 被
// Medium→block 硬拦（Layer 4 d405ff8b 已修的同类误拦在 DLP 复发，
// 因本族规则值段引号可选）。 =====

/// 代码表达式赋值不 block（降级 Low）。命中形态取自 2026-11-10 被拦
/// 内容（tools/jira/jira_client.py 实录）+ 无标点裸 RHS 标识符拷贝。
#[test]
fn test_code_expression_assignment_demoted_not_blocking() {
    let engine = DlpEngine::new(true, "block");
    for code in [
        r#"password = value.get("username")"#,
        r#"password = getpass.getpass("Jira password: ")"#,
        "username, password = read_credentials(args.credentials_stdin)",
        "pwd = home_dir",
        "access_token = session_token",
        "password: hunter2secret9",
    ] {
        let result = engine.scan_text(code);
        assert!(
            !result.should_block,
            "代码表达式不得 block: {code:?} -> {:?}",
            result.matches
        );
        assert!(
            result
                .matches
                .iter()
                .all(|m| m.confidence == DlpConfidence::Low),
            "无引号值段必须降级 Low: {code:?} -> {:?}",
            result.matches
        );
    }
}

/// 引号字面量赋值保持 Medium 照拦（单/双引号两形态；YAML 冒号形态同）。
#[test]
fn test_quoted_literal_assignment_still_blocks() {
    let engine = DlpEngine::new(true, "block");
    for line in [
        r#"password = "hunter2secret""#,
        "password = 'hunter2secret'",
        r#"password: "hunter2secret9""#,
        r#"token = "abc12345def""#,
        r#"secret_key = "deadbeefcafe0123""#,
    ] {
        let result = engine.scan_text(line);
        assert!(
            result.should_block,
            "引号字面量必须照拦: {line:?} -> {:?}",
            result.matches
        );
    }
}

/// 裸值秘密：不 block（Low）但 redact 面照旧打码——掩码能力不回退。
#[test]
fn test_bare_secret_redacted_but_not_blocking() {
    let engine = DlpEngine::new(true, "block");
    let line = "pwd = hunter2secret_value";
    let result = engine.scan_text(line);
    assert!(
        !result.should_block,
        "裸值降级 Low 不 block: {:?}",
        result.matches
    );
    let out = engine.redact_content(line);
    assert!(!out.contains("hunter2secret_value"), "裸值必须打码: {out}");
    assert!(out.contains("[REDACTED]"), "掩码占位应在场: {out}");
}

/// 全量复放锁（BUG-20261110-046）：2026-11-10 被拦 write_file 的**原始
/// 载荷逐字**（只读 Jira 客户端脚本，取自 `commit 22c94836/` 日志
/// 04.Local.md 的 args.content，JSON 转义还原；fixture 为合法 Python，
/// py_compile 验过）。合成片段锁规则行为，本锁锁真实现场：三条
/// `password = <代码表达式>` 降级 Low 不 block、唯一 `127.0.0.1`（
/// ip_address_public）照旧 Low——修复后整包不拦。同时钉伴生修复：
/// summary 按去重规则数计数（旧实现把 match 数当 rule 数，报
/// 「across 4 rule(s)」实为 2 条规则——本次定位期误导源之一）。
#[test]
fn test_bug046_real_payload_replay_not_blocking() {
    let engine = DlpEngine::new(true, "block");
    let payload = include_str!("../../tests/fixtures/2026-11-10_bug046_jira_client_payload.py");
    let result = engine.scan_text(payload);
    let brief: Vec<(String, DlpConfidence)> = result
        .matches
        .iter()
        .map(|m| (m.rule_name.clone(), m.confidence))
        .collect();
    assert!(!result.should_block, "实录载荷不得 block: {brief:?}");
    assert!(
        result
            .matches
            .iter()
            .all(|m| m.confidence == DlpConfidence::Low),
        "全部命中必须降级/保持 Low: {brief:?}"
    );
    // 4 命中位 = 3× secret_password_assignment（三处赋值表达式）+
    // 1× ip_address_public（127.0.0.1 代理默认值），与被拦日志逐字一致。
    assert_eq!(result.matches.len(), 4, "命中位漂移: {brief:?}");
    assert_eq!(
        result
            .matches
            .iter()
            .filter(|m| m.rule_name == "secret_password_assignment")
            .count(),
        3,
        "三条赋值命中缺位: {brief:?}"
    );
    // 去重后 2 条规则——summary 须报「across 2 rule(s)」。
    assert!(
        result.summary.contains("across 2 rule(s)"),
        "summary 须按去重规则计数: {}",
        result.summary
    );
}

// ===== 2026-11-11 实录同族：authorization_header scheme 词散文误拦 =====
// 与 Layer 4 bearer_token/basic_auth 的长度下限修复同源同语义：无下限时
// 「Authorization: Basic mode,」散文/文档形态整行命中 Medium→block。
// 二合一规则取宽闸 20（Basic 12-19 短样本漏检面由 Layer 4 basic_auth 覆盖）。
#[test]
fn test_authorization_header_prose_not_flagged() {
    let engine = DlpEngine::new(true, "block");
    for text in [
        "Credentials are read only from JIRA_USERNAME and JIRA_PASSWORD for Basic mode,",
        "or JIRA_TOKEN for Bearer mode.",
        "    Authorization: Basic <base64-of-username-colon-password>",
        "    Authorization: Bearer <token-from-environment>",
        "Authorization: Bearer abcdefghij123456789", // 19 字符段，低于下限
    ] {
        let result = engine.scan_text(text);
        // 断言收窄到 credential 类：下限边界样本的纯数字串可能命中
        // 无关的 phone_international 启发（Low→log，与本修复无关）。
        assert!(
            !result.matches.iter().any(|m| m.category == "credential"),
            "scheme 词散文/占位符形态不得命中 credential 规则: {text:?} -> {:?}",
            result.matches
        );
    }
}

// ===== 2026-11-11 二次横扫批：authorization_header Basic 臂结构闸 validator =====
// Layer 4 basic_auth 加 RFC 7617 解码结构闸的 DLP 对称面：散文长词（长度
// 下限挡不住 ≥20 形态）与 canonical 教程示例 demote Low 只 log；真实凭据
// 仍 block。判定单一真相源 credential/exemptions.rs 的 is_placeholder_basic_auth。
#[test]
fn test_authorization_header_basic_placeholder_log_not_block() {
    let engine = DlpEngine::new(true, "block");
    for text in [
        // 散文长词 ≥20：结构闸判散文（base64 解码失败）
        "Authorization: Basic authentication-header-format-goes-here",
        // canonical 教程示例（base64("user:mypassword")，无填充 20 类字符
        // 恰达 {20,}；canonical "user:password" 18 类字符本就不达 DLP 下限）
        "Authorization: Basic dXNlcjpteXBhc3N3b3Jk",
    ] {
        let result = engine.scan_text(text);
        assert!(
            !result
                .matches
                .iter()
                .any(|m| m.category == "credential" && m.effective_action == "block"),
            "authorization Basic 散文/canonical 形态不得 block: {text:?} -> {:?}",
            result.matches
        );
    }
    // 防修过头对照：真实凭据形态（解码含 `:` 且 pass 非规范词）仍 block。
    let real = "Authorization: Basic c3ZjX2RlcGxveTpLajhuUjJwUTl3WjU=";
    let result = engine.scan_text(real);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.category == "credential" && m.effective_action == "block"),
        "authorization Basic 真实凭据必须仍 block: {:?}",
        result.matches
    );
    // Bearer 臂恒过（validator 不降级真实 token）。
    let bearer =
        "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0In0.sig";
    let result = engine.scan_text(bearer);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.category == "credential" && m.effective_action == "block"),
        "authorization Bearer 真实凭据必须仍 block: {:?}",
        result.matches
    );
}

#[test]
fn test_authorization_header_basic_placeholder_redact_preserved() {
    // redact 面同源豁免：Basic 散文/canonical 形态不被打码；真实凭据照常。
    let engine = DlpEngine::new(true, "block");
    let placeholder = "Authorization: Basic authentication-header-format-goes-here";
    assert_eq!(
        engine.redact_content(placeholder),
        placeholder,
        "Basic 散文形态不得被打码"
    );
    let canonical = "Authorization: Basic dXNlcjpteXBhc3N3b3Jk";
    assert_eq!(
        engine.redact_content(canonical),
        canonical,
        "Basic canonical 教程形态不得被打码"
    );
    let real = "Authorization: Basic c3ZjX2RlcGxveTpLajhuUjJwUTl3WjU=";
    assert_ne!(
        engine.redact_content(real),
        real,
        "Basic 真实凭据必须被打码"
    );
}

// ===== 2026-11-11 横扫批：db 连接串教程/模板占位符 demote Low（不 block）=====

#[test]
fn test_db_connection_tutorial_placeholders_log_not_block() {
    let engine = DlpEngine::new(true, "block");
    // 占位符形态：validator 判 false → demote Low → 只 log 不 block
    //（与 Layer 4 db_connection_* 豁免同源，判定单一真相源
    // credential/exemptions.rs 的 is_placeholder_db_credential）。
    for text in [
        "postgres://user:password@localhost:5432/mydb",
        "mysql://root:password@127.0.0.1:3306/app",
        "mongodb://<user>:<password>@host:27017",
        "postgres://${DB_USER}:${DB_PASSWORD}@db:5432/app",
        "redis://:secret@redis:6379/0",
    ] {
        let result = engine.scan_text(text);
        assert!(
            !result
                .matches
                .iter()
                .any(|m| m.category == "credential" && m.effective_action == "block"),
            "db 连接串教程/模板占位符不得 block: {text:?} -> {:?}",
            result.matches
        );
    }
    // 防修过头对照：真实凭据形态必须仍 block。
    let real = "postgres://svc_deploy:Kj8nR2pQ9wZ5@db.internal.example/prod";
    let result = engine.scan_text(real);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.category == "credential" && m.effective_action == "block"),
        "db 连接串真实凭据必须仍 block: {:?}",
        result.matches
    );
}

#[test]
fn test_db_connection_placeholder_redact_preserved() {
    // redact 面同源豁免：占位符连接串不被打码（与 scan_text 豁免对称，
    // 教程/模板原文可读性不损毁）；真实凭据照常替换。
    let engine = DlpEngine::new(true, "block");
    let placeholder = "DATABASE_URL=postgres://user:password@localhost:5432/mydb";
    assert_eq!(
        engine.redact_content(placeholder),
        placeholder,
        "占位符连接串不得被打码"
    );
    let real = "DATABASE_URL=postgres://svc_deploy:Kj8nR2pQ9wZ5@db.internal.example:5432/prod";
    assert_ne!(
        engine.redact_content(real),
        real,
        "真实凭据连接串必须被打码"
    );
}

// ---- 三次横扫批回归锁（2026-11-11，与 Layer 4 credential.rs 同源判据）----
// 负样本 = demote Low 只 log 不 block；防修过头对照 = 真实形态保持原档拦截。

#[test]
fn test_third_sweep_doc_placeholder_forms_demoted() {
    let engine = DlpEngine::new(true, "block");

    // AWS 文档 canonical（AKIAIOSFODNN7EXAMPLE / EXAMPLEKEY）→ demote。
    let aws_canonical = [
        "aws_access_key=AKIAIOSFODNN7EXAMPLE",
        "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    ];
    for text in aws_canonical {
        let result = engine.scan_text(text);
        assert!(
            !result
                .matches
                .iter()
                .any(|m| m.category == "credential" && m.effective_action == "block"),
            "AWS canonical 不得 block: {text:?} -> {:?}",
            result.matches
        );
    }
    // 防修过头：非 canonical 真实形态仍 block。
    let real_aws = "aws_access_key=AKIANCDC6VFQTFGRMBKW";
    let result = engine.scan_text(real_aws);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.rule_name == "aws_access_key" && m.effective_action == "block"),
        "aws 真实形态必须仍 block: {:?}",
        result.matches
    );

    // azure_api_key / generic_base64_key 值段散文占位符 → demote。
    let prose_values = [
        "azure_api_key = YOUR_AZURE_SUBSCRIPTION_KEY_REPLACE_WITH_REAL_VALUE",
        "apikey = YOUR_API_KEY_HERE_MULTIPLIED_FOR_LENGTH_GATE",
    ];
    for text in prose_values {
        let result = engine.scan_text(text);
        assert!(
            !result
                .matches
                .iter()
                .any(|m| m.category == "credential" && m.effective_action == "block"),
            "keyed 值段散文占位符不得 block: {text:?} -> {:?}",
            result.matches
        );
    }

    // github_token 值段散文占位符 → demote；真实混合形态仍 block。
    let gh_placeholder = "ghp_YOUR_GITHUB_PAT_TOKEN_HERE_REPLACE_ME_WITH_A_REAL_ONE_X";
    let result = engine.scan_text(gh_placeholder);
    assert!(
        !result
            .matches
            .iter()
            .any(|m| m.rule_name == "github_token" && m.effective_action == "block"),
        "github token 占位符不得 block: {:?}",
        result.matches
    );
    let real_gh = "ghp_AbCdEf1234567890AbCdEf1234567890AbCdEf";
    let result = engine.scan_text(real_gh);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.rule_name == "github_token" && m.effective_action == "block"),
        "github token 真实形态必须仍 block: {:?}",
        result.matches
    );

    // authorization Bearer 臂散文占位符 → demote；真实 JWT 仍 block
    //（真实 Basic 侧由 test_authorization_header_basic_placeholder_log_not_block 钉死）。
    let bearer_prose = "Authorization: Bearer your-bearer-token-here-in-documentation";
    let result = engine.scan_text(bearer_prose);
    assert!(
        !result
            .matches
            .iter()
            .any(|m| m.category == "credential" && m.effective_action == "block"),
        "authorization Bearer 散文不得 block: {:?}",
        result.matches
    );
}

#[test]
fn test_third_sweep_quoted_tutorial_and_pem_demoted() {
    let engine = DlpEngine::new(true, "block");

    // secret_*_assignment 教程复合词引号值 → demote Low。
    let tutorial = [
        r#"password = "your-password""#,
        r#"token = "replace-with-your-token""#,
        r#"client_secret = "my-secret-here""#,
    ];
    for text in tutorial {
        let result = engine.scan_text(text);
        assert!(
            !result
                .matches
                .iter()
                .any(|m| m.category == "credential" && m.effective_action == "block"),
            "教程复合词引号值不得 block: {text:?} -> {:?}",
            result.matches
        );
    }
    // 防修过头：真实口令引号值保持 Medium block。
    let real_pw = r#"password = "Kj8nR2pQ9wZ5""#;
    let result = engine.scan_text(real_pw);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.category == "credential" && m.effective_action == "block"),
        "真实口令引号值必须仍 block: {:?}",
        result.matches
    );

    // private_key 族 PEM 格式示意 → demote；真实 64 字符体仍 block。
    let illustration = [
        "-----BEGIN RSA PRIVATE KEY-----\nXXXX\n-----END RSA PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----\n<your-key-here>",
    ];
    for text in illustration {
        let result = engine.scan_text(text);
        assert!(
            !result
                .matches
                .iter()
                .any(|m| m.category == "credential" && m.effective_action == "block"),
            "PEM 格式示意不得 block: {text:?} -> {:?}",
            result.matches
        );
    }
    let real_pem = concat!(
        "-----BEGIN RSA PRIVATE KEY-----\n",
        "MIIEpAIBAAKCAQEA7dGd4vWrbO8pRT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAo\n"
    );
    let result = engine.scan_text(real_pem);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.rule_name == "private_key_rsa" && m.effective_action == "block"),
        "真实 PEM 体必须仍 block: {:?}",
        result.matches
    );
}

// ---- 三次横扫批 redact 面对称回归锁（2026-11-11 自查补齐）----
// 六组 validator 判 false 的教程/文档/模板形态在出站打码路径（redact_content）
// 照常保留——不打码损毁可读性（与 db/authorization 钩子同型同原则）；
// 真实形态照常替换 [REDACTED]。

#[test]
fn test_third_sweep_redact_face_preserves_doc_forms() {
    let engine = DlpEngine::new(true, "block");

    // 教程/文档/模板形态：redact 后原样保留。
    let doc_forms = [
        // aws canonical
        "aws_access_key=AKIAIOSFODNN7EXAMPLE",
        "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        // github 占位符
        "ghp_YOUR_GITHUB_PAT_TOKEN_HERE_REPLACE_ME_WITH_A_REAL_ONE_X",
        // keyed 值段散文
        "azure_api_key = YOUR_AZURE_SUBSCRIPTION_KEY_REPLACE_WITH_REAL_VALUE",
        // 教程引号复合词
        r#"password = "your-password""#,
        // PEM 格式示意
        "-----BEGIN RSA PRIVATE KEY-----\nXXXX\n-----END RSA PRIVATE KEY-----",
    ];
    for text in &doc_forms {
        let redacted = engine.redact_content(text);
        assert_eq!(
            redacted, *text,
            "文档形态不得打码: {text:?} -> {redacted:?}"
        );
    }

    // 防修过头：真实形态照常替换。
    let real_forms = [
        ("aws_access_key=AKIANCDC6VFQTFGRMBKW", "[REDACTED]"),
        ("ghp_AbCdEf1234567890AbCdEf1234567890AbCdEf", "[REDACTED]"),
        (
            concat!(
                "-----BEGIN RSA PRIVATE KEY-----\n",
                "MIIEpAIBAAKCAQEA7dGd4vWrbO8pRT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAo"
            ),
            "[REDACTED]",
        ),
    ];
    for (text, _) in &real_forms {
        let redacted = engine.redact_content(text);
        assert!(
            redacted.contains("[REDACTED]") && !redacted.contains(text),
            "真实形态必须打码: {text:?} -> {redacted:?}"
        );
    }
}
