//! verify_bytes 九态全态测试（S4-1：v4 Authenticode 管线）。
//!
//! 判据（goal S4-1）：Valid / Tampered / NoSignature / SignatureInvalid /
//! UnsupportedVersion / Malformed / Untrusted / Revoked / Expired 各 ≥1。
//! Revoked 需要真 CRL 服务器 + env（NEMESIS_REVOCATION_URL）+ 全局 CRL 缓存，
//! 位于 `revocation/tests.rs` 的 verify_bytes 集成段（与本文件共享 crate 根
//! GLOBAL_STATE_LOCK 串行），不在本文件重复。
//!
//! 载体覆盖：raw（九态主载体）+ PE（证书表定位 + authenticode digest 分支，
//! 最小 PE 手工构造，布局假设与 pe/tests.rs build_pe 同款）。ELF 载体的
//! footer/L 契约在 envelope/tests.rs S3-4 段覆盖，本文件不重复。

use super::*;
use crate::envelope::{FORMAT_TAG_RAW, attach_v4};
use crate::fixtures::{V4Harness, now_secs};
use crate::pe::append_certificate_table;
use p256::ecdsa::SigningKey;

// ===== 最小 PE 构造（端口自 pe/tests.rs build_pe，只留 S4-1 需要的形态）=====

const P: usize = 0x40; // e_lfanew

fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// 无证书表的最小 PE32+：1 个 section（0x200 @ 0x400），nrva=16（Security 项
/// 全零 = 未签名）。
fn base_pe() -> Vec<u8> {
    let size_of_opt: usize = 240;
    let sec_tbl = P + 24 + size_of_opt;
    let len = (sec_tbl + 40).max(0x400 + 0x200);
    let mut b = vec![0u8; len];
    b[0] = b'M';
    b[1] = b'Z';
    put32(&mut b, 0x3C, P as u32);
    b[P..P + 4].copy_from_slice(b"PE\0\0");
    put16(&mut b, P + 6, 1);
    put16(&mut b, P + 20, size_of_opt as u16);
    put16(&mut b, P + 24, 0x20b);
    put32(&mut b, P + 88, 0xDEADBEEF); // CheckSum 非零（digest 排除可观测）
    put32(&mut b, P + 132, 16); // NumberOfRvaAndSizes
    put32(&mut b, sec_tbl + 16, 0x200); // SizeOfRawData
    put32(&mut b, sec_tbl + 20, 0x400); // PointerToRawData
    b
}

/// 与 keygen 体系无关的另一把签名私钥（SignatureInvalid 用：sid 指向 leaf 证书、
/// 签名却出自别把钥匙）。
fn foreign_leaf_sk() -> SigningKey {
    SigningKey::from_bytes(&[0x5Au8; 32].into()).expect("seed is a valid scalar")
}

// ===== Valid =====

#[test]
fn valid_raw_carrier() {
    let h = V4Harness::new();
    let content = b"S4-1 valid raw payload".to_vec();
    let signed = h.sign_raw(&content, 1_800_000_000);
    let leaf_pubkey = crypto::public_key_bytes(h.h.leaf_sk.verifying_key());
    match verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        VerifyOutcome::Valid {
            signed_at,
            key_fp,
            pubkey,
        } => {
            assert_eq!(signed_at, 1_800_000_000, "signingTime 属性穿透");
            assert_eq!(pubkey, leaf_pubkey, "pubkey = 签名者证书 SPKI");
            assert_eq!(key_fp, crypto::key_fp(&leaf_pubkey));
        }
        o => panic!("expected Valid, got {o:?}"),
    }
}

