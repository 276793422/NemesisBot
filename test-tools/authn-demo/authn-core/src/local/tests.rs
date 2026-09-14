//! local 模块测试：argon2 哈希/校验 + 账号库认证语义。

use super::*;
use std::io::Write;

#[test]
fn hash_and_verify_roundtrip() {
    let hash = hash_password("correct horse battery staple").unwrap();
    assert!(
        hash.starts_with("$argon2id$"),
        "应为 argon2id PHC 格式: {hash}"
    );
    assert!(verify_password("correct horse battery staple", &hash).unwrap());
    assert!(!verify_password("wrong password", &hash).unwrap());
}

#[test]
fn same_password_different_salts() {
    let h1 = hash_password("pw").unwrap();
    let h2 = hash_password("pw").unwrap();
    assert_ne!(h1, h2, "盐必须随机——同密码两次哈希应不同");
    assert!(verify_password("pw", &h1).unwrap());
    assert!(verify_password("pw", &h2).unwrap());
}

#[test]
fn malformed_hash_is_error_not_false() {
    let err = verify_password("pw", "not-a-phc-string").unwrap_err();
    assert!(matches!(err, LocalAuthError::Hash(_)));
}

fn sample_db_json() -> String {
    let hash = hash_password("zoo-pw").unwrap();
    let hash2 = hash_password("alice-pw").unwrap();
    format!(
        r#"{{"users":[
            {{"username":"zoo","display_name":"Zoo","roles":["admin"],"password_hash":"{hash}"}},
            {{"username":"alice","display_name":"","roles":["operator"],"password_hash":"{hash2}"}}
        ]}}"#
    )
}

#[test]
fn authenticate_success_builds_identity() {
    let db = LocalUserDatabase::from_json_str(&sample_db_json()).unwrap();
    assert_eq!(db.user_count(), 2);

    let id = db.authenticate("Zoo", "zoo-pw").unwrap();
    assert_eq!(id.subject, "zoo");
    assert_eq!(id.display_name, "Zoo");
    assert_eq!(id.roles, vec!["admin"]);
    assert_eq!(id.source, AuthnSource::Local);

    // 大小写不敏感登录 + 空 display_name 回退到 username
    let id2 = db.authenticate("ALICE", "alice-pw").unwrap();
    assert_eq!(id2.subject, "alice");
    assert_eq!(id2.display_name, "alice");
    assert_eq!(id2.roles, vec!["operator"]);
}

#[test]
fn bad_password_and_unknown_user_are_same_error() {
    let db = LocalUserDatabase::from_json_str(&sample_db_json()).unwrap();
    assert_eq!(
        db.authenticate("zoo", "wrong").unwrap_err(),
        LocalAuthError::BadCredentials
    );
    assert_eq!(
        db.authenticate("nobody", "whatever").unwrap_err(),
        LocalAuthError::BadCredentials,
        "不存在用户与密码错误必须同一错误（不泄露账号存在性）"
    );
}

#[test]
fn duplicate_usernames_rejected() {
    let hash = hash_password("pw").unwrap();
    let json = format!(
        r#"{{"users":[
            {{"username":"dup","password_hash":"{hash}"}},
            {{"username":"DUP","password_hash":"{hash}"}}
        ]}}"#
    );
    let err = LocalUserDatabase::from_json_str(&json).unwrap_err();
    assert!(matches!(err, LocalAuthError::Format(_)));
}

#[test]
fn load_from_real_file_roundtrip() {
    let dir = std::env::temp_dir().join(format!("authn-core-local-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("users.json");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(sample_db_json().as_bytes()).unwrap();
    let db = LocalUserDatabase::load_from_json_file(&path).unwrap();
    assert_eq!(db.user_count(), 2);
    assert!(db.authenticate("zoo", "zoo-pw").is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}
