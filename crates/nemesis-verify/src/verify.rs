//! 验证流程（S4-1：v4 Authenticode 管线）。
//!
//! 管线序（goal S4-1）：主签名定位（PE 证书表首条目 / ELF+raw v4 footer 载体）
//! → CMS 解析 → 链到编译期根锚 + EKU → digest 比对 → 签名 → 吊销。
//!
//! # 九态 VerifyOutcome 映射
//! - 结构坏（DER / 证书表 / 载体契约 / sid 引用证书缺席）→ [`VerifyOutcome::Malformed`]
//! - 非支持版本/算法（SignedData version ≠ V1、非 SHA-256、非 ECDSA 套件）→
//!   [`VerifyOutcome::UnsupportedVersion`]
//! - 链断 / 根指纹不匹配 / 缺 codeSigning EKU → [`VerifyOutcome::Untrusted`]
//! - 摘要差（重算 vs SignedData messageDigest）→ [`VerifyOutcome::Tampered`]
//! - 签名自洽验签失败 → [`VerifyOutcome::SignatureInvalid`]
//! - 有效期过（链内任一证书）→ [`VerifyOutcome::Expired`]
//! - 吊销四维命中 → [`VerifyOutcome::Revoked`]
//! - 无主签名 → [`VerifyOutcome::NoSignature`]；全过 → [`VerifyOutcome::Valid`]
//!
//! # 信任语义（S4-2 口径，D7）
//! 链终止条件 = 自签根证书 SHA-256 指纹 ∈ `root_anchor_fps`。默认态空锚集 →
//! Untrusted（= Windows CERT_E_UNTRUSTEDROOT 语义）；opt-in 信任 = 用户显式
//! 安装自签根（S0-5 实测：发行锚不可作信任锚）。
//!
//! # 范围（诚实边界）
//! 只验证**主签名**（PE = 证书表首条目；载体 = footer CMS 本体）。嵌套签名
//! （SPC_NESTED_SIGNATURE，S3-3）展开枚举归 view/后续策略，不影响主签名验证。
//!
//! # 破坏性（S4-1）
//! v3 NMBSIG envelope 不再被 [`verify_bytes`] 消费（v3 签名文件 → NoSignature）。
//! 签发单一入口 = [`sign_content_v4`]（S5-1）；v3 签名器 [`sign_content` 已删除，
//! S5-3 随最后一个生产消费方（verify-loader sign）迁移] 与 envelope v3 代码块
//! 一并退役——v3 形态只剩测试里的 footer 字节字面量（破坏性钉死用，故意的）。

use crate::cert::{self, Certificate};
use crate::codec;
use crate::crypto;
use crate::envelope;
use anyhow::Result;
use der::Encode;
use p256::ecdsa::SigningKey;
use sha2::Digest;

/// 验证结果。
#[derive(Debug, PartialEq)]
pub enum VerifyOutcome {
    /// 签名有效、链到信任锚（根指纹 ∈ root_anchor_fps）、EKU 合规、未吊销、未过期。
    Valid {
        /// signingTime 属性（Unix epoch 秒；属性缺席 = 0——Authenticode 允许缺席）。
        signed_at: u64,
        /// 签名者公钥指纹 = SHA-256(SEC1 uncompressed 65B)（吊销 KeyFp 维度同源）。
        key_fp: [u8; 32],
        /// 签名者公钥（SEC1 uncompressed 65B，来自证书集内签名者证书 SPKI）。
        pubkey: [u8; 65],
    },
    /// 无主签名（PE 无证书表 / 载体无 v4 footer）。
    NoSignature,
    /// 内容被篡改（重算 digest ≠ SignedData 内嵌 messageDigest）。
    Tampered(String),
    /// 签名自洽验签失败（signature 对签名者证书公钥 + signedAttrs 不成立）。
    SignatureInvalid,
    /// 不可信：链断 / 根指纹不在锚集 / 缺 codeSigning EKU / 吊销不可达且 strict。
    Untrusted,
    /// 吊销（CRL 四维度之一命中：key_fp / sig_hash / content_hash / publisher）。
    Revoked {
        dim: crate::RevDim,
        value: String,
        revoked_at: u64,
        reason: String,
    },
    /// 过期（链内任一证书不在当前时间有效）。
    Expired(String),
    /// 结构合法但本实现不支持的版本/算法（SignedData version ≠ V1、非 SHA-256、
    /// 非 ECDSA 套件）。
    UnsupportedVersion(String),
    /// 结构无法解析（DER / 证书表 / 载体 / sid 引用证书缺席）。
    Malformed(String),
}

