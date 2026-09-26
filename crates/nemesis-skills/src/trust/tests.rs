//! 信任四态归约测试。

use super::*;

#[test]
fn test_unsigned_allowed_review_required() {
    let v = VerificationOutcome {
        signed: false,
        ..Default::default()
    };
    assert_eq!(v.trust_state(true), TrustState::ReviewRequired);
}

#[test]
fn test_unsigned_strict_blocked() {
    let v = VerificationOutcome {
        signed: false,
        ..Default::default()
    };
    assert_eq!(v.trust_state(false), TrustState::Blocked);
}

#[test]
fn test_valid_verified_trusted() {
    let v = VerificationOutcome {
        signed: true,
        valid: Some(true),
        trust_level: Some("verified".to_string()),
        public_key: "ab".to_string(),
        error: String::new(),
    };
    assert_eq!(v.trust_state(true), TrustState::Trusted);
}

#[test]
fn test_valid_community_review_recommended() {
    let v = VerificationOutcome {
        signed: true,
        valid: Some(true),
        trust_level: Some("community".to_string()),
        ..Default::default()
    };
    assert_eq!(v.trust_state(true), TrustState::ReviewRecommended);
}

#[test]
fn test_invalid_blocked() {
    let v = VerificationOutcome {
        signed: true,
        valid: Some(false),
        trust_level: Some("unknown".to_string()),
        error: "signature verification failed".to_string(),
        ..Default::default()
    };
    assert_eq!(v.trust_state(true), TrustState::Blocked);
    // strict 与否不影响无效签名的结论。
    assert_eq!(v.trust_state(false), TrustState::Blocked);
}

#[test]
fn test_installable_and_display() {
    assert!(TrustState::Trusted.installable());
    assert!(TrustState::ReviewRecommended.installable());
    assert!(TrustState::ReviewRequired.installable());
    assert!(!TrustState::Blocked.installable());
    assert_eq!(
        TrustState::ReviewRecommended.to_string(),
        "review-recommended"
    );
}

#[test]
fn test_serde_kebab_case() {
    assert_eq!(
        serde_json::to_string(&TrustState::ReviewRequired).unwrap(),
        "\"review-required\""
    );
    let parsed: TrustState = serde_json::from_str("\"review-recommended\"").unwrap();
    assert_eq!(parsed, TrustState::ReviewRecommended);
}
