//! D6.1 签名对抗测试：签验正常链 / 篡改 / 调换 / 吊销 / 不受信 / registry 拒收。

use std::path::Path;

use super::*;
use crate::manifest::parse_connector_manifest;
use nemesis_security::signature::{TrustLevel, TrustStore};

const SAMPLE_TOML: &str = "[connector]\nid = \"sample\"\nname = \"Sample\"\nversion = \"0.1.0\"\nkind = \"local\"\n\n[[op]]\nid = \"ping\"\nrisk = \"low\"\nclass = \"read\"\naction = \"builtin.demo\"\n";

fn write_connector(dir: &Path, text: &str) {
    std::fs::create_dir_all(dir).expect("mkdir");
    std::fs::write(dir.join(crate::registry::MANIFEST_FILE), text).expect("write toml");
}

fn trusted_store(public_key: &str, level: TrustLevel) -> TrustStore {
    let store = TrustStore::in_memory();
    store.add_key(public_key, "test-signer", level);
    store
}

#[test]
fn test_sign_and_assess_verified() {
    let ws = tempfile::tempdir().expect("ws");
    let dir = ws.path().join("sample");
    write_connector(&dir, SAMPLE_TOML);
    let pair = nemesis_security::signature::generate_key_pair().expect("keygen");
    let public_key = sign_connector_dir(&dir, &pair.private_key).expect("sign");
    assert_eq!(public_key, pair.public_key, "sign 返回签名者公钥");
    let a =
        assess_connector_signature(&dir, &trusted_store(&pair.public_key, TrustLevel::Verified))
            .expect("验签过");
    assert_eq!(a.trust, ConnectorTrust::Verified);
    assert_eq!(a.public_key, pair.public_key);
}

#[test]
fn test_unsigned_dir_is_unsigned() {
    let ws = tempfile::tempdir().expect("ws");
    let dir = ws.path().join("sample");
    write_connector(&dir, SAMPLE_TOML);
    let a = assess_connector_signature(&dir, &TrustStore::in_memory()).expect("无 sig 不算失败");
    assert_eq!(a.trust, ConnectorTrust::Unsigned);
}

#[test]
fn test_unknown_key_is_community_warn() {
    // 签名有效但密钥不在信任库 → community WARN（收录但 list 标注）。
    let ws = tempfile::tempdir().expect("ws");
    let dir = ws.path().join("sample");
    write_connector(&dir, SAMPLE_TOML);
    let pair = nemesis_security::signature::generate_key_pair().expect("keygen");
    sign_connector_dir(&dir, &pair.private_key).expect("sign");
    let a = assess_connector_signature(&dir, &TrustStore::in_memory()).expect("验签过");
    assert_eq!(a.trust, ConnectorTrust::Community);
}

#[test]
fn test_tampered_manifest_fails_loud() {
    // 对抗①：签好后改 toml 一个字节 → 验签 loud 失败。
    let ws = tempfile::tempdir().expect("ws");
    let dir = ws.path().join("sample");
    write_connector(&dir, SAMPLE_TOML);
    let pair = nemesis_security::signature::generate_key_pair().expect("keygen");
    sign_connector_dir(&dir, &pair.private_key).expect("sign");
    std::fs::write(
        dir.join(crate::registry::MANIFEST_FILE),
        SAMPLE_TOML.replace("risk = \"low\"", "risk = \"critical\""),
    )
    .expect("tamper");
    let err =
        assess_connector_signature(&dir, &trusted_store(&pair.public_key, TrustLevel::Verified))
            .expect_err("篡改必须拒");
    assert!(err.contains("签名验证失败"), "实际: {err}");
}

#[test]
fn test_swapped_signature_fails() {
    // 对抗②：把另一把私钥签的另一份清单的 sig 调换过来 → 失败。
    let ws = tempfile::tempdir().expect("ws");
    let dir_a = ws.path().join("a");
    let dir_b = ws.path().join("b");
    write_connector(&dir_a, SAMPLE_TOML);
    write_connector(&dir_b, &SAMPLE_TOML.replace("\"sample\"", "\"other\""));
    let k1 = nemesis_security::signature::generate_key_pair().expect("keygen");
    let k2 = nemesis_security::signature::generate_key_pair().expect("keygen");
    sign_connector_dir(&dir_a, &k1.private_key).expect("sign a");
    sign_connector_dir(&dir_b, &k2.private_key).expect("sign b");
    // 把 b 的签名偷给 a（清单与签名不同源）。
    std::fs::copy(dir_b.join(SIG_FILE), dir_a.join(SIG_FILE)).expect("swap");
    let err =
        assess_connector_signature(&dir_a, &trusted_store(&k1.public_key, TrustLevel::Verified))
            .expect_err("调换签名必须拒");
    assert!(err.contains("签名验证失败"), "实际: {err}");
}

