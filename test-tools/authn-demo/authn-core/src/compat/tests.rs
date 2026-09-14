//! compat 模块测试：静态 token 共存语义三态 + 常量时间比较。

use super::*;

#[test]
fn empty_and_none_are_open_mode() {
    for input in [None, Some(""), Some("   ")] {
        let ac = AccessControl::from_static_token(input).unwrap();
        assert_eq!(ac.mode(), &AccessMode::Open, "input={input:?} 应为开放模式");
        assert_eq!(ac.check(Some("anything")), AccessDecision::AllowOpen);
        assert_eq!(ac.check(None), AccessDecision::AllowOpen);
    }
}

#[test]
fn static_token_three_outcomes() {
    let ac = AccessControl::from_static_token(Some("secret-token")).unwrap();
    assert_eq!(ac.mode(), &AccessMode::StaticToken("secret-token".into()));

    // 正确 token → 放行
    assert_eq!(
        ac.check(Some("secret-token")),
        AccessDecision::AllowStaticToken
    );
    // 错误 token → 拒绝
    assert_eq!(ac.check(Some("wrong")), AccessDecision::Denied);
    // 没带 token → 拒绝
    assert_eq!(ac.check(None), AccessDecision::Denied);
}

#[test]
fn token_is_case_sensitive_and_trimmed_at_config() {
    let ac = AccessControl::from_static_token(Some("  Abc  ")).unwrap();
    assert_eq!(ac.mode(), &AccessMode::StaticToken("Abc".into()));
    assert_eq!(ac.check(Some("abc")), AccessDecision::Denied);
    assert_eq!(ac.check(Some("Abc")), AccessDecision::AllowStaticToken);
}

#[test]
fn oversized_token_rejected() {
    let huge = "x".repeat(4097);
    assert_eq!(
        AccessControl::from_static_token(Some(&huge)).unwrap_err(),
        CompatError::TokenTooLong
    );
    let boundary = "x".repeat(4096);
    assert!(AccessControl::from_static_token(Some(&boundary)).is_ok());
}

#[test]
fn ct_eq_basics() {
    assert!(super::ct_eq_str("same", "same"));
    assert!(!super::ct_eq_str("same", "diff"));
    assert!(!super::ct_eq_str("short", "shorter"));
    assert!(super::ct_eq_str("", ""));
}
