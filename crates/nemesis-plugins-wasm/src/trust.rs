//! 插件 manifest 签名校验 + 信任四态。
//!
//! 签名格式：manifest 文本剥根级 `signature` 表后的**规范字节**（toml_edit
//! 只剥不重排，作者字节顺序保留）做 Ed25519。`signature` 表四键：
//! `algorithm = "ed25519"` / `public-key`（hex64）/ `signature`（hex128）/
//! `signed-at`（RFC3339，展示用不参与校验）。
//!
//! 信任锚：TrustStore（`<workspace>/config/plugin_trust.json`，复用
//! nemesis-security TrustStore 文件格式，与 skills 信任库分文件管理）。

use crate::error::PluginError;
use crate::manifest::PluginManifest;

/// 插件信任四态（与 skills 信任四态同语义、独立类型——不跨域耦合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginTrustState {
    /// 签名有效 + Verified 级密钥。
    Trusted,
    /// 签名有效 + Community 级密钥。
    ReviewRecommended,
    /// 无签名且 `allow_unsigned` 放行。
    ReviewRequired,
    /// 篡改 / 吊销 / 不受信 / strict 模式无签名。
    Blocked,
}

impl PluginTrustState {
    /// kebab-case 字符串（lockfile / 审批卡展示）。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::ReviewRecommended => "review-recommended",
            Self::ReviewRequired => "review-required",
            Self::Blocked => "blocked",
        }
    }

    /// blocked 之外允许继续装配（后续环节仍可拦截）。
    #[must_use]
    pub fn installable(&self) -> bool {
        !matches!(self, Self::Blocked)
    }
}

/// 验签结论（install 漏斗第②步产物）。
#[derive(Debug, Clone)]
pub struct VerificationOutcome {
    /// manifest 是否带 signature 表。
    pub signed: bool,
    /// 签名验证结论（未签名 = None）。
    pub valid: Option<bool>,
    /// TrustStore 密钥级别（verified/community/unknown）。
    pub trust_level: Option<String>,
    /// 签名者公钥 hex（截断展示由调用方决定）。
    pub public_key: String,
    /// 失败细节。
    pub error: String,
}

impl VerificationOutcome {
    /// 归约四态（与 skills `trust_state` 同语义）。
    #[must_use]
    pub fn trust_state(&self, allow_unsigned: bool) -> PluginTrustState {
        if !self.signed {
            return if allow_unsigned {
                PluginTrustState::ReviewRequired
            } else {
                PluginTrustState::Blocked
            };
        }
        let valid = self.valid.unwrap_or(false);
        if valid {
            match self.trust_level.as_deref() {
                Some("verified") => PluginTrustState::Trusted,
                Some("community") => PluginTrustState::ReviewRecommended,
                _ => PluginTrustState::ReviewRecommended,
            }
        } else {
            PluginTrustState::Blocked
        }
    }
}

/// manifest 根级 signature 表（验签器视角）。
///
/// `signed-at` 为展示用字段不参与校验，结构体不声明（serde 默认忽略未知键）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct SignatureBlock {
    algorithm: String,
    public_key: String,
    signature: String,
}

/// 全文包装（只关心 signature 表；其余字段由 canonical 文本二次解析）。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawWithSignature {
    #[serde(default)]
    signature: Option<SignatureBlock>,
}

/// 从 manifest 原文剥离 `signature` 表并返回（规范字节, 签名块）。
///
/// 用 toml_edit 只移除根级 `signature` 键——不重排其余字节，作者序列化
/// 形态即规范形态。manifest 结构解析应基于返回的 canonical 文本
/// （`PluginManifest::parse(canonical)`；根级 signature 键不进 manifest
/// 结构校验）。
pub(crate) fn strip_signature(raw: &str) -> (String, Option<SignatureBlock>) {
    let block = toml_edit::de::from_str::<RawWithSignature>(raw)
        .ok()
        .and_then(|w| w.signature);
    let mut doc = match raw.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(_) => return (raw.to_string(), block),
    };
    doc.remove("signature");
    (doc.to_string(), block)
}

/// 验证 manifest 签名（第②步；TrustStore 决定信任级别）。
pub fn verify_manifest(
    raw: &str,
    trust: &nemesis_security::signature::SignatureVerifier,
) -> Result<(PluginManifest, VerificationOutcome), PluginError> {
    let (canonical, block) = strip_signature(raw);
    let manifest = PluginManifest::parse(&canonical)?.0;
    let outcome = match block {
        None => VerificationOutcome {
            signed: false,
            valid: None,
            trust_level: None,
            public_key: String::new(),
            error: String::new(),
        },
        Some(sig) => {
            if sig.algorithm != "ed25519" {
                VerificationOutcome {
                    signed: true,
                    valid: Some(false),
                    trust_level: None,
                    public_key: sig.public_key,
                    error: format!("unsupported algorithm: {}", sig.algorithm),
                }
            } else {
                let (level, is_trusted) = trust.trust_store_ref().is_trusted(&sig.public_key);
                let valid = is_trusted
                    && trust.verify_signature_bytes(
                        canonical.as_bytes(),
                        &sig.signature,
                        &sig.public_key,
                    );
                VerificationOutcome {
                    signed: true,
                    valid: Some(valid),
                    trust_level: Some(
                        match level {
                            nemesis_security::signature::TrustLevel::Verified => "verified",
                            nemesis_security::signature::TrustLevel::Community => "community",
                            _ => "unknown",
                        }
                        .to_string(),
                    ),
                    public_key: sig.public_key,
                    error: if valid {
                        String::new()
                    } else if !is_trusted {
                        "signer key not in trust store (or revoked)".into()
                    } else {
                        "signature verification failed (tampered manifest?)".into()
                    },
                }
            }
        }
    };
    Ok((manifest, outcome))
}

/// `allow_unsigned` 对 Blocked-by-unsigned 的单点豁免（与 skills 纪律一致：
/// 只豁免「无签名」，不豁免「签名无效」）。
pub fn trust_state_for(
    outcome: &VerificationOutcome,
    allow_unsigned: bool,
) -> Result<PluginTrustState, PluginError> {
    let state = outcome.trust_state(allow_unsigned);
    if state == PluginTrustState::Blocked {
        return Err(PluginError::Trust(if outcome.signed {
            outcome.error.clone()
        } else {
            "unsigned manifest and allow_unsigned=false (strict)".into()
        }));
    }
    Ok(state)
}
