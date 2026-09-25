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
//!
//! ⚠ env 串行契约（2026-09-12 CI Untrusted 假红根修）：**凡能走到 verify_bytes
//! 第⑥步（吊销）的测试必须持有 crate 根 `GLOBAL_STATE_LOCK`**——revocation
//! 测试持锁改 `NEMESIS_REVOCATION_URL`/`NEMESIS_STRICT_OFFLINE`（CI 4 核慢机
//! 上竞态窗口内：死 URL 拉取失败 → strict → OCSP 不可达 → Untrusted 假红）。
//! 在第⑤步及之前出结果的测试（NoSignature/Malformed/Unsupported/Untrusted/
//! Expired/Tampered/SignatureInvalid）不触 env，无需持锁。

use super::*;
use crate::GLOBAL_STATE_LOCK as TEST_LOCK;
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
    let _g = TEST_LOCK.lock().unwrap(); // 走到第⑥步（吊销读 env），必须持锁
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
    let _g = TEST_LOCK.lock().unwrap(); // 走到第⑥步（吊销读 env），必须持锁
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
    let _g = TEST_LOCK.lock().unwrap(); // 走到第⑥步（吊销读 env），必须持锁
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
    let _g = TEST_LOCK.lock().unwrap(); // 走到第⑥步（吊销读 env），必须持锁
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
    let _g = TEST_LOCK.lock().unwrap(); // 走到第⑥步（吊销读 env），必须持锁
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

// ---------------------------------------------------------------------------
// AGT 覆盖率批次（2026-09-24）：verify_bytes 的支线臂补测——
// - PE 首条目非 PKCS → locate_primary Malformed（146-149）
// - sid 指向的签名者证书不在证书集 → Malformed（244）
// - leaf 缺 codeSigning EKU → 链验 Untrusted（258）
// - leaf 签名字节破坏 → BadSignature 落 catch-all Untrusted（264）
// - order_chain_from：AKI↔SKI 成环 → None（213）
// - 签名者/签发者 SPKI 损坏 → 链序前置拒绝 Untrusted（2026-09-26 补测修正：
//   原计划的「第⑤步签名者公钥」「InvalidKey→证书链」两臂经 verify_bytes
//   不可达——is_self_signed 无条件解析 SPKI，坏 SPKI 证书在 order_chain_from
//   即被拒，两臂为防御性死臂，见覆盖率报告墙类 9）
// 均在第③/⑤步内出结果，不触吊销 env，无需 GLOBAL_STATE_LOCK。
// ---------------------------------------------------------------------------

/// TBS 级变异积木：build_tbs → mutate → seal（签名在变异后盖，证书自洽）。
/// 独立命名（agt_ 前缀）避免与 S4-1 段夹具混淆。
fn agt_mutated_cert(
    subject_vk: &p256::ecdsa::VerifyingKey,
    signer_sk: &p256::ecdsa::SigningKey,
    issuer_ski: &[u8],
    mutate: impl FnOnce(&mut x509_cert::certificate::TbsCertificate),
) -> crate::cert::Certificate {
    use crate::cert::{TbsInput, build_tbs, seal_certificate};
    let now = now_secs();
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
        eku_code_signing: true,
        not_before_unix: now.saturating_sub(3600),
        not_after_unix: now.saturating_add(365 * 86400),
    };
    let mut tbs = build_tbs(
        subject_vk,
        issuer_ski,
        &crate::cert::random_serial(),
        &input,
    )
    .unwrap();
    mutate(&mut tbs);
    seal_certificate(tbs, signer_sk).unwrap()
}

#[test]
fn agt_pe_first_entry_non_pkcs_yields_malformed() {
    let h = V4Harness::new();
    let pe = base_pe();
    let digest = crate::pe::authenticode_digest(&pe).expect("digest");
    let cms = crate::envelope::build_signed_data(
        &digest,
        &h.h.leaf_sk,
        1_800_000_000,
        &h.h.chain(),
        None,
        None,
    )
    .unwrap();
    let mut signed = append_certificate_table(&pe, &cms).unwrap();
    // 首条目 wCertificateType 改成 0x0001（非 PKCS_SIGNED_DATA）
    let va = pe.len().div_ceil(8) * 8;
    signed[va + 6..va + 8].copy_from_slice(&1u16.to_le_bytes());
    match verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("非 PKCS_SIGNED_DATA"), "{m}"),
        o => panic!("expected Malformed, got {o:?}"),
    }
}

