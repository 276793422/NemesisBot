//! 企业微信回调消息加解密（P25）。
//!
//! 实现企业微信「智能机器人回调」的消息加密协议——与公众号/企业微信自建应用
//! 回调同一套 WXBizMsgCrypt 协议：
//!
//! 1. **签名**：`sha1(sort([token, timestamp, nonce, encrypt]).join(""))` 的 hex，
//!    与请求携带的 `msg_signature` 比对（防伪造 + 防篡改）。
//! 2. **密钥**：`EncodingAESKey`（43 字符）尾补 `=` 后 base64 解码 → 32 字节
//!    AES-256 密钥；IV 取密钥前 16 字节。
//! 3. **明文结构**：`random(16) + msg_len(u32 大端) + msg + receiveid`。
//! 4. **填充**：企业微信规范按 **32 字节块** PKCS7 风格填充（pad 值可为 16/32），
//!    与标准 AES PKCS7（16 字节块上限）不同——解密侧必须自实现去填充，
//!    不能用 16 字节上限的通用 PKCS7 unpad（pad=32 会被误判非法）。
//!
//! 纯 Rust 实现：`sha1` + `aes`（CBC 手工串接），不引入 openssl。

use base64::Engine;
use nemesis_types::error::{NemesisError, Result};
use sha1::{Digest, Sha1};

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};

/// AES 块大小（字节）。
const AES_BLOCK: usize = 16;
/// 企业微信填充块大小（字节）——注意是 32 不是 16（协议规范如此）。
const WECOM_PAD_BLOCK: usize = 32;
/// 明文头部长度：random(16) + msg_len(4)。
const PLAINTEXT_HEADER: usize = 20;

// ---------------------------------------------------------------------------
// 签名
// ---------------------------------------------------------------------------

/// 计算企业微信回调签名：四个参数字典序排序后拼接取 SHA1 hex。
pub fn signature(token: &str, timestamp: &str, nonce: &str, encrypt: &str) -> String {
    let mut parts = [token, timestamp, nonce, encrypt];
    // sort_unstable_by 按字节序（字典序）排序——与官方实现一致
    parts.sort_unstable();
    let joined = parts.concat();

    let mut hasher = Sha1::new();
    hasher.update(joined.as_bytes());
    hex::encode(hasher.finalize())
}

/// 校验回调签名（`msg_signature` 与本地计算结果比对）。
pub fn verify_signature(
    token: &str,
    timestamp: &str,
    nonce: &str,
    encrypt: &str,
    msg_signature: &str,
) -> bool {
    // 非 HMAC 场景不存在密钥侧时序泄露面；直接字符串比对即可。
    signature(token, timestamp, nonce, encrypt) == msg_signature
}

// ---------------------------------------------------------------------------
// 密钥
// ---------------------------------------------------------------------------

/// 从 `EncodingAESKey`（43 字符）派生 32 字节 AES-256 密钥。
///
/// 官方规则：EncodingAESKey 尾部补一个 `=` 后做标准 base64 解码。
pub fn aes_key_from_encoding(encoding_aes_key: &str) -> Result<[u8; 32]> {
    let b64 = format!("{encoding_aes_key}=");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| {
            NemesisError::Channel(format!("wecom: EncodingAESKey base64 解码失败: {e}"))
        })?;
    let key: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
        NemesisError::Channel(format!(
            "wecom: EncodingAESKey 解码后长度 {} 不等于 32",
            v.len()
        ))
    })?;
    Ok(key)
}

// ---------------------------------------------------------------------------
// AES-256-CBC（手工 CBC 串接 + 企业微信 32 字节块去填充）
// ---------------------------------------------------------------------------

/// AES-256-CBC 解密（裸 CBC，不做填充校验——去填充由 `strip_wecom_pad` 负责）。
fn aes256_cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], ciphertext: &mut [u8]) -> Result<()> {
    if ciphertext.len() % AES_BLOCK != 0 || ciphertext.is_empty() {
        return Err(NemesisError::Channel(format!(
            "wecom: 密文长度 {} 不是 16 字节块的整数倍",
            ciphertext.len()
        )));
    }
    let cipher = aes::Aes256::new_from_slice(key)
        .map_err(|e| NemesisError::Channel(format!("wecom: AES key 装配失败: {e}")))?;

    let mut prev: [u8; AES_BLOCK] = *iv;
    for chunk in ciphertext.chunks_exact_mut(AES_BLOCK) {
        let mut cur = [0u8; AES_BLOCK];
        cur.copy_from_slice(chunk);
        let mut block = *GenericArray::from_slice(&cur);
        cipher.decrypt_block(&mut block);
        for i in 0..AES_BLOCK {
            chunk[i] = block[i] ^ prev[i];
        }
        prev = cur;
    }
    Ok(())
}

