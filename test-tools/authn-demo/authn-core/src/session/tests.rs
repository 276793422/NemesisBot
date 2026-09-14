//! session 模块测试：签发 / 校验 / 过期 / 吊销 / 并发上限。

use super::*;
use crate::identity::AuthnSource;

fn demo_identity(name: &str) -> Identity {
    Identity {
        subject: name.to_string(),
        display_name: name.to_string(),
        roles: vec!["operator".into()],
        source: AuthnSource::Local,
    }
}

#[test]
fn issue_then_validate_roundtrip() {
    let store = SessionStore::new(SessionConfig::default());
    let rec = store.issue(&demo_identity("zoo")).unwrap();
    assert_eq!(rec.token.len(), 43, "base64url(32B) 无填充应为 43 字符");
    assert!(rec.expires_at_unix > rec.issued_at_unix);

    let validated = store.validate(&rec.token).unwrap();
    assert_eq!(validated.subject, "zoo");
    assert_eq!(validated.token, rec.token);
}

#[test]
fn tokens_are_unique() {
    let store = SessionStore::new(SessionConfig::default());
    let a = store.issue(&demo_identity("zoo")).unwrap().token;
    let b = store.issue(&demo_identity("zoo")).unwrap().token;
    assert_ne!(a, b);
}

#[test]
fn expiry_becomes_invalid() {
    let store = SessionStore::new(SessionConfig {
        ttl_seconds: 1,
        max_sessions_per_user: 4,
    });
    let rec = store.issue(&demo_identity("zoo")).unwrap();
    assert!(store.validate(&rec.token).is_ok());
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        store.validate(&rec.token).unwrap_err(),
        SessionError::Expired
    );
    // 过期顺带清除——之后变 Invalid，且 len 归零
    assert_eq!(
        store.validate(&rec.token).unwrap_err(),
        SessionError::Invalid
    );
    assert_eq!(store.len(), 0);
}

#[test]
fn revoke_removes_session() {
    let store = SessionStore::new(SessionConfig::default());
    let rec = store.issue(&demo_identity("zoo")).unwrap();
    assert!(store.revoke(&rec.token));
    assert!(!store.revoke(&rec.token), "二次吊销应返回 false");
    assert_eq!(
        store.validate(&rec.token).unwrap_err(),
        SessionError::Invalid
    );
}

#[test]
fn unknown_token_is_invalid() {
    let store = SessionStore::new(SessionConfig::default());
    assert_eq!(store.validate("nope").unwrap_err(), SessionError::Invalid);
    assert_eq!(store.validate("").unwrap_err(), SessionError::Invalid);
}

#[test]
fn concurrent_limit_per_user_not_global() {
    let store = SessionStore::new(SessionConfig {
        ttl_seconds: 60,
        max_sessions_per_user: 2,
    });
    store.issue(&demo_identity("zoo")).unwrap();
    store.issue(&demo_identity("zoo")).unwrap();
    assert_eq!(
        store.issue(&demo_identity("zoo")).unwrap_err(),
        SessionError::LimitReached(2),
        "同用户第三会话应被拒"
    );
    // 其他用户不受影响（上限是 per-user 不是全局）
    assert!(store.issue(&demo_identity("alice")).is_ok());
}

#[test]
fn purge_expired_counts() {
    let store = SessionStore::new(SessionConfig {
        ttl_seconds: 1,
        max_sessions_per_user: 8,
    });
    store.issue(&demo_identity("zoo")).unwrap();
    store.issue(&demo_identity("alice")).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(store.purge_expired(), 2);
    assert_eq!(store.purge_expired(), 0);
}

#[test]
fn fingerprint_differs_per_token() {
    assert_ne!(token_fingerprint("tok-a"), token_fingerprint("tok-b"));
    assert!(token_fingerprint("tok-a").starts_with("demo-fp:"));
}

#[test]
fn ttl_duration_helper() {
    assert_eq!(
        ttl_duration(&SessionConfig::default()),
        Duration::from_secs(3600)
    );
}
