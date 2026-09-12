//! keygen v4 单测：生成 → 签名 → 链验证闭环 + keys.json v2 持久化 + D4 profile。
//!
//! goal S1-3 判据：keygen → 签名 → 验证闭环绿（certutil -dump 抽查见 goal §七记录）。

use super::*;
use crate::cert::verify_chain;

const NOW: u64 = 1_700_000_000;

#[test]
fn generate_then_chain_verifies() {
    let kh = generate_at(NOW).unwrap();
    let anchor = kh.root_anchor_fingerprint();
    // 锚口径 = SHA-256(根证书 DER)（S4-2 同源）。
    assert_eq!(anchor, kh.root_cert.sha256_fingerprint());
    // 闭环：keygen 产物直接过 v4 链验证（起点 / leaf 尾点——链窗口被最短命
    // 的 leaf（3y）卡住，10y/30y 时点验链诚实 Expired，另测）。
    verify_chain(&kh.chain(), &anchor, NOW).unwrap();
    let leaf_tail = NOW - NOT_BEFORE_BACKDATE_SECS + SPAN_LEAF_SECS;
    verify_chain(&kh.chain(), &anchor, leaf_tail).unwrap();
    // 超出 leaf 3y 窗口 → Expired（链有效期 = 全员交集）。
    assert_eq!(
        verify_chain(&kh.chain(), &anchor, leaf_tail + 1),
        Err(crate::cert::ChainError::Expired)
    );
}

#[test]
fn chain_order_leaf_issuing_root() {
    let kh = generate_at(NOW).unwrap();
    let chain = kh.chain();
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0].to_der(), kh.leaf_cert.to_der());
    assert_eq!(chain[1].to_der(), kh.issuing_cert.to_der());
    assert_eq!(chain[2].to_der(), kh.root_cert.to_der());
    // 逐级链接：leaf ← issuing ← root。
    assert_eq!(chain[0].aki().unwrap(), chain[1].ski().unwrap());
    assert_eq!(chain[1].aki().unwrap(), chain[2].ski().unwrap());
}

#[test]
fn d4_profile_and_spans() {
    let kh = generate_at(NOW).unwrap();

    // EKU 职责：leaf 带 codeSigning；发行锚 / 根不带。
    assert!(kh.leaf_cert.has_code_signing_eku().unwrap());
    assert!(!kh.issuing_cert.has_code_signing_eku().unwrap());
    assert!(!kh.root_cert.has_code_signing_eku().unwrap());

    // 自签形态：根自签；发行锚 / leaf 非自签。
    assert!(kh.root_cert.is_self_signed().unwrap());
    assert!(!kh.issuing_cert.is_self_signed().unwrap());
    assert!(!kh.leaf_cert.is_self_signed().unwrap());

    // CN 烧进各自 DER（s05 脚本 / certutil 抽查按这些名字找）。
    for (cert, cn) in [
        (&kh.root_cert, CN_ROOT),
        (&kh.issuing_cert, CN_ISSUING),
        (&kh.leaf_cert, CN_LEAF),
    ] {
        assert!(
            cert.to_der().windows(cn.len()).any(|w| w == cn.as_bytes()),
            "DER 应含 CN {cn}"
        );
    }

    // D4 跨度：根 30y / 发行锚 10y / leaf 3y（回拨 1h 只影响起点）。
    let span = |c: &Certificate| {
        let v = &c.parsed().unwrap().tbs_certificate.validity;
        v.not_after.to_unix_duration().as_secs() - v.not_before.to_unix_duration().as_secs()
    };
    assert_eq!(span(&kh.root_cert), SPAN_ROOT_SECS);
    assert_eq!(span(&kh.issuing_cert), SPAN_ISSUING_SECS);
    assert_eq!(span(&kh.leaf_cert), SPAN_LEAF_SECS);

    // 回拨：NOW-1h 起，NOW 当下有效。
    assert!(kh.leaf_cert.is_valid_at(NOW).unwrap());
    assert!(
        kh.leaf_cert
            .is_valid_at(NOW - NOT_BEFORE_BACKDATE_SECS)
            .unwrap()
    );
}

#[test]
fn keys_json_v2_roundtrip() {
    let kh = generate_at(NOW).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys.json");
    let path_str = path.to_str().unwrap();

    kh.save(path_str).unwrap();
    let kh2 = KeyHierarchy::load(path_str).unwrap();

    assert_eq!(kh.root_sk.to_bytes(), kh2.root_sk.to_bytes());
    assert_eq!(kh.issuing_sk.to_bytes(), kh2.issuing_sk.to_bytes());
    assert_eq!(kh.leaf_sk.to_bytes(), kh2.leaf_sk.to_bytes());
    assert_eq!(kh.root_cert.to_der(), kh2.root_cert.to_der());
    assert_eq!(kh.issuing_cert.to_der(), kh2.issuing_cert.to_der());
    assert_eq!(kh.leaf_cert.to_der(), kh2.leaf_cert.to_der());

    // 重载体系闭环仍绿、锚一致。
    assert_eq!(kh2.root_anchor_fingerprint(), kh.root_anchor_fingerprint());
    verify_chain(&kh2.chain(), &kh2.root_anchor_fingerprint(), NOW).unwrap();
}

#[test]
fn keys_json_wrong_version_rejected() {
    let kh = generate_at(NOW).unwrap();
    let mut j = kh.to_json();
    j.version = 1; // v3 文件必须走 keygen::legacy，v4 入口诚实拒绝。
    assert!(KeyHierarchy::from_json(&j).is_err());
}

#[test]
fn generate_smoke_real_clock() {
    let kh = generate().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(kh.leaf_cert.is_valid_at(now).unwrap());
    verify_chain(&kh.chain(), &kh.root_anchor_fingerprint(), now).unwrap();
}

/// 外部工具抽查导出（S1-3 判据：certutil -dump 抽查自产链字段）。
///
/// `#[ignore]`：有磁盘副作用，仅显式触发——
/// `cargo test -p nemesis-verify dump_chain_for_external_inspection -- --ignored --nocapture`
/// 导出 `%TEMP%/nb_v4_chain/{root,issuing,leaf}.der` 后用 `certutil -dump` 人审。
#[test]
#[ignore]
fn dump_chain_for_external_inspection() {
    let kh = generate().unwrap();
    let dir = std::env::temp_dir().join("nb_v4_chain");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, cert) in [
        ("root.der", &kh.root_cert),
        ("issuing.der", &kh.issuing_cert),
        ("leaf.der", &kh.leaf_cert),
    ] {
        let p = dir.join(name);
        std::fs::write(&p, cert.to_der()).unwrap();
        println!("{}", p.display());
    }
}
