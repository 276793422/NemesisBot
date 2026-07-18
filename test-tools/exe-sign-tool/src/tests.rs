//! 单元测试：合成 PE/ELF fixture + 全用例覆盖。
//!
//! 合成 fixture 不依赖真实 .exe/ELF——手构最小 PE32/PE32+ 与 ELF32/ELF64 字节
//! 布局，验证 codec 解析、签名/验证流程、排除项、多源 L、envelope 定位、
//! sym_key 处理等。

use crate::codec::{CodecError, ElfCodec, ExecutableCodec, PeCodec};
use crate::{crypto, envelope, sign_executable, verify_executable, VerifyOutcome};

/// 测试用固定 ChaCha20 对称密钥（确定性，便于调试；真实场景由 keygen 随机生成）。
const TEST_SYM_KEY: [u8; 32] = [0x42u8; 32];

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

// ---------- PE fixture ----------

/// 构造最小 PE32 / PE32+（DOS+PE sig+COFF+Optional+1 section+payload）。
fn build_pe(payload: &[u8], magic: u16) -> Vec<u8> {
    let is_plus = magic == 0x20b;
    let opt_size = if is_plus { 240 } else { 224 };
    let opt_off = 0x58usize;
    let sec_hdr_off = opt_off + opt_size;
    let payload_off = sec_hdr_off + 40; // 1 个 section header
    let total = payload_off + payload.len();
    let mut b = vec![0u8; total];
    // DOS header
    b[0..2].copy_from_slice(b"MZ");
    put_u32(&mut b, 0x3C, 0x40); // e_lfanew
    // PE sig
    b[0x40..0x44].copy_from_slice(b"PE\0\0");
    // COFF header @ 0x44
    put_u16(&mut b, 0x44, 0x14c); // Machine i386
    put_u16(&mut b, 0x46, 1); // NumberOfSections
    put_u32(&mut b, 0x48, 0xDEAD_BEEF); // TimeDateStamp（非零，验包含）
    put_u16(&mut b, 0x54, opt_size as u16); // SizeOfOptionalHeader
    // Optional header
    put_u16(&mut b, opt_off, magic);
    put_u32(&mut b, opt_off + 64, 0x1234_5678); // CheckSum
    // NumberOfRvaAndSizes：PE32 @ opt+92，PE32+ @ opt+108
    let nrva_off = if is_plus { opt_off + 108 } else { opt_off + 92 };
    put_u32(&mut b, nrva_off, 16);
    // Section header（PointerToRawData + SizeOfRawData 指向 payload）
    put_u32(&mut b, sec_hdr_off + 16, payload.len() as u32); // SizeOfRawData
    put_u32(&mut b, sec_hdr_off + 20, payload_off as u32); // PointerToRawData
    // payload
    b[payload_off..].copy_from_slice(payload);
    b
}

fn build_minimal_pe32(payload: &[u8]) -> Vec<u8> {
    build_pe(payload, 0x10b)
}
fn build_minimal_pe32plus(payload: &[u8]) -> Vec<u8> {
    build_pe(payload, 0x20b)
}

/// PE32 DataDirectory[4] 偏移（opt+128）。
const PE32_DD4_OFF: usize = 0x58 + 128;
/// PE32 CheckSum 偏移（opt+64）。
const PE32_CHECKSUM_OFF: usize = 0x58 + 64;

// ---------- ELF fixture ----------

fn build_elf(payload: &[u8], is64: bool) -> Vec<u8> {
    let ehsize = if is64 { 64 } else { 52 };
    let phentsize = if is64 { 56 } else { 32 };
    let phoff = ehsize;
    let payload_off = phoff + phentsize;
    let total = payload_off + payload.len();
    let mut b = vec![0u8; total];
    // e_ident
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = if is64 { 2 } else { 1 }; // EI_CLASS
    b[5] = 1; // EI_DATA LE
    b[6] = 1; // EI_VERSION
    put_u16(&mut b, 16, 2); // e_type = ET_EXEC
    if is64 {
        put_u64(&mut b, 32, phoff as u64); // e_phoff
    } else {
        put_u32(&mut b, 28, phoff as u32);
    }
    // e_shoff = 0（无 section header table）
    put_u16(&mut b, if is64 { 52 } else { 40 }, ehsize as u16); // e_ehsize
    put_u16(&mut b, if is64 { 54 } else { 42 }, phentsize as u16); // e_phentsize
    put_u16(&mut b, if is64 { 56 } else { 44 }, 1); // e_phnum
    put_u16(&mut b, if is64 { 58 } else { 46 }, if is64 { 64 } else { 40 }); // e_shentsize
    // program header @ phoff（1 个 PT_LOAD）
    put_u32(&mut b, phoff, 1); // p_type = PT_LOAD
    if is64 {
        put_u64(&mut b, phoff + 8, payload_off as u64); // p_offset
        put_u64(&mut b, phoff + 32, payload.len() as u64); // p_filesz
    } else {
        put_u32(&mut b, phoff + 4, payload_off as u32); // p_offset
        put_u32(&mut b, phoff + 16, payload.len() as u32); // p_filesz
    }
    b[payload_off..].copy_from_slice(payload);
    b
}

