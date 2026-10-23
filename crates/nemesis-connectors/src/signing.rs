//! connector.toml detached Ed25519 签名（D6.1，2026-10-15）。
//!
//! 签名载体 = detached 签名文件 `connector.toml.sig`（toml 保持人手可编），
//! JSON 形态 `{algorithm, public_key, signature}`，载荷 = connector.toml
//! 原始文本字节。信任库与 skills 共用（`<workspace>/config/skill_trust.json`
//! ——同一把公钥 trust 一次，装 skills 与 connectors 双侧生效）。
//!
//! 语义（对齐 plugin/skills 安装门四态，归约到收录行为）：
//! - 无 sig → [`ConnectorTrust::Unsigned`]（存量兼容，照常收录）；
//! - 密码学验签失败（篡改/调换/坏 key）→ `Err`（registry 拒收，corrupt 形态
//!   诚实入列）；
//! - 验签过但密钥已吊销 → `Err`（吊销后不可信）；
//! - 验签过 + verified → [`ConnectorTrust::Verified`] 徽标；
//! - 验签过 + community / 不在库 → [`ConnectorTrust::Community`]（收录但
//!   list 标 WARN）。

use std::path::Path;

/// detached 签名文件名（与 connector.toml 同目录）。
pub const SIG_FILE: &str = "connector.toml.sig";

// 信任库类型 re-export：CLI/WSAPI 消费方经 `nemesis_connectors::signing::`
// 取用即可，无需直接依赖 nemesis-security。
#[cfg(feature = "security")]
pub use nemesis_security::signature::{TrustLevel, TrustStore, TrustedKey};

/// 生成 Ed25519 密钥对（CLI `connector keygen`；hex64 私钥/公钥）。
#[cfg(feature = "security")]
pub fn generate_key_pair() -> Result<nemesis_security::signature::KeyPair, String> {
    nemesis_security::signature::generate_key_pair()
}

/// 收录级信任徽标（验签过之后的三态；验签失败不产生徽标而是拒收）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectorTrust {
    /// 验签过 + verified 级密钥。
    Verified,
    /// 验签过 + community 级或不在信任库（收录但标 WARN）。
    Community,
    /// 未签名（存量兼容形态）。
    #[default]
    Unsigned,
}

impl ConnectorTrust {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Community => "community",
            Self::Unsigned => "unsigned",
        }
    }
}

/// 验签评估结果（签名文件在场时的结论与证据）。
#[derive(Debug, Clone)]
pub struct SignatureAssessment {
    pub trust: ConnectorTrust,
    /// 签名者公钥（hex64，未签名时空串）。
    pub public_key: String,
}

/// detached 签名信封（sig 文件 JSON 形态）。
#[derive(serde::Serialize, serde::Deserialize)]
struct SigEnvelope {
    algorithm: String,
    public_key: String,
    signature: String,
}

/// 评估 connector 目录的签名状态。
///
/// - 无 sig 文件 → `Ok(Unsigned)`；
/// - `security` feature 关闭时在场 sig 不验（安全面未编译），诚实按 Unsigned
///   放行并 warn——与 `security.enabled = false` 同语义；
/// - 验签失败 / 密钥吊销 → `Err`（registry 拒收）。
#[cfg(feature = "security")]
pub fn assess_connector_signature(
    dir: &Path,
    trust_store: &nemesis_security::signature::TrustStore,
) -> Result<SignatureAssessment, String> {
    let sig_path = dir.join(SIG_FILE);
    if !sig_path.exists() {
        return Ok(SignatureAssessment {
            trust: ConnectorTrust::Unsigned,
            public_key: String::new(),
        });
    }
    let manifest_text =
        std::fs::read_to_string(dir.join(crate::registry::MANIFEST_FILE)).map_err(|e| {
            format!(
                "读取 {} 失败: {e}",
                dir.join(crate::registry::MANIFEST_FILE).display()
            )
        })?;
    assess_signature_text(&sig_path, &manifest_text, trust_store)
}

/// 非 security 构建的诚实退化：sig 在场也不验（安全面未编译）。
#[cfg(not(feature = "security"))]
pub fn assess_connector_signature(dir: &Path) -> Result<SignatureAssessment, String> {
    assess_signature_text(&dir.join(SIG_FILE), "")
}

