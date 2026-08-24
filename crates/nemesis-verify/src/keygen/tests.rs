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
