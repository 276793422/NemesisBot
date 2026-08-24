use super::*;
use crate::keygen::generate_hierarchy;
use crate::verify;

#[test]
fn list_single_signature() {
    let h = generate_hierarchy(0, u64::MAX);
    let signed = verify::sign_content(
        b"view-test-payload",
        &h.issuer_sk,
        12345,
        Some(&h.issuer_chain_bytes),
        None,
        None,
        None,
    )
    .unwrap();
    let list = list_signatures(&signed);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].signed_at, 12345);
    assert_eq!(list[0].pubkey, h.issuer_vk.to_bytes());
}

#[test]
fn detail_includes_cert_chain() {
    let h = generate_hierarchy(0, u64::MAX);
    let signed = verify::sign_content(
        b"detail-test",
        &h.issuer_sk,
        999,
        Some(&h.issuer_chain_bytes),
        None,
        None,
        None,
    )
    .unwrap();
    let detail = get_signature_detail(&signed, 0).unwrap();
    // chain = [issuer_cert, ca_cert]（2 级）
    assert_eq!(detail.certs.len(), 2);
    assert_eq!(detail.certs[0].subject_pubkey, h.issuer_vk.to_bytes()); // leaf = issuer
    assert_eq!(detail.certs[1].subject_pubkey, h.ca_vk.to_bytes()); // intermediate = CA
}

#[test]
fn no_signature_empty_list() {
    let list = list_signatures(b"plain bytes no signature");
    assert!(list.is_empty());
}

#[test]
fn latest_sig_hash_extracted() {
    let h = generate_hierarchy(0, u64::MAX);
    let signed = verify::sign_content(
        b"sig-hash-test",
        &h.issuer_sk,
        111,
        Some(&h.issuer_chain_bytes),
        None,
        None,
        None,
    )
    .unwrap();
    let sig_hash = latest_sig_hash(&signed).unwrap();
    // build_body 时填入 SHA-256(signature)，非全 0
    assert_ne!(sig_hash, [0u8; 32]);
    // 无签名文件 → None
    assert!(latest_sig_hash(b"no signature here").is_none());
}
