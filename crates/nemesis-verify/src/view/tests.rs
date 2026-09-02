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

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：crafted footer 的
// total_len / body_len 超界不再 panic（BUG S6-1 的 view 路径回归钉）。
// view.rs 三处 `&bytes[bs..be]` 是直接切片（非 .get()）：修复前 crafted
// total_len 在 release 也会切片越界 panic——恶意文件可崩掉调用方进程
// （nv_list_signatures 是跨进程 C ABI）。
// ---------------------------------------------------------------------------

/// IEEE 802.3 CRC32（envelope 私有实现的镜像，与 verify/tests.rs 同款）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn patch_footer_crc(file: &mut [u8], footer_off: usize) {
    let crc = crc32(&file[footer_off..footer_off + 24]);
    file[footer_off + 24..footer_off + 28].copy_from_slice(&crc.to_le_bytes());
}

fn sign_small() -> Vec<u8> {
    let sk = ed25519_dalek::SigningKey::from_bytes(&[77u8; 32]);
    verify::sign_content(b"view crafted target", &sk, 1000, None, None, None, None).unwrap()
}

#[test]
fn crafted_total_len_does_not_panic_in_view() {
    let mut signed = sign_small();
    let fo = signed.len() - crate::envelope::FOOTER_LEN;
    // total_len = 0xFFFFFFF0 >> footer_offset+FOOTER_LEN → 修复前：
    // envelope_body_range 下溢（debug）→ view 切片 panic（release）
    signed[fo + 12..fo + 16].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    patch_footer_crc(&mut signed, fo);

    // 三个入口都必须优雅失败（空列表 / None / None），绝不 panic
    assert!(list_signatures(&signed).is_empty());
    assert!(get_signature_detail(&signed, 0).is_none());
    assert!(latest_sig_hash(&signed).is_none());
}

#[test]
fn crafted_body_len_does_not_panic_in_view() {
    let mut signed = sign_small();
    let fo = signed.len() - crate::envelope::FOOTER_LEN;
    // body_len = 0xFFFFFFFF（range end = start + 4G，必越界）→ 修复前 view 切片 panic
    signed[fo + 16..fo + 20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    patch_footer_crc(&mut signed, fo);

    assert!(list_signatures(&signed).is_empty());
    assert!(get_signature_detail(&signed, 0).is_none());
    assert!(latest_sig_hash(&signed).is_none());
}
