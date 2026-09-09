//! asset_token 单测（goal 测试规划点名「资产 token 签发验证」）：
//! 签发/校验 round-trip、篡改/过期/错密钥拒绝、bundle serde 往返、
//! 引用名白名单。全部纯函数，不碰时钟（过期用固定历史时刻构造）。

use super::*;

const SECRET: &[u8] = b"test-secret-bytes-0123456789abcdef";const FAR_FUTURE: i64 = 4_102_444_800; // 2100-01-01，永不过期
const PAST: i64 = 1; // 1970-01-01，恒已过期

#[test]
fn sign_and_verify_roundtrip() {
    let token = sign_asset_token(SECRET, "report.md", FAR_FUTURE);
    assert_eq!(token.len(), 64, "HMAC-SHA256 hex = 64 chars");
    assert!(
        token.bytes().all(|b| b.is_ascii_hexdigit()),
        "token must be lowercase-ish hex"
    );
    verify_asset_token(SECRET, "report.md", FAR_FUTURE, &token)
        .expect("valid token must verify");
}

#[test]
fn tampered_inputs_rejected_as_invalid() {
    let token = sign_asset_token(SECRET, "report.md", FAR_FUTURE);

    // ref 被换（拿同一 token 请求别的资产）→ Invalid。
    assert_eq!(
        verify_asset_token(SECRET, "other.md", FAR_FUTURE, &token),
        Err(AssetTokenError::Invalid)
    );
    // expiry 被改（续命攻击）→ Invalid。
    assert_eq!(
        verify_asset_token(SECRET, "report.md", FAR_FUTURE + 3600, &token),
        Err(AssetTokenError::Invalid)
    );
    // token 单字符篡改 → Invalid。
    let mut tampered = token.clone();
    let first = tampered.as_bytes()[0];
    tampered.replace_range(
        0..1,
        if first == b'0' { "1" } else { "0" },
    );
    assert_eq!(
        verify_asset_token(SECRET, "report.md", FAR_FUTURE, &tampered),
        Err(AssetTokenError::Invalid)
    );
    // 错密钥签的 → Invalid。
    let wrong = sign_asset_token(b"other-secret", "report.md", FAR_FUTURE);
    assert_eq!(
        verify_asset_token(SECRET, "report.md", FAR_FUTURE, &wrong),
        Err(AssetTokenError::Invalid)
    );
}

#[test]
fn expired_token_reports_expired_and_tamper_wins() {
    let token = sign_asset_token(SECRET, "report.md", PAST);
    // 签名对但过期 → Expired（正常生命周期）。
    assert_eq!(
        verify_asset_token(SECRET, "report.md", PAST, &token),
        Err(AssetTokenError::Expired)
    );
    // 过期 + 篡改 → 仍 Invalid（篡改优先于生命周期上报）。
    let wrong = sign_asset_token(b"other-secret", "report.md", PAST);
    assert_eq!(
        verify_asset_token(SECRET, "report.md", PAST, &wrong),
        Err(AssetTokenError::Invalid)
    );
}

#[test]
fn empty_and_whitespace_tokens_rejected() {
    assert_eq!(
        verify_asset_token(SECRET, "report.md", FAR_FUTURE, ""),
        Err(AssetTokenError::Invalid)
    );
    assert_eq!(
        verify_asset_token(SECRET, "report.md", FAR_FUTURE, "   "),
        Err(AssetTokenError::Invalid)
    );
}

#[test]
fn bundle_serde_roundtrip_and_issue_shape() {
    let sha = "a".repeat(64);
    let bundle = issue_asset_bundle(
        SECRET,
        "spec-v2.pdf",
        &sha,
        4096,
        "http://192.168.1.10:49100/",
        600,
    );
    assert_eq!(bundle.asset_ref, "spec-v2.pdf");
    assert_eq!(bundle.sha256, sha, "integrity baseline rides with the bundle");
    assert_eq!(bundle.size, 4096);
    assert_eq!(
        bundle.node_url, "http://192.168.1.10:49100",
        "trailing slash trimmed"
    );
    verify_asset_token(SECRET, &bundle.asset_ref, bundle.expires_at, &bundle.asset_token)
        .expect("freshly issued bundle must verify");

    let json = serde_json::to_string(&bundle).unwrap();
    let back: AssetTokenBundle = serde_json::from_str(&json).unwrap();
    assert_eq!(back, bundle, "serde roundtrip preserves all fields");
    // 字段名即信封/query 契约（§5.4：asset_ref / asset_token / expires_at
    // + 完整性三件套 sha256 / size / node_url——fetch 参数逐字来源）。
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    for key in [
        "asset_ref",
        "asset_token",
        "expires_at",
        "node_url",
        "sha256",
        "size",
    ] {
        assert!(v.get(key).is_some(), "bundle JSON must carry {key}");
    }
}

#[test]
fn sanitize_ref_rejects_traversal_and_accepts_plain_names() {
    // 路径穿越/分隔符/隐藏/空/超长全拒。
    for bad in [
        "..",
        "../escape",
        "a/b",
        "a\\b",
        ".hidden",
        "..hidden",
        "",
        " ",
        "a:b",
        "中文名",
        &"a".repeat(201),
    ] {
        assert!(sanitize_asset_ref(bad).is_err(), "must reject {bad:?}");
    }
    // 常规名放行。
    for good in ["report.md", "a3f9-spec_v2-final.pdf", "build.output.log"] {
        assert!(sanitize_asset_ref(good).is_ok(), "must accept {good:?}");
    }
}

#[test]
fn secret_load_create_and_corruption_semantics() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-secret-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("config").join("asset_secret.key");

    // 首次：生成 32 字节并落盘（自动建父目录）。
    let first = load_or_create_secret(&path).expect("first call creates");
    assert_eq!(first.len(), 32);
    assert!(path.is_file(), "secret file must be written");

    // 二次：读回同一把（不重置——重置会作废已签发 token）。
    let second = load_or_create_secret(&path).expect("reload");
    assert_eq!(first, second, "reload must return the same secret");

    // 损坏（非 64 hex）→ 诚实 Err，不静默重置。
    std::fs::write(&path, "not-a-valid-hex").unwrap();
    assert!(load_or_create_secret(&path).is_err(), "corrupt file must error");
    let _ = std::fs::remove_dir_all(&dir);
}
