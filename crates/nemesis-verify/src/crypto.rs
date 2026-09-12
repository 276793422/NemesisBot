//! 签名原语（**v4：ECDSA P-256 + SHA-256，RFC 6979 确定式签名**）。
//!
//! v4 为 Authenticode 格式对齐迁移（goal `docs/PLAN/2026-09-12_authenticode-v4-goal.md`）：
//! WinVerifyTrust 的接受集为 RSA/ECDSA，Ed25519 算法级不被识别（报「签名无效」而非干净
//! UNTRUSTEDROOT），故算法层迁移至 ECDSA P-256 + SHA-256。签名经 `ecdsa` crate 的
//! `Signer<Signature>` 实现，**默认即 RFC 6979 确定式 nonce**（同消息两次签名字节一致）。
//!
//! 公钥编码口径：SEC1 **uncompressed** 点（65B，`0x04 | X | Y`）——与 X.509
//! SubjectPublicKeyInfo 内的编码一致；`key_fp` = SHA-256(该 65B 编码)。

use crate::hex_util::{hex_decode_32, hex_decode_vec, hex_encode};
use anyhow::{Result, anyhow};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use rand::rngs::OsRng;

/// P-256 密钥对（hex 编码）。
pub struct KeyPair {
    /// hex P-256 私钥标量（64 hex 字符 = 32 字节）。
    pub private_key: String,
    /// hex 公钥 SEC1 uncompressed 编码（130 hex 字符 = 65 字节）。
    pub public_key: String,
}

/// 生成新 P-256 密钥对。
pub fn generate_key_pair() -> KeyPair {
    let sk = SigningKey::random(&mut OsRng);
    KeyPair {
        private_key: hex_encode(sk.to_bytes().as_ref()),
        public_key: hex_encode(&public_key_bytes(sk.verifying_key())),
    }
}

/// 公钥 SEC1 uncompressed 编码（65B）。
pub fn public_key_bytes(vk: &VerifyingKey) -> [u8; 65] {
    let point = vk.as_affine().to_encoded_point(false);
    let mut out = [0u8; 65];
    out.copy_from_slice(point.as_bytes());
    out
}

/// 从 hex 私钥（32B 标量）构造 [`SigningKey`]。
pub fn signing_key_from_hex(hex: &str) -> Result<SigningKey> {
    let bytes = hex_decode_32(hex).map_err(|e| anyhow!("invalid private key: {}", e))?;
    SigningKey::from_bytes(&bytes.into()).map_err(|e| anyhow!("invalid private key: {e}"))
}

/// 从 hex 公钥构造 [`VerifyingKey`]（收 SEC1 uncompressed 65B / compressed 33B）。
pub fn verifying_key_from_hex(hex: &str) -> Result<VerifyingKey> {
    let bytes = hex_decode_vec(hex).map_err(|e| anyhow!("invalid public key: {e}"))?;
    if bytes.len() != 65 && bytes.len() != 33 {
        return Err(anyhow!(
            "invalid public key: expected 65 (uncompressed) or 33 (compressed) bytes, got {}",
            bytes.len()
        ));
    }
    VerifyingKey::from_sec1_bytes(&bytes).map_err(|e| anyhow!("invalid public key: {e}"))
}

/// 公钥 SHA-256 指纹（`key_fp`）—— CRL / trusted_keys 索引口径。
///
/// v4 口径 = SHA-256(SEC1 uncompressed 65B 编码)；入参收任意长度字节切片
/// （调用方负责传 [`public_key_bytes`] 的输出）。
pub fn key_fp(pubkey: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(pubkey).into()
}

/// P-256 签名 → 固定 64 字节 r||s（RFC 6979 确定式 nonce）。
pub fn p256_sign(sk: &SigningKey, msg: &[u8]) -> [u8; 64] {
    use p256::ecdsa::signature::Signer;
    let sig: Signature = sk.sign(msg);
    sig.to_bytes().into()
}

/// P-256 验签（64 字节 r||s；返回 true=有效）。
pub fn p256_verify(vk: &VerifyingKey, msg: &[u8], sig: &[u8; 64]) -> bool {
    use p256::ecdsa::signature::Verifier;
    match Signature::from_slice(sig) {
        Ok(s) => vk.verify(msg, &s).is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
