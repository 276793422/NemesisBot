//! authn-core — 身份与访问框架 demo 内核（独立验证任务，非产品集成）。
//!
//! 仿 nemesis-verify 先例：先在 test-tools 里独立跑通并验证，最后才谈移植。
//!
//! ## 三条复用边界（移植前的硬约束）
//!
//! 1. **零 nemesis-\* 依赖**——只依赖外部协议 crate（openidconnect/ldap3/argon2…），
//!    产品侧集成时翻译成本方概念。
//! 2. **builder 式配置**——绝不读产品的 config.json；配置由调用方构造传入。
//! 3. **通用出口形状**——所有认证路径收敛到一个 [`identity::Identity`]：
//!    `{ subject, display_name, roles, source }`，产品侧拿它映射到自己的用户体系。
//!
//! ## 模块地图
//!
//! | 模块 | 职责 | 对应验证项 |
//! |------|------|-----------|
//! | [`identity`] | 通用身份输出 + 来源标记 | （所有项的出口形状） |
//! | [`local`]    | 本地账号 + argon2 哈希/校验 | ③ 本地账号 + hash-password |
//! | [`session`]  | 会话签发/过期/并发上限 | ④ 会话行为 |
//! | [`compat`]   | 旧静态 token 共存语义（空=开放 / 非空=单用户） | ⑤ 兼容模式 |
//! | [`oidc`]     | OIDC：发现→PKCE→授权码→令牌→claims→角色 | ① OIDC 全流程 |
//! | [`ldap`]     | LDAP/AD：bind→search→组映射（+LDAPS/TLS 选项） | ② LDAP 全流程 |

pub mod compat;
pub mod identity;
pub mod ldap;
pub mod local;
pub mod oidc;
pub mod session;

#[cfg(test)]
mod tests;
