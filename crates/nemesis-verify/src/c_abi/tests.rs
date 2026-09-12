//! C ABI（nv_*）单测（M5 补测，quality-hardening goal 2026-08-25）。
//!
//! 导出函数是 `unsafe extern "C" fn`（not_unsafe_ptr_arg_deref 公共契约要求），
//! lib target 下进程内调用需 unsafe 包裹（契约同真实 C 调用方：指针合法），
//! 覆盖参数校验 / 状态码映射 / out 参数填充 / 截断规则（subject_meta 64B、
//! publisher 128B）——真正跨 DLL 边界（libloading 加载）的链路由
//! test-tools/verify-loader 覆盖（真机 bin）。
//!
//! env 说明：`builtin_root_anchors()`（lib.rs 单一真相源，S4-2）读运行时
//! `NEMESIS_ROOT_ANCHOR`（值形态 = 根证书 SHA-256 指纹 hex，64 字符）。
//! 涉及 env 的测试共享一把锁串行；不涉及
//! env 的测试只断言 NoSignature/参数错误（在信任判定之前出结果），不受 env 值影响。

use super::*;
use crate::cert::{self, TbsInput};
use crate::envelope::{FORMAT_TAG_RAW, attach_v4};
use crate::fixtures::V4Harness;
use crate::keygen;
use p256::ecdsa::{SigningKey, VerifyingKey};
use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};

/// env 串行锁：指向 crate 根唯一 GLOBAL_STATE_LOCK（S6 批次统一）——
/// 本模块设 NEMESIS_ROOT_ANCHOR，而 verify 流程同时读 NEMESIS_REVOCATION_URL /
/// NEMESIS_STRICT_OFFLINE / CRL_CACHE，跨模块并行会互踩，必须共享一把。
use crate::GLOBAL_STATE_LOCK as ENV_LOCK;
static FILE_SEQ: AtomicU32 = AtomicU32::new(0);

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32].into()).expect("seed is a valid scalar");
    let vk = *sk.verifying_key();
    (sk, vk)
}

/// env 根注入值：根证书 SHA-256 指纹 hex（64 字符，S4-1 形态）。
fn anchor_hex(h: &V4Harness) -> String {
    crate::hex_util::hex_encode(&h.h.root_anchor_fingerprint())
}

/// 写临时文件并返回路径（调用方负责删除）。
fn temp_bytes(bytes: &[u8]) -> std::path::PathBuf {
    let n = FILE_SEQ.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("nv_abi_test_{}_{n}.bin", std::process::id()));
    std::fs::write(&p, bytes).expect("write temp file");
    p
}

fn c_path(p: &std::path::Path) -> CString {
    CString::new(p.to_str().expect("utf-8 path")).expect("no NUL in path")
}

#[test]
fn nv_verify_target_argument_and_io_errors() {
    let mut out = NvOutcome::default();
    let path = c_path(&std::env::temp_dir());
    // null 参数
    assert_eq!(unsafe { nv_verify_target(std::ptr::null(), &mut out) }, -1);
    assert_eq!(
        unsafe { nv_verify_target(path.as_ptr(), std::ptr::null_mut()) },
        -1
    );
    // 非 UTF-8 路径
    let invalid = unsafe { CString::from_vec_unchecked(vec![0xFFu8, 0xFE, 0x00]) };
    assert_eq!(unsafe { nv_verify_target(invalid.as_ptr(), &mut out) }, -2);
    // 文件不存在
    let missing = c_path(&std::env::temp_dir().join("nv_abi_definitely_missing_9527.bin"));
    assert_eq!(unsafe { nv_verify_target(missing.as_ptr(), &mut out) }, -3);
}

#[test]
fn nv_verify_target_no_signature_file() {
    let p = temp_bytes(b"plain unsigned payload".as_ref());
    let mut out = NvOutcome::default();
    let rc = unsafe { nv_verify_target(c_path(&p).as_ptr(), &mut out) };
    let _ = std::fs::remove_file(&p);
    assert_eq!(rc, 0);
    assert_eq!(out.status, NV_NO_SIGNATURE);
    // 非 Valid 的 out 字段清零（Default 分支）
    assert_eq!(out.signed_at, 0);
    assert_eq!(out.key_fp, [0u8; 32]);
}

