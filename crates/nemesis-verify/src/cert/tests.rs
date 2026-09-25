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

// ---------------------------------------------------------------------------
// AGT 覆盖率批次（2026-09-24）：ChainError Display 全变体 / subject_public_key
// 三个失败臂 / SKI・AKI 剥除形态 / 签名算法不收 / KU 八组合 / chain blob
// 往返与全部错误臂 / 无 AKI leaf 过链。
// TBS 级变异积木：build_tbs → mutate → seal_certificate（签名在变异之后盖，
// 证书自洽）。
// ---------------------------------------------------------------------------

/// 独立密钥（seed 派生，确定性）。
fn agt_sk(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32].into()).expect("seed is a valid scalar")
}

/// craft 一个 TBS 被任意变异后的证书（签名在变异后盖上 → 结构自洽）。
fn agt_craft(
    subject_vk: &VerifyingKey,
    signer_sk: &SigningKey,
    issuer_ski: &[u8],
    eku: bool,
    mutate: impl FnOnce(&mut TbsCertificate),
) -> Certificate {
    let input = TbsInput {
        subject_cn: "AGT Crafted",
        subject_org: Some("NB Test"),
        issuer_cn: "AGT Issuer",
        issuer_org: Some("NB Test"),
        is_ca: false,
        path_len: None,
        ku_digital_signature: true,
        ku_key_cert_sign: false,
        ku_crl_sign: false,
        eku_code_signing: eku,
        not_before_unix: NOW,
        not_after_unix: FAR,
    };
    let mut tbs = build_tbs(subject_vk, issuer_ski, &random_serial(), &input).unwrap();
    mutate(&mut tbs);
    seal_certificate(tbs, signer_sk).unwrap()
}

fn agt_ca_input(cn: &'static str, issuer_cn: &'static str) -> TbsInput<'static> {
    TbsInput {
        subject_cn: cn,
        subject_org: Some("NB Test"),
        issuer_cn,
        issuer_org: Some("NB Test"),
        is_ca: true,
        path_len: None,
        ku_digital_signature: true,
        ku_key_cert_sign: true,
        ku_crl_sign: true,
        eku_code_signing: false,
        not_before_unix: NOW,
        not_after_unix: FAR,
    }
}

#[test]
fn chain_error_display_covers_all_variants() {
    assert!(
        ChainError::Malformed("boom".into())
            .to_string()
            .contains("boom")
    );
    assert_eq!(ChainError::Empty.to_string(), "empty certificate chain");
    assert_eq!(
        ChainError::UnsupportedAlgorithm.to_string(),
        "unsupported signature algorithm"
    );
    assert_eq!(ChainError::InvalidKey.to_string(), "invalid public key");
    assert_eq!(
        ChainError::BrokenChain.to_string(),
        "broken certificate chain"
    );
    assert_eq!(
        ChainError::NoRootForIssuer.to_string(),
        "no trusted root for issuer"
    );
    assert_eq!(ChainError::Expired.to_string(), "certificate expired");
    assert_eq!(
        ChainError::BadSignature.to_string(),
        "bad certificate signature"
    );
    assert_eq!(
        ChainError::MissingCodeSigningEku.to_string(),
        "missing codeSigning EKU"
    );
}

#[test]
fn subject_public_key_rejects_non_ec_missing_and_foreign_curve_params() {
    let root_sk = agt_sk(0x71);
    let leaf_sk = agt_sk(0x72);
    let root_vk = *root_sk.verifying_key();
    let leaf_vk = *leaf_sk.verifying_key();
    let root_ski = ski_value(&root_vk).unwrap();

    // SPKI 算法 OID 非 id-ecPublicKey → InvalidKey
    let bad_alg = agt_craft(&leaf_vk, &root_sk, &root_ski, true, |tbs| {
        tbs.subject_public_key_info.algorithm.oid = OID_ECDSA_WITH_SHA256;
    });
    assert!(matches!(
        bad_alg.subject_public_key(),
        Err(ChainError::InvalidKey)
    ));

    // parameters 缺失 → InvalidKey
    let no_params = agt_craft(&leaf_vk, &root_sk, &root_ski, true, |tbs| {
        tbs.subject_public_key_info.algorithm.parameters = None;
    });
    assert!(matches!(
        no_params.subject_public_key(),
        Err(ChainError::InvalidKey)
    ));

    // curve 参数非 P-256（P-384 OID）→ InvalidKey
    let p384_der = ObjectIdentifier::new_unwrap("1.3.132.0.34")
        .to_der()
        .unwrap();
    let foreign_curve = agt_craft(&leaf_vk, &root_sk, &root_ski, true, |tbs| {
        tbs.subject_public_key_info.algorithm.parameters = Some(Any::from_der(&p384_der).unwrap());
    });
    assert!(matches!(
        foreign_curve.subject_public_key(),
        Err(ChainError::InvalidKey)
    ));
}

