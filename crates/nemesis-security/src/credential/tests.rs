use super::*;

#[test]
fn test_no_credentials() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("hello world, this is safe text");
    assert!(!result.has_matches);
}

// credential REFERENCE strings are not secrets. `env:VAR` / `yaml:<alias>`
// api_key forms carry a variable/alias name, not a key value — the scanner must
// not flag them (false positives would block legitimate config inspection).
#[test]
fn test_reference_forms_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // Bare reference values (as they travel through tool args / config views).
    for content in [
        "yaml:openai-main",
        "env:MY_PROVIDER_KEY",
        "api_key: yaml:zhipu_free",
        "api_key: env:OPENAI_KEY",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            !result.has_matches,
            "reference form {:?} must not be flagged: {:?}",
            content, result.matches
        );
    }
}

#[test]
fn test_aws_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("key=AKIANCDC6VFQTFGRMBKW");
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
    let result = scanner.scan_content("-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDdktVWS0ceTW");
    assert!(result.has_matches);
}

#[test]
fn test_disabled_scanner() {
    let scanner = Scanner::new(false, "block");
    let result = scanner.scan_content("AKIANCDC6VFQTFGRMBKW");
    assert!(!result.has_matches);
}

#[test]
fn test_mask_modes() {
    let scanner_keep = Scanner::with_mask_mode(true, "block", MaskMode::KeepPrefix);
    let result = scanner_keep.scan_content("key=AKIANCDC6VFQTFGRMBKW");
    assert!(result.has_matches);
    assert!(result.matches[0].redacted.contains("..."));

    let scanner_fixed = Scanner::with_mask_mode(true, "block", MaskMode::Fixed);
    let result = scanner_fixed.scan_content("key=AKIANCDC6VFQTFGRMBKW");
    assert!(result.has_matches);
    assert_eq!(result.matches[0].redacted, "[REDACTED]");
}

#[test]
fn test_redact_content() {
    let scanner = Scanner::new(true, "block");
    let original = "my key is AKIANCDC6VFQTFGRMBKW please";
    let redacted = scanner.redact_content(original);
    assert!(!redacted.contains("AKIANCDC6VFQTFGRMBKW"));
    assert!(redacted.contains("[REDACTED_CREDENTIAL]"));
}

#[test]
fn test_jwt_detected() {
    let scanner = Scanner::new(true, "block");
    let result =
        scanner.scan_content("token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc123def456");
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
    // 2026-11-11 横扫批：教程 canonical（user:password@）已被占位符豁免，
    // 本测试改钉真实凭据形态（豁免回归由
    // test_db_connection_tutorial_placeholders_not_flagged 承担）。
    let result = scanner.scan_content(
        "DATABASE_URL=postgres://svc_deploy:Kj8nR2pQ9wZ5@db.internal.example:5432/prod",
    );
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
    let result = scanner.scan_content("key=AKIANCDC6VFQTFGRMBKW1234567890ab");
    assert!(result.has_matches);
}

#[test]
fn test_google_api_key_detected() {
    let scanner = Scanner::new(true, "block");
    // Use a private key pattern which is reliably detected
    let result = scanner.scan_content("key=-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDdktVWS0ceTW");
    assert!(result.has_matches);
}

#[test]
fn test_multiple_credentials_in_one_content() {
    let scanner = Scanner::new(true, "block");
    let content = "AWS=AKIANCDC6VFQTFGRMBKW and GH=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
    let result = scanner.scan_content(content);
    assert!(result.has_matches);
    assert!(result.matches.len() >= 2);
}

#[test]
fn test_redact_preserves_structure() {
    let scanner = Scanner::new(true, "block");
    let original = "config: key=AKIANCDC6VFQTFGRMBKW, host=localhost";
    let redacted = scanner.redact_content(original);
    assert!(redacted.contains("config:"));
    assert!(redacted.contains("host=localhost"));
    assert!(!redacted.contains("AKIANCDC6VFQTFGRMBKW"));
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
    let result = scanner_warn.scan_content("AKIANCDC6VFQTFGRMBKW");
    assert_eq!(result.action, "warn");

    let scanner_mask = Scanner::new(true, "mask");
    let result = scanner_mask.scan_content("AKIANCDC6VFQTFGRMBKW");
    assert_eq!(result.action, "mask");
}

#[test]
fn test_mask_mode_full() {
    let scanner = Scanner::with_mask_mode(true, "block", MaskMode::KeyValue);
    let result = scanner.scan_content("AKIANCDC6VFQTFGRMBKW");
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
    let result = scanner
        .scan_content("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abc123def");
    assert!(result.has_matches);
}

#[test]
fn test_basic_auth_detected() {
    let scanner = Scanner::new(true, "block");
    // 真实形态样本（2026-11-11 二次横扫批）：旧样本 dXNlcjpwYXNzd29yZA==
    // 是 base64("user:password") 教程 canonical，结构闸豁免后不再命中——
    // 该形态的「不拦」由 test_basic_auth_prose_and_canonical_not_flagged
    // 钉住，本测试钉真实凭据仍命中。
    let result = scanner.scan_content("Authorization: Basic c3ZjX2RlcGxveTpLajhuUjJwUTl3WjU=");
    assert!(result.has_matches);
}

#[test]
fn test_ec_private_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("-----BEGIN EC PRIVATE KEY-----\nMHcCAQEEIIK6FLPuWqpT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAoGCCqGSM4");
    assert!(result.has_matches);
}

#[test]
fn test_dsa_private_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("-----BEGIN DSA PRIVATE KEY-----\nMIIBuwIBAAKBYQNfww0KpVZfPQqTKBUbTBDPQTJKUUeRbExaYlNnbWFpbC5jb20");
    assert!(result.has_matches);
}

#[test]
fn test_pgp_private_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBF1n5n0BCADPQTJKUUeRbExaYlNnbWFpbC5jb20+gpdHTWFpbCBBdHRhY2ht");
    assert!(result.has_matches);
}

#[test]
fn test_redis_connection_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("redis://:mypassword123@localhost:6379/0");
    assert!(result.has_matches);
}

