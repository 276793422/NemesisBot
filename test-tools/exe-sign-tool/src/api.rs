//! 签名/验证主流程（C5：verify 云优先 + 本地 fallback soft-fail）。
//!
//! 流程：本地基础验证（签名数学有效性）→ 通过后：
//! - 配置云端且可达 → 用云端实时结果（cloud_state=Reached, source=Cloud）
//! - 云端不可达 → soft-fail 本地吊销查（cloud_state=Unreachable, source=Local）
//! - 未配云端 → 纯本地（cloud_state=NotConfigured）

use crate::{
    cloud::{CloudClient, CloudVerifyReq},
    codec::{detect_codec, detect_format, CodecError, FORMAT_TAG_RAW},
    crypto,
    envelope::{
        self, align_up, build_body_plaintext, build_footer, build_signed_meta, envelope_body_range,
        find_our_footer, parse_body, parse_footer, DOMAIN, FOOTER_AAD_LEN, FOOTER_LEN, FORMAT_VER,
    },
    policy::RevocationPolicy,
    status::{Code, CloudState, SignatureStatus, Source},
};
use anyhow::{anyhow, Result};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::RngCore;
use revoke_common::{crl_match, KeyStatus, RevDim};
use sha2::{Digest, Sha256};

fn hex_str(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 构造本地 SignatureStatus（cloud_state=NotConfigured，无元信息——仅基础失败用）。
fn local_status(code: Code, detail: impl Into<String>) -> SignatureStatus {
    SignatureStatus {
        code,
        cloud_state: CloudState::NotConfigured,
        source: Source::Local,
        signed_at: 0,
        expires_at: None,
        revoked_at: None,
        reason: None,
        detail: detail.into(),
        crl_ver: None,
        trusted_keys_ver: None,
    }
}

/// 从 hex 私钥文件加载 SigningKey。
pub fn load_signing_key(path: &std::path::Path) -> Result<SigningKey> {
    let hex = std::fs::read_to_string(path)?.trim().to_string();
    crypto::signing_key_from_hex(&hex)
}

/// 给可执行文件签名（原地追加 envelope）。C1：含 publisher/expires_at/sig_hash。
pub fn sign_executable(
    path: &std::path::Path,
    sk: &SigningKey,
    sym_key: &[u8; 32],
    signed_at: u64,
    key_id: u32,
    publisher: Option<&str>,
    expires_at: Option<u64>,
) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let codec = detect_codec(&bytes);
    let format_tag = detect_format(&bytes);

    let content_len = match codec.compute_l(&bytes)? {
        Some(l) => l,
        None => bytes.len(),
    };
    // envelope content_len 字段是 u32（4B），>4GB 文件无法表达
    if content_len > u32::MAX as usize {
        return Err(anyhow!(
            "content too large ({} bytes > 4GB); envelope content_len field is u32",
            content_len
        ));
    }
    let overlay_start = if format_tag == FORMAT_TAG_RAW { 0 } else { content_len };
    let excludes = codec.overlay_excludes(&bytes);
    if find_our_footer(&bytes, overlay_start, &excludes).is_some() {
        return Err(anyhow!(
            "file already signed (our envelope present); use --force to re-sign"
        ));
    }
    let content_hash = codec.content_hash(&bytes, content_len)?;
    let vk = sk.verifying_key();
    let fp: [u8; 32] = Sha256::digest(vk.to_bytes().as_ref()).into();

    let signed_meta = build_signed_meta(
        format_tag, 0u16, signed_at, key_id, &fp, &content_hash, publisher, expires_at,
    );
    let mut signing_msg = Vec::with_capacity(DOMAIN.len() + signed_meta.len());
    signing_msg.extend_from_slice(DOMAIN);
    signing_msg.extend_from_slice(&signed_meta);
    let signature = crypto::ed25519_sign(sk, &signing_msg);
    let sig_hash: [u8; 32] = Sha256::digest(signature).into();

    let body_plain = build_body_plaintext(
        0u16, signed_at, key_id, &fp, &signature, &content_hash, publisher, expires_at, &sig_hash,
    );

    let body_len = body_plain.len() + envelope::AEAD_TAG_LEN;
    let total_len = align_up(body_len + FOOTER_LEN, envelope::ENVELOPE_ALIGN);
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let footer = build_footer(format_tag, total_len, body_len, content_len, &nonce);

    let ciphertext = crypto::aead_seal(sym_key, &nonce, &body_plain, &footer[..FOOTER_AAD_LEN])?;
    let envelope_bytes = envelope::assemble_envelope(&ciphertext, &footer);
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.write_all(&envelope_bytes)?;
    Ok(())
}

