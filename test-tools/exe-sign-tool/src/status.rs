//! 签名验证状态（对齐微软 Authenticode / WinVerifyTrust 语义）。
//!
//! 三维度：[`Code`]（验证码）+ [`CloudState`]（云端参与）+ [`Source`]（决策来源）。
//! 调用方据此决策——**只有 `cloud_state=Reached`（云端实时核实）才视为高可信**，
//! `Unreachable`/`NotConfigured`（本地验证）不保证安全（详见 PLAN §13 本地 keystone 边界）。

/// 验证码（对齐 WinVerifyTrust 常用 HRESULT 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// 有效（ERROR_SUCCESS）
    Valid,
    /// 无签名（TRUST_E_NOSIGNATURE）
    NoSignature,
    /// 内容被篡改 / AEAD 失败 / hash 不符（TRUST_E_BAD_DIGEST）
    BadDigest,
    /// 签名本身无效（TRUST_E_CERT_SIGNATURE）
    SignatureInvalid,
    /// 过期（CERT_E_EXPIRED）
    Expired,
    /// 已吊销（CERT_E_REVOKED）
    Revoked,
    /// 发布者不受信任（CERT_E_UNTRUSTEDROOT）
    UntrustedPublisher,
    /// 结构错误 / 不支持版本（本工具扩展，非 Authenticode 标准；TRUST_E_SUBJECT_FORM_UNKNOWN）
    Malformed,
}

impl Code {
    /// 映射到 Windows HRESULT（语义对齐微软常量名；hex 值按 `wincrypt.h`/`wintrust.h`，
    /// 移植到 Windows 调用方时核对——部分值如 BadDigest/Expired 待确认是否同值）。
    pub fn to_hresult(self) -> u32 {
        match self {
            Code::Valid => 0,                       // ERROR_SUCCESS
            Code::NoSignature => 0x800B_0100,       // TRUST_E_NOSIGNATURE
            Code::BadDigest => 0x8009_6001,         // TRUST_E_BAD_DIGEST（winerror.h；与 CERT_E_EXPIRED 区分）
            Code::SignatureInvalid => 0x800B_0111,  // TRUST_E_CERT_SIGNATURE
            Code::Expired => 0x800B_0101,           // CERT_E_EXPIRED（待核，与 BadDigest 区分靠上下文）
            Code::Revoked => 0x800B_010C,           // CERT_E_REVOKED
            Code::UntrustedPublisher => 0x800B_0108, // CERT_E_UNTRUSTEDROOT
            Code::Malformed => 0x800B_0003,         // TRUST_E_SUBJECT_FORM_UNKNOWN
        }
    }
}

/// 云端参与情况。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudState {
    /// 未配置云端（纯离线验证）
    NotConfigured,
    /// 云端可达，已实时核实
    Reached,
    /// 云端配置了但不可达（本地 fallback，soft-fail 放行 + 标记）
    Unreachable,
}

/// 决策来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// 本地验证（签名有效性 + 本地 CRL/trusted-keys 缓存 + 过期）
    Local,
    /// 云端实时验证
    Cloud,
}

/// 签名验证完整状态。
#[derive(Debug, Clone)]
pub struct SignatureStatus {
    pub code: Code,
    pub cloud_state: CloudState,
    pub source: Source,
    pub signed_at: u64,
    pub expires_at: Option<u64>,
    /// 吊销时间（code=Revoked 时）。
    pub revoked_at: Option<u64>,
    /// 吊销原因（code=Revoked 时）。
    pub reason: Option<String>,
    /// 详细诊断信息（code!=Valid 时）。
    pub detail: String,
    pub crl_ver: Option<u64>,
    pub trusted_keys_ver: Option<u64>,
}

impl SignatureStatus {
    /// 是否验证通过（code=Valid）。注意：**不等于高可信**——还需 `cloud_state=Reached`。
    pub fn is_valid(&self) -> bool {
        matches!(self.code, Code::Valid)
    }

    /// 是否高可信（Valid 且云端实时核实）。
    pub fn is_cloud_verified(&self) -> bool {
        matches!(self.code, Code::Valid) && matches!(self.cloud_state, CloudState::Reached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hresult_mapping() {
        assert_eq!(Code::Valid.to_hresult(), 0);
        assert_eq!(Code::Revoked.to_hresult(), 0x800B_010C);
        assert_eq!(Code::UntrustedPublisher.to_hresult(), 0x800B_0108);
        assert_eq!(Code::NoSignature.to_hresult(), 0x800B_0100);
        // 所有失败码非 0
        for c in [
            Code::NoSignature,
            Code::BadDigest,
            Code::SignatureInvalid,
            Code::Expired,
            Code::Revoked,
            Code::UntrustedPublisher,
            Code::Malformed,
        ] {
            assert_ne!(c.to_hresult(), 0, "{:?} should be non-zero HRESULT", c);
        }
    }

    #[test]
    fn status_helpers() {
        let s = SignatureStatus {
            code: Code::Valid,
            cloud_state: CloudState::Reached,
            source: Source::Cloud,
            signed_at: 1,
            expires_at: None,
            revoked_at: None,
            reason: None,
            detail: String::new(),
            crl_ver: None,
            trusted_keys_ver: None,
        };
        assert!(s.is_valid());
        assert!(s.is_cloud_verified());
        // 本地 Valid 不算 cloud_verified
        let local = SignatureStatus {
            cloud_state: CloudState::Unreachable,
            source: Source::Local,
            ..s
        };
        assert!(local.is_valid());
        assert!(!local.is_cloud_verified());
    }
}