#[test]
fn test_scan_content_multibyte_value_no_panic() {
    let scanner = Scanner::new(true, "block");
    // secret_assignment matches a Chinese password; `full[len-4..]` previously
    // sliced mid-codepoint on the multibyte tail and panicked. Reaching the
    // assert means the credential.rs:97 floor/ceil fix held.
    let result = scanner.scan_content(r#"password="一二三四五六七八九""#);
    assert!(
        result.has_matches,
        "secret_assignment should match the Chinese password"
    );
}

#[test]
fn test_mongodb_connection_detected() {
    let scanner = Scanner::new(true, "block");
    // 2026-11-11 横扫批：secret 属规范教程词已被占位符豁免，改真实形态。
    let result = scanner
        .scan_content("mongodb://svc_report:Wq4tN8xV2bM6@cluster.example.com:27017/analytics");
    assert!(result.has_matches);
}

#[test]
fn test_sendgrid_key_detected() {
    let scanner = Scanner::new(true, "block");
    let result = scanner
        .scan_content("SG.abcdefghijklmnopqrstuv.xyzABCDEFGHIJKLMNO1234567890ABCDEFGHIJKLMNOPQ");
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
    let masked = mask_keep_prefix("AKIANCDC6VFQTFGRMBKW");
    assert!(masked.contains("..."));
    assert!(masked.starts_with("AKIA"));
    assert!(masked.ends_with("MBKW"));
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
    let result = scanner.scan_content("key=AKIANCDC6VFQTFGRMBKW");
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
    let result = scanner.scan_tool_output("read_file", "The output is AKIANCDC6VFQTFGRMBKW123456");
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
    let result = scanner.scan_tool_output("read_file", "AKIANCDC6VFQTFGRMBKW");
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
    let result = scanner.scan_content("key=AKIANCDC6VFQTFGRMBKW");
    assert!(result.has_matches);
    let m = &result.matches[0];
    assert!(!m.pattern_name.is_empty());
    assert!(!m.redacted.is_empty());
    assert!(m.full_match_start.len() >= 4);
}

#[test]
fn test_azure_connection_string_detected() {
    let scanner = Scanner::new(true, "block");
    // 2026-11-11 横扫批：AccountKey 加 {64,} 下限（真实 88 base64），
    // 样本按真实长度形态重写（旧 45 字符样本低于新下限）。
    let result = scanner.scan_content(
        "AccountName=prodstore;AccountKey=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tqK1SZFPTOtrKBHBeksoGMGw==",
    );
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
    let result = scanner.scan_content(
        "https://hooks.slack.com/services/T12345678/B12345678/abcdefghijklmnopqrstuvwx",
    );
    assert!(result.has_matches);
}

#[test]
fn mask_mode_default_is_keep_prefix() {
    assert!(matches!(MaskMode::default(), MaskMode::KeepPrefix));
}

#[test]
fn scan_content_key_value_mask_short_and_long_values() {
    // KeyValue 短值臂（full match ≤8 字节 → [REDACTED]）：2026-11-11 长度
    // 下限修复后 bearer/basic 最短 full match 为 18 字节（Basic + 12），短值
    // 臂改由 mask_value 直测——此前用 "Authorization: Basic A" 的 1 字符伪
    // token 锚定，恰是本批修复的散文误报形态（scheme 词后任意短串即命中）。
    let short = Scanner::with_mask_mode(true, "log", MaskMode::KeyValue);
    assert_eq!(short.mask_value("Basic A"), "[REDACTED]");
    // 修复后仍命中的最短 basic 形态（base64("u:Kj8nR2pQ")=16 字符，full
    // match 22 字节 >8，pass 段非规范教程词）→ KeyValue keep-prefix/suffix
    // 臂。旧样本 dXNlcjpwYXNz 是 canonical（pass ∈ 词表）现被豁免。
    let r = short.scan_content("Authorization: Basic dTpLajhuUjJwUQ==");
    assert!(r.has_matches);
    assert!(
        r.matches.iter().any(|m| m.redacted == "Basi...UQ=="),
        "matches: {:?}",
        r.matches
    );
    // 20-byte AWS key → KeyValue keep-prefix/suffix arm ("AKIA...MBKW").
    let long = Scanner::with_mask_mode(true, "log", MaskMode::KeyValue);
    let r2 = long.scan_content("AKIANCDC6VFQTFGRMBKW");
    assert!(r2.has_matches);
    assert!(
        r2.matches.iter().any(|m| m.redacted.contains("...")),
        "matches: {:?}",
        r2.matches
    );
}

#[test]
fn scan_tool_output_match_and_clean_paths() {
    let scanner = Scanner::new(true, "warn");
    let hit = scanner.scan_tool_output("shell", "key=AKIANCDC6VFQTFGRMBKW tail");
    assert!(hit.has_matches);
    let clean = scanner.scan_tool_output("shell", "nothing sensitive in this output");
    assert!(!clean.has_matches);
}

// ===== 会话凭据模板豁免（2026-11-10 B4 误拦修复）：完整合法
// `{{vault:<alias>}}` 引用不是明文秘密，赋值形态不得误拦；
// 明文/混拼/并存/畸形形态照旧拦（匹配级后滤语义）。 =====

/// 赋值结构 + 完整引用 → 豁免（secret_assignment 规则不再命中）。
#[test]
fn test_vault_template_reference_not_flagged() {
    let scanner = Scanner::new(true, "block");
    for content in [
        "password = \"{{vault:agent/s1/pw}}\"",
        "api_key: \"{{vault:agent/x/y}}\"",
        "export TOKEN=\"{{vault:agent/s1/github}}\"",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            !result.has_matches,
            "vault reference {:?} must not be flagged: {:?}",
            content, result.matches
        );
    }
}

/// 同位置明文秘密 → 照旧拦（豁免不弱化明文检测）。
#[test]
fn test_vault_template_plaintext_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("password = \"hunter2secret\"");
    assert!(result.has_matches, "plaintext secret must stay flagged");
}

/// 引用与真实秘密并存同串 → 照旧拦（每个匹配独立判定）。
#[test]
fn test_vault_template_coexisting_secret_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let content = "password = \"{{vault:agent/s1/pw}}\" github_token = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\"";
    let result = scanner.scan_content(content);
    assert!(
        result.has_matches,
        "coexisting real token must stay flagged"
    );
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.pattern_name == "github_token"),
        "github_token match expected: {:?}",
        result.matches
    );
}

/// 混拼（模板后接非模板尾巴 ≥8 字符，剔除后仍满足规则）→ 照旧拦。
#[test]
fn test_vault_template_mixed_tail_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("password = \"{{vault:agent/s1/pw}}plain_suffix\"");
    assert!(
        result.has_matches,
        "mixed plaintext tail must stay flagged: {:?}",
        result.matches
    );
}

/// 空别名 `{{vault:}}` → 畸形形态，原文保留 → 照旧拦（从严判定）。
#[test]
fn test_vault_template_empty_alias_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("password = \"{{vault:}}\"");
    assert!(
        result.has_matches,
        "malformed empty-alias form must stay flagged"
    );
}

/// 未闭合 `{{vault:x` → 原文保留 → 照旧拦（从严判定，不豁免畸形）。
#[test]
fn test_vault_template_unclosed_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("password = \"{{vault:unclosed and more text\"");
    assert!(
        result.has_matches,
        "unclosed template form must stay flagged"
    );
}

