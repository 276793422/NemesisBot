use super::*;
use crate::cert::verify_chain;

#[test]
fn hierarchy_chain_verifies_to_root() {
    let h = generate_hierarchy(0, u64::MAX);
    // issuer 经 [issuer_cert, ca_cert] 链到 root_vk
    let chain = crate::cert::parse_chain(&h.issuer_chain_bytes).unwrap();
    assert!(verify_chain(&h.issuer_vk.to_bytes(), &chain, &[h.root_vk], 1_000_000).is_ok());
}

#[test]
fn hierarchy_save_load_roundtrip() {
    let h = generate_hierarchy(0, u64::MAX);
    let json = h.to_json();
    let h2 = KeyHierarchy::from_json(&json).unwrap();
    assert_eq!(h2.root_vk, h.root_vk);
    assert_eq!(h2.ca_cert, h.ca_cert);
    assert_eq!(h2.issuer_cert, h.issuer_cert);
    assert_eq!(h2.issuer_chain_bytes, h.issuer_chain_bytes);
}

#[test]
fn issuer_signs_content_verifies_via_root() {
    // 完整闭环：issuer 签 content（带链），用 root_vk 验 → Valid
    let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap(); // verify 流程读 revocation env
    let h = generate_hierarchy(0, u64::MAX);
    let signed = crate::verify::sign_content(
        b"payload",
        &h.issuer_sk,
        1000,
        Some(&h.issuer_chain_bytes),
        None,
        None,
        None,
    )
    .unwrap();
    match crate::verify::verify_bytes(&signed, &[h.root_vk], 1000) {
        crate::verify::VerifyOutcome::Valid { pubkey, .. } => {
            assert_eq!(pubkey, h.issuer_vk.to_bytes());
        }
        o => panic!("expected Valid, got {:?}", o),
    }
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：from_json 错误臂 +
// save/load 文件持久化往返。
// ---------------------------------------------------------------------------

#[test]
fn from_json_reports_bad_hex_per_field() {
    let h = generate_hierarchy(0, u64::MAX);
    let good = h.to_json();
    let mut j = KeyHierarchyJson {
        root_sk: good.root_sk.clone(),
        ca_sk: good.ca_sk.clone(),
        ca_cert: good.ca_cert.clone(),
        issuer_sk: good.issuer_sk.clone(),
        issuer_cert: good.issuer_cert.clone(),
    };

    // 私钥 hex 长度错（root_sk / ca_sk / issuer_sk 逐个验错误消息带字段名）
    j.root_sk = "abcd".into();
    assert!(matches!(KeyHierarchy::from_json(&j), Err(ref e) if format!("{e:#}").contains("root_sk")));
    j.root_sk = good.root_sk.clone();

    j.ca_sk = "z".repeat(64);
    assert!(matches!(KeyHierarchy::from_json(&j), Err(ref e) if format!("{e:#}").contains("ca_sk")));
    j.ca_sk = good.ca_sk.clone();

    j.issuer_sk = "1234".into();
    assert!(matches!(KeyHierarchy::from_json(&j), Err(ref e) if format!("{e:#}").contains("issuer_sk")));
    j.issuer_sk = good.issuer_sk.clone();

    // 证书 hex 解不出（ca_cert 长度奇 / issuer_cert 非法字符）
    j.ca_cert = "abc".into();
    assert!(matches!(KeyHierarchy::from_json(&j), Err(ref e) if format!("{e:#}").contains("ca_cert")));
    j.ca_cert = good.ca_cert.clone();

    j.issuer_cert = "g".repeat(64);
    assert!(matches!(KeyHierarchy::from_json(&j), Err(ref e) if format!("{e:#}").contains("issuer_cert")));

    // 证书 hex 合法但字节解析失败（< 146B）→ Certificate::from_bytes 错误透传
    // （ca_cert 与 issuer_cert 两处同一臂，各自都要走到）
    j.ca_cert = hex_encode(&[0u8; 100]);
    assert!(KeyHierarchy::from_json(&j).is_err());
    j.ca_cert = good.ca_cert.clone();

    j.issuer_cert = hex_encode(&[0u8; 100]);
    assert!(KeyHierarchy::from_json(&j).is_err());
}

#[test]
fn hierarchy_save_load_file_roundtrip() {
    let h = generate_hierarchy(0, u64::MAX);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys.json");
    let path_str = path.to_str().unwrap();

    h.save(path_str).expect("save");
    assert!(path.exists(), "save 必须落盘");

    let h2 = KeyHierarchy::load(path_str).expect("load");
    assert_eq!(h2.root_vk, h.root_vk);
    assert_eq!(h2.ca_vk, h.ca_vk);
    assert_eq!(h2.issuer_vk, h.issuer_vk);
    assert_eq!(h2.ca_cert, h.ca_cert);
    assert_eq!(h2.issuer_cert, h.issuer_cert);
    assert_eq!(h2.issuer_chain_bytes, h.issuer_chain_bytes);

    // 落盘内容是合法 JSON（human-readable pretty）
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"root_sk\""));
}

#[test]
fn hierarchy_load_missing_file_errors() {
    let err = KeyHierarchy::load(r"Z:\definitely\missing\keys-9527.json");
    assert!(err.is_err(), "缺文件必须 Err 而非 panic");
}
