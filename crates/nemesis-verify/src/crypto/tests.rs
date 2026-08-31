//! 签名原语单测（S6 覆盖率批次，quality-hardening goal 2026-08-25）。
//!
//! 覆盖：密钥对生成/加载（hex → key）、非规范公钥拒绝、key_fp、
//! Ed25519 sign/verify 往返 + 篡改消息/签名拒绝。

use super::*;

#[test]
fn generate_key_pair_roundtrips_through_hex_loaders() {
    let kp = generate_key_pair();
    assert_eq!(kp.private_key.len(), 64, "私钥 hex = 64 字符");
    assert_eq!(kp.public_key.len(), 64, "公钥 hex = 64 字符");

    let sk = signing_key_from_hex(&kp.private_key).expect("load private key");
    let vk = verifying_key_from_hex(&kp.public_key).expect("load public key");

    // 重新导出必须与生成时一致（往返无损）。
    assert_eq!(hex_encode(sk.to_bytes().as_ref()), kp.private_key);
    assert_eq!(hex_encode(vk.to_bytes().as_ref()), kp.public_key);

    // 生成的密钥对能直接签名 + 验签。
    let sig = ed25519_sign(&sk, b"roundtrip message");
    assert!(ed25519_verify(&vk, b"roundtrip message", &sig));
    assert!(!ed25519_verify(&vk, b"other message", &sig));
    let mut tampered = sig;
    tampered[0] ^= 0x01;
    assert!(!ed25519_verify(&vk, b"roundtrip message", &tampered));
}

#[test]
fn generate_key_pair_produces_distinct_keys() {
    let a = generate_key_pair();
    let b = generate_key_pair();
    assert_ne!(a.private_key, b.private_key, "OsRng 两次生成必须不同");
}

#[test]
fn signing_key_from_hex_rejects_bad_input() {
    // 长度错（hex_decode_32 的 64 字符校验）
    let err = signing_key_from_hex("abcd").unwrap_err();
    assert!(format!("{err:#}").contains("invalid private key"), "{err:#}");
    // 长度对但非 hex 字符
    let err = signing_key_from_hex(&"z".repeat(64)).unwrap_err();
    assert!(format!("{err:#}").contains("invalid private key"), "{err:#}");
}

#[test]
fn verifying_key_from_hex_rejects_bad_length_and_noncanonical_point() {
    // 长度错
    let err = verifying_key_from_hex("1234").unwrap_err();
    assert!(format!("{err:#}").contains("invalid public key"), "{err:#}");
    // 64 个 hex 字符但解码出的 32B 不是合法曲线点编码（Ed25519 解压缩失败）——
    // 防止任意 32B 被当作公钥注入验证链。注意：全零在该 dalek 版本里是合法
    // 编码（可解压缩），所以要确定性探测一个真被拒的：SHA-256(i) 逐个试，
    // 256 个候选中必然存在非规范编码（约一半 32B 串解不出合法点）。
    use sha2::Digest;
    let mut found_rejected = false;
    for i in 0u8..=255 {
        let cand: [u8; 32] = sha2::Sha256::digest([i]).into();
        if verifying_key_from_hex(&hex_encode(&cand)).is_err() {
            found_rejected = true;
            break;
        }
    }
    assert!(
        found_rejected,
        "SHA-256(0..=255) 中必存在非规范曲线点编码且必须被拒"
    );
}

#[test]
fn key_fp_is_sha256_of_pubkey() {
    let kp = generate_key_pair();
    let vk = verifying_key_from_hex(&kp.public_key).unwrap();
    let fp = key_fp(&vk.to_bytes());
    use sha2::Digest;
    let expect: [u8; 32] = sha2::Sha256::digest(vk.to_bytes()).into();
    assert_eq!(fp, expect);
    // 不同公钥指纹不同。
    let other = generate_key_pair();
    let other_vk = verifying_key_from_hex(&other.public_key).unwrap();
    assert_ne!(fp, key_fp(&other_vk.to_bytes()));
}