fn build_minimal_elf32(payload: &[u8]) -> Vec<u8> {
    build_elf(payload, false)
}
fn build_minimal_elf64(payload: &[u8]) -> Vec<u8> {
    build_elf(payload, true)
}

// ---------- 公共测试辅助 ----------

/// 用 TEST_SYM_KEY 签名到临时文件，返回 (路径, TempDir, 密钥对)。
fn temp_sign(buf: &[u8]) -> (std::path::PathBuf, tempfile::TempDir, crypto::KeyPair) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.bin");
    std::fs::write(&path, buf).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1_700_000_000, 7).unwrap();
    (path, dir, kp)
}

fn vk_of(kp: &crypto::KeyPair) -> ed25519_dalek::VerifyingKey {
    crypto::verifying_key_from_hex(&kp.public_key).unwrap()
}

// ===================== 用例 =====================

#[test]
fn domain_and_meta_len() {
    assert_eq!(envelope::DOMAIN.len(), 42);
    assert_eq!(envelope::SIGNED_META_LEN, 82);
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
        let out = verify_executable(&signed, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
        assert!(
            matches!(out, VerifyOutcome::Valid { .. }),
            "roundtrip failed: {:?}",
            out
        );
    }
}

#[test]
fn tamper_content_detected() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-PAYLOAD-TAMPER"));
    let mut b = std::fs::read(&path).unwrap();
    b[0x160] ^= 0xFF; // payload 首字节
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(
        matches!(out, VerifyOutcome::Tampered(_) | VerifyOutcome::SignatureInvalid),
        "{:?}",
        out
    );
}

#[test]
fn checksum_excluded() {
    // 改 CheckSum 字段 → 签名仍有效（证 CheckSum 被排除）
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-CHECKSUM-TEST"));
    let mut b = std::fs::read(&path).unwrap();
    b[PE32_CHECKSUM_OFF] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out, VerifyOutcome::Valid { .. }), "{:?}", out);
}

#[test]
fn security_dir_excluded() {
    // 改 DataDirectory[4] 字节 → 签名仍有效（证 Security 目录项被排除）
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-SEC-DIR-TEST"));
    let mut b = std::fs::read(&path).unwrap();
    b[PE32_DD4_OFF] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out, VerifyOutcome::Valid { .. }), "{:?}", out);
}

#[test]
fn pe_authenticode_cross_check() {
    // DataDirectory[4].VA < L → 多源交叉校验失败
    let mut b = build_minimal_pe32(b"PE32-CROSSCHECK");
    put_u32(&mut b, PE32_DD4_OFF, 10); // VA < L
    put_u32(&mut b, PE32_DD4_OFF + 4, 5); // Size
    let res = PeCodec.compute_l(&b);
    assert!(matches!(res, Err(CodecError::Malformed(_))), "{:?}", res);
}

