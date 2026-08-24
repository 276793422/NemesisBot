use super::*;

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key();
    (sk, vk)
}

#[test]
fn sign_verify_raw_valid() {
    let (sk, vk) = keypair(7);
    let content = b"hello nemesis verify v3 payload";
    let signed = sign_content(content, &sk, 1000, None, None, None, None).unwrap();
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Valid {
            signed_at,
            key_fp,
            pubkey,
        } => {
            assert_eq!(signed_at, 1000);
            assert_eq!(pubkey, vk.to_bytes());
            let expected_fp: [u8; 32] = sha2::Sha256::digest(&vk.to_bytes()).into();
            assert_eq!(key_fp, expected_fp);
        }
        o => panic!("expected Valid, got {:?}", o),
    }
}

#[test]
fn sign_with_chain_valid() {
    // root 签 leaf cert；leaf_sk 签 content（envelope 带 leaf pubkey + chain=[leaf_cert]）
    let (root_sk, root_vk) = keypair(1);
    let (leaf_sk, leaf_vk) = keypair(2);
    let leaf_cert =
        cert::issue_certificate(&root_sk, &leaf_vk.to_bytes(), b"issuer-A", 0, u64::MAX);
    let chain = cert::serialize_chain(&[leaf_cert]);
    let signed = sign_content(
        b"signed with cert chain",
        &leaf_sk,
        1000,
        Some(&chain),
        None,
        None,
        None,
    )
    .unwrap();
    // verify: root_pubs=[root_vk]（不含 leaf_vk）；可信靠链 leaf→root
    match verify_bytes(&signed, &[root_vk], 1000) {
        VerifyOutcome::Valid { pubkey, .. } => assert_eq!(pubkey, leaf_vk.to_bytes()),
        o => panic!("expected Valid, got {:?}", o),
    }
}

#[test]
fn sign_with_chain_wrong_root_untrusted() {
    // 链到 root1，但验证端只信任 root2 → Untrusted
    let (root1_sk, _) = keypair(1);
    let (_, root2_vk) = keypair(9);
    let (leaf_sk, leaf_vk) = keypair(2);
    let leaf_cert =
        cert::issue_certificate(&root1_sk, &leaf_vk.to_bytes(), b"issuer-A", 0, u64::MAX);
    let chain = cert::serialize_chain(&[leaf_cert]);
    let signed = sign_content(
        b"chain to root1",
        &leaf_sk,
        1000,
        Some(&chain),
        None,
        None,
        None,
    )
    .unwrap();
    match verify_bytes(&signed, &[root2_vk], 1000) {
        VerifyOutcome::Untrusted => {}
        o => panic!("expected Untrusted, got {:?}", o),
    }
}

#[test]
fn tampered_content_detected() {
    let (sk, vk) = keypair(7);
    let mut signed =
        sign_content(b"original content", &sk, 1000, None, None, None, None).unwrap();
    signed[5] ^= 0xFF; // 篡改 content 区
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Tampered(_) => {}
        o => panic!("expected Tampered, got {:?}", o),
    }
}

#[test]
fn no_signature() {
    let (_, vk) = keypair(7);
    let plain = b"just some bytes no signature here";
    assert_eq!(verify_bytes(plain, &[vk], 1000), VerifyOutcome::NoSignature);
}

#[test]
fn untrusted_pubkey_rejected() {
    let (sk, _) = keypair(7);
    let (_, vk_other) = keypair(9);
    // 用 sk7 签（无链），只信任 vk9 → Untrusted
    let signed = sign_content(b"signed by sk7", &sk, 1000, None, None, None, None).unwrap();
    match verify_bytes(&signed, &[vk_other], 1000) {
        VerifyOutcome::Untrusted => {}
        o => panic!("expected Untrusted, got {:?}", o),
    }
}

#[test]
fn signature_tamper_detected() {
    let (sk7, _) = keypair(7);
    let mut signed =
        sign_content(b"sig tamper test", &sk7, 1000, None, None, None, None).unwrap();
    let sig_byte_off = 15 + 108; // body 偏移 108（envelope::BODY_OFF_SIG）；content_len=15
    signed[sig_byte_off] ^= 0xFF;
    match verify_bytes(&signed, &[sk7.verifying_key()], 1000) {
        VerifyOutcome::SignatureInvalid => {}
        o => panic!("expected SignatureInvalid, got {:?}", o),
    }
}
