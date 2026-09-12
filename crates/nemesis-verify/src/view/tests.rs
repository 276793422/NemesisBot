//! view 离线查看测试（S4-3：v4 Authenticode 形态）。
//!
//! 判据（goal S4-3）：view 输出含签名者/链/有效期，不下结论——列表/详情/哈希
//! 提取全走 v4 解析路径（PE 证书表 + 嵌套展开 / ELF/raw v4 footer 载体），
//! 结构坏诚实空视图，crafted 输入不 panic。

use super::*;
use crate::envelope::{self, FORMAT_TAG_RAW, attach_v4};
use crate::fixtures::{V4Harness, now_secs};
use crate::keygen;
use crate::pe::append_certificate_table;
use crate::verify::VerifyOutcome;
use crate::{cert, crypto, verify};
use sha2::Digest;

// ===== 本地最小载体夹具（pe/envelope 的 tests 模块私有，此处独立复制口径）=====

/// 最小合法 PE32：MZ + e_lfanew + PE\0\0 + COFF + Optional header
/// （nrva=16 → Security 表项可写）+ 1 个 section（raw end 0x300）。
/// 形态与 pe/tests.rs `build_pe` 同口径（模块私有，此处独立最小复制）。
fn base_pe() -> Vec<u8> {
    const P: usize = 0x40;
    fn put16(b: &mut [u8], off: usize, v: u16) {
        b[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut b = vec![0u8; 0x300];
    b[0] = b'M';
    b[1] = b'Z';
    put32(&mut b, 0x3C, P as u32);
    b[P..P + 4].copy_from_slice(b"PE\0\0");
    put16(&mut b, P + 6, 1); // NumberOfSections
    put16(&mut b, P + 20, 224); // SizeOfOptionalHeader（PE32）
    put16(&mut b, P + 24, 0x10b); // PE32 magic
    put32(&mut b, P + 88, 0xDEADBEEF); // CheckSum 非零（digest 排除才可观测）
    put32(&mut b, P + 116, 16); // NumberOfRvaAndSizes ≥ 5（Security 表项存在）
    // section table @ P+24+224：SizeOfRawData@+16 / PointerToRawData@+20
    put32(&mut b, P + 24 + 224 + 16, 0x100);
    put32(&mut b, P + 24 + 224 + 20, 0x200);
    b
}

/// 最小 ELF64 LE（envelope/tests.rs `s34_min_elf` 同口径）：头 64B + 单
/// PT_LOAD 覆盖结构区 → L = 120（overlay 不入保护域）。
fn min_elf() -> Vec<u8> {
    let (hdr, phe) = (64usize, 56usize);
    let l = hdr + phe;
    let mut b = vec![0u8; l];
    b[0..4].copy_from_slice(b"\x7fELF");
    b[4] = 2; // ELF64
    b[5] = 1; // LE
    b[32..40].copy_from_slice(&(hdr as u64).to_le_bytes()); // e_phoff
    b[40..48].copy_from_slice(&0u64.to_le_bytes()); // e_shoff = 0
    b[54..56].copy_from_slice(&(phe as u16).to_le_bytes()); // e_phentsize
    b[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    b[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    b[60..62].copy_from_slice(&0u16.to_le_bytes()); // e_shentsize
    b[hdr..hdr + 4].copy_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    b[hdr + 8..hdr + 16].copy_from_slice(&0u64.to_le_bytes()); // p_offset
    b[hdr + 32..hdr + 40].copy_from_slice(&(l as u64).to_le_bytes()); // p_filesz
    b
}

/// IEEE 802.3 CRC32（v4 footer CRC 口径，envelope 私有实现的最小复制）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= u32::from(b);
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

// ===== 列表 / 详情 =====

#[test]
fn list_single_signature() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"view-test-payload", 12345);
    let list = list_signatures(&signed);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].index, 0);
    assert_eq!(list[0].signed_at, 12345, "signingTime 属性穿透");
    assert_eq!(list[0].pubkey, crypto::public_key_bytes(&h.h.leaf_vk()));
    assert_eq!(list[0].key_fp, crypto::key_fp(&list[0].pubkey));
}

