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
    let result = scanner
        .scan_content("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abc123def");
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
    let result = scanner.scan_content("mongodb://admin:secret@cluster.example.com:27017/db");
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
    let result = scanner.scan_content(
        "AccountName=myaccount;AccountKey=abc123def456ghi789jkl012mno345pqr678stu901vwx==",
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
    // "Basic A" matches basic_auth with a 7-byte full match → KeyValue mask's
    // <=8-byte [REDACTED] arm.
    let short = Scanner::with_mask_mode(true, "log", MaskMode::KeyValue);
    let r = short.scan_content("Authorization: Basic A");
    assert!(r.has_matches);
    assert!(
        r.matches.iter().any(|m| m.redacted == "[REDACTED]"),
        "matches: {:?}",
        r.matches
    );
    // 20-byte AWS key → KeyValue keep-prefix/suffix arm ("AKIA...MPLE").
    let long = Scanner::with_mask_mode(true, "log", MaskMode::KeyValue);
    let r2 = long.scan_content("AKIAIOSFODNN7EXAMPLE");
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
    let hit = scanner.scan_tool_output("shell", "key=AKIAIOSFODNN7EXAMPLE tail");
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