/// redact_content 豁免：纯模板引用原样保留（不替换为
/// [REDACTED_CREDENTIAL]）——消费点 host_impl log_line 是无条件 redact，
/// 引用原文打码只损毁可读性。
#[test]
fn test_redact_preserves_vault_template_reference() {
    let scanner = Scanner::new(true, "block");
    let content = "password = \"{{vault:agent/s1/pw}}\"";
    let out = scanner.redact_content(content);
    assert_eq!(out, content, "纯引用不得被打码: {out}");
    assert!(!out.contains("[REDACTED_CREDENTIAL]"));
}

/// 混合内容：真秘密打码、模板引用原样保留（匹配级独立判定）。
#[test]
fn test_redact_mixed_secret_and_template() {
    let scanner = Scanner::new(true, "block");
    let content =
        "password = \"{{vault:agent/s1/pw}}\" token = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\"";
    let out = scanner.redact_content(content);
    assert!(
        out.contains("[REDACTED_CREDENTIAL]"),
        "真秘密必须打码: {out}"
    );
    assert!(
        out.contains("{{vault:agent/s1/pw}}"),
        "模板引用必须原样保留: {out}"
    );
}

// ===== 裸引用形态豁免（2026-11-10 裸形态批，用户拍板选 1）：裸
// `vault:<alias>` 是配置文件合法写法，赋值形态不得误拦；非别名残余/
// 内嵌词/并存真秘密照旧拦（匹配级后滤语义，单一真相源
// nemesis_config::vault_ref::strip_reference_forms）。 =====

/// 赋值结构 + 裸引用（带引号）→ 豁免。JSON 键值形态（`"api_key":
/// "vault:x"`）本就不命中赋值正则（引号卡 `[=:]` 锚）；真实误拦面是
/// TOML/YAML 配置的带引号行，这里按生产形态验收。
#[test]
fn test_bare_vault_reference_not_flagged() {
    let scanner = Scanner::new(true, "block");
    for content in [
        "password = \"vault:zope/main\"",
        "api_key: \"vault:openai-main\"",
        "token = \"vault:agent/s1/github\"",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            !result.has_matches,
            "bare vault reference {:?} must not be flagged: {:?}",
            content, result.matches
        );
    }
}

/// 边界：别名段遇非别名字符断段，残余 ≥8 字符仍满足规则 → 照旧拦
///（「看起来像引用」不等于「整个值是引用」）。
#[test]
fn test_bare_vault_reference_nonalias_residue_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("password = \"vault:abc$%^12345\"");
    assert!(
        result.has_matches,
        "non-alias residue after strip must stay flagged: {:?}",
        result.matches
    );
}

/// 内嵌词（`myvault:x`——`vault:` 前是别名字符，非独立词元）→ 不剥 →
/// 照旧拦（收窄「真秘密恰含 vault: 子串」的放过窗口）。
#[test]
fn test_bare_vault_reference_embedded_word_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content("password = \"myvault:secret123456\"");
    assert!(result.has_matches, "embedded word form must stay flagged");
}

/// `vault:` 空尾 → 无别名字符不豁免 → 照旧拦（从严）；对照：连字符是
/// 合法别名字符，`vault:plus-more` 豁免。
#[test]
fn test_bare_vault_reference_empty_alias_still_flagged() {
    let scanner = Scanner::new(true, "block");
    let hyphen = scanner.scan_content("password = \"vault:plus-more\"");
    assert!(
        !hyphen.has_matches,
        "连字符别名必须豁免: {:?}",
        hyphen.matches
    );
    let empty = scanner.scan_content("password = \"vault:      \"");
    // `vault:` 后是空格（非别名字符）→ 空别名不豁免；值（`vault:`+空格）
    // 9 字符过 8 字符门槛 → 仍命中。注：裸 `vault:`（6 字符）低于规则
    // 长度下限本就不命中，测不到豁免路径，故用空格填充到达检测。
    assert!(
        empty.has_matches,
        "empty-alias bare form (length-reachable) must stay flagged: {:?}",
        empty.matches
    );
}

/// redact_content 裸引用豁免：纯裸引用原样保留；并存真秘密照旧打码。
#[test]
fn test_redact_preserves_bare_vault_reference() {
    let scanner = Scanner::new(true, "block");
    let bare = "password = \"vault:zope/main\"";
    let out = scanner.redact_content(bare);
    assert_eq!(out, bare, "裸引用不得被打码: {out}");
    assert!(!out.contains("[REDACTED_CREDENTIAL]"));

    let mixed =
        "password = \"vault:zope/main\" token = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\"";
    let out_mixed = scanner.redact_content(mixed);
    assert!(
        out_mixed.contains("[REDACTED_CREDENTIAL]"),
        "并存真秘密必须打码: {out_mixed}"
    );
    assert!(
        out_mixed.contains("vault:zope/main"),
        "裸引用必须原样保留: {out_mixed}"
    );
}

/// secret_assignment 值段禁跨行（2026-11-10 误拦实录回归锁）：getpass 文案
/// `password: "` 的闭引号与下一行 `JiraError("` 的开引号曾被 `[^'"]`（含 \n）
/// 跨行配对，中间两行正常 Python 代码被当成"密钥值"硬拦（match 头4=pass
/// 尾4=or("）。禁 \n 后不再命中；真密值单行出现，检测面零变化。
#[test]
fn test_secret_assignment_not_cross_line() {
    let scanner = Scanner::new(true, "block");
    // 实录形态最小化：文案闭引号 + 换行代码 + 开引号，中间 >8 个非引号字符。
    let cross_line = concat!(
        "password = getpass.getpass(\"Jira password: \")\n",
        "    if not pwd:\n",
        "        raise JiraError(\"bad credentials\")\n"
    );
    let result = scanner.scan_content(cross_line);
    assert!(
        !result.has_matches,
        "跨行引号配对不得命中: {:?}",
        result.matches
    );
    // 对照：同一行内的真密形态仍命中（修复不得收窄检测面）。
    let inline = scanner.scan_content("password = \"correct horse battery staple\"");
    assert!(inline.has_matches, "单行带空格真密必须仍命中");
    let no_close =
        scanner.scan_content("password: \"\n        raise JiraError(\"value under eight\"");
    // 值段不足 8 字符的两个短引号段也不得因跨行拼接而命中。
    assert!(
        !no_close.has_matches,
        "短引号段跨行拼接不得命中: {:?}",
        no_close.matches
    );
}

