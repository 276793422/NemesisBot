//! C ABI（nv_*）单测（M5 补测，quality-hardening goal 2026-08-25）。
//!
//! extern "C" 函数在 lib target 下就是普通 Rust 函数，直接进程内调用即可
//! 覆盖参数校验 / 状态码映射 / out 参数填充 / 截断规则（subject_meta 64B、
//! publisher 128B）——真正跨 DLL 边界（libloading 加载）的链路由
//! test-tools/verify-loader 覆盖（真机 bin）。
//!
//! env 说明：`builtin_roots()` 读 `NEMESIS_ROOT_PUBKEY`。涉及 env 的测试
//! 共享一把锁串行；不涉及 env 的测试只断言 NoSignature/参数错误（在
//! 信任判定之前出结果），不受 env 值影响。

use super::*;
use crate::verify::sign_content;
use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::Digest;
use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};

/// env 串行锁：指向 crate 根唯一 GLOBAL_STATE_LOCK（S6 批次统一）——
/// 本模块设 NEMESIS_ROOT_PUBKEY，而 verify 流程同时读 NEMESIS_REVOCATION_URL /
/// NEMESIS_STRICT_OFFLINE / CRL_CACHE，跨模块并行会互踩，必须共享一把。
use crate::GLOBAL_STATE_LOCK as ENV_LOCK;
static FILE_SEQ: AtomicU32 = AtomicU32::new(0);

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key();
    (sk, vk)
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
    assert_eq!(nv_verify_target(std::ptr::null(), &mut out), -1);
    assert_eq!(
        nv_verify_target(path.as_ptr(), std::ptr::null_mut()),
        -1
    );
    // 非 UTF-8 路径
    let invalid = unsafe { CString::from_vec_unchecked(vec![0xFFu8, 0xFE, 0x00]) };
    assert_eq!(nv_verify_target(invalid.as_ptr(), &mut out), -2);
    // 文件不存在
    let missing = c_path(&std::env::temp_dir().join("nv_abi_definitely_missing_9527.bin"));
    assert_eq!(nv_verify_target(missing.as_ptr(), &mut out), -3);
}

#[test]
fn nv_verify_target_no_signature_file() {
    let p = temp_bytes(b"plain unsigned payload".as_ref());
    let mut out = NvOutcome::default();
    let rc = nv_verify_target(c_path(&p).as_ptr(), &mut out);
    let _ = std::fs::remove_file(&p);
    assert_eq!(rc, 0);
    assert_eq!(out.status, NV_NO_SIGNATURE);
    // 非 Valid 的 out 字段清零（Default 分支）
    assert_eq!(out.signed_at, 0);
    assert_eq!(out.key_fp, [0u8; 32]);
}

#[test]
fn nv_verify_target_valid_with_env_root() {
    // 编译期固化根（NEMESIS_BUILD_ROOT_PUBKEY）优先于运行时 env——若本
    // 二进制编译时注入了固化根，这里生成的随机根不可能匹配，跳过（该
    // 形态由 verify-loader 真机链路覆盖）。
    if BUILTIN_ROOT_PUBKEY_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let (sk, vk) = keypair(7);
    let signed = sign_content(b"nv abi payload", &sk, 424242, None, None, None, None).unwrap();
    let p = temp_bytes(&signed);
    unsafe { std::env::set_var("NEMESIS_ROOT_PUBKEY", crate::hex_util::hex_encode(&vk.to_bytes())) };

    let mut out = NvOutcome::default();
    let rc = nv_verify_target(c_path(&p).as_ptr(), &mut out);

    unsafe { std::env::remove_var("NEMESIS_ROOT_PUBKEY") };
    let _ = std::fs::remove_file(&p);

    assert_eq!(rc, 0);
    assert_eq!(out.status, NV_VALID, "env root + envelope pubkey 验签通过");
    assert_eq!(out.signed_at, 424242);
    assert_eq!(out.pubkey, vk.to_bytes());
    // key_fp = SHA-256(pubkey)
    let fp: [u8; 32] = sha2::Sha256::digest(&vk.to_bytes()).into();
    assert_eq!(out.key_fp, fp);
}

