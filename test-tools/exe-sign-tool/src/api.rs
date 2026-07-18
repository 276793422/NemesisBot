//! 签名/验证主流程。
//!
//! 外层 API：调用方提供文件、Ed25519 密钥与 ChaCha20 对称密钥（sym_key），
//! 不感知格式（PE/ELF/Raw）、不感知 envelope 内部结构。验证只返回
//! [`VerifyOutcome`]，后续处置（拒绝运行/警告/放行）由调用方决定。
//!
//! sym_key 由调用方传入（CLI 的 `--sym` 文件、环境变量 `NEMESIS_SYM_KEY`、
//! 或编译期默认，由调用方决定优先级），本模块不读环境变量——便于测试与
//! 被保护程序用固定 sym_key 自检。
//!
//! # 签名流程
//! 1. 读文件 → codec 算 L（Raw 用文件长度）
//! 2. 查重复（已有我们的 envelope → 拒绝）
//! 3. content_hash = codec.content_hash([0, content_len))
//! 4. signed_meta → Ed25519 签名
//! 5. 构造 footer → AEAD 加密 body（AAD = footer[0..36]）
//! 6. 拼装 envelope（4KB 对齐）追加到文件末尾
//!
//! # 验证流程
//! 1. codec 算 L → overlay 扫 magic 找我们的 footer（PE 跳过 Authenticode 区）
//! 2. 无 → [`VerifyOutcome::NoSignature`]；footer crc 不符 → [`Tampered`]
//! 3. 核对 content_len == L（PE/ELF）→ content_hash
//! 4. AEAD 解密（失败 → [`Tampered`]）→ 解析 body → 重组 signed_meta → Ed25519 验签
//!
//! [`Tampered`]: VerifyOutcome::Tampered

use crate::{
    codec::{detect_codec, detect_format, CodecError, FORMAT_TAG_RAW},
    crypto, envelope,
    envelope::{
        align_up, build_body_plaintext, build_footer, build_signed_meta, envelope_body_range,
        find_our_footer, parse_body, parse_footer, DOMAIN, FOOTER_AAD_LEN, FOOTER_LEN, FORMAT_VER,
    },
};
use anyhow::{anyhow, Result};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// 验证结果（只描述状态，不含处置策略——策略由调用方决定）。
#[derive(Debug)]
pub enum VerifyOutcome {
    /// 签名有效，附带签名元数据。
    Valid {
        signed_at: u64,
        key_id: u32,
        /// 签名者公钥的 SHA-256 指纹（32B）。
        key_fp: [u8; 32],
    },
    /// 未发现我们的 envelope。
    NoSignature,
    /// envelope 存在但被篡改（AEAD 认证失败 / footer 损坏 / hash 不符 / 多源不一致）。
    Tampered(String),
    /// 解密成功但 Ed25519 验签失败。
    SignatureInvalid,
    /// 不支持的 envelope 版本/算法。
    UnsupportedVersion(u8),
    /// 结构异常（total_len 不合法、越界等）。
    Malformed(String),
}

/// 从 hex 私钥文件加载 [`SigningKey`]。
pub fn load_signing_key(path: &std::path::Path) -> Result<SigningKey> {
    let hex = std::fs::read_to_string(path)?.trim().to_string();
    crypto::signing_key_from_hex(&hex)
}

