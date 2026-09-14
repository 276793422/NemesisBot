//! oidc 模块纯逻辑测试（不需要真实 IdP）：JWT 解码 + 角色/展示名提取。
//! 端到端协议测试（需 Keycloak）在本文件底部，`#[ignore]` 门控。

use super::*;
use base64::Engine;

/// 构造假 JWT（header.payload.signature，base64url 无填充）。
/// 注意：这只用于测**载荷解码**纯函数——真实签名校验由 openidconnect
/// 库在 complete() 里做，demo 不自造密码学。
fn craft_jwt(payload: &serde_json::Value) -> String {
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!(
        "{}.{}.{}",
        b64(br#"{"alg":"RS256","typ":"JWT"}"#),
        b64(payload.to_string().as_bytes()),
        b64(b"not-a-real-signature")
    )
}

#[test]
fn decode_payload_roundtrip() {
    let payload = serde_json::json!({
        "sub": "user-123",
        "preferred_username": "alice",
        "groups": ["operators", "viewers"],
        "realm_access": {"roles": ["default-roles-demo", "operator"]}
    });
    let jwt = craft_jwt(&payload);
    let decoded = decode_jwt_payload(&jwt).unwrap();
    assert_eq!(
        decoded.get("sub").and_then(|v| v.as_str()),
        Some("user-123")
    );
}

#[test]
fn decode_rejects_garbage() {
    assert!(decode_jwt_payload("no-dots").is_err());
    assert!(decode_jwt_payload("a.!!!not-base64!.c").is_err());
    assert!(
        decode_jwt_payload("a.b64_c").is_err(),
        "载荷非法 JSON 应报错"
    );
}

#[test]
fn roles_merge_groups_and_realm_roles_dedup_sorted() {
    let raw = serde_json::json!({
        "groups": ["viewers", "operators"],
        "realm_access": {"roles": ["operator", "default-roles-demo", "offline_access"]}
    });
    let roles = extract_roles(&raw);
    assert_eq!(
        roles,
        vec![
            "default-roles-demo",
            "offline_access",
            "operator",
            "operators",
            "viewers"
        ]
    );
}

#[test]
fn roles_empty_when_claims_absent() {
    assert!(extract_roles(&serde_json::json!({"sub": "x"})).is_empty());
    // realm_access 存在但无 roles 键
    assert!(extract_roles(&serde_json::json!({"realm_access": {}})).is_empty());
}

#[test]
fn display_name_prefers_name_then_preferred_username() {
    let with_name = serde_json::json!({"name": "Alice Chen", "preferred_username": "alice"});
    assert_eq!(extract_display_name(&with_name, "fb"), "Alice Chen");

    let only_username = serde_json::json!({"preferred_username": "alice"});
    assert_eq!(extract_display_name(&only_username, "fb"), "alice");

    let neither = serde_json::json!({"sub": "s1"});
    assert_eq!(extract_display_name(&neither, "fb"), "fb");
}

// ---------------- 端到端协议测试（需要 docker compose up 的 Keycloak） ----------------
// 运行：cargo test -p authn-core -- --ignored

#[tokio::test]
#[ignore = "需要 Keycloak（docker/docker-compose.yml，端口 8088）"]
async fn e2e_password_grant_against_keycloak() {
    let config = OidcConfig::new("http://localhost:8088/realms/demo", "authn-demo");
    let token = password_grant_token(&config, "alice", "alice123")
        .await
        .expect("ROPC 应成功（alice/alice123 @ realms/demo）");
    let access = token
        .get("access_token")
        .and_then(|v| v.as_str())
        .expect("应有 access_token");
    assert!(!access.is_empty());

    // 角色提取 = id_token + access_token 两个载荷合并（与 complete() 同语义）。
    // Keycloak 默认把 realm roles 放 access_token 的 realm_access，id_token 不带。
    let id_token = token
        .get("id_token")
        .and_then(|v| v.as_str())
        .expect("应有 id_token");
    let id_raw = decode_jwt_payload(id_token).unwrap();
    let access_raw = decode_jwt_payload(access).unwrap();
    let mut roles = extract_roles(&id_raw);
    roles.extend(extract_roles(&access_raw));
    assert!(
        roles.contains(&"operator".to_string()),
        "alice 应有 operator 角色，实际 {roles:?}（id_token 载荷：{id_raw}）"
    );
}