#[test]
fn nv_self_verify_states() {
    if BUILTIN_ROOT_PUBKEY_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let (sk, vk) = keypair(3);
    let signed = sign_content(b"self verify target", &sk, 1000, None, None, None, None).unwrap();
    let unsigned = temp_bytes(b"unsigned bytes".as_ref());
    let signed_path = temp_bytes(&signed);

    // ① 无根（env 清空）→ -5
    unsafe { std::env::remove_var("NEMESIS_ROOT_PUBKEY") };
    let rc_no_root = nv_self_verify(c_path(&signed_path).as_ptr());

    // ② 有根 + 已签名文件 → 0
    unsafe { std::env::set_var("NEMESIS_ROOT_PUBKEY", crate::hex_util::hex_encode(&vk.to_bytes())) };
    let rc_valid = nv_self_verify(c_path(&signed_path).as_ptr());
    // ③ 有根 + 未签名文件 → -4
    let rc_unsigned = nv_self_verify(c_path(&unsigned).as_ptr());
    // ④ null → -1
    let rc_null = nv_self_verify(std::ptr::null());

    unsafe { std::env::remove_var("NEMESIS_ROOT_PUBKEY") };
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
    assert_eq!(nv_verify_current_exe(std::ptr::null_mut()), -1);
    // 测试二进制自身无签名 → 读 exe + 验证完成（0），状态 NoSignature。
    let mut out = NvOutcome::default();
    let rc = nv_verify_current_exe(&mut out);
    assert_eq!(rc, 0);
    assert_eq!(out.status, NV_NO_SIGNATURE);
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：run_verify 状态映射全臂、
// 非法 env 根 hex 的空列表回退、nv_self_verify IO 错误码。
// （注意：lib+cdylib 双编译单元下 nv_* 的行覆盖数字不可信——lcov 对
// no_mangle 同名符号只保留一条记录；测试本身仍验证真实行为。）
// ---------------------------------------------------------------------------

/// IEEE 802.3 CRC32（envelope 私有实现的镜像，与 verify/tests.rs 同款）。
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

fn patch_footer_crc(file: &mut [u8], footer_off: usize) {
    let crc = crc32(&file[footer_off..footer_off + 24]);
    file[footer_off + 24..footer_off + 28].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn nv_verify_target_maps_all_outcome_statuses() {
    if BUILTIN_ROOT_PUBKEY_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let (sk, vk) = keypair(51);
    unsafe { std::env::set_var("NEMESIS_ROOT_PUBKEY", crate::hex_util::hex_encode(&vk.to_bytes())) };

    let mut out = NvOutcome::default();
    let status_of = |bytes: &[u8], out: &mut NvOutcome| {
        let p = temp_bytes(bytes);
        let rc = nv_verify_target(c_path(&p).as_ptr(), out);
        let _ = std::fs::remove_file(&p);
        assert_eq!(rc, 0);
        out.status
    };

    // 基准：Valid
    let valid = sign_content(b"map target", &sk, 1000, None, None, None, None).unwrap();
    assert_eq!(status_of(&valid, &mut out), NV_VALID);
    assert_eq!(out.signed_at, 1000);

    // Tampered：footer CRC 破坏
    let mut tampered = valid.clone();
    let fo = tampered.len() - 64;
    tampered[fo + 12] ^= 0x01;
    assert_eq!(status_of(&tampered, &mut out), NV_TAMPERED);

    // SignatureInvalid：body 签名字节篡改
    let mut sig_bad = valid.clone();
    let body_start = sig_bad.len() - 4096;
    sig_bad[body_start + 108] ^= 0x01;
    assert_eq!(status_of(&sig_bad, &mut out), NV_SIGNATURE_INVALID);

    // UnsupportedVersion：format_ver=2 + CRC 重算
    let mut ver2 = valid.clone();
    ver2[fo + 8] = 2;
    patch_footer_crc(&mut ver2, fo);
    assert_eq!(status_of(&ver2, &mut out), NV_UNSUPPORTED_VERSION);

    // Malformed：body_len=10 + CRC 重算
    let mut bad_body = valid.clone();
    bad_body[fo + 16..fo + 20].copy_from_slice(&10u32.to_le_bytes());
    patch_footer_crc(&mut bad_body, fo);
    assert_eq!(status_of(&bad_body, &mut out), NV_MALFORMED);

    // Expired：key_not_after=1，now 是真实系统时间（必然 > 1）
    let expired = sign_content(b"map target", &sk, 1000, None, None, Some(1), None).unwrap();
    assert_eq!(status_of(&expired, &mut out), NV_EXPIRED);

    // Untrusted：签名自洽但根是另一把（清掉 env 根 → 空根列表）
    unsafe { std::env::set_var("NEMESIS_ROOT_PUBKEY", crate::hex_util::hex_encode(&keypair(52).1.to_bytes())) };
    assert_eq!(status_of(&valid, &mut out), NV_UNTRUSTED);

    unsafe { std::env::remove_var("NEMESIS_ROOT_PUBKEY") };
}

#[test]
fn nv_verify_target_invalid_env_root_hex_yields_empty_roots() {
    if BUILTIN_ROOT_PUBKEY_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let (sk, _) = keypair(53);
    let signed = sign_content(b"invalid env root", &sk, 1, None, None, None, None).unwrap();
    let p = temp_bytes(&signed);
    // 非法 hex → hex_decode_32 Err → 空根列表（不 panic）
    unsafe { std::env::set_var("NEMESIS_ROOT_PUBKEY", "not-hex!") };
    let mut out = NvOutcome::default();
    let rc = nv_verify_target(c_path(&p).as_ptr(), &mut out);
    unsafe { std::env::remove_var("NEMESIS_ROOT_PUBKEY") };
    let _ = std::fs::remove_file(&p);
    assert_eq!(rc, 0, "非法 env 根是软失败：流程照常完成");
    assert_eq!(out.status, NV_UNTRUSTED, "空根列表 → 任何签名都 Untrusted");
}

#[test]
fn nv_self_verify_io_error_codes() {
    // 非 UTF-8 路径 → -2；文件不存在 → -3（都在根解析之前，无需 env）
    let invalid = unsafe { CString::from_vec_unchecked(vec![0xFFu8, 0xFE, 0x00]) };
    assert_eq!(nv_self_verify(invalid.as_ptr()), -2);
    let missing = c_path(&std::env::temp_dir().join("nv_abi_missing_self_9527.bin"));
    assert_eq!(nv_self_verify(missing.as_ptr()), -3);
}

#[test]
fn nv_verify_target_maps_revoked_via_local_crl_server() {
    if BUILTIN_ROOT_PUBKEY_HEX.is_some() {
        return;
    }
    let _g = ENV_LOCK.lock().unwrap();
    let (sk, vk) = keypair(54);
    unsafe { std::env::set_var("NEMESIS_ROOT_PUBKEY", crate::hex_util::hex_encode(&vk.to_bytes())) };

    // 本地 CRL 服务器：吊销本密钥（key_fp 维度）
    let fp: [u8; 32] = sha2::Sha256::digest(&vk.to_bytes()).into();
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
        &sk,
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

    let signed = sign_content(b"revoked mapping", &sk, 1000, None, None, None, None).unwrap();
    let p = temp_bytes(&signed);
    let mut out = NvOutcome::default();
    let rc = nv_verify_target(c_path(&p).as_ptr(), &mut out);
    let _ = std::fs::remove_file(&p);

    unsafe {
        std::env::remove_var("NEMESIS_REVOCATION_URL");
        std::env::remove_var("NEMESIS_ROOT_PUBKEY");
    }
    assert_eq!(rc, 0);
    assert_eq!(out.status, NV_REVOKED, "CRL 命中 key_fp → NV_REVOKED");
}

fn signed_with_chain() -> (Vec<u8>, SigningKey, SigningKey) {
    let (root_sk, _) = keypair(1);
    let (leaf_sk, leaf_vk) = keypair(2);
    let leaf_cert =
        crate::cert::issue_certificate(&root_sk, &leaf_vk.to_bytes(), b"issuer-A", 0, u64::MAX);
    let chain = crate::cert::serialize_chain(&[leaf_cert]);
    let signed = sign_content(
        b"listed payload",
        &leaf_sk,
        777,
        Some(&chain),
        Some("org-publisher"),
        None,
        None,
    )
    .unwrap();
    (signed, root_sk, leaf_sk)
}

#[test]
fn nv_list_signatures_counts_and_args() {
    let (signed, ..) = signed_with_chain();
    let p = temp_bytes(&signed);
    let path = c_path(&p);

    // null 参数 → -1
    let mut infos: [NvSigInfo; 4] = std::array::from_fn(|_| NvSigInfo::default());
    let mut count: u32 = 4;
    assert_eq!(nv_list_signatures(std::ptr::null(), infos.as_mut_ptr(), &mut count), -1);
    assert_eq!(nv_list_signatures(path.as_ptr(), std::ptr::null_mut(), &mut count), -1);
    assert_eq!(nv_list_signatures(path.as_ptr(), infos.as_mut_ptr(), std::ptr::null_mut()), -1);
    // 文件不存在 → -3
    let missing = c_path(&std::env::temp_dir().join("nv_abi_missing_list.bin"));
    assert_eq!(nv_list_signatures(missing.as_ptr(), infos.as_mut_ptr(), &mut count), -3);

    // 未签名文件 → 0 + count=0
    let plain = temp_bytes(b"no signature".as_ref());
    let mut count: u32 = 4;
    let rc = nv_list_signatures(c_path(&plain).as_ptr(), infos.as_mut_ptr(), &mut count);
    let _ = std::fs::remove_file(&plain);
    assert_eq!(rc, 0);
    assert_eq!(count, 0);

    // 单签名文件 → count=1，字段穿透（signed_at / key_fp / pubkey）
    let mut infos: [NvSigInfo; 4] = std::array::from_fn(|_| NvSigInfo::default());
    let mut count: u32 = 4;
    let rc = nv_list_signatures(path.as_ptr(), infos.as_mut_ptr(), &mut count);
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
    let rc0 = nv_list_signatures(c_path(&p2).as_ptr(), &mut one, &mut count_zero);
    let _ = std::fs::remove_file(&p2);
    assert_eq!(rc0, 0);
    assert_eq!(count_zero, 1, "capacity 0 writes nothing but reports total");
    assert_eq!(one.index, 0, "out array untouched at capacity 0");
}

#[test]
fn nv_get_signature_detail_with_chain_and_truncation() {
    let (signed, ..) = signed_with_chain();
    let p = temp_bytes(&signed);
    let path = c_path(&p);

    // index 越界 → -4；null → -1
    let mut detail = NvSigDetail::default();
    assert_eq!(nv_get_signature(path.as_ptr(), 9, &mut detail), -4);
    assert_eq!(nv_get_signature(std::ptr::null(), 0, &mut detail), -1);
    assert_eq!(nv_get_signature(path.as_ptr(), 0, std::ptr::null_mut()), -1);

    // 正常详情：cert_count=1（chain=[leaf_cert]）、publisher 穿透、meta 穿透
    let rc = nv_get_signature(path.as_ptr(), 0, &mut detail);
    let _ = std::fs::remove_file(&p);
    assert_eq!(rc, 0);
    assert_eq!(detail.cert_count, 1);
    assert_eq!(detail.signed_at, 777);
    let plen = detail.publisher_len as usize;
    assert_eq!(plen, "org-publisher".len());
    assert_eq!(&detail.publisher[..plen], b"org-publisher".as_slice());
    let mlen = detail.certs[0].subject_meta_len as usize;
    assert_eq!(mlen, b"issuer-A".len());
    assert_eq!(&detail.certs[0].subject_meta[..mlen], b"issuer-A");

    // 超长截断：publisher > 128B → 128；subject meta > 64B → 64
    let (root_sk, _) = keypair(5);
    let (leaf_sk, leaf_vk) = keypair(6);
    let long_meta: Vec<u8> = (0..100u8).collect();
    let leaf_cert = crate::cert::issue_certificate(
        &root_sk,
        &leaf_vk.to_bytes(),
        &long_meta,
        0,
        u64::MAX,
    );
    let chain = crate::cert::serialize_chain(&[leaf_cert]);
    let long_publisher = "p".repeat(200);
    let signed2 = sign_content(
        b"truncation target",
        &leaf_sk,
        1,
        Some(&chain),
        Some(&long_publisher),
        None,
        None,
    )
    .unwrap();
    let p2 = temp_bytes(&signed2);
    let mut detail2 = NvSigDetail::default();
    let rc2 = nv_get_signature(c_path(&p2).as_ptr(), 0, &mut detail2);
    let _ = std::fs::remove_file(&p2);
    assert_eq!(rc2, 0);
    assert_eq!(detail2.publisher_len, 128, "publisher capped at 128");
    assert_eq!(&detail2.publisher[..128], &long_publisher.as_bytes()[..128]);
    assert_eq!(detail2.certs[0].subject_meta_len, 64, "subject meta capped at 64");
    assert_eq!(&detail2.certs[0].subject_meta[..64], &long_meta[..64]);
}
