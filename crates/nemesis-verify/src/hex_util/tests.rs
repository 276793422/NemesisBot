//! Hex 编解码单测（S6 覆盖率批次，quality-hardening goal 2026-08-25）。
//!
//! 覆盖：hex_decode_32 长度/字符错误臂、trim、大小写不敏感；
//! hex_decode_vec 奇数长度 / 非法字符 / 往返。

use super::*;

#[test]
fn hex_encode_lowercase_roundtrip() {
    assert_eq!(hex_encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    assert_eq!(hex_encode(&[]), "");
    // 往返：decode_vec(encode(x)) == x
    let bytes: Vec<u8> = (0u8..=255).collect();
    assert_eq!(hex_decode_vec(&hex_encode(&bytes)).unwrap(), bytes);
}

#[test]
fn hex_decode_32_accepts_uppercase_and_trims() {
    let expect = [
        0xABu8, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32,
        0x10, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
        0xEE, 0xFF,
    ];
    let upper = expect
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<String>();
    assert_eq!(hex_decode_32(&upper).unwrap(), expect);
    // 两端空白被 trim。
    assert_eq!(hex_decode_32(&format!("  {upper}  ")).unwrap(), expect);
}

#[test]
fn hex_decode_32_rejects_wrong_length() {
    let err = hex_decode_32("abcd").unwrap_err();
    assert!(err.contains("expected 64 hex chars"), "{err}");
    let err = hex_decode_32(&"a".repeat(66)).unwrap_err();
    assert!(err.contains("expected 64 hex chars"), "{err}");
}

#[test]
fn hex_decode_32_rejects_invalid_byte() {
    // 第 3 个字节偏移（offset 4）出现 'z'。
    let mut s = "0".repeat(64);
    s.replace_range(4..6, "zz");
    let err = hex_decode_32(&s).unwrap_err();
    assert!(err.contains("invalid hex byte at offset 4"), "{err}");
}

#[test]
fn hex_decode_vec_errors() {
    // 奇数长度
    let err = hex_decode_vec("abc").unwrap_err();
    assert_eq!(err, "odd hex length");
    // 偶数长度但非法字符
    let err = hex_decode_vec("zz").unwrap_err();
    assert!(err.contains("invalid hex byte at offset 0"), "{err}");
    // 空串 = 空向量
    assert_eq!(hex_decode_vec("").unwrap(), Vec::<u8>::new());
}

// 非 ASCII 输入诚实拒绝（2026-10-17 全仓横扫）：字节长度门挡不住多字节
// 字符，裸字节切片曾会落在字符中间 panic——现为 Err 而非进程崩溃。
#[test]
fn hex_decode_rejects_non_ascii() {
    // hex_decode_32：64 字节但含多字节字符（字节 8 落在「中」的续字节上）。
    let mut s = "0".repeat(7);
    s.push('中'); // 3 字节，7..10
    s.push_str(&"0".repeat(64 - 10));
    assert_eq!(s.len(), 64);
    let err = hex_decode_32(&s).unwrap_err();
    assert_eq!(err, "non-ascii hex");

    // hex_decode_vec：偶数字节长度但多字节字符跨字节 2。
    let err = hex_decode_vec("0中0").unwrap_err();
    assert_eq!(err, "non-ascii hex");
}