/// 给可执行文件签名（原地追加 envelope）。
///
/// - `path`：目标可执行文件
/// - `sk`：Ed25519 私钥
/// - `sym_key`：ChaCha20 对称密钥（加密 envelope body；同一把必须用于 verify）
/// - `signed_at`：签名时间戳（Unix epoch）
/// - `key_id`：公钥标识（提示用，不作安全决策）
pub fn sign_executable(
    path: &std::path::Path,
    sk: &SigningKey,
    sym_key: &[u8; 32],
    signed_at: u64,
    key_id: u32,
) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let codec = detect_codec(&bytes);
    let format_tag = detect_format(&bytes);

    // content_len = L（PE/ELF）或文件长度（Raw）
    let content_len = match codec.compute_l(&bytes)? {
        Some(l) => l,
        None => bytes.len(),
    };

    // 重复签名检测
    let overlay_start = if format_tag == FORMAT_TAG_RAW {
        0
    } else {
        content_len
    };
    let excludes = codec.overlay_excludes(&bytes);
    if find_our_footer(&bytes, overlay_start, &excludes).is_some() {
        return Err(anyhow!(
            "file already signed (our envelope present); use --force to re-sign"
        ));
    }

    // content_hash
    let content_hash = codec.content_hash(&bytes, content_len)?;

    // 公钥指纹 = SHA-256(pubkey)
    let vk = sk.verifying_key();
    let fp: [u8; 32] = Sha256::digest(vk.to_bytes().as_ref()).into();

    // signed_meta + Ed25519 签名
    let signed_meta = build_signed_meta(format_tag, 0u16, signed_at, key_id, &fp, &content_hash);
    let mut signing_msg = Vec::with_capacity(DOMAIN.len() + signed_meta.len());
    signing_msg.extend_from_slice(DOMAIN);
    signing_msg.extend_from_slice(&signed_meta);
    let signature = crypto::ed25519_sign(sk, &signing_msg);

    // body 明文
    let body_plain = build_body_plaintext(0u16, signed_at, key_id, &fp, &signature, &content_hash);

    // footer 先构造（加密需用它作 AAD）
    let body_len = body_plain.len() + envelope::AEAD_TAG_LEN;
    let total_len = align_up(body_len + FOOTER_LEN, envelope::ENVELOPE_ALIGN);
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let footer = build_footer(format_tag, total_len, body_len, content_len, &nonce);

    // AEAD 加密（AAD = footer[0..36]），sym_key 由调用方传入
    let ciphertext = crypto::aead_seal(sym_key, &nonce, &body_plain, &footer[..FOOTER_AAD_LEN])?;

    // 拼装 envelope 并追加
    let envelope_bytes = envelope::assemble_envelope(&ciphertext, &footer);
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.write_all(&envelope_bytes)?;
    Ok(())
}