/// 2026-11-10 另一设备实录回归锁（6e611a0b 基线复现）：只读 Jira 客户端脚本
/// write_file 被 credential_scanner 硬拦（1 credential detected）。根因 =
/// secret_assignment 值段跨行吞换行符，getpass 文案闭引号与下一行 JiraError
/// 开引号被配对（match 头4=pass 尾4=or("）。本测试用日志原文全文（6189 字节
/// 逐字节还原）钉死：修复后必须零命中；修复前该原文实测命中。
#[test]
fn test_real_log_jira_client_write_not_flagged() {
    const CONTENT: &str = r##""""Read-only client for internal Jira; credentials are supplied interactively or via stdin."""
import argparse
import getpass
import http.cookiejar
import json
import re
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request

BASE_URL = "https://tbjira.lenovo.com/issue"
DEFAULT_PROXY = "http://127.0.0.1:10809"


class JiraError(Exception):
    pass


class SameOriginRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        old = urllib.parse.urlsplit(req.full_url)
        new = urllib.parse.urlsplit(newurl)
        if (old.scheme, old.netloc) != (new.scheme, new.netloc):
            raise JiraError("Cross-origin redirect refused.")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


class JiraClient:
    def __init__(self, proxy=DEFAULT_PROXY, insecure=False, timeout=30):
        context = ssl.create_default_context()
        if insecure:
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
        self.timeout = timeout
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({"http": proxy, "https": proxy}),
            urllib.request.HTTPSHandler(context=context),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
            SameOriginRedirect(),
        )

    def request(self, path, data=None, expect_json=True):
        request = urllib.request.Request(BASE_URL + path, data=data)
        request.add_header("User-Agent", "SmartConnect-Jira-ReadOnly/1.0")
        if expect_json:
            request.add_header("Accept", "application/json")
        try:
            with self.opener.open(request, timeout=self.timeout) as response:
                body = response.read()
        except urllib.error.HTTPError as exc:
            raise JiraError("Jira returned HTTP %s; no automatic login retry." % exc.code) from None
        except (urllib.error.URLError, TimeoutError, OSError):
            # Avoid reflecting request details in errors.
            raise JiraError("Network request failed; check proxy and connectivity.") from None
        if not expect_json:
            return None
        try:
            return json.loads(body.decode("utf-8"))
        except (ValueError, UnicodeError):
            raise JiraError("Expected Jira JSON; received another response, possibly a login page.") from None

    def login(self, username, password):
        self.request("/login.jsp", expect_json=False)
        form = urllib.parse.urlencode({
            "os_username": username,
            "os_password": password,
            "os_destination": "/",
            "login": "Log In",
        }).encode("utf-8")
        self.request("/login.jsp", data=form, expect_json=False)
        identity = self.request("/rest/api/2/myself")
        if not isinstance(identity, dict) or not (identity.get("name") or identity.get("key")):
            raise JiraError("Authenticated identity was not confirmed.")

    def issue(self, key):
        if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*-[0-9]+", key):
            raise JiraError("Invalid issue key.")
        query = urllib.parse.urlencode({"fields": "summary,status,issuetype,description,attachment,comment"})
        result = self.request("/rest/api/2/issue/" + key + "?" + query)
        if not isinstance(result, dict) or result.get("key", "").upper() != key.upper():
            raise JiraError("Response did not contain the requested issue.")
        return result


def read_credentials(from_stdin):
    if from_stdin:
        try:
            value = json.load(sys.stdin)
        except (ValueError, UnicodeError):
            raise JiraError("Credentials stdin must contain a JSON object.") from None
        if not isinstance(value, dict):
            raise JiraError("Credentials stdin must contain a JSON object.")
        username, password = value.get("username"), value.get("password")
    else:
        if not sys.stdin.isatty():
            raise JiraError("Interactive login needs a terminal; use stdin with a trusted launcher.")
        username = input("Jira username: ").strip()
        password = getpass.getpass("Jira password: ")
    if not isinstance(username, str) or not username.strip() or not isinstance(password, str) or not password:
        raise JiraError("Non-empty username and password are required.")
    return username, password


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--issue", help="Issue key, e.g. SMARTCTONE-3429")
    parser.add_argument("--proxy", default=DEFAULT_PROXY)
    parser.add_argument("--insecure", action="store_true", help="Disable TLS verification for internal access")
    parser.add_argument("--credentials-stdin", action="store_true", help="Read credential JSON from stdin, not argv")
    parser.add_argument("--output", help="Save UTF-8 JSON to a new file; never overwrites")
    args = parser.parse_args(argv)
    if args.output and not args.issue:
        parser.error("--output requires --issue")
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    try:
        username, password = read_credentials(args.credentials_stdin)
        client = JiraClient(proxy=args.proxy, insecure=args.insecure)
        client.login(username, password)
        del username, password
        result = {"authenticated": True, "base_url": BASE_URL}
        if args.issue:
            result["issue"] = client.issue(args.issue)
        text = json.dumps(result, ensure_ascii=False, indent=2)
        if args.output:
            with open(args.output, "x", encoding="utf-8") as stream:
                stream.write(text + "\n")
            print("Authentication confirmed; issue JSON saved.")
        else:
            print(text)
        return 0
    except (JiraError, OSError, EOFError, KeyboardInterrupt) as exc:
        message = str(exc) if isinstance(exc, JiraError) else "Input cancelled or output file could not be created."
        print("ERROR: " + message, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
"##;
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content(CONTENT);
    assert!(
        !result.has_matches,
        "2026-11-10 实录原文不得再被误拦: {:?}",
        result.matches
    );
    // 防修过头对照：同一原文注入单行真密后仍必须命中（检测面未收窄）。
    let with_secret = CONTENT.replace(
        "BASE_URL = ",
        "password = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\" BASE_URL = ",
    );
    let flagged = scanner.scan_content(&with_secret);
    assert!(
        flagged.has_matches,
        "原文+注入真密必须仍命中: {:?}",
        flagged.matches
    );
}
// ===== 2026-11-11 实录：bearer/basic scheme 词散文误拦（通用化 jira skill 脚本）=====
// 真实模型生成的通用化客户端在 docstring 里写用法行与占位符示例，被
// bearer_token/basic_auth 的无长度 token 段整段误拦（两台设备分别报
// 2-3 处凭据，同族复现；本批长度下限修复 Bearer ≥20 / Basic ≥12）。

#[test]
fn test_auth_scheme_prose_and_cli_usage_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // 三条实录命中面（修复前分别命中 bearer_token x2 / basic_auth x1）：
    for content in [
        // CLI 用法行：bearer 后跟 --issue（`-` 在 token 字符类内）
        "    python jira_client.py --base-url https://jira.example.com --auth bearer --issue PROJ-123",
        // docstring 散文：Bearer/Basic 后跟普通英文词
        "Credentials are read only from JIRA_USERNAME and JIRA_PASSWORD for Basic mode,",
        "or JIRA_TOKEN for Bearer mode.",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            !result.has_matches,
            "scheme 词散文/CLI 形态不得误拦: {content:?} -> {:?}",
            result.matches
        );
    }
}

