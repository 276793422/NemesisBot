use super::*;
use crate::sign_response;
use p256::ecdsa::SigningKey;

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32].into()).expect("seed is a valid scalar");
    let vk = *sk.verifying_key();
    (sk, vk)
}

/// 测试串行锁（全局 CRL_CACHE + env 并行竞争，参考 env-test-race-lock-pattern）。
/// 指向 crate 根唯一锁：revocation / verify / c_abi / keygen 的测试共享同一把，
/// 否则跨模块并行时 env 互踩（verify 流程都会读 NEMESIS_REVOCATION_URL 等）。
use crate::GLOBAL_STATE_LOCK as TEST_LOCK;

/// 直接喂缓存一个 CRL（绕过联网），测四维度查询逻辑。
fn seed_cache(crl: Crl) {
    *cache().lock().unwrap() = Some(CrlCache {
        crl,
        fetched_at: now_secs(),
    });
}

#[test]
fn revoke_hit_key_fp() {
    let _g = TEST_LOCK.lock().unwrap();
    let (sk, vk) = keypair(1);
    let target_fp = [0xAAu8; 32];
    let signed = sign_response(
        &Crl {
            version: 1,
            valid_until: u64::MAX,
            entries: vec![CrlEntry {
                dim: RevDim::KeyFp,
                value: hex_encode(&target_fp),
                revoked_at: 100,
                reason: "leak".into(),
            }],
        },
        &sk,
    )
    .unwrap();
    seed_cache(signed.payload);
    match check_revocation(&target_fp, &[0u8; 32], &[0u8; 32], None, &vk) {
        RevocationResult::Revoked(e) => assert_eq!(e.reason, "leak"),
        o => panic!("expected Revoked, got {:?}", o),
    }
    match check_revocation(&[0xBBu8; 32], &[0u8; 32], &[0u8; 32], None, &vk) {
        RevocationResult::NotRevoked => {}
        o => panic!("expected NotRevoked, got {:?}", o),
    }
}

#[test]
fn revoke_hit_file_hash() {
    // S4-4：四维度查序 KeyFp→SigHash→FileHash→Publisher 的 FileHash 臂
    // （此前 FileHash 只有「不命中」旁证，无命中样本）
    let _g = TEST_LOCK.lock().unwrap();
    let (sk, vk) = keypair(4);
    let target_ch = [0xCBu8; 32];
    let signed = sign_response(
        &Crl {
            version: 1,
            valid_until: u64::MAX,
            entries: vec![CrlEntry {
                dim: RevDim::FileHash,
                value: hex_encode(&target_ch),
                revoked_at: 7,
                reason: "malware".into(),
            }],
        },
        &sk,
    )
    .unwrap();
    seed_cache(signed.payload);
    // 命中 FileHash（key_fp/sig_hash 不匹配不影响）
    match check_revocation(&[0u8; 32], &[0u8; 32], &target_ch, None, &vk) {
        RevocationResult::Revoked(e) => {
            assert_eq!(e.dim, RevDim::FileHash);
            assert_eq!(e.reason, "malware");
        }
        o => panic!("expected Revoked(FileHash), got {:?}", o),
    }
    // content_hash 不匹配 → NotRevoked
    match check_revocation(&[0u8; 32], &[0u8; 32], &[0xDDu8; 32], None, &vk) {
        RevocationResult::NotRevoked => {}
        o => panic!("expected NotRevoked, got {:?}", o),
    }
}

#[test]
fn no_url_returns_unknown() {
    let _g = TEST_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var("NEMESIS_REVOCATION_URL");
    }
    *cache().lock().unwrap() = None;
    let (_, vk) = keypair(2);
    match check_revocation(&[0xAAu8; 32], &[0u8; 32], &[0u8; 32], None, &vk) {
        RevocationResult::Unknown => {}
        o => panic!("expected Unknown, got {:?}", o),
    }
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：
// 本地 std TCP 假 HTTP 服务器驱动 fetch_crl / get_crl 全部缓存与联网臂、
// strict_offline env 解析、OCSP 单条查询全臂、publisher 维度、以及
// verify_bytes 的 Revoked / strict-Unknown / soft-fail-Unknown 集成路径。
// 全部走 TEST_LOCK（crate 根全局锁）串行 + 结束时清 env + 清缓存。
// ---------------------------------------------------------------------------

