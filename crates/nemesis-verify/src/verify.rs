//! 验证流程（v3：用 envelope 自带 pubkey 验签 + 证书链确认可信）。
//!
//! v3 与 v1/v2 的根本区别：**公钥随签名走**（envelope 带 pubkey），验证端不用
//! "内置固定公钥"——而是用 envelope 里的 pubkey 验签（自洽），再确认该 pubkey 可信。
//!
//! # 可信确认（T2 证书链版）
//! - envelope 带 `cert_chain`：走 [`crate::cert::verify_chain`]——leaf pubkey 经链
//!   （leaf + intermediates）一路签发到内置根公钥（`root_pubs`）。
//! - envelope 无 `cert_chain`（单根自签场景）：回退到 `pubkey ∈ root_pubs` 直接信任。

use crate::cert;
use crate::codec;
use crate::crypto;
use crate::envelope;
use anyhow::Result;
use ed25519_dalek::{SigningKey, VerifyingKey};
use sha2::Digest;

/// 验证结果。
#[derive(Debug, PartialEq)]
pub enum VerifyOutcome {
    /// 签名有效、公钥可信（经链或直接信任）、未吊销、未过期。
    Valid {
        signed_at: u64,
        key_fp: [u8; 32],
        pubkey: [u8; 32],
    },
    /// 无 envelope（文件未签名）。
    NoSignature,
    /// envelope 在但被篡改（footer crc / content_hash 不符 / 结构损坏）。
    Tampered(String),
    /// 签名自洽验签失败（signature 对 pubkey + signed_meta 不成立）。
    SignatureInvalid,
    /// 签名自洽但 pubkey 不可信（链不到根 / 链断裂 / 不在 root_pubs）。
    Untrusted,
    /// 吊销（CRL 四维度之一命中：key_fp / sig_hash / content_hash / publisher）。
    Revoked {
        dim: crate::RevDim,
        value: String,
        revoked_at: u64,
        reason: String,
    },
    /// 过期（证书 valid_not_after 或签名 key_not_after 超过当前时间）。
    Expired(String),
    /// 不支持的 envelope 格式版本（附带实际版本号）。
    UnsupportedVersion(u8),
    /// 结构无法解析。
    Malformed(String),
}

/// 对原始内容签名，返回 `content[..content_len] ++ envelope`（签名后的完整文件）。
///
/// `content_len`：PE/ELF 由 codec 算 L；Raw = `content.len()`。
/// envelope 带 `pubkey`（sk 对应公钥，随签名走）+ 可选 `cert_chain`（leaf + intermediates）。
pub fn sign_content(
    content: &[u8],
    sk: &SigningKey,
    signed_at: u64,
    cert_chain: Option<&[u8]>,
    publisher: Option<&str>,
    key_not_after: Option<u64>,
    ts_token: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let format_tag = codec::detect_format(content);
    let codec = codec::detect_codec(content);
    let l = codec.compute_l(content)?;
    let content_len = l.unwrap_or(content.len());
    let content_hash = codec.content_hash(content, content_len)?;

    let vk = sk.verifying_key();
    let pubkey = vk.to_bytes();
    let key_fp = crypto::key_fp(&pubkey);
    let cert_chain_hash: [u8; 32] = match cert_chain {
        Some(c) => sha2::Sha256::digest(c).into(),
        None => [0u8; 32],
    };

    let signed_meta = envelope::build_signed_meta(
        format_tag,
        0,
        signed_at,
        &key_fp,
        &content_hash,
        &pubkey,
        &cert_chain_hash,
        key_not_after,
        publisher,
    );
    let mut msg = Vec::with_capacity(envelope::DOMAIN.len() + signed_meta.len());
    msg.extend_from_slice(envelope::DOMAIN);
    msg.extend_from_slice(&signed_meta);
    let signature = crypto::ed25519_sign(sk, &msg);

    let body = envelope::build_body(
        0,
        signed_at,
        &key_fp,
        &content_hash,
        &pubkey,
        &signature,
        cert_chain,
        publisher,
        key_not_after,
        ts_token,
    );
    let total_len = envelope::align_up(body.len() + envelope::FOOTER_LEN, envelope::ENVELOPE_ALIGN);
    let footer = envelope::build_footer(format_tag, total_len, body.len(), content_len);
    let envelope_bytes = envelope::assemble_envelope(&body, &footer);

    // 多签名叠加：保留 content 全部（含已有 overlay 的旧 envelope），新 envelope append 末尾。
    // content_hash 仍基于 [0, content_len)（PE/ELF 的 L / Raw = content.len()），已有 overlay 不影响。
    // PE/ELF 支持多签名（L 分离 content 与 overlay）；Raw 无 L，二次签会含旧 envelope（不支持多签名，
    // 但 Raw 单签名不受影响——content 全 = file 无 overlay + env）。
    let mut signed = Vec::with_capacity(content.len() + envelope_bytes.len());
    signed.extend_from_slice(content);
    signed.extend_from_slice(&envelope_bytes);
    Ok(signed)
}