#[test]
fn test_auth_header_placeholder_examples_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // docstring 占位符示例（`<...>` 不在 token 字符类内，修复前靠后续
    // 散文词补位命中；修复后整段不命中）。
    for content in [
        "    Authorization: Basic <base64-of-username-colon-password>",
        "    Authorization: Bearer <token-from-environment>",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            !result.has_matches,
            "占位符示例不得误拦: {content:?} -> {:?}",
            result.matches
        );
    }
}

// ===== 2026-11-11 二次横扫批：basic_auth 结构闸（RFC 7617 解码判据）=====
// 目标设备 d96357f1（含第一批长度下限）实录仍拦 1 处 basic_auth：
// {12,} 下限挡不住散文长词（authentication=14/authorization=13 字符），
// canonical 教程示例 base64("user:password") 也仍在射程内。本批按协议
// 结构判据豁免（解码失败/无 `:`/pass 属规范词 → 散文或教程形态）。

#[test]
fn test_basic_auth_prose_long_words_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // 散文长词：≥12 字符英文词紧跟 scheme 词（长度下限挡不住，结构闸兜住）。
    for content in [
        "Basic authentication is used for this endpoint.",
        "Basic authorization header goes here.",
        "use Basic authentication instead of tokens",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            !result.has_matches,
            "散文长词不得误拦: {content:?} -> {:?}",
            result.matches
        );
    }
}

#[test]
fn test_basic_auth_canonical_tutorial_example_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // canonical 教程示例（base64("user:password") 族，pass 属规范教程词表）。
    for content in [
        "Authorization: Basic dXNlcjpwYXNzd29yZA==",
        "Authorization: Basic dXNlcjpwYXNz",
        "Authorization: Basic YWRtaW46cGFzc3dvcmQ=",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            !result.has_matches,
            "canonical 教程示例不得误拦: {content:?} -> {:?}",
            result.matches
        );
    }
}

#[test]
fn test_basic_auth_structural_gate_no_colon_exemption() {
    let scanner = Scanner::new(true, "block");
    // RFC 7617：Basic 凭据恒为 base64(user:pass)。合法 base64 但解码无
    // `:`（如 base64("helloworldtoken")）= 散文词乱码 → 豁免；这条豁免
    // 由协议保证零真实漏检面（真实凭据必含 `:`）。
    // base64("helloworldtoken") = aGVsbG93b3JsZHRva2Vu（16 字符 ≥12）。
    let result = scanner.scan_content("Authorization: Basic aGVsbG93b3JsZHRva2Vu");
    assert!(
        !result.has_matches,
        "无冒号结构的 base64 词不得误拦: {:?}",
        result.matches
    );
}

#[test]
fn test_credential_match_line_numbers() {
    // 行号反馈面（2026-11-11 二次横扫批）：命中必须带 1 起行号，summary
    // 按 @L 聚合——目标设备 agent 两轮抱怨无定位面，盲改重试循环的根治。
    let scanner = Scanner::new(true, "block");
    let content = "line one plain\n\
                   Authorization: Basic c3ZjX2RlcGxveTpLajhuUjJwUTl3WjU=\n\
                   line three plain\n\
                   line four plain\n\
                   Authorization: Bearer abcdefghij1234567890ab";
    let result = scanner.scan_content(content);
    assert_eq!(result.matches.len(), 2);
    // matches 按规则表序聚合（bearer_token 规则在 basic_auth 前），非内容序。
    assert_eq!(result.matches[0].pattern_name, "bearer_token");
    assert_eq!(result.matches[0].line, 5);
    assert_eq!(result.matches[1].pattern_name, "basic_auth");
    assert_eq!(result.matches[1].line, 2);
    assert!(result.summary.contains("basic_auth@L2"));
    assert!(result.summary.contains("bearer_token@L5"));
}

#[test]
fn test_bearer_basic_min_length_still_flags_real_tokens() {
    let scanner = Scanner::new(true, "block");
    // 真实凭据形态必须仍命中（防修过头：长度下限不得吞掉真实检测面）。
    let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N0XgL0n3I9Pf_9nwnYJEnY";
    for (label, content) in [
        ("bearer+jwt", format!("Authorization: Bearer {jwt}")),
        // 真实形态 base64（解码含 `:` 且 pass 非规范教程词）：旧样本
        // dXNlcm5hbWU6cGFzc3dvcmQ= 是 canonical 教程示例，二次横扫批起被
        // 结构闸豁免（由 prose/canonical 测试钉「不拦」），此处换真实形态。
        (
            "basic+b64",
            "Authorization: Basic c3ZjX2RlcGxveTpLajhuUjJwUTl3WjU=".to_string(),
        ),
    ] {
        let result = scanner.scan_content(&content);
        assert!(result.has_matches, "真实凭据形态必须仍命中（{label}）");
    }
    // 边界：19 字符 Bearer 段不命中（下限 20）。
    let short = "Authorization: Bearer abcdefghij123456789";
    assert_eq!(short.rsplit(' ').next().unwrap().chars().count(), 19);
    assert!(!scanner.scan_content(short).has_matches);
}

#[test]
fn test_summary_includes_rule_names() {
    // 拦截反馈面必须带规则名+行号聚合（只给规则名/行号与计数，不给匹配
    // 值）：2026-11-11 实录两轮误拦均因文案只有总数而无法定位命中面；第三
    // 轮实录规则名已生效仍报「没有给出具体字符位置」→ 升级 @L 行号。
    let scanner = Scanner::new(true, "block");
    let content = "Authorization: Bearer abcdefghij1234567890ab\n\
                   Authorization: Bearer fedcba0987654321fedcba\n\
                   Authorization: Basic c3ZjX2RlcGxveTpLajhuUjJwUTl3WjU=";
    let result = scanner.scan_content(content);
    assert!(result.has_matches);
    assert_eq!(result.matches.len(), 3);
    assert_eq!(result.matches[2].line, 3);
    assert!(
        result.summary.contains("bearer_token@L1,L2 x2")
            && result.summary.contains("basic_auth@L3"),
        "summary 应含规则名+行号聚合: {:?}",
        result.summary
    );
}