/// 起 std TCP 假 HTTP 服务器（阻塞 reqwest 的对端）。按 path 精确分派 JSON body；
/// 未注册的 path 直接断连（模拟端点故障 → fetch 失败）。线程随测试进程退出。
fn serve_map(routes: Vec<(&'static str, String)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for stream in listener.incoming() {
            let mut s = match stream {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split(' ').nth(1).unwrap_or("").to_string();
            match routes.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes());
                }
                None => drop(s), // 断连 = 端点故障
            }
        }
    });
    format!("http://{addr}")
}

/// 一个"死"URL：bind 后立即 drop → 连接必然被拒（无需等待超时）。
fn dead_url() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

fn set_env_url(url: &str) {
    unsafe { std::env::set_var("NEMESIS_REVOCATION_URL", url) };
}

fn clear_revocation_env() {
    unsafe {
        std::env::remove_var("NEMESIS_REVOCATION_URL");
        std::env::remove_var("NEMESIS_STRICT_OFFLINE");
    }
    *cache().lock().unwrap() = None;
}

fn crl_with(version: u64, entries: Vec<CrlEntry>) -> Crl {
    Crl {
        version,
        valid_until: u64::MAX,
        entries,
    }
}

#[test]
fn strict_offline_env_parsing() {
    let _g = TEST_LOCK.lock().unwrap();
    for (val, expect) in [
        ("1", true),
        ("true", true),
        ("TRUE", true), // eq_ignore_ascii_case
        ("0", false),
        ("yes", false), // 只认 1/true
    ] {
        unsafe { std::env::set_var("NEMESIS_STRICT_OFFLINE", val) };
        assert_eq!(strict_offline(), expect, "NEMESIS_STRICT_OFFLINE={val}");
    }
    unsafe { std::env::remove_var("NEMESIS_STRICT_OFFLINE") };
    assert!(!strict_offline(), "未设置 → soft-fail");
}

#[test]
fn fetch_crl_validates_root_signature() {
    let _g = TEST_LOCK.lock().unwrap();
    let (root_sk, root_vk) = keypair(31);
    let signed = sign_response(&crl_with(7, vec![]), &root_sk).unwrap();
    let base = serve_map(vec![("/v1/crl", serde_json::to_string(&signed).unwrap())]);

    // 正确根 → 拿到 CRL（版本穿透）
    let crl = fetch_crl(&base, &root_vk).expect("root-signed CRL must fetch");
    assert_eq!(crl.version, 7);

    // 错根验证 → 验签失败（防 MITM 伪造"未吊销"）
    let (_, other_vk) = keypair(32);
    let err = fetch_crl(&base, &other_vk).unwrap_err();
    assert!(
        format!("{err:#}").contains("CRL signature invalid"),
        "{err:#}"
    );
}

#[test]
fn get_crl_refetches_when_cache_expired() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let (root_sk, root_vk) = keypair(33);
    let v2 = sign_response(&crl_with(2, vec![]), &root_sk).unwrap();
    let base = serve_map(vec![("/v1/crl", serde_json::to_string(&v2).unwrap())]);
    // 喂一个已过期的 v1 缓存（TTL 3600s；fetched_at = now - 2*TTL）
    *cache().lock().unwrap() = Some(CrlCache {
        crl: crl_with(1, vec![]),
        fetched_at: now_secs() - 2 * CRL_TTL_SECS,
    });
    set_env_url(&base);

    let got = get_crl(&root_vk).expect("expired cache must refetch");
    assert_eq!(got.version, 2, "必须拿到新拉的 v2 而非过期 v1");
    // 新 CRL 已写回缓存
    let cached_ver = cache().lock().unwrap().as_ref().map(|c| c.crl.version);
    assert_eq!(cached_ver, Some(2));
    clear_revocation_env();
}

#[test]
fn get_crl_soft_fail_uses_stale_cache_on_fetch_error() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let (_, vk) = keypair(34);
    *cache().lock().unwrap() = Some(CrlCache {
        crl: crl_with(1, vec![]),
        fetched_at: now_secs() - 2 * CRL_TTL_SECS,
    });
    set_env_url(&dead_url()); // 拉取失败 + 非 strict

    let got = get_crl(&vk).expect("soft-fail must fall back to stale cache");
    assert_eq!(got.version, 1);
    clear_revocation_env();
}

