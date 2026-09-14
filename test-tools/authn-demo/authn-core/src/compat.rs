//! 旧静态 token 共存语义（验证项 ⑤）。
//!
//! 这是 NemesisBot `web.auth_token` 语义的**通用化**形态：
//! - 未设置 / 空串 → **开放模式**（不做认证，任何人都放行）
//! - 设置了非空值 → **单用户静态 token 模式**（持正确 token 放行，其余拒绝）
//!
//! 比较 use `subtle` 常量时间比较，避免逐字节提前返回的时序侧信道
//! （demo 也按生产标准写——移植时这段可以直接搬）。

use subtle::ConstantTimeEq;
use thiserror::Error;

/// 访问模式。`AccessControl::from_static_token` 是唯一构造入口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessMode {
    /// 未设置静态 token：开放模式
    Open,
    /// 已设置静态 token：单用户模式（携带值只存在于内存）
    StaticToken(String),
}

/// 访问判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecision {
    /// 开放模式放行
    AllowOpen,
    /// 静态 token 匹配放行
    AllowStaticToken,
    /// 拒绝
    Denied,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompatError {
    #[error("静态 token 超过安全长度上限（4096）")]
    TokenTooLong,
}

/// 静态 token 访问控制。
#[derive(Debug, Clone)]
pub struct AccessControl {
    mode: AccessMode,
}

impl AccessControl {
    /// 从"是否配置了静态 token"构造。`None` 或空串 → 开放模式。
    /// 这是旧单 token 配置键（如 `web.auth_token`）的通用化入口。
    pub fn from_static_token(token: Option<&str>) -> Result<Self, CompatError> {
        match token.map(str::trim) {
            None | Some("") => Ok(Self {
                mode: AccessMode::Open,
            }),
            Some(t) if t.len() > 4096 => Err(CompatError::TokenTooLong),
            Some(t) => Ok(Self {
                mode: AccessMode::StaticToken(t.to_string()),
            }),
        }
    }

    pub fn mode(&self) -> &AccessMode {
        &self.mode
    }

    /// 判定一次出示的凭据。`presented` 为 `None` 表示请求根本没带 token。
    pub fn check(&self, presented: Option<&str>) -> AccessDecision {
        match &self.mode {
            AccessMode::Open => AccessDecision::AllowOpen,
            AccessMode::StaticToken(expected) => match presented {
                Some(p) if ct_eq_str(p, expected) => AccessDecision::AllowStaticToken,
                _ => AccessDecision::Denied,
            },
        }
    }
}

/// 常量时间字符串比较（长度不等时提前返回——长度本身不视为秘密，
/// 与主流框架口径一致；注释留痕，移植时可按产品策略收紧）。
fn ct_eq_str(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.ct_eq(b))
}

#[cfg(test)]
mod tests;
