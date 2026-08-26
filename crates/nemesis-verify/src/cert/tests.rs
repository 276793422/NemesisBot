use super::*;

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key();
    (sk, vk)
}

const NOW: u64 = 5_000_000;
const VALID_FROM: u64 = 0;
const VALID_TO: u64 = u64::MAX;

#[test]
fn cert_serialize_roundtrip() {
    let (sk, vk) = keypair(1);
    let cert = issue_certificate(&sk, &vk.to_bytes(), b"root", VALID_FROM, VALID_TO);
    let bytes = cert.to_bytes();
    let back = Certificate::from_bytes(&bytes).unwrap();
    assert_eq!(back, cert);
}

#[test]
fn single_level_chain_root_signs_leaf() {
    // 根直接签发行方(leaf)
    let (root_sk, root_vk) = keypair(1);
    let (leaf_sk, leaf_vk) = keypair(2);
    let leaf_cert = issue_certificate(
        &root_sk,
        &leaf_vk.to_bytes(),
        b"issuer-A",
        VALID_FROM,
        VALID_TO,
    );
    let chain = serialize_chain(&[leaf_cert]);
    // verify_chain: leaf_pubkey = leaf_vk, chain, root_pubs = [root_vk]
    assert!(
        verify_chain(
            &leaf_vk.to_bytes(),
            &parse_chain(&chain).unwrap(),
            &[root_vk],
            NOW
        )
        .is_ok()
    );
    // leaf_sk 能签 exe，envelope.pubkey = leaf_vk，链到 root_vk
    let _ = leaf_sk;
}

#[test]
fn two_level_chain_via_intermediate() {
    // root → intermediate → leaf
    let (root_sk, root_vk) = keypair(1);
    let (inter_sk, inter_vk) = keypair(2);
    let (leaf_sk, leaf_vk) = keypair(3);
    let inter_cert = issue_certificate(
        &root_sk,
        &inter_vk.to_bytes(),
        b"intermediate",
        VALID_FROM,
        VALID_TO,
    );
    let leaf_cert = issue_certificate(
        &inter_sk,
        &leaf_vk.to_bytes(),
        b"issuer-A",
        VALID_FROM,
        VALID_TO,
    );
    // chain = [leaf_cert, inter_cert]（leaf 在前，不含 root_cert）
    let chain = serialize_chain(&[leaf_cert, inter_cert]);
    assert!(
        verify_chain(
            &leaf_vk.to_bytes(),
            &parse_chain(&chain).unwrap(),
            &[root_vk],
            NOW
        )
        .is_ok()
    );
    let _ = leaf_sk;
}

#[test]
fn missing_intermediate_rejected() {
    // root → inter → leaf，但 chain 只含 leaf_cert（缺 inter_cert）
    // → leaf_cert.issuer_key_fp = inter fp，root_pubs 只有 root → NoRootForIssuer
    let (_root_sk, root_vk) = keypair(1); // root_sk 不参与签发（leaf 由 inter 签）
    let (inter_sk, _inter_vk) = keypair(2); // inter_sk 签 leaf；inter_vk 此处不用
    let (_, leaf_vk) = keypair(3);
    let leaf_cert = issue_certificate(
        &inter_sk,
        &leaf_vk.to_bytes(),
        b"issuer-A",
        VALID_FROM,
        VALID_TO,
    );
    let chain = serialize_chain(&[leaf_cert]);
    let err = verify_chain(
        &leaf_vk.to_bytes(),
        &parse_chain(&chain).unwrap(),
        &[root_vk],
        NOW,
    )
    .unwrap_err();
    assert_eq!(err, ChainError::NoRootForIssuer);
}

#[test]
fn wrong_root_rejected() {
    let (root_sk, _root_vk) = keypair(1);
    let (_, other_root_vk) = keypair(9);
    let (_, leaf_vk) = keypair(2);
    let leaf_cert = issue_certificate(
        &root_sk,
        &leaf_vk.to_bytes(),
        b"issuer-A",
        VALID_FROM,
        VALID_TO,
    );
    let chain = serialize_chain(&[leaf_cert]);
    // 用另一把 root 验 → NoRootForIssuer
    let err = verify_chain(
        &leaf_vk.to_bytes(),
        &parse_chain(&chain).unwrap(),
        &[other_root_vk],
        NOW,
    )
    .unwrap_err();
    assert_eq!(err, ChainError::NoRootForIssuer);
}

#[test]
fn leaf_mismatch_rejected() {
    let (root_sk, root_vk) = keypair(1);
    let (_, leaf_vk) = keypair(2);
    let (_, other_vk) = keypair(5);
    let leaf_cert = issue_certificate(
        &root_sk,
        &leaf_vk.to_bytes(),
        b"issuer-A",
        VALID_FROM,
        VALID_TO,
    );
    let chain = serialize_chain(&[leaf_cert]);
    // envelope.pubkey = other_vk，但 leaf_cert.subject = leaf_vk → LeafMismatch
    let err = verify_chain(
        &other_vk.to_bytes(),
        &parse_chain(&chain).unwrap(),
        &[root_vk],
        NOW,
    )
    .unwrap_err();
    assert_eq!(err, ChainError::LeafMismatch);
}

