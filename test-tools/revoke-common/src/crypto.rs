//! 加密与签名原语。
//!
//! - **ChaCha20-Poly1305（AEAD）**：加密 envelope body，同时提供机密性 + 完整性。
//!   攻击者精细篡改密文 → 认证失败 → 被识别为"篡改"而非"无签名"。
//! - **Ed25519**：签名 signing_msg（`DOMAIN ++ signed_meta`），是真正的认证锚点。
//! - **对称密钥来源**：环境变量 `NEMESIS_SYM_KEY`（hex）优先，否则编译期
//!   [`BUILTIN_SYM_KEY_DEFAULT`]。被保护程序自检建议用编译期固定值（不读 env）。

use crate::hex_util::{hex_decode_32, hex_encode};
use anyhow::{anyhow, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::{rngs::OsRng, RngCore};

/// 编译期默认对称密钥（占位全 0；发布前替换为真实随机值，并固化进被保护程序）。
///
/// 安全说明：内置对称密钥可被静态逆向提取，提供的是 obscurity + AEAD 完整性
/// 层（抬高攻击成本），不是密码学强保护。真正的认证锚是固定的 Ed25519 公钥。
pub const BUILTIN_SYM_KEY_DEFAULT: [u8; 32] = [0u8; 32];

/// 对称密钥环境变量名（hex 编码 32 字节）。
pub const SYM_KEY_ENV: &str = "NEMESIS_SYM_KEY";

/// 获取对称密钥：环境变量 `NEMESIS_SYM_KEY`（hex）优先，否则 [`BUILTIN_SYM_KEY_DEFAULT`]。
pub fn get_sym_key() -> [u8; 32] {
    if let Ok(hex) = std::env::var(SYM_KEY_ENV) {
        if let Ok(k) = hex_decode_32(&hex) {
            return k;
        }
    }
    BUILTIN_SYM_KEY_DEFAULT
}

/// 生成随机 ChaCha20 对称密钥（32 字节，OsRng）。
///
/// 供 `keygen` 输出 `exe_sign.sym`，固化进签名工具与被保护程序（取代占位全 0 的
/// [`BUILTIN_SYM_KEY_DEFAULT`]）。同一把 sym_key 必须同时用于 sign 与 verify。
pub fn generate_sym_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

/// 从 hex 字符串构造对称密钥（32 字节）。
pub fn sym_key_from_hex(hex: &str) -> Result<[u8; 32]> {
    hex_decode_32(hex).map_err(|e| anyhow!("invalid sym key: {}", e))
}

/// Ed25519 密钥对（hex 编码）。
pub struct KeyPair {
    /// hex 编码 Ed25519 私钥（64 hex 字符 = 32 字节种子）。
    pub private_key: String,
    /// hex 编码 Ed25519 公钥（64 hex 字符 = 32 字节）。
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

/// ChaCha20-Poly1305 加密。返回 密文 ‖ 16B Poly1305 tag。
pub fn aead_seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| anyhow!("AEAD encrypt failed: {}", e))
}

/// ChaCha20-Poly1305 解密。认证失败返回 `Err`（调用方据此判 `Tampered`）。
pub fn aead_open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|e| anyhow!("AEAD decrypt failed: {}", e))
}

/// Ed25519 签名 → 64 字节签名。
pub fn ed25519_sign(sk: &SigningKey, msg: &[u8]) -> [u8; 64] {
    let sig: Signature = sk.sign(msg);
    sig.to_bytes()
}

/// Ed25519 验签（返回 true=有效）。
pub fn ed25519_verify(vk: &VerifyingKey, msg: &[u8], sig: &[u8; 64]) -> bool {
    let s = Signature::from_bytes(sig);
    vk.verify(msg, &s).is_ok()
}