#[test]
fn detail_includes_cert_chain() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"detail-test", 999);
    let detail = get_signature_detail(&signed, 0).unwrap();
    // 展示序 leaf→root（AKI→SKI 结构排序），chain = [leaf, issuing, root] 含根 3 级
    assert_eq!(detail.certs.len(), 3);
    assert_eq!(
        detail.certs[0].subject_pubkey,
        crypto::public_key_bytes(&h.h.leaf_vk())
    );
    assert_eq!(
        detail.certs[1].subject_pubkey,
        crypto::public_key_bytes(&h.h.issuing_vk())
    );
    assert_eq!(
        detail.certs[2].subject_pubkey,
        crypto::public_key_bytes(&h.h.root_vk())
    );
    // CN 提取（subject_meta）与 AKI（issuer_key_fp）在 keygen profile 下必在
    assert_eq!(
        detail.certs[0].subject_meta.as_deref(),
        Some(keygen::CN_LEAF)
    );
    assert_eq!(
        detail.certs[1].subject_meta.as_deref(),
        Some(keygen::CN_ISSUING)
    );
    assert_eq!(
        detail.certs[2].subject_meta.as_deref(),
        Some(keygen::CN_ROOT)
    );
    // leaf 的 AKI = issuing 的 SKI 同口径值
    let leaf_aki = detail.certs[0].issuer_key_fp.unwrap();
    let ski32: [u8; 32] = cert::ski_value(&h.h.issuing_vk())
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(leaf_aki, ski32);
    // 有效期：起点 ≤ 终点，且当下在窗口内（keygen 真时钟生成）
    let now = now_secs();
    assert!(detail.certs[0].valid_not_before <= now);
    assert!(now <= detail.certs[0].valid_not_after);
    // 索引越界诚实 None
    assert!(get_signature_detail(&signed, 1).is_none());
}

#[test]
fn publisher_is_opus_program_name() {
    let h = V4Harness::new();
    let signed = h.sign_raw_opus(
        b"publisher payload",
        7,
        &h.h.leaf_sk,
        &h.h.chain(),
        Some("org-publisher"),
        Some("https://example.org"),
    );
    let detail = get_signature_detail(&signed, 0).unwrap();
    assert_eq!(detail.publisher.as_deref(), Some("org-publisher"));
    // opus 缺席 → publisher None（视图照常）
    let plain = h.sign_raw(b"no opus", 7);
    assert!(get_signature_detail(&plain, 0).unwrap().publisher.is_none());
}

// ===== PE 证书表 + 嵌套签名枚举 =====

#[test]
fn pe_view_lists_primary_signature() {
    let h = V4Harness::new();
    let pe = base_pe();
    // view 不校验 digest（查看不下结论）：CMS 内嵌 digest 与文件是否一致归
    // verify 管线，这里只断言证书表里的主签名可见
    let cms = h.build_cms(&pe, 555, &h.h.leaf_sk, &h.h.chain());
    let signed = append_certificate_table(&pe, &cms).expect("append table");

    let list = list_signatures(&signed);
    assert_eq!(list.len(), 1, "单签名 PE：主签名 1 条");
    assert_eq!(list[0].signed_at, 555);
    let detail = get_signature_detail(&signed, 0).unwrap();
    assert_eq!(detail.certs.len(), 3, "展示链含根 3 级");
    assert!(latest_sig_hash(&signed).is_some());
}

#[test]
fn pe_view_enumerates_nested_signature() {
    let h = V4Harness::new();
    let other = V4Harness::new();
    let pe = base_pe();

    // 主签名 = other 链（他方形态），嵌套 = h 链（自方）；嵌套挂进主 CMS 树
    // （SPC_NESTED_SIGNATURE unauthAttrs，微软 signtool /as 同形态）
    let host_cms = other.build_cms(&pe, 111, &other.h.leaf_sk, &other.h.chain());
    let nested_cms = h.build_cms(&pe, 222, &h.h.leaf_sk, &h.h.chain());
    let combined =
        envelope::append_nested_signature(&host_cms, &nested_cms).expect("nest into host");
    let signed = append_certificate_table(&pe, &combined).expect("append table");

    let list = list_signatures(&signed);
    assert_eq!(list.len(), 2, "主 + 嵌套都可见（先序展开）");
    assert_eq!(list[0].signed_at, 111, "index 0 = 主签名");
    assert_eq!(list[1].signed_at, 222, "index 1 = 嵌套签名");
    // 两条签名的签名者证书不同（各自证书集）
    assert_ne!(list[0].pubkey, list[1].pubkey);
    // 详情逐条可取，展示链 leaf→root
    let d1 = get_signature_detail(&signed, 1).unwrap();
    assert_eq!(
        d1.certs[0].subject_pubkey,
        crypto::public_key_bytes(&h.h.leaf_vk())
    );
    // 视图不越界：index 2 诚实 None
    assert!(get_signature_detail(&signed, 2).is_none());
}