#[test]
fn elf_l_with_section_header_table() {
    // ELF32 带 section header table 在 payload 之后 → L = max(PT_LOAD, SHT) = SHT end
    let payload = b"ELF32-SHT-TEST";
    let phoff = 52usize;
    let phentsize = 32usize;
    let payload_off = phoff + phentsize; // 84
    let sht_off = payload_off + payload.len();
    let sht_size = 40;
    let total = sht_off + sht_size;
    let mut b = vec![0u8; total];
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = 1;
    b[5] = 1;
    b[6] = 1;
    put_u16(&mut b, 16, 2);
    put_u32(&mut b, 28, phoff as u32); // e_phoff
    put_u32(&mut b, 32, sht_off as u32); // e_shoff
    put_u16(&mut b, 40, 52); // e_ehsize
    put_u16(&mut b, 42, phentsize as u16); // e_phentsize
    put_u16(&mut b, 44, 1); // e_phnum
    put_u16(&mut b, 46, 40); // e_shentsize
    put_u16(&mut b, 48, 1); // e_shnum
    put_u32(&mut b, phoff, 1); // PT_LOAD
    put_u32(&mut b, phoff + 4, payload_off as u32); // p_offset
    put_u32(&mut b, phoff + 16, payload.len() as u32); // p_filesz
    b[payload_off..payload_off + payload.len()].copy_from_slice(payload);
    let l = ElfCodec.compute_l(&b).unwrap().unwrap();
    assert_eq!(l, sht_off + sht_size, "L should be SHT end");
}

#[test]
fn authenticode_plus_ours_overlay() {
    // PE 先挂伪造 Authenticode（在 overlay），再签我们的 → 都能定位、互不影响
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
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1_700_000_000, 1).unwrap();
    let signed = std::fs::read(&path).unwrap();
    assert!(
        matches!(
            verify_executable(&signed, &vk_of(&kp), &TEST_SYM_KEY).unwrap(),
            VerifyOutcome::Valid { .. }
        )
    );

    // 改证书区域字节（overlay 内，非 content）→ 签名仍有效
    let mut b2 = signed.clone();
    b2[cert_va] ^= 0xFF;
    let out = verify_executable(&b2, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(
        matches!(out, VerifyOutcome::Valid { .. }),
        "cert region (overlay) change should not break sig: {:?}",
        out
    );
}

#[test]
fn aead_tamper_detected_as_tampered() {
    // 改 envelope 密文 body → AEAD 认证失败 → Tampered（非 NoSignature）
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-AEAD-TAMPER"));
    let mut b = std::fs::read(&path).unwrap();
    // 从 footer 读 total_len，定位密文 body 起点（envelope 第一字节，在 padding 之前）
    let foff = b.len() - envelope::FOOTER_LEN;
    let total_len = u32::from_le_bytes([b[foff + 12], b[foff + 13], b[foff + 14], b[foff + 15]]) as usize;
    let envelope_start = b.len() - total_len;
    b[envelope_start] ^= 0xFF; // 密文 body 首字节（AEAD 覆盖区）
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out, VerifyOutcome::Tampered(_)), "{:?}", out);
}

#[test]
fn no_signature() {
    let b = build_minimal_pe32(b"PE32-UNSIGNED");
    let kp = crypto::generate_key_pair();
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out, VerifyOutcome::NoSignature), "{:?}", out);
}

#[test]
fn wrong_key_rejected() {
    let (path, _d, _kp) = temp_sign(&build_minimal_pe32(b"PE32-WRONGKEY"));
    let b = std::fs::read(&path).unwrap();
    let other = crypto::generate_key_pair();
    let out = verify_executable(&b, &vk_of(&other), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out, VerifyOutcome::SignatureInvalid), "{:?}", out);
}

#[test]
fn double_sign_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.bin");
    std::fs::write(&path, &build_minimal_pe32(b"PE32-DOUBLE")).unwrap();
    let kp = crypto::generate_key_pair();
    let sk = crypto::signing_key_from_hex(&kp.private_key).unwrap();
    sign_executable(&path, &sk, &TEST_SYM_KEY, 1, 1).unwrap();
    let res = sign_executable(&path, &sk, &TEST_SYM_KEY, 2, 2);
    assert!(res.is_err(), "double sign must be rejected");
}

#[test]
fn footer_tamper_detected() {
    // 改 footer crc 区字节 → crc 不符 → Tampered（非 NoSignature）
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-FOOTER-TAMPER"));
    let mut b = std::fs::read(&path).unwrap();
    let crc_byte = b.len() - envelope::FOOTER_LEN + 36; // footer[36] = crc 起点
    b[crc_byte] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out, VerifyOutcome::Tampered(_)), "{:?}", out);
}

#[test]
fn envelope_4kb_aligned() {
    let (path, _d, _kp) = temp_sign(&build_minimal_pe32(b"PE32-ALIGN"));
    let b = std::fs::read(&path).unwrap();
    let foff = b.len() - envelope::FOOTER_LEN;
    let total_len = u32::from_le_bytes([b[foff + 12], b[foff + 13], b[foff + 14], b[foff + 15]]) as usize;
    assert_eq!(total_len % envelope::ENVELOPE_ALIGN, 0);
    assert!(total_len >= envelope::FOOTER_LEN);
}