#[test]
fn test_revoked_key_is_rejected() {
    // 对抗③：吊销后 → 不可信（即使签名本身密码学有效）。
    let ws = tempfile::tempdir().expect("ws");
    let dir = ws.path().join("sample");
    write_connector(&dir, SAMPLE_TOML);
    let pair = nemesis_security::signature::generate_key_pair().expect("keygen");
    sign_connector_dir(&dir, &pair.private_key).expect("sign");
    let store = trusted_store(&pair.public_key, TrustLevel::Verified);
    store
        .revoke_key_by_public_key(&pair.public_key)
        .expect("revoke");
    let err = assess_connector_signature(&dir, &store).expect_err("吊销后必须拒");
    assert!(err.contains("吊销"), "实际: {err}");
}

#[test]
fn test_bad_sig_file_format_is_loud_error() {
    // 坏 JSON / 未知算法同走拒收（不给静默降级机会）。
    let ws = tempfile::tempdir().expect("ws");
    let dir = ws.path().join("sample");
    write_connector(&dir, SAMPLE_TOML);
    std::fs::write(dir.join(SIG_FILE), "not json").expect("write bad sig");
    let err =
        assess_connector_signature(&dir, &TrustStore::in_memory()).expect_err("坏 sig 必须拒");
    assert!(err.contains("格式非法"), "实际: {err}");
    std::fs::write(
        dir.join(SIG_FILE),
        r#"{"algorithm":"rsa","public_key":"aa","signature":"bb"}"#,
    )
    .expect("write wrong algo");
    let err =
        assess_connector_signature(&dir, &TrustStore::in_memory()).expect_err("未知算法必须拒");
    assert!(err.contains("ed25519"), "实际: {err}");
}

#[test]
fn test_registry_rejects_tampered_and_honors_verified() {
    // registry 集成：verified 徽标透出；篡改条目走 corrupt 拒收（scan + find 双面）。
    let ws = tempfile::tempdir().expect("ws");
    let dir = ws.path().join("connectors").join("sample");
    write_connector(&dir, SAMPLE_TOML);
    let pair = nemesis_security::signature::generate_key_pair().expect("keygen");
    // 信任库落 workspace/config/skill_trust.json（与 registry 的
    // workspace_trust_store 同路径——真链路而非旁路注入）。
    let store = crate::signing::workspace_trust_store(ws.path());
    store.add_key(&pair.public_key, "test-signer", TrustLevel::Verified);
    sign_connector_dir(&dir, &pair.private_key).expect("sign");

    let loaded = crate::registry::scan_connectors(ws.path());
    assert_eq!(loaded.len(), 1);
    let l = &loaded[0];
    assert!(l.error.is_none(), "签名完好不应拒收: {:?}", l.error);
    assert_eq!(l.trust, ConnectorTrust::Verified);
    assert!(parse_connector_manifest("x").is_err()); // sanity：parse 函数在场

    // find_connector 正常放行（M1：trust 随返回值透出）。
    let (found, found_trust) =
        crate::registry::find_connector(ws.path(), "sample").expect("完好连接器可用");
    assert_eq!(found.id, "sample");
    assert_eq!(found_trust, ConnectorTrust::Verified);

    // 篡改 → scan corrupt 入列 + find 拒用。
    std::fs::write(
        dir.join(crate::registry::MANIFEST_FILE),
        SAMPLE_TOML.replace("risk = \"low\"", "risk = \"critical\""),
    )
    .expect("tamper");
    let loaded = crate::registry::scan_connectors(ws.path());
    assert_eq!(loaded.len(), 1, "拒收条目仍诚实入列");
    assert!(loaded[0].connector.is_none(), "篡改条目不得过带");
    let err = loaded[0].error.as_deref().expect("corrupt 注记");
    assert!(err.contains("签名验证失败"), "实际: {err}");
    let find_err =
        crate::registry::find_connector(ws.path(), "sample").expect_err("执行面同样拒用");
    match find_err {
        crate::registry::ConnectorLookupError::Invalid { error, .. } => {
            assert!(error.contains("签名验证失败"), "实际: {error}");
        }
        other => panic!("应为 Invalid，实际 {other:?}"),
    }
}