#[test]
fn nv_verify_target_valid_with_env_root() {
    // 编译期固化根锚（NEMESIS_BUILD_ROOT_ANCHOR）优先于运行时 env——若本
    // 二进制编译时注入了固化锚，这里生成的随机根不可能匹配，跳过（该
    // 形态由 verify-loader 真机链路覆盖）。
    if crate::BUILTIN_ROOT_ANCHOR_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let h = V4Harness::new();
    let signed = h.sign_raw(b"nv abi payload", 424242);
    let p = temp_bytes(&signed);
    unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", anchor_hex(&h)) };

    let mut out = NvOutcome::default();
    let rc = unsafe { nv_verify_target(c_path(&p).as_ptr(), &mut out) };

    unsafe { std::env::remove_var("NEMESIS_ROOT_ANCHOR") };
    let _ = std::fs::remove_file(&p);

    assert_eq!(rc, 0);
    assert_eq!(out.status, NV_VALID, "env 根锚 + CMS 链验证通过");
    assert_eq!(out.signed_at, 424242, "signingTime 属性穿透");
    let leaf_pubkey = crate::crypto::public_key_bytes(&h.h.leaf_vk());
    assert_eq!(out.pubkey, leaf_pubkey, "pubkey = 签名者证书 SPKI");
    // key_fp = SHA-256(pubkey 65B)
    assert_eq!(out.key_fp, crate::crypto::key_fp(&leaf_pubkey));
}

// ===== 最小 PE 构造（端口自 verify/tests.rs base_pe，模块私有不可跨文件）=====

const P: usize = 0x40; // e_lfanew

fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// 无证书表的最小 PE32+：1 个 section（0x200 @ 0x400），nrva=16。
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

#[test]
fn nv_verify_target_valid_pe_certificate_table() {
    // S4-5：C ABI 走 PE Certificate Table 正路径——nv_self_verify 对 PE 形态
    // DLL 的定位判据与之同源（verify_bytes::locate_primary）；libloading 真机
    // 跨 DLL 边界链路归 verify-loader（S5-3 四通路回归）。
    if crate::BUILTIN_ROOT_ANCHOR_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let h = V4Harness::new();
    // Authenticode 语义：CMS messageDigest = PE 的 authenticode digest（排除
    // CheckSum 字段与证书表区），追加证书表只写排除区 → digest 前后不变。
    let pe = base_pe();
    let digest = crate::pe::authenticode_digest(&pe).expect("authenticode digest");
    let cms =
        crate::envelope::build_signed_data(&digest, &h.h.leaf_sk, 31337, &h.h.chain(), None, None)
            .expect("build_signed_data");
    let signed = crate::pe::append_certificate_table(&pe, &cms).expect("append table");

    let p = temp_bytes(&signed);
    unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", anchor_hex(&h)) };
    let mut out = NvOutcome::default();
    let rc = unsafe { nv_verify_target(c_path(&p).as_ptr(), &mut out) };
    unsafe { std::env::remove_var("NEMESIS_ROOT_ANCHOR") };
    let _ = std::fs::remove_file(&p);

    assert_eq!(rc, 0);
    assert_eq!(
        out.status, NV_VALID,
        "PE Certificate Table v4 CMS → C ABI Valid"
    );
    assert_eq!(out.signed_at, 31337);
    let leaf_pubkey = crate::crypto::public_key_bytes(&h.h.leaf_vk());
    assert_eq!(out.pubkey, leaf_pubkey, "pubkey = 签名者证书 SPKI");
}

