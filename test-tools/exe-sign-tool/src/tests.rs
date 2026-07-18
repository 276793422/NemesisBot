//! 单元测试：合成 PE/ELF fixture + 全用例覆盖（含 C3 本地吊销）。

use crate::codec::{CodecError, ElfCodec, ExecutableCodec, PeCodec};
use crate::status::Code;
use crate::{crypto, envelope, policy::RevocationPolicy, sign_executable, verify_executable};
use revoke_common::{Crl, CrlEntry, KeyStatus, RevDim, TrustedKey, TrustedKeyList};
use sha2::{Digest, Sha256};

/// 测试用固定 ChaCha20 对称密钥。
const TEST_SYM_KEY: [u8; 32] = [0x42u8; 32];

/// 默认策略（offline，now=0 不判过期；不查 CRL/trusted-keys）。
fn policy() -> RevocationPolicy<'static> {
    RevocationPolicy::offline(0)
}

// ---------- 字节写入辅助 ----------
fn put_u16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

// ---------- PE/ELF fixture（同前）----------
fn build_pe(payload: &[u8], magic: u16) -> Vec<u8> {
    let is_plus = magic == 0x20b;
    let opt_size = if is_plus { 240 } else { 224 };
    let opt_off = 0x58usize;
    let sec_hdr_off = opt_off + opt_size;
    let payload_off = sec_hdr_off + 40;
    let total = payload_off + payload.len();
    let mut b = vec![0u8; total];
    b[0..2].copy_from_slice(b"MZ");
    put_u32(&mut b, 0x3C, 0x40);
    b[0x40..0x44].copy_from_slice(b"PE\0\0");
    put_u16(&mut b, 0x44, 0x14c);
    put_u16(&mut b, 0x46, 1);
    put_u32(&mut b, 0x48, 0xDEAD_BEEF);
    put_u16(&mut b, 0x54, opt_size as u16);
    put_u16(&mut b, opt_off, magic);
    put_u32(&mut b, opt_off + 64, 0x1234_5678);
    let nrva_off = if is_plus { opt_off + 108 } else { opt_off + 92 };
    put_u32(&mut b, nrva_off, 16);
    put_u32(&mut b, sec_hdr_off + 16, payload.len() as u32);
    put_u32(&mut b, sec_hdr_off + 20, payload_off as u32);
    b[payload_off..].copy_from_slice(payload);
    b
}
fn build_minimal_pe32(p: &[u8]) -> Vec<u8> { build_pe(p, 0x10b) }
fn build_minimal_pe32plus(p: &[u8]) -> Vec<u8> { build_pe(p, 0x20b) }
const PE32_DD4_OFF: usize = 0x58 + 128;
const PE32_CHECKSUM_OFF: usize = 0x58 + 64;

fn build_elf(payload: &[u8], is64: bool) -> Vec<u8> {
    let ehsize = if is64 { 64 } else { 52 };
    let phentsize = if is64 { 56 } else { 32 };
    let phoff = ehsize;
    let payload_off = phoff + phentsize;
    let total = payload_off + payload.len();
    let mut b = vec![0u8; total];
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = if is64 { 2 } else { 1 };
    b[5] = 1;
    b[6] = 1;
    put_u16(&mut b, 16, 2);
    if is64 { put_u64(&mut b, 32, phoff as u64) } else { put_u32(&mut b, 28, phoff as u32) }
    put_u16(&mut b, if is64 { 52 } else { 40 }, ehsize as u16);
    put_u16(&mut b, if is64 { 54 } else { 42 }, phentsize as u16);
    put_u16(&mut b, if is64 { 56 } else { 44 }, 1);
    put_u16(&mut b, if is64 { 58 } else { 46 }, if is64 { 64 } else { 40 });
    put_u32(&mut b, phoff, 1);
    if is64 {
        put_u64(&mut b, phoff + 8, payload_off as u64);
        put_u64(&mut b, phoff + 32, payload.len() as u64);
    } else {
        put_u32(&mut b, phoff + 4, payload_off as u32);
        put_u32(&mut b, phoff + 16, payload.len() as u32);
    }
    b[payload_off..].copy_from_slice(payload);
    b
}
fn build_minimal_elf32(p: &[u8]) -> Vec<u8> { build_elf(p, false) }
fn build_minimal_elf64(p: &[u8]) -> Vec<u8> { build_elf(p, true) }