#[test]
fn get_crl_strict_without_cache_is_none() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let (_, vk) = keypair(35);
    *cache().lock().unwrap() = None;
    set_env_url(&dead_url());
    unsafe { std::env::set_var("NEMESIS_STRICT_OFFLINE", "1") };

    assert!(
        get_crl(&vk).is_none(),
        "strict + 拉取失败 + 无缓存 → None(Unknown)"
    );
    clear_revocation_env();
}

#[test]
fn check_revocation_publisher_dimension() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let (_, vk) = keypair(36);
    seed_cache(crl_with(
        1,
        vec![CrlEntry {
            dim: RevDim::Publisher,
            value: "evil-corp".into(),
            revoked_at: 8,
            reason: "supply-chain".into(),
        }],
    ));
    match check_revocation(&[0u8; 32], &[0u8; 32], &[0u8; 32], Some("evil-corp"), &vk) {
        RevocationResult::Revoked(e) => {
            assert_eq!(e.dim, RevDim::Publisher);
            assert_eq!(e.reason, "supply-chain");
        }
        o => panic!("expected Revoked(Publisher), got {:?}", o),
    }
    // 非 evil-corp / 不带 publisher → NotRevoked
    assert!(matches!(
        check_revocation(&[0u8; 32], &[0u8; 32], &[0u8; 32], Some("good-corp"), &vk),
        RevocationResult::NotRevoked
    ));
    assert!(matches!(
        check_revocation(&[0u8; 32], &[0u8; 32], &[0u8; 32], None, &vk),
        RevocationResult::NotRevoked
    ));
}

fn ocsp_resp(code: &str) -> crate::revocation::OcspResp {
    crate::revocation::OcspResp {
        code: code.into(),
        dim: Some(RevDim::SigHash),
        value: Some("deadbeef".into()),
        revoked_at: Some(42),
        reason: Some("leak".into()),
        crl_ver: 1,
    }
}

#[test]
fn ocsp_check_single_all_arms() {
    let _g = TEST_LOCK.lock().unwrap();
    let (root_sk, root_vk) = keypair(37);
    let (other_sk, _other_vk) = keypair(38);

    // ① revoked + 根签 → Some(entry)
    let revoked = sign_response(&ocsp_resp("revoked"), &root_sk).unwrap();
    let base_ok = serve_map(vec![(
        "/v1/crl/query",
        serde_json::to_string(&revoked).unwrap(),
    )]);
    set_env_url(&base_ok);
    let entry = ocsp_check_single(&[0u8; 32], &[0u8; 32], &[0u8; 32], None, &root_vk);
    let e = entry.expect("revoked+root-signed → Some");
    assert_eq!(e.dim, RevDim::SigHash);
    assert_eq!(e.value, "deadbeef");
    assert_eq!(e.revoked_at, 42);

    // ② 验签失败（错根签）→ None
    let forged = sign_response(&ocsp_resp("revoked"), &other_sk).unwrap();
    let base_bad = serve_map(vec![(
        "/v1/crl/query",
        serde_json::to_string(&forged).unwrap(),
    )]);
    set_env_url(&base_bad);
    assert!(ocsp_check_single(&[0u8; 32], &[0u8; 32], &[0u8; 32], None, &root_vk).is_none());

    // ③ code=valid → None
    let valid = sign_response(&ocsp_resp("valid"), &root_sk).unwrap();
    let base_valid = serve_map(vec![(
        "/v1/crl/query",
        serde_json::to_string(&valid).unwrap(),
    )]);
    set_env_url(&base_valid);
    assert!(ocsp_check_single(&[0u8; 32], &[0u8; 32], &[0u8; 32], None, &root_vk).is_none());

    // ④ 端点死（send 失败）→ None
    set_env_url(&dead_url());
    assert!(ocsp_check_single(&[0u8; 32], &[0u8; 32], &[0u8; 32], None, &root_vk).is_none());

    // ⑤ 未配置 URL → None（revocation_url()? 提前返回）
    unsafe { std::env::remove_var("NEMESIS_REVOCATION_URL") };
    assert!(ocsp_check_single(&[0u8; 32], &[0u8; 32], &[0u8; 32], None, &root_vk).is_none());

    clear_revocation_env();
}