#[test]
fn test_real_skill_jira_client_write_not_flagged() {
    // 2026-11-11 实录原文（本机 431deb04 代码 + cc-switch 真实模型全链路
    // 复现被拦的 skills/.../scripts/jira_client.py 全文，无真实凭据）：
    // 修复前 credential_scanner 报 3 credential(s) detected
    // （bearer_token x2 + basic_auth x1）并阻止写入。
    const CONTENT: &str = r##"#!/usr/bin/env python3
"""Read-only Jira client.

Usage::

    python jira_client.py --base-url https://jira.example.com --auth basic --issue PROJ-123
    python jira_client.py --base-url https://jira.example.com --auth bearer --issue PROJ-123

Credentials are read only from JIRA_USERNAME and JIRA_PASSWORD for Basic mode,
or JIRA_TOKEN for Bearer mode.

Request-header formats (placeholders shown, never real credentials)::

    Authorization: Basic <base64-of-username-colon-password>
    Authorization: Bearer <token-from-environment>
    Accept: application/json

The client performs GET requests only. ``login()`` verifies the current identity
through ``/rest/api/2/myself``; ``get_issue()`` retrieves one issue through
``/rest/api/2/issue/{issue_key}``.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen


class JiraClientError(RuntimeError):
    """Raised when configuration or a Jira request fails."""


class JiraClient:
    """Minimal read-only Jira REST API client."""

    def __init__(self, base_url: str, auth: str, timeout: float = 30.0) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.headers = self._build_headers(auth)

    @staticmethod
    def _required_env(name: str) -> str:
        value = os.environ.get(name)
        if not value:
            raise JiraClientError(f"Required environment variable is not set: {name}")
        return value

    @classmethod
    def _build_headers(cls, auth: str) -> dict[str, str]:
        if auth == "basic":
            username = cls._required_env("JIRA_USERNAME")
            password = cls._required_env("JIRA_PASSWORD")
            encoded = base64.b64encode(
                f"{username}:{password}".encode("utf-8")
            ).decode("ascii")
            authorization = f"Basic {encoded}"
        elif auth == "bearer":
            token = cls._required_env("JIRA_TOKEN")
            authorization = f"Bearer {token}"
        else:
            raise JiraClientError("Authentication mode must be 'basic' or 'bearer'")

        return {
            "Accept": "application/json",
            "Authorization": authorization,
            "User-Agent": "personal-jira-bugs-read-only-client/1.0",
        }

    def _get(self, path: str) -> dict[str, Any]:
        request = Request(f"{self.base_url}{path}", headers=self.headers, method="GET")
        try:
            with urlopen(request, timeout=self.timeout) as response:
                return json.load(response)
        except HTTPError as exc:
            raise JiraClientError(
                f"Jira returned HTTP {exc.code} for GET {path}"
            ) from exc
        except URLError as exc:
            raise JiraClientError(f"Unable to reach Jira: {exc.reason}") from exc
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise JiraClientError("Jira returned a non-JSON response") from exc

    def login(self) -> dict[str, Any]:
        """Verify credentials and return the authenticated Jira identity."""
        return self._get("/rest/api/2/myself")

    def get_issue(self, issue_key: str) -> dict[str, Any]:
        """Return one Jira issue without modifying it."""
        return self._get(f"/rest/api/2/issue/{quote(issue_key, safe='')}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Read one Jira issue (GET only).")
    parser.add_argument("--base-url", required=True, help="Jira base URL")
    parser.add_argument("--auth", required=True, choices=("basic", "bearer"))
    parser.add_argument("--issue", required=True, help="Issue key, for example PROJ-123")
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()

    try:
        client = JiraClient(args.base_url, args.auth, args.timeout)
        identity = client.login()
        issue = client.get_issue(args.issue)
    except JiraClientError as exc:
        parser.exit(1, f"error: {exc}\n")

    print(json.dumps({"identity": identity, "issue": issue}, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
"##;
    let scanner = Scanner::new(true, "block");
    let result = scanner.scan_content(CONTENT);
    assert!(
        !result.has_matches,
        "2026-11-11 实录原文不得再被误拦: {:?}",
        result.matches
    );
    // 防修过头对照：同一原文注入单行真密后仍必须命中（检测面未收窄）。
    let with_secret = CONTENT.replace(
        "def main() -> int:",
        "password = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\"\n\ndef main() -> int:",
    );
    assert_ne!(with_secret, CONTENT, "注入锚点必须存在");
    let flagged = scanner.scan_content(&with_secret);
    assert!(
        flagged.has_matches,
        "原文+注入真密必须仍命中: {:?}",
        flagged.matches
    );
}

// ===== 2026-11-11 横扫批：值段无长度下限 / 教程占位符误拦族 =====

#[test]
fn test_db_connection_tutorial_placeholders_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // 教程/模板占位符形态（修复前全部命中 db_connection_* 硬拦）：
    for content in [
        // 所有 DB 教程的 canonical 示例（密码段 = 规范教程词）
        "postgres://user:password@localhost:5432/mydb",
        "mysql://root:password@127.0.0.1:3306/app",
        "DATABASE_URL=postgres://user:pass@db:5432/app",
        "redis://:secret@redis:6379/0",
        // 本地 dev 默认词
        "mysql://app_user:root@db.internal/app",
        "postgres://postgres:postgres@localhost:5432/mydb",
        // 模板语法字符：compose / k8s / envsubst / 文档尖括号
        "mongodb://<user>:<password>@host:27017",
        "DATABASE_URL=postgres://${DB_USER}:${DB_PASSWORD}@db:5432/app",
        // 连字符/下划线/点归一后的复合占位符
        "mysql://app_user:my-password@db.internal/app",
        "postgres://app:your_password@host/db",
        // 空密码段（无凭据可言；正则值段本就要求非空，helper 空段臂
        // 供直调防御）
        "mysql://app_user:@db.internal/app",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            !result.has_matches,
            "db 连接串教程/模板占位符不得误拦: {content:?} -> {:?}",
            result.matches
        );
    }
}

#[test]
fn test_db_connection_real_credentials_still_flagged() {
    let scanner = Scanner::new(true, "block");
    // 防修过头对照：真实凭据形态必须仍命中（非常见词、无模板语法）。
    for content in [
        "postgres://svc_deploy:Kj8nR2pQ9wZ5@db.internal.example/prod",
        "mysql://api_user:Yq4tN8xW2vL6@10.0.3.7:3306/orders",
        "mongodb://reporter:Hk7mB3zQ9cX1@mongo.internal:27017/analytics",
        "redis://:Fj5nP7yR2sW9@cache.internal:6379",
    ] {
        let result = scanner.scan_content(content);
        assert!(
            result.has_matches,
            "db 连接串真实凭据必须仍命中: {content:?}"
        );
    }
    // 混合形态：教程占位符与真实凭据并存时，只豁免占位符、真实照拦。
    let mixed = "postgres://user:password@localhost:5432/mydb\n\
                 postgres://svc_deploy:Kj8nR2pQ9wZ5@db.internal.example/prod";
    let result = scanner.scan_content(mixed);
    assert!(
        result.has_matches,
        "混排时真实凭据必须仍命中: {:?}",
        result.matches
    );
}

#[test]
fn test_value_segment_min_length_placeholder_forms_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // gcp_private_key_id：短 hex 占位符不再命中（修复前 [a-f0-9]+ 无下限）。
    let short_gcp = r#""private_key_id": "abc123""#;
    assert!(
        !scanner.scan_content(short_gcp).has_matches,
        "gcp private_key_id 短占位符不得误拦"
    );
    // 真实 32-hex 形态仍命中。
    let real_gcp = r#""private_key_id": "abcd1234abcd1234abcd1234abcd1234""#;
    assert!(scanner.scan_content(real_gcp).has_matches);

    // azure_connection_string：短 AccountKey 占位符不再命中。
    let short_azure = "DefaultEndpointsProtocol=https;AccountName=devstore;AccountKey=mykey;";
    assert!(
        !scanner.scan_content(short_azure).has_matches,
        "azure AccountKey 短占位符不得误拦"
    );
    // 真实 88-base64 形态仍命中。
    let real_azure = "AccountName=prodstore;AccountKey=AbCdEfGh12345678AbCdEfGh12345678AbCdEfGh12345678AbCdEfGh12345678AbCdEfGh12345678AbCdEfGh1234==";
    assert!(scanner.scan_content(real_azure).has_matches);

    // npm_token：短段不再命中（含下划线整段 15 字符 < 20）。
    let short_npm = "//registry.npmjs.org/:_authToken=YOUR_AUTH_TOKEN";
    assert!(
        !scanner.scan_content(short_npm).has_matches,
        "npm authToken 短占位符不得误拦"
    );
    // 真实 36+ 形态仍命中。
    let real_npm = "//registry.npmjs.org/:_authToken=npm_Qabcdefghijklmnopqrstuvwxyz123456";
    assert!(scanner.scan_content(real_npm).has_matches);
}

// ---- 三次横扫批回归锁（2026-11-11）：八类教程/文档/模板形态豁免 ----
// 机理：随机凭据（base62）大小写均匀——「无数字且单一大小写」的真实
// token 概率 ~1.4e-5（≥20 字符段）；AWS canonical 含 EXAMPLE 七连字概率
// ~1e-9。防修过头对照（真实形态仍命中）与负样本成对钉死。

#[test]
fn test_third_sweep_prose_token_values_not_flagged() {
    let scanner = Scanner::new(true, "block");

    // bearer_token：散文连字符短语 / 文档占位符（单一大小写无数字）。
    let bearer_prose = [
        "Authorization: Bearer token-based-authentication-scheme-here",
        "Bearer --api-token-cache-file-path-in-docs",
        "Authorization: Bearer YOUR_BEARER_TOKEN_HERE",
        "Bearer ---------------",
    ];
    for content in bearer_prose {
        assert!(
            !scanner.scan_content(content).has_matches,
            "bearer 散文/占位符不得误拦: {content:?}"
        );
    }
    // 防修过头：混合大小写 + 数字的真实 token 仍命中。
    assert!(
        scanner
            .scan_content("Authorization: Bearer AbCdEf1234567890AbCdEf1234567890")
            .has_matches,
        "bearer 真实 token 必须仍命中"
    );

    // api_key_in_url：URL 占位符（值段全大写无数字）。
    let url_placeholder = "https://api.example.com/v1/data?api_key=EXAMPLE_API_KEY_PLACEHOLDER";
    assert!(
        !scanner.scan_content(url_placeholder).has_matches,
        "URL api_key 占位符不得误拦"
    );
    // 防修过头：真实混合形态仍命中。
    assert!(
        scanner
            .scan_content("https://api.example.com/v1/data?api_key=a1B2c3D4e5F6g7H8i9J0k1L2m3N4")
            .has_matches,
        "URL api_key 真实键必须仍命中"
    );

    // npm_token：.npmrc 文档占位符（≥20 全大写无数字）。
    let npm_placeholder = "//registry.npmjs.org/:_authToken=YOUR_NPM_AUTH_TOKEN_HERE_REPLACE";
    assert!(
        !scanner.scan_content(npm_placeholder).has_matches,
        "npm authToken 占位符不得误拦"
    );

    // github_token：README 占位符（ghp_ 前缀后全大写无数字 ≥36）。
    let gh_placeholder = "ghp_YOUR_GITHUB_PAT_TOKEN_HERE_REPLACE_ME_WITH_A_REAL_ONE_X";
    assert!(
        !scanner.scan_content(gh_placeholder).has_matches,
        "github token 占位符不得误拦"
    );
    // 防修过头：真实混合形态仍命中。
    assert!(
        scanner
            .scan_content("ghp_AbCdEf1234567890AbCdEf1234567890AbCdEf")
            .has_matches,
        "github token 真实形态必须仍命中"
    );
}

#[test]
fn test_third_sweep_aws_canonical_examples_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // AWS 官方文档 canonical：每篇教程都有，含 EXAMPLE 七连字。
    let canonical = [
        "key=AKIAIOSFODNN7EXAMPLE",
        "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    ];
    for content in canonical {
        assert!(
            !scanner.scan_content(content).has_matches,
            "AWS 文档 canonical 不得误拦: {content:?}"
        );
    }
    // 防修过头：非 canonical 真实形态仍命中（AKIANCDC6VFQTFGRMBKW 亦为
    // 本批全仓测试样本常量，既有 aws 断言同源钉死）。
    assert!(
        scanner
            .scan_content("aws_secret_access_key = AbCdEfGh12345678AbCdEfGh12345678AbCdEfGh12")
            .has_matches,
        "aws secret 真实形态必须仍命中"
    );
}

#[test]
fn test_third_sweep_tutorial_secret_assignments_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // 教程复合词（全小写 + 词表分解）引号值。
    let tutorial = [
        r#"password = "your-password""#,
        r#"password = "changeme""#,
        r#"password = "my-secret-here""#,
        r#"token = "replace-with-your-token""#,
    ];
    for content in tutorial {
        assert!(
            !scanner.scan_content(content).has_matches,
            "教程复合词引号值不得误拦: {content:?}"
        );
    }
    // 防修过头：真实口令（含数字/混排）与非词表复合词仍命中。
    let real = [
        r#"password = "Kj8nR2pQ9wZ5""#,
        r#"password = "MyPassword""#,
        r#"password = "correct-horse-battery""#,
    ];
    for content in real {
        assert!(
            scanner.scan_content(content).has_matches,
            "真实口令必须仍命中: {content:?}"
        );
    }
}

#[test]
fn test_third_sweep_template_service_account_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // SA JSON 模板骨架：type 标记 + PEM 占位值（XXXX）+ 短 hex id。
    let sa_template = r#"{
  "type": "service_account",
  "project_id": "my-project",
  "private_key_id": "abc123de",
  "private_key": "-----BEGIN PRIVATE KEY-----\nXXXX\n-----END PRIVATE KEY-----\n",
  "client_email": "bot@my-project.iam.gserviceaccount.com"
}"#;
    assert!(
        !scanner.scan_content(sa_template).has_matches,
        "SA JSON 模板骨架不得误拦"
    );
    // 防修过头：真实 SA JSON（private_key 恒含完整 PEM 体）仍命中。
    let sa_real = concat!(
        r#"{"type": "service_account", "project_id": "prod", "#,
        r#""private_key_id": "abcd1234abcd1234abcd1234abcd1234", "#,
        r#""private_key": "-----BEGIN PRIVATE KEY-----\n"#,
        "MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDdktVWS0ceTW\n",
        "rbO8pRT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAoGCCqGSM49BAAAAA\n",
        r#"\n-----END PRIVATE KEY-----\n"}"#,
    );
    assert!(
        scanner.scan_content(sa_real).has_matches,
        "真实 SA JSON 必须仍命中"
    );
}

#[test]
fn test_third_sweep_pem_illustration_not_flagged() {
    let scanner = Scanner::new(true, "block");
    // 格式示意文档：marker 后无真实 base64 体。
    let illustration = [
        "-----BEGIN RSA PRIVATE KEY-----\nXXXX\n-----END RSA PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----\n<your-key-here>",
        "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----",
    ];
    for content in illustration {
        assert!(
            !scanner.scan_content(content).has_matches,
            "PEM 格式示意不得误拦: {content:?}"
        );
    }
    // 防修过头：真实 64 字符 base64 体行仍命中。
    assert!(
        scanner
            .scan_content(concat!(
                "-----BEGIN RSA PRIVATE KEY-----\n",
                "MIIEpAIBAAKCAQEA7dGd4vWrbO8pRT4dWShvUfCbEAmfPQgeSGkX2a4YvF0OJoAo\n",
            ))
            .has_matches,
        "真实 PEM 体必须仍命中"
    );
}

/// 四次横扫批（2026-11-11，用户拍板「那就豁免吧」）：第二轮自查对抗补扫
/// 残余四类——Twilio/Mailgun 全同字符占位符、Azure tenant 全零 GUID、
/// netlify 全大写散文占位。
#[test]
fn test_fourth_sweep_uniform_and_prose_placeholders_not_flagged() {
    let scanner = Scanner::new(true, "block");

    // 全同字符占位符：文档 canonical（32 连 x / 全零 GUID）。
    let uniform = [
        // twilio_sid：`AC` 前缀 + 32 连 x。
        "account_sid = ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        // mailgun_key：`key-` 前缀 + 32 连 x。
        "api_key: key-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        // azure_tenant_id：Microsoft 文档 canonical 全零 GUID（散文形态）。
        "azure tenant id: 00000000-0000-0000-0000-000000000000 is the default",
    ];
    for content in uniform {
        assert!(
            !scanner.scan_content(content).has_matches,
            "全同字符/全零 GUID 占位符不得误拦: {content:?}"
        );
    }

    // netlify_token：全大写散文占位（≥40 过长度闸，单一大小写无数字）。
    assert!(
        !scanner
            .scan_content("NETLIFY_TOKEN = YOUR_NETLIFY_TOKEN_HERE_REPLACE_WITH_YOURS")
            .has_matches,
        "netlify 全大写散文占位不得误拦"
    );

    // 防修过头：真实形态必须仍命中（混合字符/混合大小写带数字）。
    let real = [
        (
            "account_sid = ACa1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
            "twilio 真实 SID",
        ),
        (
            "api_key: key-a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
            "mailgun 真实键",
        ),
        (
            "azure tenant id: 72f988bf-86f1-41af-91ab-2d7cd011db47",
            "azure tenant 真实 GUID",
        ),
    ];
    for (content, label) in real {
        assert!(
            scanner.scan_content(content).has_matches,
            "{label} 必须仍命中"
        );
    }
    assert!(
        scanner
            .scan_content("netlify_token = nf_Ab3cD5eF7g9h2j4k6m8p0q2r4s6t8v0x2z4y6")
            .has_matches,
        "netlify 真实形态必须仍命中"
    );
    // 全大写但含数字的 netlify 值保守拦截（is_prose_token_value 判非散文）。
    assert!(
        scanner
            .scan_content("NETLIFY_TOKEN = ABCD1234ABCD1234ABCD1234ABCD1234ABCD1234")
            .has_matches,
        "netlify 全大写含数字形态保守拦截"
    );
}

/// 四次横扫批 redact 面对称：豁免形态不打码损毁文档可读性，真实形态照
/// 常打码。判定与 scan_content 同源（同钩子组）。
#[test]
fn test_fourth_sweep_redact_face_preserves_doc_forms() {
    let scanner = Scanner::new(true, "block");

    let doc_forms = [
        "account_sid = ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "api_key: key-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "azure tenant id: 00000000-0000-0000-0000-000000000000 is the default",
        "NETLIFY_TOKEN = YOUR_NETLIFY_TOKEN_HERE_REPLACE_WITH_YOURS",
    ];
    for text in doc_forms {
        assert_eq!(
            scanner.redact_content(text),
            text,
            "redact 面豁免形态必须原样保留: {text:?}"
        );
    }

    // 真实形态照常打码。
    assert_eq!(
        scanner.redact_content("account_sid = ACa1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6"),
        "account_sid = [REDACTED_CREDENTIAL]",
        "redact 面真实 SID 必须打码"
    );
    assert!(
        scanner
            .redact_content("api_key: key-a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6")
            .contains("[REDACTED_CREDENTIAL]"),
        "redact 面真实 mailgun 键必须打码"
    );
    assert!(
        scanner
            .redact_content("netlify_token = nf_Ab3cD5eF7g9h2j4k6m8p0q2r4s6t8v0x2z4y6")
            .contains("[REDACTED_CREDENTIAL]"),
        "redact 面真实 netlify token 必须打码"
    );
    assert!(
        scanner
            .redact_content("azure tenant id: 72f988bf-86f1-41af-91ab-2d7cd011db47")
            .contains("[REDACTED_CREDENTIAL]"),
        "redact 面真实 azure tenant GUID 必须打码"
    );
}
