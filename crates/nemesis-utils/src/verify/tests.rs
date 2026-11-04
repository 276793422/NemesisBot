//! BUG-20261104-368 行为锁（与 nemesis-web api_handlers/extra_tests.rs 同款四臂）。
use super::verify_token;

#[test]
fn exact_match_passes() {
    assert!(verify_token("secret-token-123", "secret-token-123"));
}

#[test]
fn content_mismatch_fails() {
    assert!(!verify_token("secret-token-124", "secret-token-123"));
}

#[test]
fn wrong_length_fails() {
    assert!(!verify_token("short", "a-much-longer-expected-token"));
    assert!(!verify_token("", "nonempty"));
}

#[test]
fn empty_expected_allows_anything() {
    // 「未配置 token = 不设闸」语义；调用方要强制非空须自带闸。
    assert!(verify_token("anything", ""));
    assert!(verify_token("", ""));
}

#[test]
fn unicode_multibyte_equivalence() {
    // 非 ASCII 多字节 token：按 bytes 折叠，与 == 语义一致。
    let t = "令牌-🔐-token";
    assert!(verify_token(t, t));
    let mut other = t.to_string();
    other.replace_range(0..3, "密"); // 同字节数、内容不同
    assert_eq!(verify_token(t, &other), t == other);
}
