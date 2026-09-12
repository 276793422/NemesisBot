//! 密钥体系生成与持久化（**v4：X.509 三级链**，Authenticode 对齐 D4）。
//!
//! # 层级与有效期（D4 默认跨度）
//! ```text
//! root（离线自签，30y，永不安装）→ 发行锚 CA（10y，opt-in 可安装）
//!   → leaf 代码签名证书（3y，EKU codeSigning）→ exe
//! ```
//!
//! # keys.json v2
//! hex 编码 P-256 标量 + hex 编码 X.509 证书 DER；`version: 2`。
//! **护栏 2**：该文件含全部三级私钥，永不进 git / 日志 / 报告 / 代码注释。
//!
//! # 根锚导出
//! [`KeyHierarchy::root_anchor_fingerprint`] = SHA-256(根证书 DER)——编译期锚注入
//! 与链终止条件（S4-2）的同一口径。
//!
//! # 低层积木
//! [`issue_x509`]（配 [`crate::cert::TbsInput`]）是公开的低层签发积木：测试用它
//! 组合断链 / 过期 / EKU 错 / 签名坏 / 根不匹配等失败形态证书。

use crate::cert::{self, Certificate, TbsInput};
use crate::crypto;
use crate::hex_util::{hex_decode_vec, hex_encode};
use anyhow::{Result, anyhow};
use p256::ecdsa::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// keys.json v2 版本号。
const KEYS_JSON_VERSION: u32 = 2;

/// 有效期回拨（1h，容时钟偏差）。
const NOT_BEFORE_BACKDATE_SECS: u64 = 3600;

/// D4 默认跨度：根 30y。
const SPAN_ROOT_SECS: u64 = 30 * 365 * 86400;
/// D4 默认跨度：发行锚 10y。
const SPAN_ISSUING_SECS: u64 = 10 * 365 * 86400;
/// D4 默认跨度：leaf 3y。
const SPAN_LEAF_SECS: u64 = 3 * 365 * 86400;

/// 主体 CN（根）。
pub const CN_ROOT: &str = "NemesisBot Root CA";
/// 主体 CN（发行锚）。
pub const CN_ISSUING: &str = "NemesisBot Issuing CA";
/// 主体 CN（leaf 代码签名）。
pub const CN_LEAF: &str = "NemesisBot Code Signing";
/// 组织 O。
pub const ORG: &str = "NemesisBot";

/// JSON 持久化形式（v2：hex 标量 + hex DER 证书）。
#[derive(Debug, Serialize, Deserialize)]
pub struct KeyHierarchyJson {
    pub version: u32,
    pub root_sk: String,
    pub root_cert: String,
    pub issuing_sk: String,
    pub issuing_cert: String,
    pub leaf_sk: String,
    pub leaf_cert: String,
}

/// 完整密钥体系（内存，签发端持有；三级私钥 + 三级 X.509 证书）。
pub struct KeyHierarchy {
    pub root_sk: SigningKey,
    pub root_cert: Certificate,
    pub issuing_sk: SigningKey,
    pub issuing_cert: Certificate,
    pub leaf_sk: SigningKey,
    pub leaf_cert: Certificate,
}

impl KeyHierarchy {
    /// 根公钥。
    pub fn root_vk(&self) -> VerifyingKey {
        *self.root_sk.verifying_key()
    }

    /// 发行锚公钥。
    pub fn issuing_vk(&self) -> VerifyingKey {
        *self.issuing_sk.verifying_key()
    }

    /// leaf 公钥。
    pub fn leaf_vk(&self) -> VerifyingKey {
        *self.leaf_sk.verifying_key()
    }

    /// 根锚指纹 = SHA-256(根证书 DER)（S4-2 口径；编译期注入 / 链终止同源）。
    pub fn root_anchor_fingerprint(&self) -> [u8; 32] {
        self.root_cert.sha256_fingerprint()
    }

    /// 签发链 = `[leaf, issuing, root]`（leaf 在前，**含根**——S0-4 实证：CMS 证书集
    /// 缺根时默认态报 CERT_E_CHAINING 而非干净 UNTRUSTEDROOT；嵌入 ≠ 信任）。
    pub fn chain(&self) -> Vec<Certificate> {
        vec![
            self.leaf_cert.clone(),
            self.issuing_cert.clone(),
            self.root_cert.clone(),
        ]
    }
}

/// 生成新密钥体系（D4 默认跨度：30y / 10y / 3y，起点回拨 1h）。
pub fn generate() -> Result<KeyHierarchy> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow!("系统时钟早于 UNIX_EPOCH: {e}"))?
        .as_secs();
    generate_at(now)
}

