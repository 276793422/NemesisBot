//! 签名原语单测（v4：ECDSA P-256 + SHA-256，RFC 6979 确定式签名）。
//!
//! 覆盖：密钥对生成/加载（hex → key，uncompressed 65B / compressed 33B）、
//! key_fp（SHA-256(uncompressed 点)）、P-256 sign/verify 往返 + 篡改消息/签名拒绝、
//! RFC 6979 确定性（同消息两次签名逐字节一致）。

use super::*;

#[test]
fn generate_key_pair_roundtrips_through_hex_loaders() {
    let kp = generate_key_pair();
    assert_eq!(kp.private_key.len(), 64, "P-256 私钥标量 hex = 64 字符");
    assert_eq!(
        kp.public_key.len(),
        130,
        "SEC1 uncompressed 65B hex = 130 字符"
    );
    assert!(kp.public_key.starts_with("04"), "uncompressed 前缀 0x04");

    let sk = signing_key_from_hex(&kp.private_key).expect("load private key");
    let vk = verifying_key_from_hex(&kp.public_key).expect("load public key");

    // 重新导出必须与生成时一致（往返无损；公钥统一 uncompressed 归一化）。
    assert_eq!(hex_encode(sk.to_bytes().as_ref()), kp.private_key);
    assert_eq!(hex_encode(&public_key_bytes(&vk)), kp.public_key);

    // 生成的密钥对能直接签名 + 验签（消息/签名双篡改拒绝）。
    let sig = p256_sign(&sk, b"roundtrip message");
    assert!(p256_verify(&vk, b"roundtrip message", &sig));
    assert!(!p256_verify(&vk, b"other message", &sig));
    let mut tampered = sig;
    tampered[0] ^= 0x01;
    assert!(!p256_verify(&vk, b"roundtrip message", &tampered));
}

#[test]
fn generate_key_pair_produces_distinct_keys() {
    let a = generate_key_pair();
    let b = generate_key_pair();
    assert_ne!(a.private_key, b.private_key, "OsRng 两次生成必须不同");
}

#[test]
fn rfc6979_signature_is_deterministic() {
    // RFC 6979：nonce 由消息+私钥确定性推导，同消息两次签名必须逐字节一致。
    // （与随机 nonce ECDSA 的行为分界；Authenticode 可重现签名的基础。）
    let kp = generate_key_pair();
    let sk = signing_key_from_hex(&kp.private_key).unwrap();
    let msg = b"determinism probe";
    let s1 = p256_sign(&sk, msg);
    let s2 = p256_sign(&sk, msg);
    assert_eq!(s1, s2, "RFC 6979 确定式签名：同消息同密钥两次签名必须一致");
    // 不同消息 → 签名不同。
    let s3 = p256_sign(&sk, b"different message");
    assert_ne!(s1, s3);
}

#[test]
fn signing_key_from_hex_rejects_bad_input() {
    // 长度错（hex_decode_32 的 64 字符校验）
    let err = signing_key_from_hex("abcd").unwrap_err();
    assert!(
        format!("{err:#}").contains("invalid private key"),
        "{err:#}"
    );
    // 长度对但非 hex 字符
    let err = signing_key_from_hex(&"z".repeat(64)).unwrap_err();
    assert!(
        format!("{err:#}").contains("invalid private key"),
        "{err:#}"
    );
    // 长度对但标量为 0（P-256 私钥必须是非零标量）
    let err = signing_key_from_hex(&"0".repeat(64)).unwrap_err();
    assert!(
        format!("{err:#}").contains("invalid private key"),
        "{err:#}"
    );
}

#[test]
fn verifying_key_from_hex_rejects_bad_length_and_invalid_point() {
    // 长度错（非 65/33 字节）
    let err = verifying_key_from_hex("1234").unwrap_err();
    assert!(format!("{err:#}").contains("invalid public key"), "{err:#}");
    // 65B 但前缀错（uncompressed 必须是 0x04）
    let bad_prefix = [0x03u8; 65];
    let err = verifying_key_from_hex(&hex_encode(&bad_prefix)).unwrap_err();
    assert!(format!("{err:#}").contains("invalid public key"), "{err:#}");
    // 65B 前缀对但 (x,y) 不在曲线上（全零点）
    let mut not_on_curve = vec![0x04u8];
    not_on_curve.extend_from_slice(&[0u8; 64]);
    let err = verifying_key_from_hex(&hex_encode(&not_on_curve)).unwrap_err();
    assert!(format!("{err:#}").contains("invalid public key"), "{err:#}");
}

#[test]
fn verifying_key_accepts_compressed_sec1() {
    // compressed 33B（0x02/0x03 前缀）同点可载入，uncompressed 重编码一致。
    let kp = generate_key_pair();
    let vk = verifying_key_from_hex(&kp.public_key).unwrap();
    let uncompressed = public_key_bytes(&vk);
    // 由 uncompressed 手工推 compressed：奇偶前缀 + X(32B)。
    let mut compressed = Vec::with_capacity(33);
    compressed.push(if uncompressed[64] & 1 == 0 {
        0x02
    } else {
        0x03
    });
    compressed.extend_from_slice(&uncompressed[1..33]);
    let vk2 = verifying_key_from_hex(&hex_encode(&compressed)).expect("compressed load");
    assert_eq!(hex_encode(&public_key_bytes(&vk2)), kp.public_key);
}

#[test]
fn key_fp_is_sha256_of_uncompressed_pubkey() {
    let kp = generate_key_pair();
    let vk = verifying_key_from_hex(&kp.public_key).unwrap();
    let pubkey = public_key_bytes(&vk);
    let fp = key_fp(&pubkey);
    use sha2::Digest;
    let expect: [u8; 32] = sha2::Sha256::digest(pubkey).into();
    assert_eq!(fp, expect);
    // 不同公钥指纹不同。
    let other = generate_key_pair();
    let other_vk = verifying_key_from_hex(&other.public_key).unwrap();
    assert_ne!(fp, key_fp(&public_key_bytes(&other_vk)));
}