// ----- verify_bytes 集成（Revoked / strict-Unknown 拒 / soft-fail 放行）
// S4-1 起 verify_bytes 走 v4 Authenticode 管线：夹具 = V4Harness（真三级链 +
// CMS + raw 载体）；CRL 由 harness 根私钥签（check_revocation 用链根证书公钥验）。

#[test]
fn verify_bytes_revoked_via_crl() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let h = crate::fixtures::V4Harness::new();
    let signed = h.sign_raw(b"revocation integration", 1_800_000_000);
    use sha2::Digest;
    let pubkey = crate::crypto::public_key_bytes(&h.h.leaf_vk());
    let fp: [u8; 32] = sha2::Sha256::digest(pubkey).into();
    let crl = sign_response(
        &crl_with(
            2,
            vec![CrlEntry {
                dim: RevDim::KeyFp,
                value: hex_encode(&fp),
                revoked_at: 9,
                reason: "leak".into(),
            }],
        ),
        &h.h.root_sk,
    )
    .unwrap();
    let base = serve_map(vec![("/v1/crl", serde_json::to_string(&crl).unwrap())]);
    set_env_url(&base);

    match crate::verify::verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        crate::verify::VerifyOutcome::Revoked {
            dim, value, reason, ..
        } => {
            assert_eq!(dim, RevDim::KeyFp);
            assert_eq!(value, hex_encode(&fp));
            assert_eq!(reason, "leak");
        }
        o => panic!("expected Revoked, got {:?}", o),
    }
    clear_revocation_env();
}

#[test]
fn verify_bytes_revoked_via_file_hash() {
    // S4-4：FileHash 维度走完整 v4 管线——verify_bytes 传给 check_revocation 的
    // content_hash = CMS 内嵌 content_digest（raw 载体 = SHA-256(内容)），
    // 本测钉住该接线值端到端正确（防接成「文件 SHA」之类的错值静默漏报）。
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let h = crate::fixtures::V4Harness::new();
    let content = b"file hash revocation";
    let signed = h.sign_raw(content, 1_800_000_000);
    use sha2::Digest;
    let content_digest: [u8; 32] = sha2::Sha256::digest(content).into();
    let crl = sign_response(
        &crl_with(
            2,
            vec![CrlEntry {
                dim: RevDim::FileHash,
                value: hex_encode(&content_digest),
                revoked_at: 11,
                reason: "malware".into(),
            }],
        ),
        &h.h.root_sk,
    )
    .unwrap();
    let base = serve_map(vec![("/v1/crl", serde_json::to_string(&crl).unwrap())]);
    set_env_url(&base);

    match crate::verify::verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        crate::verify::VerifyOutcome::Revoked {
            dim, value, reason, ..
        } => {
            assert_eq!(dim, RevDim::FileHash);
            assert_eq!(value, hex_encode(&content_digest));
            assert_eq!(reason, "malware");
        }
        o => panic!("expected Revoked(FileHash), got {:?}", o),
    }
    clear_revocation_env();
}

#[test]
fn verify_bytes_soft_fail_unknown_still_valid() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let h = crate::fixtures::V4Harness::new();
    let signed = h.sign_raw(b"soft fail", 1_800_000_000);
    set_env_url(&dead_url()); // CRL 不可达 + 无缓存 + 非 strict
    assert!(matches!(
        crate::verify::verify_bytes(&signed, &h.anchor_fps(), now_secs()),
        crate::verify::VerifyOutcome::Valid { .. }
    ));
    clear_revocation_env();
}

#[test]
fn verify_bytes_strict_unknown_rejects_as_untrusted() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let h = crate::fixtures::V4Harness::new();
    let signed = h.sign_raw(b"strict reject", 1_800_000_000);
    set_env_url(&dead_url());
    unsafe { std::env::set_var("NEMESIS_STRICT_OFFLINE", "1") };
    // CRL 不可达 → Unknown → strict → OCSP 也不可达 → Untrusted
    assert!(matches!(
        crate::verify::verify_bytes(&signed, &h.anchor_fps(), now_secs()),
        crate::verify::VerifyOutcome::Untrusted
    ));
    clear_revocation_env();
}

