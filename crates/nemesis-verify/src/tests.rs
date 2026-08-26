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

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：hex_decode_64 错误臂
// （签名 hex 长度错 / 非法字符）经 verify_response 透出。
// ---------------------------------------------------------------------------

#[test]
fn verify_response_rejects_malformed_sig_hex() {
    let (sk, vk) = root_key(1);
    let mut signed = sign_response(
        &Crl { version: 1, valid_until: 1, entries: vec![] },
        &sk,
    )
    .unwrap();

    // 长度 ≠ 128 hex 字符 → hex_decode_64 长度错误臂
    signed.sig = "abcd".into();
    let err = verify_response(&signed, &vk).unwrap_err();
    assert!(format!("{err:#}").contains("expected 128 hex chars"), "{err:#}");

    // 长度对但含非 hex 字符 → 逐字节解析错误臂
    signed.sig = "g".repeat(128);
    let err = verify_response(&signed, &vk).unwrap_err();
    assert!(format!("{err:#}").contains("invalid hex"), "{err:#}");

    // 128 个合法 hex 字符但签名不匹配 → Ok(false)（而非 Err）
    signed.sig = "0".repeat(128);
    assert_eq!(verify_response(&signed, &vk).unwrap(), false);
}

#[test]
fn hex_decode_64_accepts_uppercase_and_trims() {
    // 直接钉私有 helper 的容错语义（tests.rs 是 lib.rs 子模块，可访问）。
    let sig = [0xABu8; 64];
    let upper: String = sig.iter().map(|b| format!("{:02X}", b)).collect();
    let via_resp = SignedResponse {
        payload: Crl { version: 1, valid_until: 1, entries: vec![] },
        sig: format!("  {upper}  "),
    };
    let (_, vk) = root_key(1);
    // sig 不匹配（全 AB 非真签名）但解析必须成功 → Ok(false)
    assert_eq!(verify_response::<Crl>(&via_resp, &vk).unwrap(), false);
}