// ===== ELF 载体 =====

#[test]
fn elf_carrier_view_lists_signature() {
    let h = V4Harness::new();
    let elf = min_elf();
    let cms = h.build_cms(&elf, 321, &h.h.leaf_sk, &h.h.chain());
    let signed = envelope::sign_carrier_v4(&elf, &cms).expect("sign carrier");

    let list = list_signatures(&signed);
    assert_eq!(list.len(), 1, "ELF 载体签名可见");
    assert_eq!(list[0].signed_at, 321);
    assert!(get_signature_detail(&signed, 0).is_some());
}

#[test]
fn no_signature_empty_list() {
    assert!(list_signatures(b"plain bytes no signature").is_empty());
    assert!(list_signatures(&[]).is_empty());
    // PE 无证书表 → 空视图
    assert!(list_signatures(&base_pe()).is_empty());
    // v3 legacy envelope（footer 字节内联，v3 签名器已随 S5-3 删除）→ v4 视图
    // 不可见（S4-1 破坏性迁移的 view 侧钉死）
    let mut legacy = b"v3 legacy".to_vec();
    let mut footer = [0u8; 64];
    footer[0..8].copy_from_slice(b"NMBSIG\x03\x00");
    legacy.extend_from_slice(&footer);
    assert!(list_signatures(&legacy).is_empty());
    assert!(latest_sig_hash(&legacy).is_none());
}

// ===== sig_hash 提取 =====

#[test]
fn latest_sig_hash_extracted() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"sig-hash-test", 111);
    let sig_hash = latest_sig_hash(&signed).unwrap();
    // = SHA-256(SignerInfo.signature DER)（与 verify 吊销 SigHash 维度同口径）
    let ps = envelope::parse_signed_data(
        &envelope::extract_carrier_v4(&signed)
            .expect("carrier")
            .cms_der,
    )
    .expect("parse");
    let expect: [u8; 32] = sha2::Sha256::digest(&ps.signature).into();
    assert_eq!(sig_hash, expect);
    // 无签名文件 → None
    assert!(latest_sig_hash(b"no signature here").is_none());
}

// ===== crafted / 畸形输入诚实失败（不 panic；nv_list_signatures 是跨进程 C ABI）=====

#[test]
fn malformed_carrier_yields_empty_view() {
    // footer 在但 CMS DER 不可解析 → 三入口全部诚实空/None
    let signed = attach_v4(b"garbage cms carrier", b"not-a-der", FORMAT_TAG_RAW, 19);
    assert!(list_signatures(&signed).is_empty());
    assert!(get_signature_detail(&signed, 0).is_none());
    assert!(latest_sig_hash(&signed).is_none());
}

#[test]
fn crafted_footer_yields_empty_view() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"crafted footer target", 1);
    let flen = envelope::FOOTER_LEN;

    // ① footer CRC 区内字节破坏 → parse_footer_v4 CRC 拒绝 → 空视图
    let mut bad = signed.clone();
    let fo = bad.len() - flen;
    bad[fo + 12] ^= 0xFF; // format_tag 落在 CRC 覆盖区 [0..36)
    assert!(list_signatures(&bad).is_empty());
    assert!(get_signature_detail(&bad, 0).is_none());
    assert!(latest_sig_hash(&bad).is_none());

    // ② cms_len 抬满（S2-3 防钳家族：footer 自身 CRC 合法但区间越界）
    let mut bad2 = signed;
    let fo2 = bad2.len() - flen;
    bad2[fo2 + 32..fo2 + 36].copy_from_slice(&u32::MAX.to_le_bytes());
    let crc = crc32(&bad2[fo2..fo2 + envelope::FOOTER_CRC_LEN_V4]);
    bad2[fo2 + 36..fo2 + 40].copy_from_slice(&crc.to_le_bytes());
    assert!(list_signatures(&bad2).is_empty());
    assert!(get_signature_detail(&bad2, 0).is_none());
    assert!(latest_sig_hash(&bad2).is_none());
}

// ===== 视图与验证同源（sanity：view 可见的签名 verify 也能定位并通过）=====

#[test]
fn view_and_verify_agree_on_primary() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"agreement target", 42);
    assert_eq!(list_signatures(&signed).len(), 1);
    assert!(matches!(
        verify::verify_bytes(&signed, &h.anchor_fps(), now_secs()),
        VerifyOutcome::Valid { .. }
    ));
}
