use super::*;

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key();
    (sk, vk)
}

#[test]
fn sign_verify_raw_valid() {
    let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap(); // verify 流程读 revocation env
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
            let expected_fp: [u8; 32] = sha2::Sha256::digest(vk.to_bytes()).into();
            assert_eq!(key_fp, expected_fp);
        }
        o => panic!("expected Valid, got {:?}", o),
    }
}

#[test]
fn sign_with_chain_valid() {
    let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
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
    let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
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
    let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
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

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：手工改 footer / body 字节
// 驱动 verify_bytes 的全部错误出口。这些出口全部位于吊销检查之前，不依赖 env。
// ---------------------------------------------------------------------------

/// 与 envelope::crc32 相同的 IEEE 802.3 实现（envelope 内私有不可 import，此处
/// 镜像；一致性由「patch 后 parse_footer 必须成功」的测试钉住）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// 修改 footer 字段后重算 CRC（footer[24..28) = crc32(footer[0..24))）。
fn patch_footer_crc(file: &mut [u8], footer_off: usize) {
    let crc = crc32(&file[footer_off..footer_off + 24]);
    file[footer_off + 24..footer_off + 28].copy_from_slice(&crc.to_le_bytes());
}

/// raw 签名文件的 footer 偏移（单 envelope，footer 在文件末尾）。
fn footer_off_of(signed: &[u8]) -> usize {
    signed.len() - envelope::FOOTER_LEN
}

/// 探测一个 VerifyingKey::from_bytes 拒绝的 32B 编码（确定性：SHA-256(i) 序列）。
fn rejected_pubkey() -> [u8; 32] {
    for i in 0u8..=255 {
        let cand: [u8; 32] = sha2::Sha256::digest([i]).into();
        if VerifyingKey::from_bytes(&cand).is_err() {
            return cand;
        }
    }
    panic!("SHA-256(0..=255) 中必存在被拒的非规范编码");
}

#[test]
fn footer_crc_corrupt_is_tampered() {
    let (sk, vk) = keypair(11);
    let mut signed = sign_content(b"crc target", &sk, 1000, None, None, None, None).unwrap();
    let fo = footer_off_of(&signed);
    signed[fo + 12] ^= 0x01; // total_len 字节在 CRC 覆盖范围内，不重算 CRC
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Tampered(m) => assert!(m.contains("footer"), "{m}"),
        o => panic!("expected Tampered(footer), got {:?}", o),
    }
}

#[test]
fn footer_version_patch_is_unsupported_version() {
    let (sk, vk) = keypair(12);
    let mut signed = sign_content(b"ver target", &sk, 1000, None, None, None, None).unwrap();
    let fo = footer_off_of(&signed);
    signed[fo + 8] = 2; // format_ver=2（伪 v2 footer，magic 仍是 v3）
    patch_footer_crc(&mut signed, fo);
    assert_eq!(
        verify_bytes(&signed, &[vk], 1000),
        VerifyOutcome::UnsupportedVersion(2)
    );
}

#[test]
fn body_len_overrun_is_malformed() {
    let (sk, vk) = keypair(13);
    let mut signed = sign_content(b"range target", &sk, 1000, None, None, None, None).unwrap();
    let fo = footer_off_of(&signed);
    // body_len = 0x10000 → 超界 → BUG S6-1 修复后钳成 (0,0) 空区间 →
    // parse_body 空 body → Malformed("body: body too short")
    signed[fo + 16..fo + 20].copy_from_slice(&0x10000u32.to_le_bytes());
    patch_footer_crc(&mut signed, fo);
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("body too short"), "{m}"),
        o => panic!("expected Malformed(range), got {:?}", o),
    }
}

#[test]
fn crafted_total_len_underflow_is_malformed() {
    // BUG S6-1 回归钉：total_len > footer_offset+FOOTER_LEN 时修复前是
    // usize 下溢（debug panic 'attempt to subtract with overflow'）。
    let (sk, vk) = keypair(23);
    let mut signed = sign_content(b"underflow target", &sk, 1000, None, None, None, None).unwrap();
    let fo = footer_off_of(&signed);
    signed[fo + 12..fo + 16].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes()); // total_len 超界
    patch_footer_crc(&mut signed, fo);
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("body too short"), "{m}"),
        o => panic!("expected Malformed(range), got {:?}", o),
    }
}