fn temp_sign(buf: &[u8]) -> (std::path::PathBuf, tempfile::TempDir, crypto::KeyPair) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.bin");
    std::fs::write(&path, buf).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1_700_000_000, 7, None, None).unwrap();
    (path, dir, kp)
}
fn vk_of(kp: &crypto::KeyPair) -> ed25519_dalek::VerifyingKey {
    crypto::verifying_key_from_hex(&kp.public_key).unwrap()
}
fn key_fp_hex(kp: &crypto::KeyPair) -> String {
    let vk = vk_of(kp);
    let fp: [u8; 32] = Sha256::digest(vk.to_bytes().as_ref()).into();
    fp.iter().map(|b| format!("{:02x}", b)).collect()
}

// ===================== 基础用例 =====================

#[test]
fn domain_len() {
    assert_eq!(envelope::DOMAIN.len(), 42);
}

#[test]
fn roundtrip_all_formats() {
    for buf in [
        build_minimal_pe32(b"PE32-PAYLOAD-XYZ123"),
        build_minimal_pe32plus(b"PE32PLUS-PAYLOAD-XYZ"),
        build_minimal_elf32(b"ELF32-PAYLOAD-XYZ12"),
        build_minimal_elf64(b"ELF64-PAYLOAD-XYZ12"),
        b"RAW-FIRMWARE-BLOB-1234567890".to_vec(),
    ] {
        let (path, _d, kp) = temp_sign(&buf);
        let signed = std::fs::read(&path).unwrap();
        let out = verify_executable(&signed, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
        assert!(matches!(out.code, Code::Valid), "roundtrip: {:?}", out);
    }
}

#[test]
fn tamper_content_detected() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-PAYLOAD-TAMPER"));
    let mut b = std::fs::read(&path).unwrap();
    b[0x160] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::BadDigest | Code::SignatureInvalid), "{:?}", out);
}

#[test]
fn checksum_excluded() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-CHECKSUM-TEST"));
    let mut b = std::fs::read(&path).unwrap();
    b[PE32_CHECKSUM_OFF] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::Valid), "{:?}", out);
}

#[test]
fn security_dir_excluded() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-SEC-DIR-TEST"));
    let mut b = std::fs::read(&path).unwrap();
    b[PE32_DD4_OFF] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::Valid), "{:?}", out);
}

#[test]
fn pe_authenticode_cross_check() {
    let mut b = build_minimal_pe32(b"PE32-CROSSCHECK");
    put_u32(&mut b, PE32_DD4_OFF, 10);
    put_u32(&mut b, PE32_DD4_OFF + 4, 5);
    let res = PeCodec.compute_l(&b);
    assert!(matches!(res, Err(CodecError::Malformed(_))), "{:?}", res);
}

#[test]
fn elf_l_with_section_header_table() {
    let payload = b"ELF32-SHT-TEST";
    let phoff = 52usize;
    let phentsize = 32usize;
    let payload_off = phoff + phentsize;
    let sht_off = payload_off + payload.len();
    let sht_size = 40;
    let total = sht_off + sht_size;
    let mut b = vec![0u8; total];
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = 1; b[5] = 1; b[6] = 1;
    put_u16(&mut b, 16, 2);
    put_u32(&mut b, 28, phoff as u32);
    put_u32(&mut b, 32, sht_off as u32);
    put_u16(&mut b, 40, 52);
    put_u16(&mut b, 42, phentsize as u16);
    put_u16(&mut b, 44, 1);
    put_u16(&mut b, 46, 40);
    put_u16(&mut b, 48, 1);
    put_u32(&mut b, phoff, 1);
    put_u32(&mut b, phoff + 4, payload_off as u32);
    put_u32(&mut b, phoff + 16, payload.len() as u32);
    b[payload_off..payload_off + payload.len()].copy_from_slice(payload);
    let l = ElfCodec.compute_l(&b).unwrap().unwrap();
    assert_eq!(l, sht_off + sht_size);
}

#[test]
fn authenticode_plus_ours_overlay() {
    let payload = b"PE32-WITH-AUTH";
    let mut b = build_minimal_pe32(payload);
    let cert = b"FAKE-AUTHENTICODE-SIGNATURE-DATA";
    let cert_va = b.len();
    b.extend_from_slice(cert);
    put_u32(&mut b, PE32_DD4_OFF, cert_va as u32);
    put_u32(&mut b, PE32_DD4_OFF + 4, cert.len() as u32);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.exe");
    std::fs::write(&path, &b).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1_700_000_000, 1, None, None).unwrap();
    let signed = std::fs::read(&path).unwrap();
    assert!(matches!(
        verify_executable(&signed, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap().code,
        Code::Valid
    ));
    let mut b2 = signed.clone();
    b2[cert_va] ^= 0xFF;
    let out = verify_executable(&b2, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::Valid), "cert region change: {:?}", out);
}