#[test]
fn ski_and_aki_none_when_extensions_stripped() {
    let root_sk = agt_sk(0x73);
    let root_vk = *root_sk.verifying_key();
    let root_ski = ski_value(&root_vk).unwrap();

    // 剥 SKI → ski() == Ok(None)
    let no_ski = agt_craft(&root_vk, &root_sk, &root_ski, false, |tbs| {
        if let Some(exts) = tbs.extensions.as_mut() {
            exts.retain(|e| e.extn_id != OID_CE_SUBJECT_KEY_IDENTIFIER);
        }
    });
    assert_eq!(no_ski.ski().unwrap(), None);

    // 剥 AKI → aki() == Ok(None)；is_self_signed 走 (None, Some(_)) => true 臂
    //（AKI 缺省 = 自签惯例形态；签名照验通过）
    let no_aki = agt_craft(&root_vk, &root_sk, &root_ski, false, |tbs| {
        if let Some(exts) = tbs.extensions.as_mut() {
            exts.retain(|e| e.extn_id != OID_CE_AUTHORITY_KEY_IDENTIFIER);
        }
    });
    assert_eq!(no_aki.aki().unwrap(), None);
    assert!(matches!(no_aki.is_self_signed(), Ok(true)));

    // 两者皆缺 → `_ => false`（不可判定，不认）
    let no_both = agt_craft(&root_vk, &root_sk, &root_ski, false, |tbs| {
        if let Some(exts) = tbs.extensions.as_mut() {
            exts.retain(|e| {
                e.extn_id != OID_CE_SUBJECT_KEY_IDENTIFIER
                    && e.extn_id != OID_CE_AUTHORITY_KEY_IDENTIFIER
            });
        }
    });
    assert!(matches!(no_both.is_self_signed(), Ok(false)));
}

#[test]
fn verify_signature_by_rejects_unsupported_signature_algorithm() {
    let root_sk = agt_sk(0x74);
    let root_vk = *root_sk.verifying_key();
    let root = issue_x509(
        &root_vk,
        &root_sk,
        &ski_value(&root_vk).unwrap(),
        agt_ca_input("R", "R"),
    )
    .unwrap();
    assert!(root.signature_algorithm_ok());

    // signature_algorithm 在 TBS 外——改 OID 后重编码，证书结构仍可解析、
    // 签名字节不动，但算法不收 → UnsupportedAlgorithm
    let mut x = X509Certificate::from_der(root.to_der()).unwrap();
    x.signature_algorithm.oid = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
    let bad = Certificate::from_der(&x.to_der().unwrap()).unwrap();
    assert!(!bad.signature_algorithm_ok());
    let vk = bad.subject_public_key().unwrap();
    assert!(matches!(
        bad.verify_signature_by(&vk),
        Err(ChainError::UnsupportedAlgorithm)
    ));
}

#[test]
fn subject_cn_some_and_none() {
    let (chain, _, _, _) = fixture(NOW, FAR, true);
    assert!(chain[0].subject_cn().unwrap().is_some());

    // subject 只挂 O RDN（无 CN ATV）→ Ok(None)
    let root_sk = agt_sk(0x75);
    let leaf_sk = agt_sk(0x76);
    let root_vk = *root_sk.verifying_key();
    let leaf_vk = *leaf_sk.verifying_key();
    let o_only = agt_craft(
        &leaf_vk,
        &root_sk,
        &ski_value(&root_vk).unwrap(),
        true,
        |tbs| {
            tbs.subject = RdnSequence(vec![rd(OID_AT_O, "Org Only").unwrap()]);
        },
    );
    assert_eq!(o_only.subject_cn().unwrap(), None);
}

