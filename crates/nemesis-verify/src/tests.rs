use super::*;

/// 测试用：从固定种子构造密钥（确定性，无需 rand 依赖）。
fn root_key(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key();
    (sk, vk)
}

#[test]
fn crl_match_basic() {
    let crl = Crl {
        version: 1,
        valid_until: u64::MAX,
        entries: vec![
            CrlEntry {
                dim: RevDim::KeyFp,
                value: "abc".into(),
                revoked_at: 1,
                reason: "leak".into(),
            },
            CrlEntry {
                dim: RevDim::Publisher,
                value: "evil".into(),
                revoked_at: 2,
                reason: "bad".into(),
            },
        ],
    };
    assert!(crl_match(&crl, RevDim::KeyFp, "abc").is_some());
    assert!(crl_match(&crl, RevDim::Publisher, "evil").is_some());
    assert!(crl_match(&crl, RevDim::KeyFp, "none").is_none());
}

#[test]
fn sign_verify_roundtrip() {
    let (sk, vk) = root_key(1);
    let payload = Crl {
        version: 3,
        valid_until: 99,
        entries: vec![],
    };
    let signed = sign_response(&payload, &sk).unwrap();
    assert!(verify_response(&signed, &vk).unwrap());
}

#[test]
fn verify_rejects_tampered_payload() {
    let (sk, vk) = root_key(1);
    let mut signed = sign_response(
        &Crl {
            version: 1,
            valid_until: 1,
            entries: vec![],
        },
        &sk,
    )
    .unwrap();
    signed.payload.version = 999; // 篡改 payload
    assert!(!verify_response(&signed, &vk).unwrap());
}

#[test]
fn verify_rejects_wrong_key() {
    let (sk, _) = root_key(1);
    let (_, vk2) = root_key(2); // 不同种子 → 不同公钥
    let signed = sign_response(
        &Crl {
            version: 1,
            valid_until: 1,
            entries: vec![],
        },
        &sk,
    )
    .unwrap();
    assert!(!verify_response(&signed, &vk2).unwrap());
}