#[test]
fn verify_bytes_strict_ocsp_fallback_revoked() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let h = crate::fixtures::V4Harness::new();
    let content = b"strict ocsp";
    let signed = h.sign_raw(content, 1_800_000_000);
    // v4 SigHash 维度 = SHA-256(SignerInfo.signature DER)
    use sha2::Digest;
    let cms = h.build_cms(content, 1_800_000_000, &h.h.leaf_sk, &h.h.chain());
    let ps = crate::envelope::parse_signed_data(&cms).unwrap();
    let sig_hash: [u8; 32] = sha2::Sha256::digest(&ps.signature).into();
    let ocsp = crate::revocation::OcspResp {
        code: "revoked".into(),
        dim: Some(RevDim::SigHash),
        value: Some(hex_encode(&sig_hash)),
        revoked_at: Some(42),
        reason: Some("leak".into()),
        crl_ver: 1,
    };
    let ocsp = sign_response(&ocsp, &h.h.root_sk).unwrap();
    // 只注册 /v1/crl/query；/v1/crl 无路由 → 断连 → CRL 拉取失败
    let base = serve_map(vec![(
        "/v1/crl/query",
        serde_json::to_string(&ocsp).unwrap(),
    )]);
    set_env_url(&base);
    unsafe { std::env::set_var("NEMESIS_STRICT_OFFLINE", "1") };

    match crate::verify::verify_bytes(&signed, &h.anchor_fps(), now_secs()) {
        crate::verify::VerifyOutcome::Revoked { dim, value, .. } => {
            assert_eq!(dim, RevDim::SigHash);
            assert_eq!(value, hex_encode(&sig_hash));
        }
        o => panic!("expected Revoked via OCSP fallback, got {:?}", o),
    }
    clear_revocation_env();
}

// ===== 本地 CRL 快照四件套（皮肤管理面 P3：revocation_meta / crl_match_meta /
// root_pubkey_from_anchored / load_crl_snapshot）=====

use crate::fixtures::{V4Harness, now_secs as fx_now};

/// 签名文件的四维元数据齐全且口径正确（raw 载体：content_hash = 签发时刻
/// 内容摘要 = sha256(content)；key_fp = leaf 公钥指纹）。
#[test]
fn snapshot_meta_signed_raw() {
    let h = V4Harness::new();
    let content = b"nbskin-fixture-v1";
    let signed = h.sign_raw(content, fx_now());
    let meta = revocation_meta(&signed);
    assert!(meta.key_fp.is_some(), "signed file must expose key_fp");
    let leaf_fp = crate::crypto::key_fp(&crate::crypto::public_key_bytes(
        &h.h.leaf_sk.verifying_key(),
    ));
    assert_eq!(meta.key_fp.as_deref(), Some(hex_encode(&leaf_fp).as_str()));
    assert!(meta.sig_hash.is_some());
    let want_content: [u8; 32] = sha2::Sha256::digest(content).into();
    assert_eq!(
        meta.content_hash.as_deref(),
        Some(hex_encode(&want_content).as_str())
    );
    assert_eq!(meta.publisher.as_deref(), Some(crate::keygen::CN_LEAF));
}

/// opus programName → Publisher 维度。
#[test]
fn snapshot_meta_publisher_from_opus() {
    let h = V4Harness::new();
    let signed = h.sign_raw_opus(b"pkg", fx_now(), &h.h.leaf_sk, &h.h.chain(), Some("Acme"), None);
    let meta = revocation_meta(&signed);
    // Publisher 维度口径 = 签名者证书 subject CN（verify_bytes 记账同源），
    // opus programName 只是 view 展示——不进吊销维度。
    assert_eq!(meta.publisher.as_deref(), Some(crate::keygen::CN_LEAF));
}