#[test]
fn agt_sid_signer_missing_from_cert_set_is_malformed() {
    use cms::cert::IssuerAndSerialNumber;
    use cms::content_info::ContentInfo;
    use cms::signed_data::{SignedData, SignerIdentifier};
    use der::{Decode, Encode};
    use x509_cert::serial_number::SerialNumber;

    let h = V4Harness::new();
    let content = b"sid missing payload".to_vec();
    let cms = h.build_cms(&content, 1_800_000_000, &h.h.leaf_sk, &h.h.chain());
    // sid 不在 build_signed_data 的签名覆盖面（签名只盖 signedAttrs）——
    // parse → 换 serial → re-encode 不破签名，但 sid 从此对不上证书集任何成员
    let mut ci = ContentInfo::from_der(cms.as_slice()).unwrap();
    let mut sd = SignedData::from_der(ci.content.to_der().unwrap().as_slice()).unwrap();
    let mut si = sd.signer_infos.0.iter().next().unwrap().clone();
    match &mut si.sid {
        SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
            serial_number, ..
        }) => {
            *serial_number = SerialNumber::new(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        }
        _ => panic!("expected IssuerAndSerialNumber sid"),
    }
    sd.signer_infos =
        cms::signed_data::SignerInfos(der::asn1::SetOfVec::try_from(vec![si]).unwrap());
    ci.content = der::Any::from_der(sd.to_der().unwrap().as_slice()).unwrap();
    let signed = crate::envelope::attach_v4(
        &content,
        &ci.to_der().unwrap(),
        crate::envelope::FORMAT_TAG_RAW,
        content.len(),
    );
    match verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("sid"), "{m}"),
        o => panic!("expected Malformed, got {o:?}"),
    }
}

#[test]
fn agt_leaf_without_code_signing_eku_is_untrusted() {
    use crate::cert::{TbsInput, ski_value};
    let h = V4Harness::new();
    let now = now_secs();
    let no_eku_leaf = crate::keygen::issue_x509(
        h.h.leaf_sk.verifying_key(),
        &h.h.issuing_sk,
        &ski_value(h.h.issuing_sk.verifying_key()).unwrap(),
        TbsInput {
            subject_cn: "No EKU Leaf",
            subject_org: Some("NB Test"),
            issuer_cn: "Issuing",
            issuer_org: Some("NB Test"),
            is_ca: false,
            path_len: None,
            ku_digital_signature: true,
            ku_key_cert_sign: false,
            ku_crl_sign: false,
            eku_code_signing: false,
            not_before_unix: now.saturating_sub(3600),
            not_after_unix: now.saturating_add(365 * 86400),
        },
    )
    .unwrap();
    let certs = vec![no_eku_leaf, h.h.issuing_cert.clone(), h.h.root_cert.clone()];
    let signed = h.sign_raw_with(b"no eku payload", 1_800_000_000, &h.h.leaf_sk, &certs);
    assert!(
        matches!(
            verify_bytes(&signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Untrusted
        ),
        "leaf 缺 codeSigning EKU → Untrusted"
    );
}

#[test]
fn agt_leaf_signature_algorithm_foreign_is_malformed_chain_error() {
    // leaf 的 signature_algorithm 在 TBS 外——换成 sha256WithRSAEncryption 后
    // 结构/链序/有效期/EKU 全好（order_chain_from 放行），verify_chain 在
    // verify_signature_by 的算法闸处 UnsupportedAlgorithm → Malformed「证书链」
    use der::{Decode, Encode};
    let h = V4Harness::new();
    let mut x = x509_cert::certificate::Certificate::from_der(h.h.leaf_cert.to_der()).unwrap();
    x.signature_algorithm.oid = der::asn1::ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
    let leaf_bad = crate::cert::Certificate::from_der(&x.to_der().unwrap()).unwrap();
    let certs = vec![leaf_bad, h.h.issuing_cert.clone(), h.h.root_cert.clone()];
    let signed = h.sign_raw_with(b"sig alg payload", 1_800_000_000, &h.h.leaf_sk, &certs);
    match verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("证书链"), "{m}"),
        o => panic!("expected Malformed, got {o:?}"),
    }
}