/// v4 载体内容摘要（**单一真相源**）：CMS `messageDigest` 的取值来源，
/// [`sign_content_v4`]（签发）与 registry 记账（revoke-server FileHash 吊销维度）
/// 必须同源消费——verify_bytes 吊销比对传的是 CMS 内嵌 `content_digest`，
/// 记账侧存别的口径（如 codec 文件哈希）会让 FileHash 吊销永远查不中。
///
/// - PE → [`crate::pe::authenticode_digest`]（排除 CheckSum 字段与证书表区，
///   无 L 上界 overlay 全含）
/// - ELF/raw → codec `content_hash`（ELF 保护域 [0,L)、raw 全文件）
pub fn v4_content_digest(content: &[u8]) -> Result<[u8; 32]> {
    if codec::detect_format(content) == codec::FORMAT_TAG_PE {
        crate::pe::authenticode_digest(content).map_err(anyhow::Error::from)
    } else {
        let c = codec::detect_codec(content);
        let content_len = c.compute_l(content).map_err(anyhow::Error::from)?;
        c.content_hash(content, content_len.unwrap_or(content.len()))
            .map_err(anyhow::Error::from)
    }
}

/// v4 Authenticode 签名装配单一入口（S5-1）：按载体形态自动分派，与
/// [`verify_bytes`] 的主签名定位同源。
///
/// - PE → CMS（messageDigest = [`v4_content_digest`]）+ Certificate Table 追加
///   （只写排除区，digest 前后不变）
/// - ELF/raw → CMS + v4 footer 载体（[`envelope::sign_carrier_v4`]）
///
/// `certs[0]` = 签名者证书（[`envelope::build_signed_data`] 契约）；`publisher`
/// = opus programName（可选，view 展示穿透）。v3 [`sign_content`] 的替代。
pub fn sign_content_v4(
    content: &[u8],
    sk: &SigningKey,
    signed_at: u64,
    certs: &[Certificate],
    publisher: Option<&str>,
) -> Result<Vec<u8>> {
    let is_pe = codec::detect_format(content) == codec::FORMAT_TAG_PE;
    let digest = v4_content_digest(content)?;
    let cms = envelope::build_signed_data(&digest, sk, signed_at, certs, publisher, None)?;
    if is_pe {
        // append_certificate_table 返回 CodecError（pe 模块契约），统一到 anyhow
        crate::pe::append_certificate_table(content, &cms).map_err(anyhow::Error::from)
    } else {
        envelope::sign_carrier_v4(content, &cms)
    }
}

// ===== S4-1：v4 Authenticode 验证管线 =====

/// 主签名来源（决定 digest 重算口径）。
enum PrimarySource {
    /// PE：证书表首条目（Windows primary），digest = [`crate::pe::authenticode_digest`]。
    CertificateTable { cms_der: Vec<u8> },
    /// ELF/raw：v4 footer 载体，digest = 载体 content_hash（extract 已交叉核 L 契约）。
    Carrier {
        cms_der: Vec<u8>,
        content_digest: [u8; 32],
    },
}

