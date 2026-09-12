//! 查看接口（离线展示签名 + 证书链，**不下结论**）。
//!
//! 对应"查看 vs 验证分离"的查看侧（Windows 文件属性→数字签名 tab 的"查看证书"）：
//! 纯本地解析，不联网、不查吊销、不下 Valid/Revoked 结论——只展示签名元数据 + 证书链。
//!
//! v4 Authenticode（S4-3）：签名枚举 = PE（Certificate Table 主签名 +
//! SPC_NESTED_SIGNATURE 嵌套树展开）+ ELF/raw（v4 footer 载体单个）。证书链展示
//! 顺序经 AKI→SKI 结构排序（leaf→root，复用 verify 的结构排序；链断降级证书集
//! 原序展示）。解析失败的个体签名诚实跳过；无签名 / 载体坏 = 空视图。

use crate::{cert, codec, crypto, envelope, verify};
use sha2::{Digest, Sha256};

/// 单签名摘要（列表项）。
#[derive(Debug)]
pub struct SigInfo {
    /// 索引（PE：0 = 主签名，其后为嵌套展开序；ELF/raw：恒 0）。
    pub index: usize,
    /// signingTime 属性（epoch 秒；属性缺席 = 0，Authenticode 允许缺席）。
    pub signed_at: u64,
    /// 签名者公钥指纹 = SHA-256(SEC1 uncompressed 65B)。
    pub key_fp: [u8; 32],
    /// 签名者公钥（SEC1 uncompressed 65B，取自证书集内签名者证书 SPKI）。
    pub pubkey: [u8; 65],
}

/// 单张证书的展示信息（v4 X.509；离线提取，不下结论）。
#[derive(Debug)]
pub struct CertInfo {
    /// 主体公钥（SEC1 uncompressed 65B）。
    pub subject_pubkey: [u8; 65],
    /// 签发者 key_fp（= AKI keyIdentifier）。缺 AKI 时 None。
    pub issuer_key_fp: Option<[u8; 32]>,
    /// 有效期起（unix 秒）。
    pub valid_not_before: u64,
    /// 有效期止（unix 秒）。
    pub valid_not_after: u64,
    /// 主体 CN（subject DN 的 CN ATV；展示用 best-effort）。
    pub subject_meta: Option<String>,
}

/// 单签名详情（含证书链逐级）。
#[derive(Debug)]
pub struct SigDetail {
    pub info: SigInfo,
    /// 证书链（X.509，leaf 在前含根；AKI→SKI 结构排序，链断降级证书集原序）。
    pub certs: Vec<CertInfo>,
    /// opus programName（CMS SPC_SP_OPUS_INFO；展示用，缺席 = None）。
    pub publisher: Option<String>,
}

/// 列所有签名（PE：主签名 + 嵌套展开；ELF/raw：载体单个）。
pub fn list_signatures(bytes: &[u8]) -> Vec<SigInfo> {
    parse_all(bytes)
        .iter()
        .enumerate()
        .filter_map(|(i, ps)| sig_info_from(ps, i))
        .collect()
}

/// 单签名详情（含 cert chain）。
pub fn get_signature_detail(bytes: &[u8], index: usize) -> Option<SigDetail> {
    let parsed = parse_all(bytes);
    let ps = parsed.get(index)?;
    let signer = ps.certs.iter().find(|c| verify::signer_matches(c, ps))?;
    // 展示顺序 leaf→root（AKI→SKI 结构排序；链断 = 证书集原序照常展示）
    let chain = verify::order_chain_from(signer, &ps.certs).unwrap_or_else(|| ps.certs.clone());
    Some(SigDetail {
        info: sig_info_from(ps, index)?,
        certs: chain.iter().filter_map(cert_info).collect(),
        publisher: ps.program_name.clone(),
    })
}

/// 提取首个（主）签名的 sig_hash（= SHA-256(SignerInfo.signature DER)）。
/// 供 revoke-server registry 登记 / CRL 单签名吊销维度。
pub fn latest_sig_hash(bytes: &[u8]) -> Option<[u8; 32]> {
    let ps = parse_all(bytes).into_iter().next()?;
    Some(Sha256::digest(&ps.signature).into())
}

/// 文件内全部可解析签名（视图序）：PE = 主签名 + 嵌套树展开；ELF/raw = 载体单个。
/// 结构坏 / 无签名 → 空视图（查看接口不下结论，也不因个别坏块崩溃）。
fn parse_all(bytes: &[u8]) -> Vec<envelope::ParsedSignature> {
    collect_cms_blobs(bytes)
        .iter()
        .filter_map(|cms| envelope::parse_signed_data(cms).ok())
        .collect()
}

/// 提取文件内全部 CMS DER（视图序；解析失败的个体在 parse_all 被跳过）。
fn collect_cms_blobs(bytes: &[u8]) -> Vec<Vec<u8>> {
    if codec::detect_format(bytes) == codec::FORMAT_TAG_PE {
        // 主签名 = 证书表首条目（与 verify 定位同判据）；嵌套从主 ContentInfo 树展开
        let Ok(entries) = crate::pe::read_certificate_entries(bytes) else {
            return Vec::new();
        };
        let Some(primary) = entries.first() else {
            return Vec::new();
        };
        if primary.cert_type != crate::pe::WIN_CERT_TYPE_PKCS_SIGNED_DATA {
            return Vec::new();
        }
        return envelope::enumerate_signatures(&primary.certificate).unwrap_or_default();
    }
    // ELF/raw：v4 footer 载体（extract 自带 footer CRC / 区间防钳 / 载体契约交叉校验）
    match envelope::extract_carrier_v4(bytes) {
        Ok(v) => vec![v.cms_der],
        Err(_) => Vec::new(),
    }
}

/// ParsedSignature → SigInfo（签名者定位 = sid ↔ 证书集成员，与 verify 同判据；
/// 纯结构匹配，无信任判断）。
fn sig_info_from(ps: &envelope::ParsedSignature, index: usize) -> Option<SigInfo> {
    let signer = ps.certs.iter().find(|c| verify::signer_matches(c, ps))?;
    let pubkey = crypto::public_key_bytes(&signer.subject_public_key().ok()?);
    Some(SigInfo {
        index,
        signed_at: ps.signing_time.unwrap_or(0),
        key_fp: crypto::key_fp(&pubkey),
        pubkey,
    })
}

/// 单张证书 → 展示信息（解析失败的字段降级为 None/0，查看接口不因单字段坏而拒整张）。
fn cert_info(c: &cert::Certificate) -> Option<CertInfo> {
    let parsed = c.parsed().ok()?;
    let subject_pubkey = c
        .subject_public_key()
        .map(|vk| crypto::public_key_bytes(&vk))
        .ok()?;
    let issuer_key_fp = c
        .aki()
        .ok()
        .flatten()
        .and_then(|k| <[u8; 32]>::try_from(k).ok());
    let validity = &parsed.tbs_certificate.validity;
    Some(CertInfo {
        subject_pubkey,
        issuer_key_fp,
        valid_not_before: validity.not_before.to_unix_duration().as_secs(),
        valid_not_after: validity.not_after.to_unix_duration().as_secs(),
        subject_meta: c.subject_cn().ok().flatten(),
    })
}

#[cfg(test)]
mod tests;
