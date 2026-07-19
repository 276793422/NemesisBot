//! 本地吊销策略（verify 时查过期 + CRL + trusted-keys）。
//!
//! CRL / TrustedKeyList 由调用方加载并用吊销根公钥（`crpub`）验签后传入
//! （本地 keystore L0–L1：编译期锚点 + 数据签名验证）。`verify` 只查内容
//! （不验签——验签在加载时做）。
//!
//! 详见 PLAN §13 本地 keystone 分层（L0–L4）+ 边界声明。

use revoke_common::{Crl, TrustedKeyList};

/// 本地吊销策略。
pub struct RevocationPolicy<'a> {
    /// 当前时间（Unix epoch），用于过期判断。
    pub now: u64,
    /// 本地 CRL 缓存（已用 `crpub` 验签；`None`=无缓存，不查吊销列表）。
    pub crl: Option<&'a Crl>,
    /// 受信任公钥列表（已用 `crpub` 验签；`None`=不查 trusted-keys）。
    pub trusted_keys: Option<&'a TrustedKeyList>,
}

impl<'a> RevocationPolicy<'a> {
    /// 无吊销策略（不查 CRL/trusted-keys；仍可判过期）。
    pub fn offline(now: u64) -> Self {
        Self {
            now,
            crl: None,
            trusted_keys: None,
        }
    }
}