#[test]
fn expired_rejected() {
    let (root_sk, root_vk) = keypair(1);
    let (_, leaf_vk) = keypair(2);
    // 有效期 [100, 200]，now = 500 → Expired
    let leaf_cert = issue_certificate(&root_sk, &leaf_vk.to_bytes(), b"issuer-A", 100, 200);
    let chain = serialize_chain(&[leaf_cert]);
    let err = verify_chain(
        &leaf_vk.to_bytes(),
        &parse_chain(&chain).unwrap(),
        &[root_vk],
        500,
    )
    .unwrap_err();
    assert_eq!(err, ChainError::Expired);
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：解析错误臂 + 剩余
// ChainError 变体（Empty / BrokenChain / BadSignature）+ not_before 侧过期。
// ---------------------------------------------------------------------------

#[test]
fn not_before_in_future_rejected() {
    let (root_sk, root_vk) = keypair(1);
    let (_, leaf_vk) = keypair(2);
    // 有效期 [10000, 20000]，now = 5000 < not_before → Expired（另一侧）
    let leaf_cert = issue_certificate(&root_sk, &leaf_vk.to_bytes(), b"issuer-A", 10000, 20000);
    let chain = serialize_chain(&[leaf_cert]);
    let err = verify_chain(
        &leaf_vk.to_bytes(),
        &parse_chain(&chain).unwrap(),
        &[root_vk],
        5000,
    )
    .unwrap_err();
    assert_eq!(err, ChainError::Expired);
}

#[test]
fn verify_chain_empty_rejected() {
    let (_, vk) = keypair(1);
    assert_eq!(
        verify_chain(&vk.to_bytes(), &[], &[], NOW),
        Err(ChainError::Empty)
    );
}

#[test]
fn broken_chain_fingerprint_mismatch_rejected() {
    // leaf 由 interA 签，但链里放的是 interB 的证书 → chain[1].subject 指纹
    // ≠ chain[0].issuer_key_fp → BrokenChain。
    let (inter_a_sk, _) = keypair(2);
    let (inter_b_sk, _) = keypair(7);
    let (root_sk, root_vk) = keypair(1);
    let (_, leaf_vk) = keypair(3);
    let leaf_cert = issue_certificate(
        &inter_a_sk,
        &leaf_vk.to_bytes(),
        b"issuer-A",
        VALID_FROM,
        VALID_TO,
    );
    let inter_b_cert = issue_certificate(
        &root_sk,
        &inter_b_sk.verifying_key().to_bytes(),
        b"inter-B",
        VALID_FROM,
        VALID_TO,
    );
    let chain = serialize_chain(&[leaf_cert, inter_b_cert]);
    let err = verify_chain(
        &leaf_vk.to_bytes(),
        &parse_chain(&chain).unwrap(),
        &[root_vk],
        NOW,
    )
    .unwrap_err();
    assert_eq!(err, ChainError::BrokenChain);
}

#[test]
fn tampered_cert_signature_rejected() {
    let (root_sk, root_vk) = keypair(1);
    let (_, leaf_vk) = keypair(2);
    let mut leaf_cert =
        issue_certificate(&root_sk, &leaf_vk.to_bytes(), b"issuer-A", VALID_FROM, VALID_TO);
    leaf_cert.signature[0] ^= 0x01; // 篡改签名
    let chain = serialize_chain(&[leaf_cert]);
    let err = verify_chain(
        &leaf_vk.to_bytes(),
        &parse_chain(&chain).unwrap(),
        &[root_vk],
        NOW,
    )
    .unwrap_err();
    assert_eq!(err, ChainError::BadSignature);
}

#[test]
fn cert_from_bytes_too_short_errors() {
    let err = Certificate::from_bytes(&[0u8; 145]).unwrap_err();
    assert!(format!("{err:#}").contains("cert too short"), "{err:#}");
}

#[test]
fn cert_from_bytes_meta_len_overrun_errors() {
    let (sk, vk) = keypair(1);
    let cert = issue_certificate(&sk, &vk.to_bytes(), b"meta", VALID_FROM, VALID_TO);
    let mut bytes = cert.to_bytes();
    // 把 meta_len 改成 60000（声明远超实际长度）→ 截断错误。
    bytes[80..82].copy_from_slice(&60000u16.to_le_bytes());
    let err = Certificate::from_bytes(&bytes).unwrap_err();
    assert!(format!("{err:#}").contains("cert truncated"), "{err:#}");
}

#[test]
fn parse_chain_truncation_errors() {
    let (root_sk, _) = keypair(1);
    let (_, leaf_vk) = keypair(2);
    let cert = issue_certificate(&root_sk, &leaf_vk.to_bytes(), b"m", VALID_FROM, VALID_TO);
    let chain = serialize_chain(&[cert.clone()]);

    // < 2 字节 → "chain too short for count"
    let err = parse_chain(&[0u8]).unwrap_err();
    assert!(format!("{err:#}").contains("chain too short"), "{err:#}");

    // 只留 count + 2 字节（cert_len 被截）→ "chain truncated at cert_len"
    let short_len = &chain[..4];
    let err = parse_chain(short_len).unwrap_err();
    assert!(format!("{err:#}").contains("truncated at cert_len"), "{err:#}");

    // count 声明 1，cert_len 完整但 cert bytes 被截 → "truncated at cert bytes"
    let mut truncated = chain[..2 + 4].to_vec(); // count + cert_len
    truncated.extend_from_slice(&cert.to_bytes()[..100]); // 只给 100B cert
    let err = parse_chain(&truncated).unwrap_err();
    assert!(format!("{err:#}").contains("truncated at cert bytes"), "{err:#}");

    // 正常全长 → Ok
    assert_eq!(parse_chain(&chain).unwrap().len(), 1);
}