#[test]
fn valid_pe_certificate_table() {
    let h = V4Harness::new();
    let pe = base_pe();
    // Authenticode 语义：CMS messageDigest = 整个 PE 文件的 authenticode digest
    // （排除 CheckSum 字段与证书表区），不是任意内容串的 SHA-256。追加证书表
    // 只写排除区（Security 目录 + EOF 表数据），digest 前后不变。
    let digest = crate::pe::authenticode_digest(&pe).expect("authenticode digest");
    let cms = crate::envelope::build_signed_data(
        &digest,
        &h.h.leaf_sk,
        1_800_000_000,
        &h.h.chain(),
        None,
        None,
    )
    .expect("build_signed_data");
    let signed = append_certificate_table(&pe, &cms).expect("append table");
    assert!(
        matches!(
            verify_bytes(&signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Valid { .. }
        ),
        "PE 证书表主签名 → Valid"
    );
}

// ===== sign_content_v4（S5-1：v4 签发单一入口，格式分派 + digest 接线）=====

#[test]
fn sign_content_v4_raw_roundtrip_valid() {
    let h = V4Harness::new();
    let signed = sign_content_v4(b"s51 raw payload", &h.h.leaf_sk, 42_000, &h.h.chain(), None)
        .expect("sign_content_v4 raw");
    match verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        VerifyOutcome::Valid { signed_at, .. } => assert_eq!(signed_at, 42_000),
        o => panic!("expected Valid, got {o:?}"),
    }
}

#[test]
fn sign_content_v4_pe_roundtrip_valid() {
    // PE 臂：helper 内部走 authenticode_digest + Certificate Table（exe-sign-tool
    // sign 的实际路径），签名文件 verify_bytes Valid 且 opus publisher 可 view 穿透。
    let h = V4Harness::new();
    let pe = base_pe();
    let signed = sign_content_v4(
        &pe,
        &h.h.leaf_sk,
        43_000,
        &h.h.chain(),
        Some("org-publisher"),
    )
    .expect("sign_content_v4 pe");
    match verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        VerifyOutcome::Valid { signed_at, .. } => assert_eq!(signed_at, 43_000),
        o => panic!("expected Valid, got {o:?}"),
    }
    assert!(crate::view::latest_sig_hash(&signed).is_some());
}

// ===== 最小 ELF64 LE 构造（端口自 envelope/tests.rs s34_min_elf，模块私有不可跨文件）=====

/// PT_LOAD 全覆盖形态：codec L = 全文件长（overlay 域为空），S5-2 用它覆盖
/// [`sign_content_v4`] ELF 臂的 `compute_l` Some(L) 路径。
fn min_elf64_le() -> Vec<u8> {
    let hdr: usize = 64;
    let phe: usize = 56;
    let l = hdr + phe;
    let mut b = vec![0u8; l];
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = 2; // ELFCLASS64
    b[5] = 1; // ELFDATA2LSB
    b[32..40].copy_from_slice(&(hdr as u64).to_le_bytes()); // e_phoff
    b[40..48].copy_from_slice(&0u64.to_le_bytes()); // e_shoff = 0（无 section 表）
    b[54..56].copy_from_slice(&(phe as u16).to_le_bytes()); // e_phentsize
    b[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    b[58..60].copy_from_slice(&64u16.to_le_bytes());
    b[60..62].copy_from_slice(&0u16.to_le_bytes());
    b[hdr..hdr + 4].copy_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    b[hdr + 8..hdr + 16].copy_from_slice(&0u64.to_le_bytes()); // p_offset
    b[hdr + 32..hdr + 40].copy_from_slice(&(l as u64).to_le_bytes()); // p_filesz = 全长
    b
}

#[test]
fn sign_content_v4_elf_roundtrip_valid() {
    // ELF 臂：footer 载体 + 保护域 = codec L（compute_l Some(L) 路径——raw 走
    // None 臂、PE 走 authenticode，只有 ELF 覆盖这里）。
    let h = V4Harness::new();
    let elf = min_elf64_le();
    let signed = sign_content_v4(&elf, &h.h.leaf_sk, 44_000, &h.h.chain(), None)
        .expect("sign_content_v4 elf");
    match verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        VerifyOutcome::Valid { signed_at, .. } => assert_eq!(signed_at, 44_000),
        o => panic!("expected Valid, got {o:?}"),
    }
}

// ===== NoSignature =====