#[test]
fn nv_self_verify_states() {
    if crate::BUILTIN_ROOT_ANCHOR_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let h = V4Harness::new();
    let signed = h.sign_raw(b"self verify target", 1000);
    let unsigned = temp_bytes(b"unsigned bytes".as_ref());
    let signed_path = temp_bytes(&signed);

    // ① 无根（env 清空）→ -5
    unsafe { std::env::remove_var("NEMESIS_ROOT_ANCHOR") };
    let rc_no_root = unsafe { nv_self_verify(c_path(&signed_path).as_ptr()) };

    // ② 有根锚 + 已签名文件 → 0
    unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", anchor_hex(&h)) };
    let rc_valid = unsafe { nv_self_verify(c_path(&signed_path).as_ptr()) };
    // ③ 有根锚 + 未签名文件 → -4
    let rc_unsigned = unsafe { nv_self_verify(c_path(&unsigned).as_ptr()) };
    // ④ null → -1
    let rc_null = unsafe { nv_self_verify(std::ptr::null()) };

    unsafe { std::env::remove_var("NEMESIS_ROOT_ANCHOR") };
    let _ = std::fs::remove_file(&signed_path);
    let _ = std::fs::remove_file(&unsigned);

    assert_eq!(rc_no_root, -5, "no builtin/env root → -5");
    assert_eq!(rc_valid, 0, "signed with matching root → 0");
    assert_eq!(rc_unsigned, -4, "unsigned → verify fails → -4");
    assert_eq!(rc_null, -1);
}

