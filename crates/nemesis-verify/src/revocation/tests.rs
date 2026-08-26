use super::*;
use crate::sign_response;
use ed25519_dalek::SigningKey;

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key();
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
    Crl { version, valid_until: u64::MAX, entries }
}

#[test]
fn strict_offline_env_parsing() {
    let _g = TEST_LOCK.lock().unwrap();
    for (val, expect) in [
        ("1", true),
        ("true", true),
        ("TRUE", true),   // eq_ignore_ascii_case
        ("0", false),
        ("yes", false),   // 只认 1/true
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
    let base = serve_map(vec![(
        "/v1/crl",
        serde_json::to_string(&signed).unwrap(),
    )]);

    // 正确根 → 拿到 CRL（版本穿透）
    let crl = fetch_crl(&base, &root_vk).expect("root-signed CRL must fetch");
    assert_eq!(crl.version, 7);

    // 错根验证 → 验签失败（防 MITM 伪造"未吊销"）
    let (_, other_vk) = keypair(32);
    let err = fetch_crl(&base, &other_vk).unwrap_err();
    assert!(format!("{err:#}").contains("CRL signature invalid"), "{err:#}");
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

    assert!(get_crl(&vk).is_none(), "strict + 拉取失败 + 无缓存 → None(Unknown)");
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
    let (other_sk, other_vk) = keypair(38);

    // ① revoked + 根签 → Some(entry)
    let revoked = sign_response(&ocsp_resp("revoked"), &root_sk).unwrap();
    let base_ok = serve_map(vec![("/v1/crl/query", serde_json::to_string(&revoked).unwrap())]);
    set_env_url(&base_ok);
    let entry = ocsp_check_single(&[0u8; 32], &[0u8; 32], &[0u8; 32], None, &root_vk);
    let e = entry.expect("revoked+root-signed → Some");
    assert_eq!(e.dim, RevDim::SigHash);
    assert_eq!(e.value, "deadbeef");
    assert_eq!(e.revoked_at, 42);

    // ② 验签失败（错根签）→ None
    let forged = sign_response(&ocsp_resp("revoked"), &other_sk).unwrap();
    let base_bad = serve_map(vec![("/v1/crl/query", serde_json::to_string(&forged).unwrap())]);
    set_env_url(&base_bad);
    assert!(ocsp_check_single(&[0u8; 32], &[0u8; 32], &[0u8; 32], None, &root_vk).is_none());

    // ③ code=valid → None
    let valid = sign_response(&ocsp_resp("valid"), &root_sk).unwrap();
    let base_valid = serve_map(vec![("/v1/crl/query", serde_json::to_string(&valid).unwrap())]);
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

// ----- verify_bytes 集成（Revoked / strict-Unknown 拒 / soft-fail 放行）-----

#[test]
fn verify_bytes_revoked_via_crl() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let (sk, vk) = keypair(39);
    let signed = crate::verify::sign_content(b"revocation integration", &sk, 1000, None, None, None, None)
        .unwrap();
    use sha2::Digest;
    let fp: [u8; 32] = sha2::Sha256::digest(&vk.to_bytes()).into();
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
        &sk,
    )
    .unwrap();
    let base = serve_map(vec![("/v1/crl", serde_json::to_string(&crl).unwrap())]);
    set_env_url(&base);

    match crate::verify::verify_bytes(&signed, &[vk], 1000) {
        crate::verify::VerifyOutcome::Revoked { dim, value, reason, .. } => {
            assert_eq!(dim, RevDim::KeyFp);
            assert_eq!(value, hex_encode(&fp));
            assert_eq!(reason, "leak");
        }
        o => panic!("expected Revoked, got {:?}", o),
    }
    clear_revocation_env();
}

#[test]
fn verify_bytes_soft_fail_unknown_still_valid() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let (sk, vk) = keypair(40);
    let signed = crate::verify::sign_content(b"soft fail", &sk, 1000, None, None, None, None).unwrap();
    set_env_url(&dead_url()); // CRL 不可达 + 无缓存 + 非 strict
    assert!(matches!(
        crate::verify::verify_bytes(&signed, &[vk], 1000),
        crate::verify::VerifyOutcome::Valid { .. }
    ));
    clear_revocation_env();
}

#[test]
fn verify_bytes_strict_unknown_rejects_as_untrusted() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let (sk, vk) = keypair(41);
    let signed = crate::verify::sign_content(b"strict reject", &sk, 1000, None, None, None, None).unwrap();
    set_env_url(&dead_url());
    unsafe { std::env::set_var("NEMESIS_STRICT_OFFLINE", "1") };
    // CRL 不可达 → Unknown → strict → OCSP 也不可达 → Untrusted
    assert!(matches!(
        crate::verify::verify_bytes(&signed, &[vk], 1000),
        crate::verify::VerifyOutcome::Untrusted
    ));
    clear_revocation_env();
}

#[test]
fn verify_bytes_strict_ocsp_fallback_revoked() {
    let _g = TEST_LOCK.lock().unwrap();
    clear_revocation_env();
    let (sk, vk) = keypair(42);
    let signed = crate::verify::sign_content(b"strict ocsp", &sk, 1000, None, None, None, None).unwrap();
    // 只注册 /v1/crl/query；/v1/crl 无路由 → 断连 → CRL 拉取失败
    let ocsp = sign_response(&ocsp_resp("revoked"), &sk).unwrap();
    let base = serve_map(vec![("/v1/crl/query", serde_json::to_string(&ocsp).unwrap())]);
    set_env_url(&base);
    unsafe { std::env::set_var("NEMESIS_STRICT_OFFLINE", "1") };

    match crate::verify::verify_bytes(&signed, &[vk], 1000) {
        crate::verify::VerifyOutcome::Revoked { dim, .. } => assert_eq!(dim, RevDim::SigHash),
        o => panic!("expected Revoked via OCSP fallback, got {:?}", o),
    }
    clear_revocation_env();
}
