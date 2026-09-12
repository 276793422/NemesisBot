use super::*;

/// 测试用：从固定种子构造密钥（确定性，无需 rand 依赖）。
/// 种子 ≤ 54 时为合法 P-256 标量（canonical check 拒 0 与 ≥n）。
fn root_key(seed: u8) -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::from_bytes(&[seed; 32].into()).expect("seed is a valid scalar");
    let vk = *sk.verifying_key();
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
        &Crl {
            version: 1,
            valid_until: 1,
            entries: vec![],
        },
        &sk,
    )
    .unwrap();

    // 长度 ≠ 128 hex 字符 → hex_decode_64 长度错误臂
    signed.sig = "abcd".into();
    let err = verify_response(&signed, &vk).unwrap_err();
    assert!(
        format!("{err:#}").contains("expected 128 hex chars"),
        "{err:#}"
    );

    // 长度对但含非 hex 字符 → 逐字节解析错误臂
    signed.sig = "g".repeat(128);
    let err = verify_response(&signed, &vk).unwrap_err();
    assert!(format!("{err:#}").contains("invalid hex"), "{err:#}");

    // 128 个合法 hex 字符但签名不匹配 → Ok(false)（而非 Err）
    signed.sig = "0".repeat(128);
    assert!(!verify_response(&signed, &vk).unwrap());
}

#[test]
fn hex_decode_64_accepts_uppercase_and_trims() {
    // 直接钉私有 helper 的容错语义（tests.rs 是 lib.rs 子模块，可访问）。
    let sig = [0xABu8; 64];
    let upper: String = sig.iter().map(|b| format!("{:02X}", b)).collect();
    let via_resp = SignedResponse {
        payload: Crl {
            version: 1,
            valid_until: 1,
            entries: vec![],
        },
        sig: format!("  {upper}  "),
    };
    let (_, vk) = root_key(1);
    // sig 不匹配（全 AB 非真签名）但解析必须成功 → Ok(false)
    assert!(!verify_response::<Crl>(&via_resp, &vk).unwrap());
}

// ---------------------------------------------------------------------------
// S4-2：根锚注入三臂（resolve_root_anchors 纯函数 + fail-closed 语义）。
// 纯函数不触 env，无需 GLOBAL_STATE_LOCK。
// ---------------------------------------------------------------------------

#[test]
fn root_anchor_resolution_three_arms() {
    let (_, root_vk) = root_key(3);
    let fp = crypto::key_fp(&crypto::public_key_bytes(&root_vk));
    let anchor = hex_encode(&fp); // 私有 hex_encode 64 字符小写
    assert_eq!(anchor.len(), 64);

    // ① build 注入优先：builtin 合法 → 只用 builtin，runtime 被忽略（防篡改）
    assert_eq!(
        resolve_root_anchors(Some(&anchor), Some(&"f".repeat(64))),
        vec![fp]
    );
    // runtime 缺席同样成立
    assert_eq!(resolve_root_anchors(Some(&anchor), None), vec![fp]);

    // ② env fallback：builtin 缺席 + runtime 合法 → 用 runtime
    assert_eq!(resolve_root_anchors(None, Some(&anchor)), vec![fp]);

    // ③ 双臂皆缺 → 空集（D7 默认不可信状态）
    assert_eq!(resolve_root_anchors(None, None), Vec::<[u8; 32]>::new());

    // ④ fail-closed：builtin 存在但非法 → 空集，**不回落 runtime**（防 env 顶替信任根）
    assert_eq!(
        resolve_root_anchors(Some("not-hex!"), Some(&anchor)),
        Vec::<[u8; 32]>::new()
    );

    // ⑤ runtime 非法（无 builtin）→ 空集
    assert_eq!(
        resolve_root_anchors(None, Some("zz")),
        Vec::<[u8; 32]>::new()
    );

    // ⑥ 容差：首尾空白可 trim（部署脚本友好），长度错/奇数长度拒绝
    assert_eq!(
        resolve_root_anchors(Some(&format!("  {anchor}  ")), None),
        vec![fp]
    );
    assert_eq!(
        resolve_root_anchors(Some(&anchor[..63]), None),
        Vec::<[u8; 32]>::new()
    );
}