#[test]
fn ku_flagset_all_eight_combos() {
    let input = |ds: bool, kcs: bool, crl: bool| TbsInput {
        subject_cn: "KU",
        subject_org: None,
        issuer_cn: "KU",
        issuer_org: None,
        is_ca: false,
        path_len: None,
        ku_digital_signature: ds,
        ku_key_cert_sign: kcs,
        ku_crl_sign: crl,
        eku_code_signing: false,
        not_before_unix: NOW,
        not_after_unix: FAR,
    };
    for (ds, kcs, crl) in [
        (true, true, true),
        (true, true, false),
        (true, false, true),
        (true, false, false),
        (false, true, true),
        (false, true, false),
        (false, false, true),
    ] {
        assert!(
            ku_flagset(&input(ds, kcs, crl)).is_ok(),
            "ds={ds} kcs={kcs} crl={crl} 必须合法"
        );
    }
    // 全空 → profile 非法
    assert!(matches!(
        ku_flagset(&input(false, false, false)),
        Err(ChainError::Malformed(_))
    ));
}

#[test]
fn parse_serialize_chain_blob_roundtrip_and_all_error_arms() {
    let (chain, _, _, _) = fixture(NOW, FAR, true);

    // parse_chain：ok + 坏 DER 传播
    let ders: Vec<Vec<u8>> = chain.iter().map(|c| c.to_der().to_vec()).collect();
    assert_eq!(parse_chain(&ders).unwrap().len(), 3);
    assert!(parse_chain(&[b"junk".to_vec()]).is_err());

    // serialize_chain：空链只有计数头
    let empty = serialize_chain(&[]);
    assert_eq!(empty.len(), 2);
    assert_eq!(&empty[..2], &0u16.to_le_bytes());

    // 往返
    let blob = serialize_chain(&chain);
    assert_eq!(parse_chain_blob(&blob).unwrap().len(), 3);

    // too short / 缺 len header / cert span 越界 / 末张坏 DER
    assert!(matches!(
        parse_chain_blob(&[0u8]),
        Err(ChainError::Malformed(_))
    ));
    assert!(parse_chain_blob(&[1, 0]).is_err());
    let mut bad_span = vec![1u8, 0];
    bad_span.extend_from_slice(&999u32.to_le_bytes());
    assert!(parse_chain_blob(&bad_span).is_err());
    let mut bad_der = vec![1u8, 0];
    bad_der.extend_from_slice(&4u32.to_le_bytes());
    bad_der.extend_from_slice(b"junk");
    assert!(parse_chain_blob(&bad_der).is_err());
}

#[test]
fn verify_chain_tolerates_leaf_without_aki() {
    // RFC 5280：AKI 可省略。leaf 无 AKI → 匹配跳过、签名照验 → 链 Valid
    let root_sk = agt_sk(0x77);
    let issuing_sk = agt_sk(0x78);
    let leaf_sk = agt_sk(0x79);
    let root_vk = *root_sk.verifying_key();
    let issuing_vk = *issuing_sk.verifying_key();
    let leaf_vk = *leaf_sk.verifying_key();
    let root = issue_x509(
        &root_vk,
        &root_sk,
        &ski_value(&root_vk).unwrap(),
        agt_ca_input("R", "R"),
    )
    .unwrap();
    let issuing = issue_x509(
        &issuing_vk,
        &root_sk,
        &ski_value(&root_vk).unwrap(),
        agt_ca_input("I", "R"),
    )
    .unwrap();
    let leaf = agt_craft(
        &leaf_vk,
        &issuing_sk,
        &ski_value(&issuing_vk).unwrap(),
        true,
        |tbs| {
            if let Some(exts) = tbs.extensions.as_mut() {
                exts.retain(|e| e.extn_id != OID_CE_AUTHORITY_KEY_IDENTIFIER);
            }
        },
    );
    let chain = vec![leaf, issuing, root];
    let anchor = chain[2].sha256_fingerprint();
    assert!(verify_chain(&chain, &anchor, NOW).is_ok());
}
