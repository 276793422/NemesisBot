//! 库级集成测试（纯逻辑，无外部服务）：本地账号 → 会话全链路。
//! 这是"一次登录在产品侧会发生什么"的最小模型。

use crate::compat::{AccessControl, AccessDecision};
use crate::identity::AuthnSource;
use crate::local::{hash_password, LocalUserDatabase};
use crate::session::{SessionConfig, SessionError, SessionStore};

fn db() -> LocalUserDatabase {
    let hash = hash_password("pw-zoo").unwrap();
    LocalUserDatabase::from_json_str(&format!(
        r#"{{"users":[{{"username":"zoo","display_name":"Zoo","roles":["admin"],"password_hash":"{hash}"}}]}}"#
    ))
    .unwrap()
}

#[test]
fn local_login_issues_and_validates_session() {
    let db = db();
    let identity = db.authenticate("zoo", "pw-zoo").unwrap();

    let store = SessionStore::new(SessionConfig {
        ttl_seconds: 60,
        max_sessions_per_user: 2,
    });
    let rec = store.issue(&identity).unwrap();
    let validated = store.validate(&rec.token).unwrap();
    assert_eq!(validated.subject, "zoo");
    assert_eq!(validated.expires_at_unix - validated.issued_at_unix, 60);
}

#[test]
fn full_chain_then_revoke_denies_access() {
    let identity = db().authenticate("zoo", "pw-zoo").unwrap();
    let store = SessionStore::new(SessionConfig::default());
    let rec = store.issue(&identity).unwrap();
    assert!(store.validate(&rec.token).is_ok());
    store.revoke(&rec.token);
    assert_eq!(
        store.validate(&rec.token).unwrap_err(),
        SessionError::Invalid
    );
}

#[test]
fn static_token_compat_coexists_with_login_awareness() {
    // 场景：系统配置了静态 token（旧模式），同时本地账号体系在旁路验证。
    // 产品集成时的语义：静态 token 是"旧入口"的兼容凭据，登录态是"新入口"。
    let ac = AccessControl::from_static_token(Some("legacy-token")).unwrap();

    // 旧入口：带 legacy-token 的请求照常放行
    assert_eq!(
        ac.check(Some("legacy-token")),
        AccessDecision::AllowStaticToken
    );
    // 不带/带错 → 拒绝（开放语义被配置收紧）
    assert_eq!(ac.check(None), AccessDecision::Denied);

    // 新入口的会话 token 与静态 token 无关——它走 session store 校验
    let identity = db().authenticate("zoo", "pw-zoo").unwrap();
    assert_eq!(identity.source, AuthnSource::Local);
    let store = SessionStore::new(SessionConfig::default());
    let rec = store.issue(&identity).unwrap();
    assert!(store.validate(&rec.token).is_ok());
}
