//! 签名原语（**v3：纯 Ed25519，去 AEAD / SYM_KEY**）。
//!
//! v3 移除了 v1/v2 的 ChaCha20-Poly1305 envelope 加密层——body 明文（像 PKCS#7）。
//! 理由：机密性价值低（签名信息非秘密）、完整性靠签名链 + 根锚、去掉全局 SYM_KEY 依赖。
//! **Ed25519 是真正的认证锚点**。

use crate::hex_util::{hex_decode_32, hex_encode};
use anyhow::{Result, anyhow};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;

/// Ed25519 密钥对（hex 编码）。
pub struct KeyPair {
    /// hex Ed25519 私钥（64 hex 字符 = 32 字节种子）。
    pub private_key: String,
    /// hex Ed25519 公钥（64 hex 字符 = 32 字节）。
    pub public_key: String,
}

/// 生成新 Ed25519 密钥对。
pub fn generate_key_pair() -> KeyPair {
    let mut csprng = OsRng;
    let sk = SigningKey::generate(&mut csprng);
    let vk = sk.verifying_key();
    KeyPair {
        private_key: hex_encode(sk.to_bytes().as_ref()),
        public_key: hex_encode(vk.to_bytes().as_ref()),
    }
}

/// 从 hex 私钥构造 [`SigningKey`]。
pub fn signing_key_from_hex(hex: &str) -> Result<SigningKey> {
    let bytes = hex_decode_32(hex).map_err(|e| anyhow!("invalid private key: {}", e))?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// 从 hex 公钥构造 [`VerifyingKey`]。
pub fn verifying_key_from_hex(hex: &str) -> Result<VerifyingKey> {
    let bytes = hex_decode_32(hex).map_err(|e| anyhow!("invalid public key: {}", e))?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| anyhow!("invalid public key: {}", e))
}

/// 公钥 SHA-256 指纹（`key_fp`）—— CRL / trusted_keys 索引用。
pub fn key_fp(pubkey: &[u8; 32]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(pubkey).into()
}

/// Ed25519 签名 → 64 字节。
pub fn ed25519_sign(sk: &SigningKey, msg: &[u8]) -> [u8; 64] {
    let sig: Signature = sk.sign(msg);
    sig.to_bytes()
}

/// Ed25519 验签（返回 true=有效）。
pub fn ed25519_verify(vk: &VerifyingKey, msg: &[u8], sig: &[u8; 64]) -> bool {
    let s = Signature::from_bytes(sig);
    vk.verify(msg, &s).is_ok()
}