/// 定位主签名（诚实失败按九态直接返回）。
fn locate_primary(bytes: &[u8]) -> Result<PrimarySource, VerifyOutcome> {
    if codec::detect_format(bytes) == codec::FORMAT_TAG_PE {
        let entries = crate::pe::read_certificate_entries(bytes)
            .map_err(|e| VerifyOutcome::Malformed(format!("证书表: {e}")))?;
        let first = match entries.first() {
            Some(f) => f,
            None => return Err(VerifyOutcome::NoSignature),
        };
        if first.cert_type != crate::pe::WIN_CERT_TYPE_PKCS_SIGNED_DATA {
            return Err(VerifyOutcome::Malformed(format!(
                "主条目类型 {:#06x} 非 PKCS_SIGNED_DATA",
                first.cert_type
            )));
        }
        return Ok(PrimarySource::CertificateTable {
            cms_der: first.certificate.clone(),
        });
    }
    match envelope::extract_carrier_v4(bytes) {
        Ok(v) => Ok(PrimarySource::Carrier {
            cms_der: v.cms_der,
            content_digest: v.content_digest,
        }),
        Err(e) if e.to_string().contains("无 v4 footer") => Err(VerifyOutcome::NoSignature),
        Err(e) => Err(VerifyOutcome::Malformed(format!("载体: {e}"))),
    }
}

/// parse_signed_data 的错误分类（结构坏 vs 非支持版本——envelope 层用消息前缀
/// 区分两类诚实失败，crate 内部契约）。
fn classify_parse_err(e: anyhow::Error) -> VerifyOutcome {
    let s = e.to_string();
    if s.starts_with("unsupported SignedData") {
        VerifyOutcome::UnsupportedVersion(s)
    } else {
        VerifyOutcome::Malformed(s)
    }
}

/// sid（IssuerAndSerialNumber）↔ 证书集成员匹配：序列号字节相等 + issuer Name
/// DER 相等（parse 期双方都按 DER 规范形编码，字节可比）。
/// pub(crate)：view 签名者定位同判据复用（纯结构匹配，无信任判断）。
pub(crate) fn signer_matches(cert: &Certificate, ps: &envelope::ParsedSignature) -> bool {
    let Ok(x) = cert.parsed() else {
        return false;
    };
    if x.tbs_certificate.serial_number.as_bytes() != ps.signer_serial.as_slice() {
        return false;
    }
    match x.tbs_certificate.issuer.to_der() {
        Ok(d) => d == ps.signer_issuer_der,
        Err(_) => false,
    }
}

/// 证书集 → 有序链 `[signer, issuing, ..., root]`。
///
/// 证书集是 SetOfVec（按编码字节序，**非链序**，S3-3 实证），须按 AKI→SKI
/// 逐级上溯排序。终止 = 当前证书自签。诚实失败（None → Untrusted）：
/// AKI 找不到对应 SKI 的父证书（链断）/ 环 / 非自签却无 AKI 可循。
/// pub(crate)：view 展示排序同源复用（纯结构排序，链断时 view 降级原序展示）。
pub(crate) fn order_chain_from(
    signer: &Certificate,
    pool: &[Certificate],
) -> Option<Vec<Certificate>> {
    let mut chain = vec![signer.clone()];
    loop {
        let cur = chain.last().expect("chain 非空");
        if cur.is_self_signed().ok()? {
            return Some(chain);
        }
        let aki = cur.aki().ok().flatten()?;
        let parent = pool
            .iter()
            .find(|c| c.ski().ok().flatten().as_deref() == Some(aki.as_slice()))?;
        if chain.iter().any(|c| c == parent) {
            return None; // 环
        }
        chain.push(parent.clone());
    }
}

