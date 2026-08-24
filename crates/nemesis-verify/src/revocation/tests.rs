use super::*;
use crate::sign_response;
use ed25519_dalek::SigningKey;

fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key();
    (sk, vk)
}

/// 测试串行锁（全局 CRL_CACHE + env 并行竞争，参考 env-test-race-lock-pattern）。
static TEST_LOCK: Mutex<()> = Mutex::new(());

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
