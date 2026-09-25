//! T1（扩展计划 D3）：工具收据（tool receipts）——防幻觉执行证明。
//!
//! 每个真实执行的 registry 工具结果在进 loop 守卫前，由
//! [`generate_receipt`] 以实例私有 [`ReceiptKey`] 计算 HMAC-SHA256
//! 执行证明：`nmb-receipt-{ts_ms}-{b64url(sig)}`。收据不进 LLM 上下文、
//! 不落盘（key 每实例随机生成），只入 [`crate::turn_guard::TurnGuard`]
//! 的本轮收据环。结果文本自称成功但无收据（未来路径绕过执行点注入
//! "成功"结果 / 谎报执行）会被
//! [`crate::turn_guard::TurnGuard::record_tool_outcome_verified`]
//! 以合成失败签名喂 escalation 判定。
//!
//! args/result 先 SHA-256 再入 HMAC（两段式摘要）：
//! 长文本不直接进 MAC，签名长度恒定。

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// 收据文本前缀。
pub const RECEIPT_PREFIX: &str = "nmb-receipt-";

/// 收据签名密钥——每 AgentLoop 实例一把，随机生成，不进 LLM 上下文
/// 不落盘（进程内一致性核对用，非跨进程凭据）。
pub struct ReceiptKey([u8; 32]);

impl ReceiptKey {
    /// 随机生成实例密钥（`OsRng` OS 熵源，32 字节全随机）。
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        Self(key)
    }
}

/// 当前墙钟毫秒（收据时间戳分量）。
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 计算执行收据：`nmb-receipt-{ts_ms}-{b64url(HMAC-SHA256(key,
/// tool || 0x00 || SHA256(args) || SHA256(result) || ts_ms))}`。
pub fn generate_receipt(
    key: &ReceiptKey,
    tool: &str,
    args: &str,
    result: &str,
    ts_ms: u64,
) -> String {
    let sig = receipt_sig(key, tool, args, result, ts_ms);
    format!(
        "{RECEIPT_PREFIX}{ts_ms}-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig)
    )
}

/// 校验收据：重算签名后恒时比较（长度先行短路，内容逐字节 XOR 累或）。
pub fn verify_receipt(
    key: &ReceiptKey,
    tool: &str,
    args: &str,
    result: &str,
    ts_ms: u64,
    receipt: &str,
) -> bool {
    let expect = generate_receipt(key, tool, args, result, ts_ms);
    if expect.len() != receipt.len() {
        return false;
    }
    // 恒时比较（subtle 语义的手写等价，免新增依赖）。
    let a = expect.as_bytes();
    let b = receipt.as_bytes();
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// HMAC 签名本体（tool 域分隔 + 两段式摘要 + 时间戳）。
fn receipt_sig(key: &ReceiptKey, tool: &str, args: &str, result: &str, ts_ms: u64) -> [u8; 32] {
    let args_hash = Sha256::digest(args.as_bytes());
    let result_hash = Sha256::digest(result.as_bytes());
    let mut mac = HmacSha256::new_from_slice(&key.0).expect("hmac accepts any key length");
    mac.update(tool.as_bytes());
    mac.update(&[0u8]);
    mac.update(&args_hash);
    mac.update(&result_hash);
    mac.update(&ts_ms.to_le_bytes());
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
mod tests;