#[test]
fn aead_tamper_detected() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-AEAD-TAMPER"));
    let mut b = std::fs::read(&path).unwrap();
    let foff = b.len() - envelope::FOOTER_LEN;
    let total_len = u32::from_le_bytes([b[foff + 12], b[foff + 13], b[foff + 14], b[foff + 15]]) as usize;
    let envelope_start = b.len() - total_len;
    b[envelope_start] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::BadDigest), "{:?}", out);
}

#[test]
fn no_signature() {
    let b = build_minimal_pe32(b"PE32-UNSIGNED");
    let kp = crypto::generate_key_pair();
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::NoSignature), "{:?}", out);
}

#[test]
fn wrong_key_rejected() {
    let (path, _d, _kp) = temp_sign(&build_minimal_pe32(b"PE32-WRONGKEY"));
    let b = std::fs::read(&path).unwrap();
    let other = crypto::generate_key_pair();
    let out = verify_executable(&b, &vk_of(&other), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::SignatureInvalid), "{:?}", out);
}

#[test]
fn double_sign_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.bin");
    std::fs::write(&path, &build_minimal_pe32(b"PE32-DOUBLE")).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1, 1, None, None).unwrap();
    let res = sign_executable(&path, &sk, &TEST_SYM_KEY, 2, 2, None, None);
    assert!(res.is_err());
}

#[test]
fn footer_tamper_detected() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-FOOTER-TAMPER"));
    let mut b = std::fs::read(&path).unwrap();
    let crc_byte = b.len() - envelope::FOOTER_LEN + 36;
    b[crc_byte] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::BadDigest), "{:?}", out);
}

#[test]
fn envelope_4kb_aligned() {
    let (path, _d, _kp) = temp_sign(&build_minimal_pe32(b"PE32-ALIGN"));
    let b = std::fs::read(&path).unwrap();
    let foff = b.len() - envelope::FOOTER_LEN;
    let total_len = u32::from_le_bytes([b[foff + 12], b[foff + 13], b[foff + 14], b[foff + 15]]) as usize;
    assert_eq!(total_len % envelope::ENVELOPE_ALIGN, 0);
}

#[test]
fn pe32plus_security_dir_excluded() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32plus(b"PE32PLUS-SEC-DIR"));
    let mut b = std::fs::read(&path).unwrap();
    b[0x58 + 144] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::Valid), "{:?}", out);
}

#[test]
fn pe32plus_checksum_excluded() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32plus(b"PE32PLUS-CKSUM"));
    let mut b = std::fs::read(&path).unwrap();
    b[PE32_CHECKSUM_OFF] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::Valid), "{:?}", out);
}

#[test]
fn elf64_tamper_content() {
    let (path, _d, kp) = temp_sign(&build_minimal_elf64(b"ELF64-TAMPER-CONTENT"));
    let mut b = std::fs::read(&path).unwrap();
    b[120] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::BadDigest | Code::SignatureInvalid), "{:?}", out);
}

#[test]
fn raw_no_signature() {
    let kp = crypto::generate_key_pair();
    let raw = b"RAW-UNSIGNED-BLOB-12345";
    let out = verify_executable(raw, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::NoSignature), "{:?}", out);
}

#[test]
fn raw_tamper_and_wrong_key() {
    let (path, _d, kp) = temp_sign(b"RAW-TAMPER-BLOB-1234567890");
    let mut b = std::fs::read(&path).unwrap();
    b[5] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::BadDigest | Code::SignatureInvalid), "{:?}", out);
    let other = crypto::generate_key_pair();
    let signed = std::fs::read(&path).unwrap();
    let out2 = verify_executable(&signed, &vk_of(&other), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out2.code, Code::SignatureInvalid), "{:?}", out2);
}

#[test]
fn generate_sym_key_is_unique_and_32() {
    let a = crypto::generate_sym_key();
    let b = crypto::generate_sym_key();
    assert_eq!(a.len(), 32);
    assert_ne!(a, b);
    assert_ne!(a, crypto::BUILTIN_SYM_KEY_DEFAULT);
}