#[test]
fn agt_broken_leaf_signature_falls_into_catch_all_untrusted() {
    let h = V4Harness::new();
    // leaf 签名字节翻一位（TBS 外——AKI/SKI/有效期/EKU/结构全好）：
    // verify_chain → BadSignature → catch-all `_ => Untrusted`
    use der::{Decode, Encode};
    let mut x = x509_cert::certificate::Certificate::from_der(h.h.leaf_cert.to_der()).unwrap();
    let mut sig = x.signature.raw_bytes().to_vec();
    let n = sig.len();
    sig[n - 1] ^= 0xFF;
    x.signature = der::asn1::BitString::new(0, sig).unwrap();
    let leaf_bad = crate::cert::Certificate::from_der(&x.to_der().unwrap()).unwrap();
    let certs = vec![leaf_bad, h.h.issuing_cert.clone(), h.h.root_cert.clone()];
    let signed = h.sign_raw_with(b"broken sig payload", 1_800_000_000, &h.h.leaf_sk, &certs);
    assert!(
        matches!(
            verify_bytes(&signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Untrusted
        ),
        "BadSignature 落 catch-all 臂 → Untrusted"
    );
}

#[test]
fn agt_signer_spki_corrupted_rejected_by_chain_order_untrusted() {
    // 签名者（leaf）SPKI 损坏（TBS 变异后重盖签名，证书自洽、SKI/AKI 完好）：
    // 链序上溯的第一步 is_self_signed 需解析主体 SPKI → InvalidKey →
    // order_chain_from None → Untrusted。原目标「第⑤步 Malformed『签名者
    // 公钥』」经 verify_bytes 不可达（链序前置拦截，防御性死臂——见覆盖率
    // 报告墙类 9），本测试钉死真实行为 = 诚实拒绝。
    use crate::cert::ski_value;
    let h = V4Harness::new();
    let leaf_bad = agt_mutated_cert(
        h.h.leaf_sk.verifying_key(),
        &h.h.issuing_sk,
        &ski_value(h.h.issuing_sk.verifying_key()).unwrap(),
        |tbs| {
            // 33 字节垃圾 = 压缩 SEC1 点长度，但不是合法 P-256 点
            tbs.subject_public_key_info.subject_public_key =
                der::asn1::BitString::new(0, vec![0xFF; 33]).unwrap();
        },
    );
    let certs = vec![leaf_bad, h.h.issuing_cert.clone(), h.h.root_cert.clone()];
    let signed = h.sign_raw_with(b"signer spki payload", 1_800_000_000, &h.h.leaf_sk, &certs);
    assert!(
        matches!(
            verify_bytes(&signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Untrusted
        ),
        "坏 SPKI 签名者证书 → 链序拒绝 → Untrusted"
    );
}

#[test]
fn agt_issuer_spki_corrupted_rejected_by_chain_order_untrusted() {
    // 发行锚（issuing）SPKI 损坏：leaf 的 AKI 命中其 SKI 完成上溯，但下一轮
    // is_self_signed 解析坏 SPKI 失败 → 链序 None → Untrusted。verify_chain
    // 的 InvalidKey → Malformed「证书链」臂同样被链序前置拦截（防御性死臂）。
    use crate::cert::ski_value;
    let h = V4Harness::new();
    let issuing_bad = agt_mutated_cert(
        h.h.issuing_sk.verifying_key(),
        &h.h.root_sk,
        &ski_value(h.h.root_sk.verifying_key()).unwrap(),
        |tbs| {
            tbs.subject_public_key_info.subject_public_key =
                der::asn1::BitString::new(0, vec![0xFF; 33]).unwrap();
        },
    );
    let certs = vec![h.h.leaf_cert.clone(), issuing_bad, h.h.root_cert.clone()];
    let signed = h.sign_raw_with(b"issuer spki payload", 1_800_000_000, &h.h.leaf_sk, &certs);
    assert!(
        matches!(
            verify_bytes(&signed, &h.anchor_fps(), now_secs()),
            VerifyOutcome::Untrusted
        ),
        "坏 SPKI 签发者证书 → 链序拒绝 → Untrusted"
    );
}

#[test]
fn agt_order_chain_from_detects_cycle() {
    use crate::cert::{TbsInput, ski_value};
    use p256::ecdsa::SigningKey;

    let leaf_sk = SigningKey::from_bytes(&[0xA1u8; 32].into()).unwrap();
    let a_sk = SigningKey::from_bytes(&[0xA2u8; 32].into()).unwrap();
    let leaf_vk = *leaf_sk.verifying_key();
    let a_vk = *a_sk.verifying_key();
    let now = now_secs();
    let prof = |cn: &'static str| TbsInput {
        subject_cn: cn,
        subject_org: None,
        issuer_cn: "X",
        issuer_org: None,
        is_ca: false,
        path_len: None,
        ku_digital_signature: true,
        ku_key_cert_sign: false,
        ku_crl_sign: false,
        eku_code_signing: false,
        not_before_unix: now.saturating_sub(3600),
        not_after_unix: now.saturating_add(365 * 86400),
    };
    // leaf.AKI = A 的 SKI（A 签 leaf）；A.AKI = leaf 的 SKI（leaf 签 A）→ 环
    let leaf = crate::keygen::issue_x509(&leaf_vk, &a_sk, &ski_value(&a_vk).unwrap(), prof("Leaf"))
        .unwrap();
    let a = crate::keygen::issue_x509(&a_vk, &leaf_sk, &ski_value(&leaf_vk).unwrap(), prof("A"))
        .unwrap();
    let pool = vec![leaf.clone(), a];
    assert!(
        order_chain_from(&leaf, &pool).is_none(),
        "AKI↔SKI 互指成环必须返回 None"
    );
}
