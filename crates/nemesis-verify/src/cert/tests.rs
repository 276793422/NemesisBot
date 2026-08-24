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
    let (root_sk, root_vk) = keypair(1);
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