/// AES-256-CBC 加密（裸 CBC；填充由调用方按企业微信 32 字节块规则先行完成）。
fn aes256_cbc_encrypt(key: &[u8; 32], iv: &[u8; 16], plaintext: &mut [u8]) {
    let cipher = aes::Aes256::new_from_slice(key).expect("key 固定 32 字节必可装配");
    let mut prev: [u8; AES_BLOCK] = *iv;
    for chunk in plaintext.chunks_exact_mut(AES_BLOCK) {
        for i in 0..AES_BLOCK {
            chunk[i] ^= prev[i];
        }
        let mut block = *GenericArray::from_slice(chunk);
        cipher.encrypt_block(&mut block);
        chunk.copy_from_slice(&block);
        prev.copy_from_slice(&block);
    }
}

/// 企业微信规范去填充：末字节为 pad 长度 n（1..=32），剥掉末尾 n 字节。
///
/// 注意 pad 上限是 32（企业微信按 32 字节块填充），不能用标准 PKCS7 的
/// 16 上限——这也是不通用 `cbc` crate unpad 的原因。
fn strip_wecom_pad(plaintext: &[u8]) -> Result<&[u8]> {
    if plaintext.is_empty() {
        return Err(NemesisError::Channel("wecom: 解密结果为空".to_string()));
    }
    let pad = *plaintext.last().expect("非空切片必有末元素") as usize;
    if pad == 0 || pad > WECOM_PAD_BLOCK || pad > plaintext.len() {
        return Err(NemesisError::Channel(format!("wecom: 非法填充长度 {pad}")));
    }
    Ok(&plaintext[..plaintext.len() - pad])
}

/// 企业微信规范填充（加密侧）：按 32 字节块，
/// pad 值 = 块大小 - (len % 32)，pad 字节取值即 pad 值（1..=32）。
fn apply_wecom_pad(plaintext_len: usize) -> Vec<u8> {
    let amount = WECOM_PAD_BLOCK - (plaintext_len % WECOM_PAD_BLOCK);
    vec![amount as u8; amount]
}

// ---------------------------------------------------------------------------
// 完整加解密
// ---------------------------------------------------------------------------

/// 解密一条回调消息。
///
/// 输入 `encrypt` 为 base64 密文；返回 `(msg 明文, receiveid)`。
/// receiveid 在企业微信自建应用场景是 corpid、智能机器人场景可能为空串；
/// 上层可按配置选择是否强校验。
pub fn decrypt_message(aes_key: &[u8; 32], encrypt_b64: &str) -> Result<(String, String)> {
    let mut ciphertext = base64::engine::general_purpose::STANDARD
        .decode(encrypt_b64.trim().as_bytes())
        .map_err(|e| NemesisError::Channel(format!("wecom: 密文 base64 解码失败: {e}")))?;

    let iv: [u8; 16] = aes_key[..16]
        .try_into()
        .expect("aes_key 固定 32 字节，前 16 字节必可转型");

    aes256_cbc_decrypt(aes_key, &iv, &mut ciphertext)?;
    let plaintext = strip_wecom_pad(&ciphertext)?;

    if plaintext.len() < PLAINTEXT_HEADER {
        return Err(NemesisError::Channel(format!(
            "wecom: 明文长度 {} 不足 20 字节头部",
            plaintext.len()
        )));
    }

    let msg_len =
        u32::from_be_bytes([plaintext[16], plaintext[17], plaintext[18], plaintext[19]]) as usize;
    let remaining = plaintext.len() - PLAINTEXT_HEADER;
    if msg_len > remaining {
        return Err(NemesisError::Channel(format!(
            "wecom: 消息长度字段 {msg_len} 超出剩余明文 {remaining} 字节"
        )));
    }

    let msg = String::from_utf8_lossy(&plaintext[PLAINTEXT_HEADER..PLAINTEXT_HEADER + msg_len])
        .to_string();
    let receiveid = String::from_utf8_lossy(&plaintext[PLAINTEXT_HEADER + msg_len..]).to_string();

    Ok((msg, receiveid))
}

/// 加密一条消息（协议对称面）：`random(16) + len(u32 BE) + msg + receiveid`
/// 按企业微信 32 字节块填充后 AES-256-CBC 加密再 base64。
///
/// 生产入站链路只解密；本函数供测试构造合法密文（以及未来被动回复扩展）。
pub fn encrypt_message(aes_key: &[u8; 32], msg: &str, receiveid: &str) -> Result<String> {
    let msg_bytes = msg.as_bytes();
    let receiveid_bytes = receiveid.as_bytes();
    let raw_len = PLAINTEXT_HEADER + msg_bytes.len() + receiveid_bytes.len();

    let mut plaintext = Vec::with_capacity(raw_len + WECOM_PAD_BLOCK);
    plaintext.extend_from_slice(&rand::random::<[u8; 16]>());
    plaintext.extend_from_slice(&(msg_bytes.len() as u32).to_be_bytes());
    plaintext.extend_from_slice(msg_bytes);
    plaintext.extend_from_slice(receiveid_bytes);
    plaintext.extend_from_slice(&apply_wecom_pad(raw_len));

    let iv: [u8; 16] = aes_key[..16]
        .try_into()
        .expect("aes_key 固定 32 字节，前 16 字节必可转型");
    aes256_cbc_encrypt(aes_key, &iv, &mut plaintext);

    Ok(base64::engine::general_purpose::STANDARD.encode(&plaintext))
}
