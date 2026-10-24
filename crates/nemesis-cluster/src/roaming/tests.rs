//! roaming 模块 helper 测试：node_id hex 校验（on-curve / 长度 / 字符）。

use super::{node_id_from_hex, random_valid_iroh_hex};

#[test]
fn node_id_from_hex_accepts_valid_and_case_insensitive() {
    let hex = random_valid_iroh_hex();
    assert_eq!(hex.len(), 64, "node_id hex 应为 64 字符");
    let parsed = node_id_from_hex(&hex).expect("合法 hex 应通过");
    let upper = node_id_from_hex(&hex.to_uppercase()).expect("大写 hex 应通过");
    assert_eq!(parsed, upper, "大小写不敏感");
}

#[test]
fn node_id_from_hex_rejects_garbage() {
    assert!(node_id_from_hex("zzzz").is_err(), "非 hex 字符应拒绝");
    assert!(node_id_from_hex("abcd").is_err(), "长度不足应拒绝");
    assert!(node_id_from_hex(&"ab".repeat(33)).is_err(), "33 字节应拒绝");
}

#[test]
fn node_id_from_hex_rejects_non_curve_point() {
    // "ab"*32：长度对、字符对，但解压不出合法曲线点（x² 非二次剩余）——
    // VerifyingKey::from_bytes 应拒绝（与 iroh EndpointId::from_bytes 同
    // 语义，announce 入池闸的底座）。A1 实录：iroh 拒 ab*32 报
    // 「data is not a valid public key」；注意全 0xff 不行——
    // curve25519-dalek 域解码隐式 mod-p 归约，0xff*32 归约后恰好是合法点。
    assert!(
        node_id_from_hex(&"ab".repeat(32)).is_err(),
        "解压失败的字节串应拒绝"
    );
}

#[test]
fn random_valid_iroh_hex_yields_distinct_valid_ids() {
    let a = random_valid_iroh_hex();
    let b = random_valid_iroh_hex();
    assert_ne!(a, b, "两次调用应互异");
    assert!(node_id_from_hex(&a).is_ok());
    assert!(node_id_from_hex(&b).is_ok());
}