#[test]
fn nv_verify_current_exe_is_unsigned_in_tests() {
    // null out 参数 → -1（参数校验臂，current_exe 之前）
    assert_eq!(unsafe { nv_verify_current_exe(std::ptr::null_mut()) }, -1);
    // 测试二进制自身无签名 → 读 exe + 验证完成（0），状态 NoSignature。
    let mut out = NvOutcome::default();
    let rc = unsafe { nv_verify_current_exe(&mut out) };
    assert_eq!(rc, 0);
    assert_eq!(out.status, NV_NO_SIGNATURE);
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：run_verify 状态映射全臂、
// 非法 env 根 hex 的空列表回退、nv_self_verify IO 错误码。
// （注意：lib+cdylib 双编译单元下 nv_* 的行覆盖数字不可信——lcov 对
// no_mangle 同名符号只保留一条记录；测试本身仍验证真实行为。）
// ---------------------------------------------------------------------------

#[test]
fn nv_verify_target_maps_all_outcome_statuses() {
    use cms::content_info::{CmsVersion, ContentInfo};
    use cms::signed_data::SignedData;
    use der::{Decode, Encode};

    if crate::BUILTIN_ROOT_ANCHOR_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let h = V4Harness::new();
    unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", anchor_hex(&h)) };

    let mut out = NvOutcome::default();
    let status_of = |bytes: &[u8], out: &mut NvOutcome| {
        let p = temp_bytes(bytes);
        let rc = unsafe { nv_verify_target(c_path(&p).as_ptr(), out) };
        let _ = std::fs::remove_file(&p);
        assert_eq!(rc, 0);
        out.status
    };

    // 基准：Valid
    let valid = h.sign_raw(b"map target", 1000);
    assert_eq!(status_of(&valid, &mut out), NV_VALID);
    assert_eq!(out.signed_at, 1000);

    // Tampered：内容域字节翻转（载体 content_hash 失配）
    let mut tampered = valid.clone();
    tampered[2] ^= 0x01;
    assert_eq!(status_of(&tampered, &mut out), NV_TAMPERED);

    // SignatureInvalid：sid 指向 leaf 证书、签名出自别把私钥
    let foreign = SigningKey::from_bytes(&[0x5Bu8; 32].into()).unwrap();
    let sig_bad = h.sign_raw_with(b"map target", 1000, &foreign, &h.h.chain());
    assert_eq!(status_of(&sig_bad, &mut out), NV_SIGNATURE_INVALID);

    // UnsupportedVersion：SignedData.version V2（version 不在签名覆盖面，可改）
    let cms = h.build_cms(b"map target", 1000, &h.h.leaf_sk, &h.h.chain());
    let mut ci = ContentInfo::from_der(cms.as_slice()).unwrap();
    let mut sd = SignedData::from_der(ci.content.to_der().unwrap().as_slice()).unwrap();
    sd.version = CmsVersion::V2;
    ci.content = der::Any::from_der(sd.to_der().unwrap().as_slice()).unwrap();
    let ver2 = attach_v4(
        b"map target".as_slice(),
        &ci.to_der().unwrap(),
        FORMAT_TAG_RAW,
        b"map target".len(),
    );
    assert_eq!(status_of(&ver2, &mut out), NV_UNSUPPORTED_VERSION);

    // Malformed：footer 在但 CMS DER 不可解析
    let bad_body = attach_v4(
        b"map target".as_slice(),
        b"not-a-der",
        FORMAT_TAG_RAW,
        b"map target".len(),
    );
    assert_eq!(status_of(&bad_body, &mut out), NV_MALFORMED);

    // Expired：整链签发于过去（leaf 3y 限已过）→ c_abi 用真实系统时间验证
    let t0 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 100_000_000; // ≈3.17y 前 > leaf 有效期（3y + 1h backdate）
    let expired_h = V4Harness {
        h: keygen::generate_at(t0).unwrap(),
    };
    let expired = expired_h.sign_raw(b"expired map target", t0 + 1000);
    assert_eq!(status_of(&expired, &mut out), NV_EXPIRED);

    // Untrusted：锚集只有别的根（清换 env 锚）
    let other = V4Harness::new();
    unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", anchor_hex(&other)) };
    assert_eq!(status_of(&valid, &mut out), NV_UNTRUSTED);

    unsafe { std::env::remove_var("NEMESIS_ROOT_ANCHOR") };
}

#[test]
fn nv_verify_target_invalid_env_root_hex_yields_empty_roots() {
    if crate::BUILTIN_ROOT_ANCHOR_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let h = V4Harness::new();
    let signed = h.sign_raw(b"invalid env root", 1);
    let p = temp_bytes(&signed);
    // 非法 hex → hex_decode_32 Err → 空锚集（不 panic）
    unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", "not-hex!") };
    let mut out = NvOutcome::default();
    let rc = unsafe { nv_verify_target(c_path(&p).as_ptr(), &mut out) };
    unsafe { std::env::remove_var("NEMESIS_ROOT_ANCHOR") };
    let _ = std::fs::remove_file(&p);
    assert_eq!(rc, 0, "非法 env 锚是软失败：流程照常完成");
    assert_eq!(out.status, NV_UNTRUSTED, "空锚集 → 任何签名都 Untrusted");
}

#[test]
fn nv_self_verify_io_error_codes() {
    // 非 UTF-8 路径 → -2；文件不存在 → -3（都在根解析之前，无需 env）
    let invalid = unsafe { CString::from_vec_unchecked(vec![0xFFu8, 0xFE, 0x00]) };
    assert_eq!(unsafe { nv_self_verify(invalid.as_ptr()) }, -2);
    let missing = c_path(&std::env::temp_dir().join("nv_abi_missing_self_9527.bin"));
    assert_eq!(unsafe { nv_self_verify(missing.as_ptr()) }, -3);
}

#[test]
fn nv_verify_target_maps_revoked_via_local_crl_server() {
    if crate::BUILTIN_ROOT_ANCHOR_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let h = V4Harness::new();
    unsafe { std::env::set_var("NEMESIS_ROOT_ANCHOR", anchor_hex(&h)) };

    // 本地 CRL 服务器：吊销签名者密钥（key_fp 维度）；CRL 由根私钥签（客户端验签前提）
    let leaf_pubkey = crate::crypto::public_key_bytes(&h.h.leaf_vk());
    let fp = crate::crypto::key_fp(&leaf_pubkey);
    let crl = crate::sign_response(
        &crate::Crl {
            version: 1,
            valid_until: u64::MAX,
            entries: vec![crate::CrlEntry {
                dim: crate::RevDim::KeyFp,
                value: crate::hex_util::hex_encode(&fp),
                revoked_at: 3,
                reason: "leak".into(),
            }],
        },
        &h.h.root_sk,
    )
    .unwrap();
    let body = serde_json::to_string(&crl).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for stream in listener.incoming() {
            let mut s = match stream {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut buf = [0u8; 8192];
            let _ = s.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    unsafe { std::env::set_var("NEMESIS_REVOCATION_URL", &base) };

    let signed = h.sign_raw(b"revoked mapping", 1000);
    let p = temp_bytes(&signed);
    let mut out = NvOutcome::default();
    let rc = unsafe { nv_verify_target(c_path(&p).as_ptr(), &mut out) };
    let _ = std::fs::remove_file(&p);

    unsafe {
        std::env::remove_var("NEMESIS_REVOCATION_URL");
        std::env::remove_var("NEMESIS_ROOT_ANCHOR");
    }
    assert_eq!(rc, 0);
    assert_eq!(out.status, NV_REVOKED, "CRL 命中 key_fp → NV_REVOKED");
}

/// v4 链 fixture：真实三级体系（keygen::generate），chain=[leaf, issuing, root]；
/// publisher = CMS opus programName（v4 形态）。
fn signed_with_chain() -> Vec<u8> {
    let h = V4Harness::new();
    h.sign_raw_opus(
        b"listed payload",
        777,
        &h.h.leaf_sk,
        &h.h.chain(),
        Some("org-publisher"),
        None,
    )
}

#[test]
fn nv_list_signatures_counts_and_args() {
    let signed = signed_with_chain();
    let p = temp_bytes(&signed);
    let path = c_path(&p);

    // null 参数 → -1
    let mut infos: [NvSigInfo; 4] = std::array::from_fn(|_| NvSigInfo::default());
    let mut count: u32 = 4;
    assert_eq!(
        unsafe { nv_list_signatures(std::ptr::null(), infos.as_mut_ptr(), &mut count) },
        -1
    );
    assert_eq!(
        unsafe { nv_list_signatures(path.as_ptr(), std::ptr::null_mut(), &mut count) },
        -1
    );
    assert_eq!(
        unsafe { nv_list_signatures(path.as_ptr(), infos.as_mut_ptr(), std::ptr::null_mut()) },
        -1
    );
    // 文件不存在 → -3
    let missing = c_path(&std::env::temp_dir().join("nv_abi_missing_list.bin"));
    assert_eq!(
        unsafe { nv_list_signatures(missing.as_ptr(), infos.as_mut_ptr(), &mut count) },
        -3
    );

    // 未签名文件 → 0 + count=0
    let plain = temp_bytes(b"no signature".as_ref());
    let mut count: u32 = 4;
    let rc = unsafe { nv_list_signatures(c_path(&plain).as_ptr(), infos.as_mut_ptr(), &mut count) };
    let _ = std::fs::remove_file(&plain);
    assert_eq!(rc, 0);
    assert_eq!(count, 0);

    // 单签名文件 → count=1，字段穿透（signed_at / key_fp / pubkey）
    let mut infos: [NvSigInfo; 4] = std::array::from_fn(|_| NvSigInfo::default());
    let mut count: u32 = 4;
    let rc = unsafe { nv_list_signatures(path.as_ptr(), infos.as_mut_ptr(), &mut count) };
    let _ = std::fs::remove_file(&p);
    assert_eq!(rc, 0);
    assert_eq!(count, 1);
    assert_eq!(infos[0].index, 0);
    assert_eq!(infos[0].signed_at, 777);
    assert_ne!(infos[0].key_fp, [0u8; 32]);

    // 容量 0（out 非空但 *count=0）：不写数组，count 仍报总数
    let p2 = temp_bytes(&signed);
    let mut one = NvSigInfo::default();
    let mut count_zero: u32 = 0;
    let rc0 = unsafe { nv_list_signatures(c_path(&p2).as_ptr(), &mut one, &mut count_zero) };
    let _ = std::fs::remove_file(&p2);
    assert_eq!(rc0, 0);
    assert_eq!(count_zero, 1, "capacity 0 writes nothing but reports total");
    assert_eq!(one.index, 0, "out array untouched at capacity 0");
}

#[test]
fn nv_get_signature_detail_with_chain_and_truncation() {
    let signed = signed_with_chain();
    let p = temp_bytes(&signed);
    let path = c_path(&p);

    // index 越界 → -4；null → -1
    let mut detail = NvSigDetail::default();
    assert_eq!(
        unsafe { nv_get_signature(path.as_ptr(), 9, &mut detail) },
        -4
    );
    assert_eq!(
        unsafe { nv_get_signature(std::ptr::null(), 0, &mut detail) },
        -1
    );
    assert_eq!(
        unsafe { nv_get_signature(path.as_ptr(), 0, std::ptr::null_mut()) },
        -1
    );

    // 正常详情：cert_count=3（chain=[leaf, issuing, root]，v4 含根）、
    // publisher 穿透、leaf CN 穿透
    let rc = unsafe { nv_get_signature(path.as_ptr(), 0, &mut detail) };
    let _ = std::fs::remove_file(&p);
    assert_eq!(rc, 0);
    assert_eq!(detail.cert_count, 3);
    assert_eq!(detail.signed_at, 777);
    let plen = detail.publisher_len as usize;
    assert_eq!(plen, "org-publisher".len());
    assert_eq!(&detail.publisher[..plen], b"org-publisher".as_slice());
    let mlen = detail.certs[0].subject_meta_len as usize;
    assert_eq!(mlen, keygen::CN_LEAF.len());
    assert_eq!(
        &detail.certs[0].subject_meta[..mlen],
        keygen::CN_LEAF.as_bytes()
    );

    // 超长截断：publisher > 128B → 128；subject meta > 64B → 64。
    // CN 用 issue_x509 直配（keygen 固定 profile 出不了长 CN）；
    // view 路径不验签，单证书集即可（链断 → 降级证书集原序展示）。
    let h = V4Harness::new();
    let (root_sk, root_vk) = keypair(5);
    let (leaf_sk, leaf_vk) = keypair(6);
    let long_meta: &'static str = Box::leak("x".repeat(100).into_boxed_str());
    // GeneralizedTime 编不出 year>9999 → not_after 用安全大值（2100 年）
    let far_future: u64 = 4_102_444_800;
    let long_cn_cert = keygen::issue_x509(
        &leaf_vk,
        &root_sk,
        &cert::ski_value(&root_vk).unwrap(),
        TbsInput {
            subject_cn: long_meta,
            subject_org: None,
            issuer_cn: "T",
            issuer_org: None,
            is_ca: false,
            path_len: None,
            ku_digital_signature: true,
            ku_key_cert_sign: false,
            ku_crl_sign: false,
            eku_code_signing: true,
            not_before_unix: 0,
            not_after_unix: far_future,
        },
    )
    .unwrap();
    let long_publisher = "p".repeat(200);
    let signed2 = h.sign_raw_opus(
        b"truncation target",
        1,
        &leaf_sk,
        &[long_cn_cert],
        Some(&long_publisher),
        None,
    );
    let p2 = temp_bytes(&signed2);
    let mut detail2 = NvSigDetail::default();
    let rc2 = unsafe { nv_get_signature(c_path(&p2).as_ptr(), 0, &mut detail2) };
    let _ = std::fs::remove_file(&p2);
    assert_eq!(rc2, 0);
    assert_eq!(detail2.publisher_len, 128, "publisher capped at 128");
    assert_eq!(&detail2.publisher[..128], &long_publisher.as_bytes()[..128]);
    assert_eq!(
        detail2.certs[0].subject_meta_len, 64,
        "subject meta capped at 64"
    );
    assert_eq!(
        &detail2.certs[0].subject_meta[..64],
        &long_meta.as_bytes()[..64]
    );
}
