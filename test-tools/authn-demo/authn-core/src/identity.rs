//! 通用身份输出——crate 的唯一"出口形状"（复用边界 #3）。
//!
//! 移植时产品侧把 `Identity` 翻译成自己的用户概念（例如把 `roles`
//! 映射到 ABAC subject、把 `source` 映射到审计链的登录方式字段）。

use serde::{Deserialize, Serialize};

/// 认证来源——语义标记，产品侧据此决定会话策略/审计措辞。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthnSource {
    /// OIDC IdP（如 Keycloak / 企业 SSO）
    Oidc { issuer: String, client_id: String },
    /// LDAP / Active Directory
    Ldap { server: String, user_dn: String },
    /// 本地账号文件
    Local,
    /// 旧静态 token（单用户兼容模式）
    StaticToken,
}

/// 一次成功认证的通用结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    /// 全局唯一主体标识（OIDC=sub / LDAP=userDN / 本地=username）
    pub subject: String,
    /// 展示名（OIDC=name 或 preferred_username / LDAP=cn / 本地=display_name）
    pub display_name: String,
    /// 角色/组列表（OIDC=groups+realm roles / LDAP=组 CN / 本地=静态声明）
    pub roles: Vec<String>,
    /// 认证来源
    pub source: AuthnSource,
}