/// 验证 `bytes`（完整文件，主签名）。
///
/// `root_anchor_fps`：验证端信任锚集合（自签根证书 SHA-256 指纹；编译期固化 +
/// 运行时 fallback 的注入装配归 S4-2/c_abi）。空集 = 默认不可信状态（D7）。
/// `now`：当前时间（Unix epoch 秒），用于证书有效期校验。
pub fn verify_bytes(bytes: &[u8], root_anchor_fps: &[[u8; 32]], now: u64) -> VerifyOutcome {
    // ① 主签名定位
    let primary = match locate_primary(bytes) {
        Ok(p) => p,
        Err(o) => return o,
    };

    // ② CMS 解析（结构 + 自洽校验在 envelope 层完成）
    let ps = match envelope::parse_signed_data(match &primary {
        PrimarySource::CertificateTable { cms_der } => cms_der,
        PrimarySource::Carrier { cms_der, .. } => cms_der,
    }) {
        Ok(ps) => ps,
        Err(e) => return classify_parse_err(e),
    };

    // ③ 链排序 + 链到根验证（含 EKU）；根指纹 ∈ 锚集（信任终止）
    let signer = match ps.certs.iter().find(|c| signer_matches(c, &ps)) {
        Some(s) => s.clone(),
        None => {
            return VerifyOutcome::Malformed("sid 引用的签名者证书不在证书集".into());
        }
    };
    let chain = match order_chain_from(&signer, &ps.certs) {
        Some(c) => c,
        None => return VerifyOutcome::Untrusted,
    };
    let root = chain.last().expect("chain 非空").clone();
    let root_fp = root.sha256_fingerprint();
    match cert::verify_chain(&chain, &root_fp, now) {
        Ok(()) => {}
        Err(cert::ChainError::Expired) => {
            return VerifyOutcome::Expired("certificate expired".into());
        }
        Err(cert::ChainError::MissingCodeSigningEku) => return VerifyOutcome::Untrusted,
        Err(
            e @ (cert::ChainError::Malformed(_)
            | cert::ChainError::InvalidKey
            | cert::ChainError::UnsupportedAlgorithm),
        ) => return VerifyOutcome::Malformed(format!("证书链: {e}")),
        Err(_) => return VerifyOutcome::Untrusted,
    }
    if !root_anchor_fps.contains(&root_fp) {
        return VerifyOutcome::Untrusted; // 链没到编译期根锚（D7 默认态）
    }

    // ④ digest 比对（摘要差 → Tampered；PE 无 L 上界 overlay 全含，载体域 = codec 裁决）
    let recomputed = match &primary {
        PrimarySource::CertificateTable { .. } => match crate::pe::authenticode_digest(bytes) {
            Ok(d) => d,
            Err(e) => return VerifyOutcome::Malformed(format!("authenticode digest: {e}")),
        },
        PrimarySource::Carrier { content_digest, .. } => *content_digest,
    };
    if recomputed != ps.content_digest {
        return VerifyOutcome::Tampered("content digest mismatch".into());
    }

    // ⑤ 签名验签：消息 = signedAttrs SET OF DER（RFC 5652 §5.4），密钥 = 签名者证书 SPKI
    let signer_vk = match signer.subject_public_key() {
        Ok(vk) => vk,
        Err(e) => return VerifyOutcome::Malformed(format!("签名者公钥: {e}")),
    };
    let der_sig = match <ecdsa::der::Signature<p256::NistP256>>::try_from(ps.signature.as_slice()) {
        Ok(s) => s,
        Err(_) => return VerifyOutcome::Malformed("signature 非 DER ECDSA 形态".into()),
    };
    use ecdsa::signature::Verifier;
    if signer_vk.verify(&ps.signature_message, &der_sig).is_err() {
        return VerifyOutcome::SignatureInvalid;
    }

    // ⑥ 吊销（四维度；soft-fail 默认，Unknown → strict 拒 / OCSP fallback）
    let signer_pubkey = crypto::public_key_bytes(&signer_vk);
    let key_fp = crypto::key_fp(&signer_pubkey);
    let sig_hash: [u8; 32] = sha2::Sha256::digest(&ps.signature).into();
    let publisher = signer.subject_cn().ok().flatten();
    let root_vk = match root.subject_public_key() {
        Ok(vk) => vk,
        Err(_) => return VerifyOutcome::Untrusted,
    };
    match crate::revocation::check_revocation(
        &key_fp,
        &sig_hash,
        &ps.content_digest,
        publisher.as_deref(),
        &root_vk,
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
                    &key_fp,
                    &sig_hash,
                    &ps.content_digest,
                    publisher.as_deref(),
                    &root_vk,
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

    VerifyOutcome::Valid {
        signed_at: ps.signing_time.unwrap_or(0),
        key_fp,
        pubkey: signer_pubkey,
    }
}

#[cfg(test)]
mod tests;
