//! Hex 编码/解码工具。
//!
//! 本地实现，避免引入额外的 hex crate。编码为小写十六进制字符串，
//! 解码严格校验长度与字符有效性。与 nemesis-security / nemesis-skills
//! 中已有的 hex 工具保持一致的语义。

/// 将字节切片编码为小写十六进制字符串。
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 将十六进制字符串解码为 32 字节数组。
///
/// 要求输入恰为 64 个十六进制字符（不区分大小写），否则返回错误。
/// 主要用于 Ed25519 公私钥（32 字节）。
pub fn hex_decode_32(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", hex.len()));
    }
    let mut arr = [0u8; 32];
    for i in 0..32 {
        arr[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("invalid hex byte at offset {}: {}", i * 2, e))?;
    }
    Ok(arr)
}

/// 将十六进制字符串解码为变长字节向量。
///
/// 要求输入长度为偶数且全部为有效十六进制字符。
pub fn hex_decode_vec(hex: &str) -> Result<Vec<u8>, String> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return Err("odd hex length".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| format!("invalid hex byte at offset {}: {}", i, e))
        })
        .collect()
}
