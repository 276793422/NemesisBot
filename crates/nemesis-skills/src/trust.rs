//! 安装门信任四态（P11 验签上膛）。
//!
//! 验签结果（Ed25519 签名 + TrustStore）与「无签名按策略」归约成四个稳定态，
//! 供审批卡展示与安装记录落盘：
//! - `trusted`：签名有效且密钥是 Verified 级
//! - `review-recommended`：签名有效但密钥只是 Community 级
//! - `review-required`：无签名且 `allow_unsigned` 放行（兼容存量）
//! - `blocked`：篡改 / 密钥吊销 / 不受信密钥 / 无签名且 strict 拒绝

use serde::{Deserialize, Serialize};

/// 四态信任标记（serde 值用 kebab-case，与文档口径一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrustState {
    /// 签名有效 + Verified 级密钥。
    #[serde(rename = "trusted")]
    Trusted,
    /// 签名有效 + Community 级密钥（可装，建议复核）。
    #[serde(rename = "review-recommended")]
    ReviewRecommended,
    /// 无签名且策略放行（建议复核）。
    #[serde(rename = "review-required")]
    ReviewRequired,
    /// 篡改 / 吊销 / 不受信 / strict 模式下无签名。
    #[serde(rename = "blocked")]
    Blocked,
}

impl TrustState {
    /// kebab-case 字符串（落盘 lockfile / 展示用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::ReviewRecommended => "review-recommended",
            Self::ReviewRequired => "review-required",
            Self::Blocked => "blocked",
        }
    }

    /// blocked 之外的状态都允许继续走安装门（后续环节仍可拦截）。
    pub fn installable(&self) -> bool {
        !matches!(self, Self::Blocked)
    }
}

impl std::fmt::Display for TrustState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 验签输入摘要（与签名格式解耦，安装门只关心结论与证据）。
#[derive(Debug, Clone, Default)]
pub struct VerificationOutcome {
    /// 技能目录里是否带 `.signature` 文件。
    pub signed: bool,
    /// 签名验证结论（未签名时为 None）。
    pub valid: Option<bool>,
    /// TrustStore 中的密钥级别（未签名时为 None）。
    /// 字符串形态避免对 nemesis-security 的 feature 依赖（见下方 from_parts）。
    pub trust_level: Option<String>,
    /// 签名者公钥（hex，截断展示由调用方决定）。
    pub public_key: String,
    /// 人类可读的错误细节（验签失败原因）。
    pub error: String,
}

impl VerificationOutcome {
    /// 按 P11 语义归约出信任四态。
    ///
    /// - 无签名：`allow_unsigned=true` -> ReviewRequired；false（strict）-> Blocked。
    /// - 签名有效：Verified -> Trusted；Community -> ReviewRecommended；
    ///   Unknown（理论上 valid 蕴含已受信，防御性归 ReviewRecommended）。
    /// - 签名无效：Revoked -> Blocked（吊销）；其余（篡改/不受信）-> Blocked。
    pub fn trust_state(&self, allow_unsigned: bool) -> TrustState {
        if !self.signed {
            return if allow_unsigned {
                TrustState::ReviewRequired
            } else {
                TrustState::Blocked
            };
        }
        let valid = self.valid.unwrap_or(false);
        if valid {
            match self.trust_level.as_deref() {
                Some("verified") => TrustState::Trusted,
                Some("community") => TrustState::ReviewRecommended,
                _ => TrustState::ReviewRecommended,
            }
        } else {
            TrustState::Blocked
        }
    }
}

#[cfg(test)]
mod tests;
