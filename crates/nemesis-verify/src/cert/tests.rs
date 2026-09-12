//! 证书链单测（v4：X.509 解析 + 链验证）。
//!
//! goal S1-2 判据六形态各 ≥1：正链 / 断链 / 过期 / EKU 错 / 签名坏 / 根不匹配，
//! 全部经 keygen 低层积木 `issue_x509` 组合。

use super::*;
use crate::crypto;
use crate::keygen::{self, issue_x509};
use sha2::{Digest, Sha256};

const NOW: u64 = 1_700_000_000;
/// 「远期」有效期上界（2100-01-01；GeneralizedTime 可编码，u64::MAX 不行）。
const FAR: u64 = 4_102_444_800;

/// 独立密钥夹具：自产 root/issuing/leaf 三级链 + 根锚指纹。
fn fixture(
    not_before: u64,
    not_after: u64,
    leaf_eku: bool,
) -> (Vec<Certificate>, [u8; 32], Certificate, Certificate) {
    let mk = || crypto::signing_key_from_hex(&crypto::generate_key_pair().private_key).unwrap();
    let root_sk = mk();
    let issuing_sk = mk();
    let leaf_sk = mk();
    let root_vk = *root_sk.verifying_key();
    let issuing_vk = *issuing_sk.verifying_key();
    let leaf_vk = *leaf_sk.verifying_key();

    let profile =
        |subject_cn: &'static str, issuer_cn: &'static str, is_ca: bool, eku: bool| TbsInput {
            subject_cn,
            subject_org: Some("NB Test"),
            issuer_cn,
            issuer_org: Some("NB Test"),
            is_ca,
            path_len: None,
            ku_digital_signature: true,
            ku_key_cert_sign: is_ca,
            ku_crl_sign: is_ca,
            eku_code_signing: eku,
            not_before_unix: not_before,
            not_after_unix: not_after,
        };

    let root = issue_x509(
        &root_vk,
        &root_sk,
        &ski_value(&root_vk).unwrap(),
        profile("NB Test Root", "NB Test Root", true, false),
    )
    .unwrap();
    let issuing = issue_x509(
        &issuing_vk,
        &root_sk,
        &ski_value(&root_vk).unwrap(),
        profile("NB Test Issuing", "NB Test Root", true, false),
    )
    .unwrap();
    let leaf = issue_x509(
        &leaf_vk,
        &issuing_sk,
        &ski_value(&issuing_vk).unwrap(),
        profile("NB Test Leaf", "NB Test Issuing", false, leaf_eku),
    )
    .unwrap();

    let root_fp = root.sha256_fingerprint();
    (
        vec![leaf.clone(), issuing.clone(), root],
        root_fp,
        leaf,
        issuing,
    )
}

#[test]
fn valid_three_level_chain_verifies() {
    let kh = keygen::generate_at(NOW).unwrap();
    let chain = kh.chain();
    assert_eq!(chain.len(), 3);

    // 正链：锚 = 根 DER SHA-256。
    verify_chain(&chain, &kh.root_anchor_fingerprint(), NOW + 100).unwrap();

    // SKI/AKI 链接：leaf.aki == issuing.ski；issuing.aki == root.ski。
    let (leaf, issuing, root) = (&chain[0], &chain[1], &chain[2]);
    assert_eq!(leaf.aki().unwrap(), issuing.ski().unwrap());
    assert_eq!(issuing.aki().unwrap(), root.ski().unwrap());
    // D4 职责：leaf 带 codeSigning EKU；根自签、发行锚非自签。
    assert!(leaf.has_code_signing_eku().unwrap());
    assert!(!issuing.has_code_signing_eku().unwrap());
    assert!(root.is_self_signed().unwrap());
    assert!(!issuing.is_self_signed().unwrap());
    // 签名套件唯一接受。
    assert!(leaf.signature_algorithm_ok());
}

#[test]
fn expired_chain_detected() {
    let (chain, root_fp, _, _) = fixture(1_000_000, 1_000_500, true);
    match verify_chain(&chain, &root_fp, 1_001_000) {
        Err(ChainError::Expired) => {}
        other => panic!("期望 Expired，实得 {:?}", other.err()),
    }
    // 边界内仍有效。
    verify_chain(&chain, &root_fp, 1_000_500).unwrap();
}

#[test]
fn missing_code_signing_eku_detected() {
    let (chain, root_fp, _, _) = fixture(0, FAR, false);
    match verify_chain(&chain, &root_fp, NOW) {
        Err(ChainError::MissingCodeSigningEku) => {}
        other => panic!("期望 MissingCodeSigningEku，实得 {:?}", other.err()),
    }
}

#[test]
fn broken_chain_detected() {
    let (chain_a, root_fp_a, leaf_a, _) = fixture(0, FAR, true);
    let (chain_b, root_fp_b, _, issuing_b) = fixture(0, FAR, true);
    // A 的 leaf 配 B 的 issuing：AKI(SKI_A) != SKI_B → 断链。
    let mixed = vec![leaf_a, issuing_b, chain_b[2].clone()];
    match verify_chain(&mixed, &root_fp_b, NOW) {
        Err(ChainError::BrokenChain) => {}
        other => panic!("期望 BrokenChain，实得 {:?}", other.err()),
    }
    // 对照：A 自己的整链 + 自己的锚 = 通过（排除夹具本身问题）。
    verify_chain(&chain_a, &root_fp_a, NOW).unwrap();
}

#[test]
fn bad_signature_detected() {
    let (chain, root_fp, leaf, issuing) = fixture(0, FAR, true);
    // 篡改 leaf TBS 内 CN 字符串一个字节（长度不变，结构仍合法）。
    let mut der = leaf.to_der().to_vec();
    let cn = b"NB Test Leaf";
    let pos = der
        .windows(cn.len())
        .position(|w| w == cn)
        .expect("CN 字节应在 DER 内");
    der[pos] ^= 0x01;
    let tampered = Certificate::from_der(&der).unwrap();
    let mixed = vec![tampered, issuing, chain[2].clone()];
    match verify_chain(&mixed, &root_fp, NOW) {
        Err(ChainError::BadSignature) => {}
        other => panic!("期望 BadSignature，实得 {:?}", other.err()),
    }
}

#[test]
fn wrong_root_anchor_detected() {
    let (chain, root_fp, leaf, issuing) = fixture(0, FAR, true);
    // 锚不匹配：随便换个指纹（leaf 自身 DER 的 SHA-256）→ 未到受信根。
    let wrong_fp: [u8; 32] = Sha256::digest(leaf.to_der()).into();
    assert_ne!(wrong_fp, root_fp);
    match verify_chain(&chain, &wrong_fp, NOW) {
        Err(ChainError::NoRootForIssuer) => {}
        other => panic!("期望 NoRootForIssuer，实得 {:?}", other.err()),
    }
    // 链截断（无根）：末级 issuing 非自签 → 同样未到受信根。
    let truncated = vec![leaf, issuing];
    match verify_chain(&truncated, &root_fp, NOW) {
        Err(ChainError::NoRootForIssuer) => {}
        other => panic!("期望 NoRootForIssuer（截断链），实得 {:?}", other.err()),
    }
    // 空链 → Empty；畸形 DER → Malformed。
    assert_eq!(verify_chain(&[], &root_fp, NOW), Err(ChainError::Empty));
    assert!(matches!(
        Certificate::from_der(&[0x03u8, 0x02, 0x01]),
        Err(ChainError::Malformed(_))
    ));
}
