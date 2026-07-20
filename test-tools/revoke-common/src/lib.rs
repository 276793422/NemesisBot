//! 吊销系统共用基础（客户端 `exe-sign-tool` + 服务端 `revoke-server` 都依赖）。
//!
//! - **SC0**：数据模型（`RevDim` / `CrlEntry` / `Crl` / `TrustedKeyList` / `SignedResponse`）
//! - **SC1**：吊销根密钥 Ed25519 签名 / 验签（服务端签响应、客户端验，防中间人伪造"未吊销"）
//!
//! 详见 `docs/PLAN/2026-07-18_signature-revocation-cloud.md`。

// codec（PE/ELF/Raw 解析 + content_hash）—— 签发/验证共用，从 exe-sign-tool 移入
pub mod codec;
pub mod crypto;
pub mod elf;
pub mod envelope;
pub mod hex_util;
pub mod pe;

use anyhow::{anyhow, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

// ===== SC0：数据模型 =====

/// 吊销维度（v1 核心四维度）。
///
/// 升级说明：新增维度在此 enum 加变体 + serde rename；CRL `version` bump。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevDim {
    /// 密钥级（吊销整把密钥）：value = key_id
    KeyId,
    /// 签名级（吊销单个签名）：value = sig_hash（SHA-256(signature)）
    SigHash,
    /// 文件级（吊销特定文件）：value = content_hash
    FileHash,
    /// 发布者级（吊销某发布者所有签名）：value = publisher 字符串
    Publisher,
}

/// CRL 单条吊销条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrlEntry {
    pub dim: RevDim,
    /// hex（key_id / sig_hash / content_hash）或 publisher 字符串。
    pub value: String,
    /// 吊销时间（Unix epoch）。
    pub revoked_at: u64,
    /// 吊销原因（自由文本，如 "key_leak" / "version_deprecated"）。
    pub reason: String,
}

/// CRL（吊销列表）。`version` 单调递增；`valid_until` 为本 CRL 有效期。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crl {
    pub version: u64,
    pub valid_until: u64,
    pub entries: Vec<CrlEntry>,
}

/// 受信任公钥状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStatus {
    Active,
    Revoked,
}

/// 受信任公钥条目（公钥轮换：新公钥发布、旧公钥吊销）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedKey {
    /// hex 公钥指纹。
    pub key_fp: String,
    pub status: KeyStatus,
    /// 该公钥信任截止时间（可选）。
    pub not_after: Option<u64>,
}

/// 受信任公钥列表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedKeyList {
    pub version: u64,
    pub valid_until: u64,
    pub keys: Vec<TrustedKey>,
}

/// 带签名的响应（服务端所有响应用此包装；客户端用吊销根公钥验签）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedResponse<T> {
    pub payload: T,
    /// hex Ed25519 签名（签的是 `payload` 的 canonical JSON bytes）。
    pub sig: String,
}

/// CRL 匹配：检查给定 (维度, 值) 是否命中 CRL。命中返回第一条（含 revoked_at/reason）。
///
/// verify 时按 key_id / sig_hash / content_hash / publisher 四维度逐项查；任一命中即 Revoked。
pub fn crl_match<'a>(crl: &'a Crl, dim: RevDim, value: &str) -> Option<&'a CrlEntry> {
    crl.entries
        .iter()
        .find(|e| e.dim == dim && e.value == value)
}

// ===== SC1：吊销根密钥签名 / 验签 =====

/// hex 编码（本地实现，避免引 hex crate）。
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// hex 解码 64 字节（Ed25519 签文）。
fn hex_decode_64(hex: &str) -> Result<[u8; 64]> {
    let hex = hex.trim();
    if hex.len() != 128 {
        return Err(anyhow!(
            "expected 128 hex chars (64 bytes), got {}",
            hex.len()
        ));
    }
    let mut arr = [0u8; 64];
    for i in 0..64 {
        arr[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| anyhow!("invalid hex at {}: {}", i * 2, e))?;
    }
    Ok(arr)
}

/// 用吊销根私钥签名 `payload`，返回 [`SignedResponse`]。
///
/// 签名对象 = `serde_json::to_vec(payload)`（struct 字段按定义序，确定性）。
pub fn sign_response<T: Serialize + Clone>(
    payload: &T,
    crkey: &SigningKey,
) -> Result<SignedResponse<T>> {
    let bytes = serde_json::to_vec(payload).map_err(|e| anyhow!("serialize payload: {}", e))?;
    let sig = crkey.sign(&bytes);
    Ok(SignedResponse {
        payload: payload.clone(),
        sig: hex_encode(sig.to_bytes().as_ref()),
    })
}

/// 验证 [`SignedResponse`] 的签名（用吊销根公钥）。`true` = 签名有效（响应可信）。
///
/// 调用方先用 `serde_json` 反序列化出 `SignedResponse<T>`（需 `T: DeserializeOwned`），
/// 再调本函数验签。验签失败 = 响应可能被篡改/伪造，调用方应视为不可信。
pub fn verify_response<T: Serialize>(signed: &SignedResponse<T>, crpub: &VerifyingKey) -> Result<bool> {
    let bytes = serde_json::to_vec(&signed.payload).map_err(|e| anyhow!("serialize payload: {}", e))?;
    let sig_bytes = hex_decode_64(&signed.sig)?;
    let sig = Signature::from_bytes(&sig_bytes);
    Ok(crpub.verify(&bytes, &sig).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用：从固定种子构造密钥（确定性，无需 rand 依赖）。
    fn crkey(seed: u8) -> (SigningKey, VerifyingKey) {
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
                CrlEntry { dim: RevDim::KeyId, value: "abc".into(), revoked_at: 1, reason: "leak".into() },
                CrlEntry { dim: RevDim::Publisher, value: "evil".into(), revoked_at: 2, reason: "bad".into() },
            ],
        };
        assert!(crl_match(&crl, RevDim::KeyId, "abc").is_some());
        assert!(crl_match(&crl, RevDim::Publisher, "evil").is_some());
        assert!(crl_match(&crl, RevDim::KeyId, "none").is_none());
    }

    #[test]
    fn sign_verify_roundtrip() {
        let (sk, vk) = crkey(1);
        let payload = Crl { version: 3, valid_until: 99, entries: vec![] };
        let signed = sign_response(&payload, &sk).unwrap();
        assert!(verify_response(&signed, &vk).unwrap());
    }

    #[test]
    fn verify_rejects_tampered_payload() {
        let (sk, vk) = crkey(1);
        let mut signed = sign_response(&Crl { version: 1, valid_until: 1, entries: vec![] }, &sk).unwrap();
        signed.payload.version = 999; // 篡改 payload
        assert!(!verify_response(&signed, &vk).unwrap());
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let (sk, _) = crkey(1);
        let (_, vk2) = crkey(2); // 不同种子 → 不同公钥
        let signed = sign_response(&Crl { version: 1, valid_until: 1, entries: vec![] }, &sk).unwrap();
        assert!(!verify_response(&signed, &vk2).unwrap());
    }
}