/// 本地吊销查（trusted-keys → CRL 四维度 → 过期）。返 (code, revoked_at, reason, crl_ver)。
/// 优先级：UntrustedPublisher > Revoked > Expired > Valid。
fn check_local_revocation(
    body: &envelope::ParsedBody,
    policy: &RevocationPolicy,
    key_fp_hex: &str,
    sig_hash_hex: &str,
    content_hash_hex: &str,
) -> (Code, Option<u64>, Option<String>, Option<u64>) {
    // ① trusted-keys
    if let Some(tkl) = policy.trusted_keys {
        let active = tkl.keys.iter().any(|k| k.key_fp == key_fp_hex && k.status == KeyStatus::Active);
        if !active {
            return (Code::UntrustedPublisher, None, None, None);
        }
    }
    // ② CRL 四维度（Revoked 优先于 Expired）。sig_hash 全 0（旧签名无 TLV）跳过 SigHash 维度，
    //    防 CRL 含 SigHash="00..00" 误吊销所有旧签名。
    if let Some(crl) = policy.crl {
        let publisher = body.publisher.as_deref().unwrap_or("");
        let sig_hash_valid = body.sig_hash != [0u8; 32];
        let hit = crl_match(crl, RevDim::KeyId, key_fp_hex)
            .or_else(|| {
                if sig_hash_valid {
                    crl_match(crl, RevDim::SigHash, sig_hash_hex)
                } else {
                    None
                }
            })
            .or_else(|| crl_match(crl, RevDim::FileHash, content_hash_hex))
            .or_else(|| crl_match(crl, RevDim::Publisher, publisher));
        if let Some(e) = hit {
            return (Code::Revoked, Some(e.revoked_at), Some(e.reason.clone()), Some(crl.version));
        }
    }
    // ③ 过期
    if let Some(exp) = body.expires_at {
        if policy.now > exp {
            return (Code::Expired, None, None, policy.crl.map(|c| c.version));
        }
    }
    (Code::Valid, None, None, policy.crl.map(|c| c.version))
}