/// 非 security 构建的文本直验退化：与 security 版同签名（调用方 cfg 无关
/// 地单读），sig 在场也不验（安全面未编译）。
#[cfg(not(feature = "security"))]
pub fn assess_signature_text(
    sig_path: &Path,
    _manifest_text: &str,
) -> Result<SignatureAssessment, String> {
    if sig_path.exists() {
        tracing::warn!(
            "[connector.sign] security feature 未编译，{} 的签名不验（按未签名放行）",
            sig_path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
    }
    Ok(SignatureAssessment {
        trust: ConnectorTrust::Unsigned,
        public_key: String::new(),
    })
}

/// 清单文本直验（scan/find 单读路径）：registry 持有的解析用清单字节**原样**
/// 传入验签——解析与验签必须同一份文本，双读窗口里把清单换回去会让
/// 「验签通过的字节」与「实际解析执行的字节」错位（require_signed 可被
/// 双读绕过：恶意清单被解析、原清单被验证）。
#[cfg(feature = "security")]
pub fn assess_signature_text(
    sig_path: &Path,
    manifest_text: &str,
    trust_store: &nemesis_security::signature::TrustStore,
) -> Result<SignatureAssessment, String> {
    if !sig_path.exists() {
        return Ok(SignatureAssessment {
            trust: ConnectorTrust::Unsigned,
            public_key: String::new(),
        });
    }
    assess_signed_text(sig_path, manifest_text, trust_store)
}

/// 验签核心：sig 信封 + 调用方持有的清单文本 → 密码学验签 → 信任库归约。
#[cfg(feature = "security")]
fn assess_signed_text(
    sig_path: &Path,
    manifest_text: &str,
    trust_store: &nemesis_security::signature::TrustStore,
) -> Result<SignatureAssessment, String> {
    let sig_text = std::fs::read_to_string(sig_path)
        .map_err(|e| format!("读取 {} 失败: {e}", sig_path.display()))?;
    let envelope: SigEnvelope = serde_json::from_str(&sig_text).map_err(|e| {
        format!("签名文件格式非法（应为 {{algorithm, public_key, signature}} JSON）: {e}")
    })?;
    if envelope.algorithm != "ed25519" {
        return Err(format!(
            "未知签名算法 `{}`（本实现只认 ed25519）",
            envelope.algorithm
        ));
    }
    if !nemesis_security::signature::verify_signature_ed25519(
        manifest_text.as_bytes(),
        &envelope.signature,
        &envelope.public_key,
    ) {
        return Err(format!(
            "签名验证失败：{} 与 {} 不匹配（清单被篡改或签名文件调换）",
            crate::registry::MANIFEST_FILE,
            SIG_FILE
        ));
    }
    let (level, _trusted) = trust_store.is_trusted(&envelope.public_key);
    match level {
        nemesis_security::signature::TrustLevel::Revoked => {
            Err("签名密钥已吊销（revoked）——不可信".to_string())
        }
        nemesis_security::signature::TrustLevel::Verified => Ok(SignatureAssessment {
            trust: ConnectorTrust::Verified,
            public_key: envelope.public_key,
        }),
        // community 与不在库（Unknown）同归 Community：收录但 list 标 WARN。
        _ => Ok(SignatureAssessment {
            trust: ConnectorTrust::Community,
            public_key: envelope.public_key,
        }),
    }
}

/// workspace 级共享信任库（与 skills 同一文件）。
#[cfg(feature = "security")]
pub fn workspace_trust_store(workspace: &Path) -> nemesis_security::signature::TrustStore {
    nemesis_security::signature::TrustStore::new(Some(
        workspace.join("config").join("skill_trust.json"),
    ))
}

/// 对 connector 目录签发 detached 签名（CLI `connector sign`）。
///
/// `private_key_hex` = hex64 Ed25519 私钥（32 字节 seed）。覆盖写已有 sig。
#[cfg(feature = "security")]
pub fn sign_connector_dir(dir: &Path, private_key_hex: &str) -> Result<String, String> {
    let manifest_path = dir.join(crate::registry::MANIFEST_FILE);
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读取 {} 失败: {e}", manifest_path.display()))?;
    let signature = nemesis_security::signature::sign_content_hex(&manifest_text, private_key_hex)?;
    let public_key = public_key_of(private_key_hex)?;
    let envelope = SigEnvelope {
        algorithm: "ed25519".to_string(),
        public_key: public_key.clone(),
        signature,
    };
    let sig_json =
        serde_json::to_string_pretty(&envelope).map_err(|e| format!("签名信封序列化失败: {e}"))?;
    let sig_path = dir.join(SIG_FILE);
    std::fs::write(&sig_path, sig_json)
        .map_err(|e| format!("写 {} 失败: {e}", sig_path.display()))?;
    Ok(public_key)
}

/// 从 hex64 私钥推出对应公钥（hex64）。
#[cfg(feature = "security")]
fn public_key_of(private_key_hex: &str) -> Result<String, String> {
    let seed = hex::decode(private_key_hex.trim()).map_err(|e| format!("私钥不是合法 hex: {e}"))?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|v: Vec<u8>| format!("私钥应为 32 字节（hex64），实际 {} 字节", v.len()))?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    Ok(hex::encode(signing_key.verifying_key().to_bytes()))
}

#[cfg(all(test, feature = "security"))]
mod tests;