#[test]
fn wrong_sym_key_rejected() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-WRONG-SYM-KEY"));
    let b = std::fs::read(&path).unwrap();
    let wrong_sym = [0x99u8; 32];
    let out = verify_executable(&b, &vk_of(&kp), &wrong_sym, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::BadDigest), "{:?}", out);
}

#[test]
fn publisher_expires_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pub.bin");
    std::fs::write(&path, &build_minimal_pe32(b"PE32-PUBLISHER-TEST")).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1_700_000_000, 1, Some("NemesisBot"), Some(1_800_000_000)).unwrap();
    let b = std::fs::read(&path).unwrap();
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy(), None).unwrap();
    assert!(matches!(out.code, Code::Valid), "{:?}", out);
}

// ===================== C3 本地吊销用例 =====================

#[test]
fn crl_revokes_by_key_fp() {
    // CRL 含签名者 key_fp → verify 命中 Revoked
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-CRL-REVOKE"));
    let b = std::fs::read(&path).unwrap();
    let crl = Crl {
        version: 1,
        valid_until: u64::MAX,
        entries: vec![CrlEntry {
            dim: RevDim::KeyId,
            value: key_fp_hex(&kp),
            revoked_at: 1,
            reason: "key_leak".into(),
        }],
    };
    let policy = RevocationPolicy { now: 0, crl: Some(&crl), trusted_keys: None };
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy, None).unwrap();
    assert!(matches!(out.code, Code::Revoked), "{:?}", out);
}

#[test]
fn crl_revokes_by_publisher() {
    // 按 publisher 维度吊销
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pub2.bin");
    std::fs::write(&path, &build_minimal_pe32(b"PE32-CRL-PUB")).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1_700_000_000, 1, Some("EvilCorp"), None).unwrap();
    let b = std::fs::read(&path).unwrap();
    let crl = Crl {
        version: 1,
        valid_until: u64::MAX,
        entries: vec![CrlEntry {
            dim: RevDim::Publisher,
            value: "EvilCorp".into(),
            revoked_at: 1,
            reason: "bad_publisher".into(),
        }],
    };
    let policy = RevocationPolicy { now: 0, crl: Some(&crl), trusted_keys: None };
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy, None).unwrap();
    assert!(matches!(out.code, Code::Revoked), "{:?}", out);
}

#[test]
fn expired_detected() {
    // expires_at 在过去，policy.now 超过 → Expired
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exp.bin");
    std::fs::write(&path, &build_minimal_pe32(b"PE32-EXP")).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1000, 1, None, Some(2000)).unwrap();
    let b = std::fs::read(&path).unwrap();
    let policy = RevocationPolicy::offline(3000); // now=3000 > expires_at=2000
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy, None).unwrap();
    assert!(matches!(out.code, Code::Expired), "{:?}", out);
}

#[test]
fn untrusted_publisher_detected() {
    // trusted_keys 不含签名者 → UntrustedPublisher
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-UNTRUSTED"));
    let b = std::fs::read(&path).unwrap();
    let tkl = TrustedKeyList {
        version: 1,
        valid_until: u64::MAX,
        keys: vec![TrustedKey {
            key_fp: "00".repeat(32), // 不是签名者
            status: KeyStatus::Active,
            not_after: None,
        }],
    };
    let policy = RevocationPolicy { now: 0, crl: None, trusted_keys: Some(&tkl) };
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy, None).unwrap();
    assert!(matches!(out.code, Code::UntrustedPublisher), "{:?}", out);
}

#[test]
fn revoked_takes_priority_over_expired() {
    // 既过期又在 CRL → Revoked（优先）
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("both.bin");
    std::fs::write(&path, &build_minimal_pe32(b"PE32-BOTH")).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1000, 1, None, Some(2000)).unwrap();
    let b = std::fs::read(&path).unwrap();
    let crl = Crl {
        version: 1,
        valid_until: u64::MAX,
        entries: vec![CrlEntry {
            dim: RevDim::KeyId,
            value: key_fp_hex(&kp),
            revoked_at: 1,
            reason: "leak".into(),
        }],
    };
    let policy = RevocationPolicy { now: 3000, crl: Some(&crl), trusted_keys: None };
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY, &policy, None).unwrap();
    assert!(matches!(out.code, Code::Revoked), "Revoked must beat Expired: {:?}", out);
}