#[test]
fn crafted_small_total_len_with_big_body_len_is_out_of_bounds() {
    // BUG S6-1 钳制的边界补充钉：total_len/body_len 各自 ≤ avail 时钳制放行，
    // 但 start+body_len 仍可越过文件末尾（total_len=FOOTER_LEN、body_len=avail
    // → body_end = footer_off+avail > len）——bytes.get() 兜底臂必须接住，
    // 归 Malformed("body range out of bounds")，不能 panic。
    let (sk, vk) = keypair(24);
    let mut signed = sign_content(b"bounds target", &sk, 1000, None, None, None, None).unwrap();
    let fo = footer_off_of(&signed);
    let avail = fo + crate::envelope::FOOTER_LEN;
    signed[fo + 12..fo + 16].copy_from_slice(&(crate::envelope::FOOTER_LEN as u32).to_le_bytes()); // total_len=64
    signed[fo + 16..fo + 20].copy_from_slice(&(avail as u32).to_le_bytes()); // body_len=avail
    patch_footer_crc(&mut signed, fo);
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("body range out of bounds"), "{m}"),
        o => panic!("expected Malformed(out of bounds), got {:?}", o),
    }
}

#[test]
fn body_too_short_is_malformed() {
    let (sk, vk) = keypair(14);
    let mut signed = sign_content(b"short body", &sk, 1000, None, None, None, None).unwrap();
    let fo = footer_off_of(&signed);
    signed[fo + 16..fo + 20].copy_from_slice(&10u32.to_le_bytes()); // body_len=10 < 172
    patch_footer_crc(&mut signed, fo);
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("body too short"), "{m}"),
        o => panic!("expected Malformed(body), got {:?}", o),
    }
}

#[test]
fn raw_content_len_exceeds_file_is_malformed() {
    let (sk, vk) = keypair(15);
    let mut signed = sign_content(b"clen target", &sk, 1000, None, None, None, None).unwrap();
    let fo = footer_off_of(&signed);
    let over = (signed.len() + 1000) as u32;
    signed[fo + 20..fo + 24].copy_from_slice(&over.to_le_bytes());
    patch_footer_crc(&mut signed, fo);
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("content_len"), "{m}"),
        o => panic!("expected Malformed(content_len), got {:?}", o),
    }
}

#[test]
fn broken_pe_falls_back_to_raw_then_fails_content_hash() {
    // MZ 前缀但 PE 结构非法 → codec.compute_l Err → 按 Raw 处理（overlay_start=0）
    // → footer 仍可定位 → body 可解析 → 但 content_hash 阶段 PeCodec 解析失败
    // → Malformed("content_hash: ...")。同时证明 Err 分支不会 panic / NoSignature。
    let (sk, _) = keypair(16);
    let signed = sign_content(b"raw payload", &sk, 1000, None, None, None, None).unwrap();
    let env_only = &signed[signed.len() - envelope::ENVELOPE_ALIGN..];
    let mut f = b"MZ".to_vec();
    f.extend(std::iter::repeat_n(0u8, 0x100 - 2)); // e_lfanew=0 → 无 PE 签名
    f.extend_from_slice(env_only);
    match verify_bytes(&f, &[sk.verifying_key()], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("content_hash"), "{m}"),
        o => panic!("expected Malformed(content_hash), got {:?}", o),
    }
}

#[test]
fn noncanonical_pubkey_bytes_is_malformed() {
    // body.pubkey 换成解不出曲线点的 32B → VerifyingKey::from_bytes Err →
    // Malformed（发生在验签之前，无需伪造签名）。
    let (sk, vk) = keypair(17);
    let mut signed = sign_content(b"bad pubkey", &sk, 1000, None, None, None, None).unwrap();
    let fo = footer_off_of(&signed);
    let mut f = [0u8; envelope::FOOTER_LEN];
    f.copy_from_slice(&signed[fo..]);
    let pf = envelope::parse_footer(&f).unwrap();
    let body_start = fo + envelope::FOOTER_LEN - pf.total_len;
    signed[body_start + 76..body_start + 108].copy_from_slice(&rejected_pubkey());
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("invalid pubkey bytes"), "{m}"),
        o => panic!("expected Malformed(pubkey), got {:?}", o),
    }
}