/// 验证 `bytes`（完整文件，含 envelope）。
///
/// `root_pubs`：验证端内置的根公钥（信任终止点）。
/// `now`：当前时间（Unix epoch），用于证书有效期校验。
///
/// 可信确认：envelope 带 cert_chain 走链验证（leaf 经链到 `root_pubs`）；
/// 无链（单根自签）回退到 `pubkey ∈ root_pubs`。
pub fn verify_bytes(bytes: &[u8], root_pubs: &[VerifyingKey], now: u64) -> VerifyOutcome {
    let codec = codec::detect_codec(bytes);
    let l = match codec.compute_l(bytes) {
        Ok(opt) => opt,
        Err(_) => None, // 解析失败按 Raw 处理（从 content_len）
    };
    let overlay_start = l.unwrap_or(0);
    let excludes = codec.overlay_excludes(bytes);

    // 定位 footer
    let footer_off = match envelope::find_our_footer(bytes, overlay_start, &excludes) {
        Some(off) => off,
        None => return VerifyOutcome::NoSignature,
    };
    let mut footer = [0u8; envelope::FOOTER_LEN];
    footer.copy_from_slice(&bytes[footer_off..footer_off + envelope::FOOTER_LEN]);
    let pf = match envelope::parse_footer(&footer) {
        Ok(f) => f,
        Err(e) => return VerifyOutcome::Tampered(format!("footer: {}", e)),
    };
    if pf.format_ver != envelope::FORMAT_VER {
        return VerifyOutcome::UnsupportedVersion(pf.format_ver);
    }

    // 解析 body（明文）
    let (body_start, body_end) = envelope::envelope_body_range(footer_off, &pf);
    let body_bytes = match bytes.get(body_start..body_end) {
        Some(b) => b,
        None => return VerifyOutcome::Malformed("body range out of bounds".into()),
    };
    let body = match envelope::parse_body(body_bytes) {
        Ok(b) => b,
        Err(e) => return VerifyOutcome::Malformed(format!("body: {}", e)),
    };

    // content_len：PE/ELF 必须等于 L；Raw 用 footer.content_len
    let content_len = match l {
        Some(l_val) => {
            if pf.content_len != l_val {
                return VerifyOutcome::Malformed(format!(
                    "content_len {} != L {}",
                    pf.content_len, l_val
                ));
            }
            l_val
        }
        None => pf.content_len,
    };
    if content_len > bytes.len() {
        return VerifyOutcome::Malformed(format!(
            "content_len {} > file len {}",
            content_len,
            bytes.len()
        ));
    }

    // content_hash 重算比对
    let recomputed = match codec.content_hash(bytes, content_len) {
        Ok(h) => h,
        Err(e) => return VerifyOutcome::Malformed(format!("content_hash: {}", e)),
    };
    if recomputed != body.content_hash {
        return VerifyOutcome::Tampered("content_hash mismatch".into());
    }

    // 验签（用 envelope 自带 pubkey）
    let cert_chain_hash: [u8; 32] = match &body.cert_chain {
        Some(c) => sha2::Sha256::digest(c).into(),
        None => [0u8; 32],
    };
    let signed_meta = envelope::build_signed_meta(
        pf.format_tag,
        body.flags,
        body.signed_at,
        &body.key_fp,
        &body.content_hash,
        &body.pubkey,
        &cert_chain_hash,
        body.key_not_after,
        body.publisher.as_deref(),
    );
    let mut msg = Vec::with_capacity(envelope::DOMAIN.len() + signed_meta.len());
    msg.extend_from_slice(envelope::DOMAIN);
    msg.extend_from_slice(&signed_meta);
    let vk = match VerifyingKey::from_bytes(&body.pubkey) {
        Ok(vk) => vk,
        Err(_) => return VerifyOutcome::Malformed("invalid pubkey bytes".into()),
    };
    if !crypto::ed25519_verify(&vk, &msg, &body.signature) {
        return VerifyOutcome::SignatureInvalid;
    }

    // 可信确认 + 有效期细分（P2a）：链 Expired → Expired；链断/不到根 → Untrusted
    let chain_ok = match &body.cert_chain {
        Some(chain_bytes) => match cert::parse_chain(chain_bytes) {
            Ok(chain) => match cert::verify_chain(&body.pubkey, &chain, root_pubs, now) {
                Ok(()) => true,
                Err(cert::ChainError::Expired) => {
                    return VerifyOutcome::Expired("certificate expired".into());
                }
                Err(_) => return VerifyOutcome::Untrusted,
            },
            Err(_) => return VerifyOutcome::Malformed("cert_chain parse failed".into()),
        },
        None => {
            if root_pubs.iter().any(|t| t.as_bytes() == &body.pubkey) {
                true
            } else {
                return VerifyOutcome::Untrusted;
            }
        }
    };
    if !chain_ok {
        return VerifyOutcome::Untrusted;
    }

    // 签名有效期 key_not_after（D4）
    if let Some(kna) = body.key_not_after {
        if now > kna {
            return VerifyOutcome::Expired(format!("key_not_after {} exceeded", kna));
        }
    }

    // 吊销检查（P2a：联网 CRL，数据模式；Unknown 按 soft-fail/strict 处置）
    if let Some(root) = root_pubs.first() {
        match crate::revocation::check_revocation(
            &body.key_fp,
            &body.sig_hash,
            &body.content_hash,
            body.publisher.as_deref(),
            root,
        ) {
            crate::revocation::RevocationResult::Revoked(entry) => {
                return VerifyOutcome::Revoked {
                    dim: entry.dim,
                    value: entry.value,
                    revoked_at: entry.revoked_at,
                    reason: entry.reason,
                };
            }
            crate::revocation::RevocationResult::NotRevoked => {} // 继续 Valid
            crate::revocation::RevocationResult::Unknown => {
                // CRL 不可达：strict 试 OCSP 单条 fallback，仍不可达才拒；soft-fail 放行
                if crate::revocation::strict_offline() {
                    if let Some(entry) = crate::revocation::ocsp_check_single(
                        &body.key_fp,
                        &body.sig_hash,
                        &body.content_hash,
                        body.publisher.as_deref(),
                        root,
                    ) {
                        return VerifyOutcome::Revoked {
                            dim: entry.dim,
                            value: entry.value,
                            revoked_at: entry.revoked_at,
                            reason: entry.reason,
                        };
                    }
                    return VerifyOutcome::Untrusted; // OCSP 也不可达 → strict 拒
                }
                // soft-fail：Unknown 放行（继续 Valid）
            }
        }
    }

    VerifyOutcome::Valid {
        signed_at: body.signed_at,
        key_fp: body.key_fp,
        pubkey: body.pubkey,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    #[test]
    fn sign_verify_raw_valid() {
        let (sk, vk) = keypair(7);
        let content = b"hello nemesis verify v3 payload";
        let signed = sign_content(content, &sk, 1000, None, None, None, None).unwrap();
        match verify_bytes(&signed, &[vk], 1000) {
            VerifyOutcome::Valid { signed_at, key_fp, pubkey } => {
                assert_eq!(signed_at, 1000);
                assert_eq!(pubkey, vk.to_bytes());
                let expected_fp: [u8; 32] = sha2::Sha256::digest(&vk.to_bytes()).into();
                assert_eq!(key_fp, expected_fp);
            }
            o => panic!("expected Valid, got {:?}", o),
        }
    }

    #[test]
    fn sign_with_chain_valid() {
        // root 签 leaf cert；leaf_sk 签 content（envelope 带 leaf pubkey + chain=[leaf_cert]）
        let (root_sk, root_vk) = keypair(1);
        let (leaf_sk, leaf_vk) = keypair(2);
        let leaf_cert = cert::issue_certificate(&root_sk, &leaf_vk.to_bytes(), b"issuer-A", 0, u64::MAX);
        let chain = cert::serialize_chain(&[leaf_cert]);
        let signed = sign_content(b"signed with cert chain", &leaf_sk, 1000, Some(&chain), None, None, None).unwrap();
        // verify: root_pubs=[root_vk]（不含 leaf_vk）；可信靠链 leaf→root
        match verify_bytes(&signed, &[root_vk], 1000) {
            VerifyOutcome::Valid { pubkey, .. } => assert_eq!(pubkey, leaf_vk.to_bytes()),
            o => panic!("expected Valid, got {:?}", o),
        }
    }

    #[test]
    fn sign_with_chain_wrong_root_untrusted() {
        // 链到 root1，但验证端只信任 root2 → Untrusted
        let (root1_sk, _) = keypair(1);
        let (_, root2_vk) = keypair(9);
        let (leaf_sk, leaf_vk) = keypair(2);
        let leaf_cert = cert::issue_certificate(&root1_sk, &leaf_vk.to_bytes(), b"issuer-A", 0, u64::MAX);
        let chain = cert::serialize_chain(&[leaf_cert]);
        let signed = sign_content(b"chain to root1", &leaf_sk, 1000, Some(&chain), None, None, None).unwrap();
        match verify_bytes(&signed, &[root2_vk], 1000) {
            VerifyOutcome::Untrusted => {}
            o => panic!("expected Untrusted, got {:?}", o),
        }
    }

    #[test]
    fn tampered_content_detected() {
        let (sk, vk) = keypair(7);
        let mut signed = sign_content(b"original content", &sk, 1000, None, None, None, None).unwrap();
        signed[5] ^= 0xFF; // 篡改 content 区
        match verify_bytes(&signed, &[vk], 1000) {
            VerifyOutcome::Tampered(_) => {}
            o => panic!("expected Tampered, got {:?}", o),
        }
    }

    #[test]
    fn no_signature() {
        let (_, vk) = keypair(7);
        let plain = b"just some bytes no signature here";
        assert_eq!(verify_bytes(plain, &[vk], 1000), VerifyOutcome::NoSignature);
    }

    #[test]
    fn untrusted_pubkey_rejected() {
        let (sk, _) = keypair(7);
        let (_, vk_other) = keypair(9);
        // 用 sk7 签（无链），只信任 vk9 → Untrusted
        let signed = sign_content(b"signed by sk7", &sk, 1000, None, None, None, None).unwrap();
        match verify_bytes(&signed, &[vk_other], 1000) {
            VerifyOutcome::Untrusted => {}
            o => panic!("expected Untrusted, got {:?}", o),
        }
    }

    #[test]
    fn signature_tamper_detected() {
        let (sk7, _) = keypair(7);
        let mut signed = sign_content(b"sig tamper test", &sk7, 1000, None, None, None, None).unwrap();
        let sig_byte_off = 15 + 108; // body 偏移 108（envelope::BODY_OFF_SIG）；content_len=15
        signed[sig_byte_off] ^= 0xFF;
        match verify_bytes(&signed, &[sk7.verifying_key()], 1000) {
            VerifyOutcome::SignatureInvalid => {}
            o => panic!("expected SignatureInvalid, got {:?}", o),
        }
    }
}