#[test]
fn no_signature_raw_and_pe() {
    let h = V4Harness::new();
    assert_eq!(
        verify_bytes(b"plain raw bytes", &h.anchor_fps(), 1),
        VerifyOutcome::NoSignature,
        "raw 无 v4 footer → NoSignature"
    );
    assert_eq!(
        verify_bytes(&base_pe(), &h.anchor_fps(), 1),
        VerifyOutcome::NoSignature,
        "PE 无证书表 → NoSignature"
    );
}

// ===== Tampered（摘要差）=====

#[test]
fn tampered_content_detected() {
    let h = V4Harness::new();
    let content = b"tamper target payload".to_vec();
    let signed = h.sign_raw(&content, 1_800_000_000);

    // raw：内容域字节翻转 → 载体 content_hash 变 → Tampered
    let mut bad = signed;
    bad[3] ^= 0x01;
    assert!(
        matches!(
            verify_bytes(&bad, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Tampered(_)
        ),
        "raw 内容翻转 → Tampered"
    );

    // PE：先按 authenticode digest 签出 Valid 基线，再翻转 section 内容
    // → 重算 digest 变 → Tampered
    let pe = base_pe();
    let digest = crate::pe::authenticode_digest(&pe).expect("authenticode digest");
    let cms = crate::envelope::build_signed_data(
        &digest,
        &h.h.leaf_sk,
        1_800_000_000,
        &h.h.chain(),
        None,
        None,
    )
    .expect("build_signed_data");
    let mut pe_signed = append_certificate_table(&pe, &cms).unwrap();
    pe_signed[0x500] ^= 0x80;
    assert!(
        matches!(
            verify_bytes(&pe_signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Tampered(_)
        ),
        "PE 内容翻转 → Tampered"
    );

    // ELF：footer 载体，翻转摘要域 [0,L) 内非结构字节（e_entry@24——载体
    // framing 只解析 L 相关字段，e_entry 不在其中）→ Tampered。结构字段
    // 翻转（如 p_filesz）走 Malformed fail-closed，是另一条诚实路径。
    let elf = sign_content_v4(
        &min_elf64_le(),
        &h.h.leaf_sk,
        1_800_000_000,
        &h.h.chain(),
        None,
    )
    .expect("sign_content_v4 elf");
    let mut elf_bad = elf;
    elf_bad[24] ^= 0xFF;
    assert!(
        matches!(
            verify_bytes(&elf_bad, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Tampered(_)
        ),
        "ELF 摘要域翻转 → Tampered"
    );
}

// ===== SignatureInvalid =====

#[test]
fn signature_invalid_on_foreign_sk() {
    let h = V4Harness::new();
    let content = b"foreign key payload".to_vec();
    // sid = leaf 证书（issuer+serial），签名却出自别把私钥 → 验签必败
    let signed = h.sign_raw_with(&content, 1_800_000_000, &foreign_leaf_sk(), &h.h.chain());
    assert!(
        matches!(
            verify_bytes(&signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::SignatureInvalid
        ),
        "错配 sk → SignatureInvalid"
    );
}

// ===== UnsupportedVersion =====

#[test]
fn unsupported_version_non_v1_signed_data() {
    use cms::content_info::{CmsVersion, ContentInfo};
    use cms::signed_data::SignedData;
    use der::{Decode, Encode};

    let h = V4Harness::new();
    let content = b"version bump payload".to_vec();
    let cms = h.build_cms(&content, 1_800_000_000, &h.h.leaf_sk, &h.h.chain());

    // version 不在签名覆盖面（RFC 5652：签名只盖 signedAttrs）→ parse → 改
    // V2 → re-encode 不破签名；Authenticode 钉 V1 → UnsupportedVersion
    let mut ci = ContentInfo::from_der(cms.as_slice()).expect("ContentInfo parse");
    let mut sd =
        SignedData::from_der(ci.content.to_der().unwrap().as_slice()).expect("SignedData parse");
    sd.version = CmsVersion::V2;
    ci.content = der::Any::from_der(sd.to_der().unwrap().as_slice()).unwrap();
    let signed = attach_v4(
        &content,
        &ci.to_der().unwrap(),
        FORMAT_TAG_RAW,
        content.len(),
    );

    match verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        VerifyOutcome::UnsupportedVersion(msg) => {
            assert!(msg.contains("unsupported SignedData"), "{msg}");
        }
        o => panic!("expected UnsupportedVersion, got {o:?}"),
    }
}

// ===== Malformed =====

#[test]
fn malformed_garbage_cms() {
    let h = V4Harness::new();
    let content = b"garbage cms carrier".to_vec();

    // 载体路径：footer 在但 CMS DER 不可解析
    let signed = attach_v4(&content, b"not-a-der", FORMAT_TAG_RAW, content.len());
    assert!(
        matches!(
            verify_bytes(&signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Malformed(_)
        ),
        "载体垃圾 CMS → Malformed"
    );

    // PE 路径：证书表条目在（PKCS 类型标记）但内容非 CMS
    let pe_signed = append_certificate_table(&base_pe(), b"still-not-der").unwrap();
    assert!(
        matches!(
            verify_bytes(&pe_signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Malformed(_)
        ),
        "PE 表垃圾 CMS → Malformed"
    );
}

// ===== Untrusted =====

#[test]
fn untrusted_empty_anchor_set_is_default_state() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"default state payload", 1_800_000_000);
    // D7：默认态（未安装任何根）→ 链验不到信任锚 → Untrusted
    assert_eq!(
        verify_bytes(&signed, &[], now_secs()),
        VerifyOutcome::Untrusted,
        "空锚集（默认态）→ Untrusted"
    );
}

#[test]
fn untrusted_foreign_anchor() {
    let h = V4Harness::new();
    let other = V4Harness::new();
    let signed = h.sign_raw(b"foreign anchor payload", 1_800_000_000);
    assert_eq!(
        verify_bytes(&signed, &other.anchor_fps(), now_secs()),
        VerifyOutcome::Untrusted,
        "锚集只有别的根 → Untrusted"
    );
}

#[test]
fn untrusted_broken_chain_missing_parent() {
    let h = V4Harness::new();
    let content = b"broken chain payload".to_vec();
    // 证书集只有 leaf（AKI 找不到发行锚 SKI）→ 链断 → Untrusted
    let leaf_only: Vec<crate::cert::Certificate> = vec![h.h.leaf_cert.clone()];
    let signed = h.sign_raw_with(&content, 1_800_000_000, &h.h.leaf_sk, &leaf_only);
    assert!(
        matches!(
            verify_bytes(&signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Untrusted
        ),
        "链断（缺发行锚）→ Untrusted"
    );
}

// ===== Expired =====

#[test]
fn expired_when_now_outside_validity() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"expired payload", 1_800_000_000);
    // now = u64::MAX 必然越过证书链有效期（leaf 3y / issuing 10y / root 30y）
    match verify_bytes(&signed, &h.anchor_fps(), u64::MAX) {
        VerifyOutcome::Expired(msg) => assert!(msg.contains("expired"), "{msg}"),
        o => panic!("expected Expired, got {o:?}"),
    }
}

// ===== 破坏性变更钉死：v3 NMBSIG envelope 不再被消费 =====

#[test]
fn legacy_v3_envelope_yields_no_signature() {
    // v3 NMBSIG footer 字节内联（v3 签名器 sign_content 已随 S5-3 删除）——
    // v3 形态文件在 v4 管线 = 无主签名（goal S4-1 破坏性声明）。
    let mut legacy = b"v3 legacy envelope".to_vec();
    let mut footer = [0u8; 64];
    footer[0..8].copy_from_slice(b"NMBSIG\x03\x00");
    footer[8] = 3; // format_ver = 3
    legacy.extend_from_slice(&footer);
    let h = V4Harness::new();
    assert_eq!(
        verify_bytes(&legacy, &h.anchor_fps(), now_secs()),
        VerifyOutcome::NoSignature,
        "v3 NMBSIG envelope 在 v4 管线 = 无主签名（goal 破坏性声明）"
    );
}