#[test]
fn cert_chain_window_expired_is_expired() {
    let (root_sk, root_vk) = keypair(18);
    let (leaf_sk, leaf_vk) = keypair(19);
    let leaf_cert = cert::issue_certificate(&root_sk, &leaf_vk.to_bytes(), b"issuer", 100, 200);
    let chain = cert::serialize_chain(&[leaf_cert]);
    let signed = sign_content(b"expired chain", &leaf_sk, 1000, Some(&chain), None, None, None).unwrap();
    match verify_bytes(&signed, &[root_vk], 500) {
        VerifyOutcome::Expired(m) => assert!(m.contains("certificate expired"), "{m}"),
        o => panic!("expected Expired(chain), got {:?}", o),
    }
}

#[test]
fn garbage_cert_chain_is_malformed() {
    // 签名时就把 chain 当垃圾（签名覆盖 cert_chain_hash，自洽）→ 验签通过后
    // parse_chain 失败 → Malformed("cert_chain parse failed")。
    let (sk, vk) = keypair(20);
    let garbage: &[u8] = &[0x00, 0x01]; // count=1 但无 cert_len
    let signed = sign_content(b"garbage chain", &sk, 1000, Some(garbage), None, None, None).unwrap();
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("cert_chain parse failed"), "{m}"),
        o => panic!("expected Malformed(chain), got {:?}", o),
    }
}

#[test]
fn key_not_after_exceeded_is_expired() {
    let (sk, vk) = keypair(21);
    let signed = sign_content(b"kna target", &sk, 1000, None, None, Some(1500), None).unwrap();
    match verify_bytes(&signed, &[vk], 2000) {
        VerifyOutcome::Expired(m) => assert!(m.contains("key_not_after"), "{m}"),
        o => panic!("expected Expired(kna), got {:?}", o),
    }
    // now ≤ kna → 仍 Valid
    let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    assert!(matches!(verify_bytes(&signed, &[vk], 1400), VerifyOutcome::Valid { .. }));
}

/// 最小合法 PE32（与 pe/tests.rs 的 build_pe 同构的紧凑版）。
fn make_min_pe() -> Vec<u8> {
    const P: usize = 0x40;
    let size_of_opt = 224usize;
    let sec_tbl = P + 24 + size_of_opt;
    let raw_ptr = sec_tbl + 40;
    let mut b = vec![0u8; raw_ptr + 0x100];
    b[0] = b'M';
    b[1] = b'Z';
    b[0x3C..0x40].copy_from_slice(&(P as u32).to_le_bytes());
    b[P..P + 4].copy_from_slice(b"PE\0\0");
    b[P + 6..P + 8].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
    b[P + 20..P + 22].copy_from_slice(&(size_of_opt as u16).to_le_bytes());
    b[P + 24..P + 26].copy_from_slice(&0x10bu16.to_le_bytes()); // PE32
    b[P + 116..P + 120].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes
    b[sec_tbl + 16..sec_tbl + 20].copy_from_slice(&0x100u32.to_le_bytes()); // SizeOfRawData
    b[sec_tbl + 20..sec_tbl + 24].copy_from_slice(&(raw_ptr as u32).to_le_bytes()); // PointerToRawData
    b
}

#[test]
fn pe_roundtrip_valid_and_content_len_mismatch_malformed() {
    let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let (sk, vk) = keypair(22);
    let pe = make_min_pe();
    let mut signed = sign_content(&pe, &sk, 1000, None, None, None, None).unwrap();
    let l = codec::detect_codec(&signed).compute_l(&signed).unwrap().unwrap();

    // PE 的 sign+verify 全链路 → Valid
    assert!(matches!(verify_bytes(&signed, &[vk], 1000), VerifyOutcome::Valid { .. }));

    // footer.content_len ≠ L → Malformed（PE 内容长度与结构 L 必须一致）
    let fo = footer_off_of(&signed);
    signed[fo + 20..fo + 24].copy_from_slice(&((l + 1) as u32).to_le_bytes());
    patch_footer_crc(&mut signed, fo);
    match verify_bytes(&signed, &[vk], 1000) {
        VerifyOutcome::Malformed(m) => assert!(m.contains("content_len"), "{m}"),
        o => panic!("expected Malformed(content_len != L), got {:?}", o),
    }
}