/// 指定基准时刻生成（可测性；生产入口是 [`generate`]）。
pub fn generate_at(now_unix: u64) -> Result<KeyHierarchy> {
    let nb = now_unix.saturating_sub(NOT_BEFORE_BACKDATE_SECS);

    // 根：自签，30y，CA 无 pathLen 约束，KU keyCertSign+CRLSign。
    let root_sk = crypto::signing_key_from_hex(&crypto::generate_key_pair().private_key)?;
    let root_vk = root_sk.verifying_key();
    let root_cert = issue_x509(
        root_vk,
        &root_sk,
        &cert::ski_value(root_vk)?,
        TbsInput {
            subject_cn: CN_ROOT,
            subject_org: Some(ORG),
            issuer_cn: CN_ROOT,
            issuer_org: Some(ORG),
            is_ca: true,
            path_len: None,
            ku_digital_signature: false,
            ku_key_cert_sign: true,
            ku_crl_sign: true,
            eku_code_signing: false,
            not_before_unix: nb,
            not_after_unix: nb + SPAN_ROOT_SECS,
        },
    )?;

    // 发行锚 CA：root 签发，10y，CA pathLen=0，KU digitalSignature+keyCertSign+CRLSign。
    let issuing_sk = crypto::signing_key_from_hex(&crypto::generate_key_pair().private_key)?;
    let issuing_vk = issuing_sk.verifying_key();
    let issuing_cert = issue_x509(
        issuing_vk,
        &root_sk,
        &cert::ski_value(root_vk)?,
        TbsInput {
            subject_cn: CN_ISSUING,
            subject_org: Some(ORG),
            issuer_cn: CN_ROOT,
            issuer_org: Some(ORG),
            is_ca: true,
            path_len: Some(0),
            ku_digital_signature: true,
            ku_key_cert_sign: true,
            ku_crl_sign: true,
            eku_code_signing: false,
            not_before_unix: nb,
            not_after_unix: nb + SPAN_ISSUING_SECS,
        },
    )?;

    // leaf：发行锚签发，3y，终端实体，KU digitalSignature，EKU codeSigning（D4）。
    let leaf_sk = crypto::signing_key_from_hex(&crypto::generate_key_pair().private_key)?;
    let leaf_vk = leaf_sk.verifying_key();
    let leaf_cert = issue_x509(
        leaf_vk,
        &issuing_sk,
        &cert::ski_value(issuing_vk)?,
        TbsInput {
            subject_cn: CN_LEAF,
            subject_org: Some(ORG),
            issuer_cn: CN_ISSUING,
            issuer_org: Some(ORG),
            is_ca: false,
            path_len: None,
            ku_digital_signature: true,
            ku_key_cert_sign: false,
            ku_crl_sign: false,
            eku_code_signing: true,
            not_before_unix: nb,
            not_after_unix: nb + SPAN_LEAF_SECS,
        },
    )?;

    Ok(KeyHierarchy {
        root_sk,
        root_cert,
        issuing_sk,
        issuing_cert,
        leaf_sk,
        leaf_cert,
    })
}

/// 低层签发积木：按 [`TbsInput`] profile 签一张 X.509 证书。
///
/// 自签时 `issuer_sk` = 主体私钥、`issuer_ski` = 主体自身 SKI。
/// 测试用不同 TbsInput / 错配 issuer 组合失败形态（断链 / 过期 / EKU 错 / 签名坏 / 根不匹配）。
pub fn issue_x509(
    subject_vk: &VerifyingKey,
    issuer_sk: &SigningKey,
    issuer_ski: &[u8],
    input: TbsInput<'_>,
) -> Result<Certificate, cert::ChainError> {
    let tbs = cert::build_tbs(subject_vk, issuer_ski, &cert::random_serial(), &input)?;
    cert::seal_certificate(tbs, issuer_sk)
}

impl KeyHierarchy {
    pub fn to_json(&self) -> KeyHierarchyJson {
        KeyHierarchyJson {
            version: KEYS_JSON_VERSION,
            root_sk: hex_encode(self.root_sk.to_bytes().as_ref()),
            root_cert: hex_encode(self.root_cert.to_der()),
            issuing_sk: hex_encode(self.issuing_sk.to_bytes().as_ref()),
            issuing_cert: hex_encode(self.issuing_cert.to_der()),
            leaf_sk: hex_encode(self.leaf_sk.to_bytes().as_ref()),
            leaf_cert: hex_encode(self.leaf_cert.to_der()),
        }
    }

    pub fn from_json(j: &KeyHierarchyJson) -> Result<Self> {
        if j.version != KEYS_JSON_VERSION {
            return Err(anyhow!(
                "keys.json 版本不支持: {}（期望 {KEYS_JSON_VERSION}；旧版 v3 Ed25519 文件不兼容，请重新生成密钥体系并重签）",
                j.version
            ));
        }
        let root_sk = crypto::signing_key_from_hex(&j.root_sk)?;
        let issuing_sk = crypto::signing_key_from_hex(&j.issuing_sk)?;
        let leaf_sk = crypto::signing_key_from_hex(&j.leaf_sk)?;
        let root_cert = Certificate::from_der(
            &hex_decode_vec(&j.root_cert).map_err(|e| anyhow!("root_cert: {e}"))?,
        )
        .map_err(|e| anyhow!("root_cert: {e}"))?;
        let issuing_cert = Certificate::from_der(
            &hex_decode_vec(&j.issuing_cert).map_err(|e| anyhow!("issuing_cert: {e}"))?,
        )
        .map_err(|e| anyhow!("issuing_cert: {e}"))?;
        let leaf_cert = Certificate::from_der(
            &hex_decode_vec(&j.leaf_cert).map_err(|e| anyhow!("leaf_cert: {e}"))?,
        )
        .map_err(|e| anyhow!("leaf_cert: {e}"))?;
        Ok(KeyHierarchy {
            root_sk,
            root_cert,
            issuing_sk,
            issuing_cert,
            leaf_sk,
            leaf_cert,
        })
    }

    pub fn save(&self, path: &str) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.to_json())?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        let j: KeyHierarchyJson = serde_json::from_slice(&data)?;
        Self::from_json(&j)
    }
}

#[cfg(test)]
mod tests;