/// 验证可执行文件签名。
///
/// `sym_key` 必须与签名时使用的同一把 ChaCha20 对称密钥一致。
pub fn verify_executable(
    bytes: &[u8],
    vk: &VerifyingKey,
    sym_key: &[u8; 32],
) -> Result<VerifyOutcome> {
    let codec = detect_codec(bytes);
    let format_tag = detect_format(bytes);

    // L（PE/ELF）；Raw 为 None。compute_l 早期错误视为无签名（非可执行/过短）。
    let l_opt = match codec.compute_l(bytes) {
        Ok(o) => o,
        Err(CodecError::NotAnExecutable) | Err(CodecError::Truncated) => {
            return Ok(VerifyOutcome::NoSignature);
        }
        Err(e) => return Ok(VerifyOutcome::Malformed(format!("compute_l: {}", e))),
    };
    let overlay_start = if format_tag == FORMAT_TAG_RAW {
        0
    } else {
        l_opt.unwrap_or(0)
    };
    let excludes = codec.overlay_excludes(bytes);

    // 找我们的 footer（只定位 magic；crc 在 parse_footer 验，不符报 Tampered）
    let footer_offset = match find_our_footer(bytes, overlay_start, &excludes) {
        Some(o) => o,
        None => return Ok(VerifyOutcome::NoSignature),
    };
    let mut footer = [0u8; FOOTER_LEN];
    footer.copy_from_slice(&bytes[footer_offset..footer_offset + FOOTER_LEN]);
    let parsed = match parse_footer(&footer) {
        Ok(p) => p,
        Err(e) => return Ok(VerifyOutcome::Tampered(format!("footer: {}", e))),
    };

    // 版本/算法校验
    if parsed.format_ver != FORMAT_VER
        || parsed.sig_algo != envelope::SIG_ALGO_ED25519
        || parsed.enc_algo != envelope::ENC_ALGO_CHACHA20
    {
        return Ok(VerifyOutcome::UnsupportedVersion(parsed.format_ver));
    }

    // envelope 边界校验
    if parsed.total_len < parsed.body_len + FOOTER_LEN
        || parsed.total_len % envelope::ENVELOPE_ALIGN != 0
        || footer_offset + FOOTER_LEN < parsed.total_len
    {
        return Ok(VerifyOutcome::Malformed("invalid total_len".into()));
    }
    let envelope_start = footer_offset + FOOTER_LEN - parsed.total_len;
    if envelope_start < overlay_start {
        return Ok(VerifyOutcome::Malformed("envelope before overlay start".into()));
    }

    // content_len：PE/ELF 核对 == L（多源一致性）；Raw 用 footer 值
    let content_len = if let Some(l) = l_opt {
        if parsed.content_len != l {
            return Ok(VerifyOutcome::Tampered(format!(
                "content_len {} != L {}",
                parsed.content_len, l
            )));
        }
        l
    } else {
        parsed.content_len
    };
    if content_len > bytes.len() {
        return Ok(VerifyOutcome::Malformed("content_len > file len".into()));
    }

    // 密文 body 范围
    let (body_start, body_end) = envelope_body_range(footer_offset, &parsed);
    if body_end > bytes.len() || body_start > body_end {
        return Ok(VerifyOutcome::Malformed("body range out of bounds".into()));
    }
    let ciphertext = &bytes[body_start..body_end];

    // content_hash
    let content_hash = match codec.content_hash(bytes, content_len) {
        Ok(h) => h,
        Err(e) => return Ok(VerifyOutcome::Malformed(format!("content_hash: {}", e))),
    };

    // AEAD 解密（失败 → Tampered），sym_key 由调用方传入
    let aad = &bytes[footer_offset..footer_offset + FOOTER_AAD_LEN];
    let plaintext = match crypto::aead_open(sym_key, &parsed.nonce, ciphertext, aad) {
        Ok(p) => p,
        Err(_) => return Ok(VerifyOutcome::Tampered("AEAD authentication failed".into())),
    };

    // 解析 body
    let body = match parse_body(&plaintext) {
        Ok(b) => b,
        Err(e) => return Ok(VerifyOutcome::Malformed(format!("parse_body: {}", e))),
    };

    // content_hash 早判
    if body.content_hash != content_hash {
        return Ok(VerifyOutcome::Tampered("content_hash mismatch".into()));
    }

    // 重组 signed_meta + Ed25519 验签
    let signed_meta = build_signed_meta(
        parsed.format_tag,
        body.flags,
        body.signed_at,
        body.key_id,
        &body.key_fp,
        &content_hash,
    );
    let mut signing_msg = Vec::with_capacity(DOMAIN.len() + signed_meta.len());
    signing_msg.extend_from_slice(DOMAIN);
    signing_msg.extend_from_slice(&signed_meta);
    if !crypto::ed25519_verify(vk, &signing_msg, &body.signature) {
        return Ok(VerifyOutcome::SignatureInvalid);
    }

    Ok(VerifyOutcome::Valid {
        signed_at: body.signed_at,
        key_id: body.key_id,
        key_fp: body.key_fp,
    })
}

/// 自检：验证当前进程可执行文件。
///
/// 注意：纯软件自检能防"被动篡改"+抬高成本，但不能防"主动 patch 绕过"
/// （攻击者控制运行环境时可直接 patch 掉本调用）。防后者需 secure boot/TPM/外部预验。
pub fn verify_current_exe(vk: &VerifyingKey, sym_key: &[u8; 32]) -> Result<VerifyOutcome> {
    let path = std::env::current_exe()?;
    let bytes = std::fs::read(path)?;
    Ok(verify_executable(&bytes, vk, sym_key)?)
}