/// 未签文件：签名维诚实缺省，content_hash 回落 v4_content_digest（全文件）。
#[test]
fn snapshot_meta_unsigned_fallback() {
    let content = b"unsigned-package-bytes";
    let meta = revocation_meta(content);
    assert_eq!(meta.key_fp, None);
    assert_eq!(meta.sig_hash, None);
    assert_eq!(meta.publisher, None);
    let want = crate::verify::v4_content_digest(content).unwrap();
    assert_eq!(meta.content_hash.as_deref(), Some(hex_encode(&want).as_str()));
}

/// crl_match_meta 四维各自可命中（与 check_revocation 同序，逐维独立构造）。
#[test]
fn snapshot_match_four_dims() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"dims", fx_now());
    let meta = revocation_meta(&signed);
    let mk = |dim: RevDim, value: String| Crl {
        version: 1,
        valid_until: u64::MAX,
        entries: vec![CrlEntry { dim, value, revoked_at: 1, reason: "t".into() }],
    };
    let key_fp = meta.key_fp.clone().unwrap();
    let sig_hash = meta.sig_hash.clone().unwrap();
    let content_hash = meta.content_hash.clone().unwrap();
    assert_eq!(
        crl_match_meta(&mk(RevDim::KeyFp, key_fp), &meta).unwrap().dim,
        RevDim::KeyFp
    );
    assert_eq!(
        crl_match_meta(&mk(RevDim::SigHash, sig_hash), &meta).unwrap().dim,
        RevDim::SigHash
    );
    assert_eq!(
        crl_match_meta(&mk(RevDim::FileHash, content_hash), &meta).unwrap().dim,
        RevDim::FileHash
    );
    // Publisher：换 opus 签名取 CN
    let signed2 = h.sign_raw_opus(b"dims2", fx_now(), &h.h.leaf_sk, &h.h.chain(), Some("Acme"), None);
    let meta2 = revocation_meta(&signed2);
    assert_eq!(
        crl_match_meta(&mk(RevDim::Publisher, crate::keygen::CN_LEAF.into()), &meta2)
            .unwrap()
            .dim,
        RevDim::Publisher
    );
    // 无关条目 = 未命中
    assert!(crl_match_meta(&mk(RevDim::KeyFp, "deadbeef".into()), &meta).is_none());
}

/// 锚定根公钥：链根在锚集内 → Ok（且能验根签的 CRL）；锚集为空 → Err。
#[test]
fn snapshot_root_pubkey_anchored() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"anchor-me", fx_now());
    let vk = root_pubkey_from_anchored(&signed, &h.anchor_fps()).expect("anchored root");
    // 根公钥确实能验根签的数据（roundtrip 由下一测试覆盖），这里验锚拒：
    assert!(root_pubkey_from_anchored(&signed, &[]).is_err());
    let _ = vk;
}

/// 快照 roundtrip：根签 CRL → JSON → load_crl_snapshot（锚定根公钥验签）→
/// 原样 Crl；换假根公钥 / 篡改 payload → 拒。
#[test]
fn snapshot_load_roundtrip_and_reject() {
    let h = V4Harness::new();
    let signed = h.sign_raw(b"crl-host", fx_now());
    let root_vk = root_pubkey_from_anchored(&signed, &h.anchor_fps()).unwrap();
    let crl = Crl {
        version: 7,
        valid_until: u64::MAX,
        entries: vec![CrlEntry {
            dim: RevDim::KeyFp,
            value: "ab".repeat(32),
            revoked_at: 42,
            reason: "leak".into(),
        }],
    };
    let resp = sign_response(&crl, &h.h.root_sk).unwrap();
    let json = serde_json::to_string(&resp).unwrap();
    let loaded = load_crl_snapshot(&json, &root_vk).unwrap();
    assert_eq!(loaded.version, 7);
    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].reason, "leak");

    // 篡改 payload（version++）→ 验签拒
    let mut bad: SignedResponse<Crl> = serde_json::from_str(&json).unwrap();
    bad.payload.version += 1;
    let bad_json = serde_json::to_string(&bad).unwrap();
    assert!(load_crl_snapshot(&bad_json, &root_vk).is_err());

    // 换无关公钥（非根）→ 验签拒
    let stranger_vk = *p256::ecdsa::SigningKey::from_bytes(&[9u8; 32].into())
        .unwrap()
        .verifying_key();
    assert!(load_crl_snapshot(&json, &stranger_vk).is_err());
}