// ---------- 补强用例（PE32+ 排除 / ELF·Raw 篡改 / Raw 无签名） ----------

#[test]
fn pe32plus_security_dir_excluded() {
    // PE32+ DataDirectory[4] 偏移与 PE32 不同（opt+144），独立验证排除
    let (path, _d, kp) = temp_sign(&build_minimal_pe32plus(b"PE32PLUS-SEC-DIR"));
    let mut b = std::fs::read(&path).unwrap();
    let dd4_off = 0x58 + 144; // PE32+ DataDirectory[4]
    b[dd4_off] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(
        matches!(out, VerifyOutcome::Valid { .. }),
        "PE32+ Security dir change should not break sig: {:?}",
        out
    );
}

#[test]
fn pe32plus_checksum_excluded() {
    let (path, _d, kp) = temp_sign(&build_minimal_pe32plus(b"PE32PLUS-CKSUM"));
    let mut b = std::fs::read(&path).unwrap();
    b[PE32_CHECKSUM_OFF] ^= 0xFF; // CheckSum @ opt+64（PE32/PE32+ 同）
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out, VerifyOutcome::Valid { .. }), "{:?}", out);
}

#[test]
fn elf64_tamper_content() {
    let (path, _d, kp) = temp_sign(&build_minimal_elf64(b"ELF64-TAMPER-CONTENT"));
    let mut b = std::fs::read(&path).unwrap();
    b[120] ^= 0xFF; // ELF64 payload 首（payload_off = 64+56）
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(
        matches!(out, VerifyOutcome::Tampered(_) | VerifyOutcome::SignatureInvalid),
        "{:?}",
        out
    );
}

#[test]
fn raw_no_signature() {
    let kp = crypto::generate_key_pair();
    let raw = b"RAW-UNSIGNED-BLOB-12345";
    let out = verify_executable(raw, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out, VerifyOutcome::NoSignature), "{:?}", out);
}

#[test]
fn raw_tamper_and_wrong_key() {
    // 篡改
    let (path, _d, kp) = temp_sign(b"RAW-TAMPER-BLOB-1234567890");
    let mut b = std::fs::read(&path).unwrap();
    b[5] ^= 0xFF;
    let out = verify_executable(&b, &vk_of(&kp), &TEST_SYM_KEY).unwrap();
    assert!(
        matches!(out, VerifyOutcome::Tampered(_) | VerifyOutcome::SignatureInvalid),
        "{:?}",
        out
    );
    // 错钥（用未篡改的签名文件 + 另一把公钥 → SignatureInvalid）
    let other = crypto::generate_key_pair();
    let signed = std::fs::read(&path).unwrap();
    let out2 = verify_executable(&signed, &vk_of(&other), &TEST_SYM_KEY).unwrap();
    assert!(matches!(out2, VerifyOutcome::SignatureInvalid), "{:?}", out2);
}

// ---------- sym_key 相关 ----------

#[test]
fn generate_sym_key_is_unique_and_32() {
    let a = crypto::generate_sym_key();
    let b = crypto::generate_sym_key();
    assert_eq!(a.len(), 32);
    assert_eq!(b.len(), 32);
    assert_ne!(a, b, "two generated sym keys should differ");
    // 非 BUILTIN_SYM_KEY_DEFAULT（全 0）
    assert_ne!(a, crypto::BUILTIN_SYM_KEY_DEFAULT);
}

#[test]
fn wrong_sym_key_rejected_as_tampered() {
    // sign 用 TEST_SYM_KEY，verify 用另一把 sym_key → AEAD 解密失败 → Tampered
    let (path, _d, kp) = temp_sign(&build_minimal_pe32(b"PE32-WRONG-SYM-KEY"));
    let b = std::fs::read(&path).unwrap();
    let wrong_sym = [0x99u8; 32];
    let out = verify_executable(&b, &vk_of(&kp), &wrong_sym).unwrap();
    assert!(
        matches!(out, VerifyOutcome::Tampered(_)),
        "wrong sym_key must be Tampered (AEAD auth fail): {:?}",
        out
    );
}