/// 验证可执行文件签名 + 吊销策略（云优先 + 本地 fallback）。
pub fn verify_executable(
    bytes: &[u8],
    vk: &VerifyingKey,
    sym_key: &[u8; 32],
    policy: &RevocationPolicy,
    cloud: Option<&CloudClient>,
) -> Result<SignatureStatus> {
    let codec = detect_codec(bytes);
    let format_tag = detect_format(bytes);

    // ── 本地基础验证（数学确定性，优先于一切吊销）──
    let l_opt = match codec.compute_l(bytes) {
        Ok(o) => o,
        Err(CodecError::NotAnExecutable) | Err(CodecError::Truncated) => {
            return Ok(local_status(Code::NoSignature, "not an executable / too short"));
        }
        Err(e) => return Ok(local_status(Code::Malformed, format!("compute_l: {}", e))),
    };
    let overlay_start = if format_tag == FORMAT_TAG_RAW { 0 } else { l_opt.unwrap_or(0) };
    let excludes = codec.overlay_excludes(bytes);

    let footer_offset = match find_our_footer(bytes, overlay_start, &excludes) {
        Some(o) => o,
        None => return Ok(local_status(Code::NoSignature, "no envelope found")),
    };
    let mut footer = [0u8; FOOTER_LEN];
    footer.copy_from_slice(&bytes[footer_offset..footer_offset + FOOTER_LEN]);
    let parsed = match parse_footer(&footer) {
        Ok(p) => p,
        Err(e) => return Ok(local_status(Code::BadDigest, format!("footer: {}", e))),
    };
    if parsed.format_ver != FORMAT_VER
        || parsed.sig_algo != envelope::SIG_ALGO_ED25519
        || parsed.enc_algo != envelope::ENC_ALGO_CHACHA20
    {
        return Ok(local_status(Code::Malformed, format!("unsupported ver/algo: {}", parsed.format_ver)));
    }
    if parsed.total_len < parsed.body_len + FOOTER_LEN
        || parsed.total_len % envelope::ENVELOPE_ALIGN != 0
        || footer_offset + FOOTER_LEN < parsed.total_len
    {
        return Ok(local_status(Code::Malformed, "invalid total_len"));
    }
    let envelope_start = footer_offset + FOOTER_LEN - parsed.total_len;
    if envelope_start < overlay_start {
        return Ok(local_status(Code::Malformed, "envelope before overlay start"));
    }
    let content_len = if let Some(l) = l_opt {
        if parsed.content_len != l {
            return Ok(local_status(Code::BadDigest, format!("content_len {} != L {}", parsed.content_len, l)));
        }
        l
    } else {
        parsed.content_len
    };
    if content_len > bytes.len() {
        return Ok(local_status(Code::Malformed, "content_len > file len"));
    }
    let (body_start, body_end) = envelope_body_range(footer_offset, &parsed);
    if body_end > bytes.len() || body_start > body_end {
        return Ok(local_status(Code::Malformed, "body range out of bounds"));
    }
    let ciphertext = &bytes[body_start..body_end];
    let content_hash = match codec.content_hash(bytes, content_len) {
        Ok(h) => h,
        Err(e) => return Ok(local_status(Code::Malformed, format!("content_hash: {}", e))),
    };
    let aad = &bytes[footer_offset..footer_offset + FOOTER_AAD_LEN];
    let plaintext = match crypto::aead_open(sym_key, &parsed.nonce, ciphertext, aad) {
        Ok(p) => p,
        Err(_) => return Ok(local_status(Code::BadDigest, "AEAD authentication failed")),
    };
    let body = match parse_body(&plaintext) {
        Ok(b) => b,
        Err(e) => return Ok(local_status(Code::Malformed, format!("parse_body: {}", e))),
    };
    if body.content_hash != content_hash {
        return Ok(local_status(Code::BadDigest, "content_hash mismatch"));
    }
    let recomputed_sig_hash: [u8; 32] = Sha256::digest(body.signature).into();
    if body.sig_hash != [0u8; 32] && body.sig_hash != recomputed_sig_hash {
        return Ok(local_status(Code::BadDigest, "sig_hash mismatch"));
    }
    let signed_meta = build_signed_meta(
        parsed.format_tag, body.flags, body.signed_at, body.key_id, &body.key_fp,
        &content_hash, body.publisher.as_deref(), body.expires_at,
    );
    let mut signing_msg = Vec::with_capacity(DOMAIN.len() + signed_meta.len());
    signing_msg.extend_from_slice(DOMAIN);
    signing_msg.extend_from_slice(&signed_meta);
    if !crypto::ed25519_verify(vk, &signing_msg, &body.signature) {
        return Ok(local_status(Code::SignatureInvalid, "Ed25519 verify failed"));
    }

    // ── 基础验证通过，进入吊销决策（云优先 + 本地 fallback）──
    let key_fp_hex = hex_str(&body.key_fp);
    let sig_hash_hex = hex_str(&body.sig_hash);
    let content_hash_hex = hex_str(&body.content_hash);

    // 云端优先（若配置）
    if let Some(c) = cloud {
        let sig_hash_opt = if body.sig_hash == [0u8; 32] {
            None // 旧签名（无 sig_hash TLV）不发 SigHash 维度，防误中 CRL
        } else {
            Some(sig_hash_hex.clone())
        };
        let req = CloudVerifyReq {
            key_fp: Some(key_fp_hex.clone()),
            sig_hash: sig_hash_opt,
            content_hash: Some(content_hash_hex.clone()),
            publisher: body.publisher.clone(),
        };
        match c.verify(&req)? {
            Some(resp) => {
                // 云端实时核实（Reached）
                let code = match resp.code.as_str() {
                    "revoked" => Code::Revoked,
                    "untrusted" => Code::UntrustedPublisher,
                    _ => {
                        // valid → 仍判本地 expires_at（过期本地确定）
                        if let Some(exp) = body.expires_at {
                            if policy.now > exp {
                                Code::Expired
                            } else {
                                Code::Valid
                            }
                        } else {
                            Code::Valid
                        }
                    }
                };
                return Ok(SignatureStatus {
                    code,
                    cloud_state: CloudState::Reached,
                    source: Source::Cloud,
                    signed_at: body.signed_at,
                    expires_at: body.expires_at,
                    revoked_at: resp.revoked_at,
                    reason: resp.reason,
                    detail: format!("cloud: {}", resp.code),
                    crl_ver: Some(resp.crl_ver),
                    trusted_keys_ver: Some(resp.trusted_keys_ver),
                });
            }
            None => {
                // 云端不可达 → soft-fail 本地兜底
                let (code, revoked_at, reason, crl_ver) =
                    check_local_revocation(&body, policy, &key_fp_hex, &sig_hash_hex, &content_hash_hex);
                return Ok(SignatureStatus {
                    code,
                    cloud_state: CloudState::Unreachable,
                    source: Source::Local,
                    signed_at: body.signed_at,
                    expires_at: body.expires_at,
                    revoked_at,
                    reason,
                    detail: "cloud unreachable, local fallback".into(),
                    crl_ver,
                    trusted_keys_ver: None,
                });
            }
        }
    }

    // 无云端 → 纯本地
    let (code, revoked_at, reason, crl_ver) =
        check_local_revocation(&body, policy, &key_fp_hex, &sig_hash_hex, &content_hash_hex);
    Ok(SignatureStatus {
        code,
        cloud_state: CloudState::NotConfigured,
        source: Source::Local,
        signed_at: body.signed_at,
        expires_at: body.expires_at,
        revoked_at,
        reason,
        detail: String::new(),
        crl_ver,
        trusted_keys_ver: None,
    })
}

/// 自检：验证当前进程可执行文件。
pub fn verify_current_exe(
    vk: &VerifyingKey,
    sym_key: &[u8; 32],
    policy: &RevocationPolicy,
    cloud: Option<&CloudClient>,
) -> Result<SignatureStatus> {
    let path = std::env::current_exe()?;
    let bytes = std::fs::read(path)?;
    Ok(verify_executable(&bytes, vk, sym_key, policy, cloud)?)
}
