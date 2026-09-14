//! LDAP / AD 协议引擎（验证项 ②）：直连 bind → 组提取 → LDAPS/STARTTLS 选项。
//!
//! 两种组提取模式（AD 与 OpenLDAP 的现实差异决定的）：
//! - [`GroupExtraction::MemberOf`]——直接读用户条目的 `memberOf` 属性
//!   （**AD 默认支持**；OpenLDAP 需加载 memberof overlay）；
//! - [`GroupExtraction::MemberFilter`]——反查组：搜
//!   `(&(objectClass=groupOfNames)(member=<userDN>))`（OpenLDAP 无 overlay 时的通用姿势）。
//!
//! 诚实边界（demo 验证范围）：OpenLDAP ≠ 真实 AD——sAMAccountName/UPN 登录名、
//! 嵌套组（`ldap_matching_rule_in_chain`）、Windows 证书信任链只在真 AD 上才
//! 会暴露。demo 用 OpenLDAP 验证协议链路，接真 AD 时预期只需要改配置不改代码。

use serde::Serialize;
use thiserror::Error;

use ldap3::{LdapConnAsync, LdapConnSettings, Scope, SearchEntry};

use crate::identity::{AuthnSource, Identity};

#[derive(Debug, Error)]
pub enum LdapError {
    #[error("LDAP 连接失败（服务器不可达 / TLS 握手失败）: {0}")]
    Connect(String),
    #[error("bind 失败：用户不存在或密码错误（rc=49）")]
    InvalidCredentials,
    #[error("bind 失败（rc={rc}）: {message}")]
    BindFailed { rc: u32, message: String },
    #[error("search 失败: {0}")]
    Search(String),
    #[error("连接 setup 失败: {0}")]
    Setup(String),
}

/// 组提取模式。
#[derive(Debug, Clone)]
pub enum GroupExtraction {
    /// 读用户条目 `memberOf` 属性（AD 风格）
    MemberOf,
    /// 反查 groupOfNames：`(&(objectClass=…)(member=<userDN>))`（OpenLDAP 风格）
    MemberFilter,
}

/// LDAP 配置（builder 式——复用边界 #2）。
#[derive(Debug, Clone)]
pub struct LdapConfig {
    /// `ldap://host:1389` 或 `ldaps://host:1636`
    pub url: String,
    /// 对 ldap:// 再升级 STARTTLS（ldaps:// 时必须 false）
    pub starttls: bool,
    /// 跳过 TLS 证书校验（demo 对自签证书的开关；生产应配受信 CA）
    pub tls_no_verify: bool,
    /// 用户 DN 模板，`{}` 会被（转义后的）用户名替换。
    /// 例：`uid={},ou=people,dc=example,dc=org`；AD 常见 `{}\@corp.example.com`（UPN 直 bind）。
    pub user_dn_template: String,
    /// 反查组的搜索根（MemberFilter 模式使用）
    pub base_dn: String,
    pub group_extraction: GroupExtraction,
    /// 组条目的 objectClass（MemberFilter 模式，默认 `groupOfNames`；AD 用 `group`）
    pub group_object_class: String,
    /// 组的成员属性（MemberFilter 模式，默认 `member`；AD 用 `member`）
    pub group_member_attr: String,
}

impl LdapConfig {
    pub fn new(url: impl Into<String>, user_dn_template: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            starttls: false,
            tls_no_verify: false,
            user_dn_template: user_dn_template.into(),
            base_dn: String::new(),
            group_extraction: GroupExtraction::MemberFilter,
            group_object_class: "groupOfNames".to_string(),
            group_member_attr: "member".to_string(),
        }
    }
}

/// LDAP 认证结果。
#[derive(Debug, Clone, Serialize)]
pub struct LdapAuthResult {
    pub identity: Identity,
    /// bind 用的完整 DN
    pub user_dn: String,
    /// 原始组标识（MemberOf 模式 = memberOf DN 列表；Filter 模式 = 组 CN 列表）
    pub raw_groups: Vec<String>,
    /// 组提取阶段的非致命告警（认证成功但组读取受限时非空——诚实呈现）
    pub warnings: Vec<String>,
}

/// 完整认证：DN 构造（转义）→ simple bind → 组提取 → 通用身份。
pub async fn authenticate(
    config: &LdapConfig,
    username: &str,
    password: &str,
) -> Result<LdapAuthResult, LdapError> {
    if username.is_empty() || password.is_empty() {
        return Err(LdapError::InvalidCredentials);
    }
    // 用户名先做 DN 值转义再进模板——防 DN 注入（例如用户名里带逗号改变 RDN 结构）
    let dn = config
        .user_dn_template
        .replace("{}", &escape_dn_value(username));

    let settings = LdapConnSettings::new()
        .set_starttls(config.starttls)
        .set_no_tls_verify(config.tls_no_verify);
    let (conn, mut ldap) = LdapConnAsync::with_settings(settings, &config.url)
        .await
        .map_err(|e| LdapError::Connect(e.to_string()))?;
    ldap3::drive!(conn);

    let bind_result = match ldap.simple_bind(&dn, password).await {
        Ok(res) => res,
        Err(e) => {
            let _ = ldap.unbind().await;
            return Err(LdapError::Connect(e.to_string()));
        }
    };
    let bind_rc = bind_result.rc;
    if bind_rc == 49 {
        let _ = ldap.unbind().await;
        return Err(LdapError::InvalidCredentials);
    }
    if let Err(e) = bind_result.success() {
        let _ = ldap.unbind().await;
        return Err(LdapError::BindFailed {
            rc: bind_rc,
            message: e.to_string(),
        });
    }

    let (raw_groups, warnings) = extract_groups(&mut ldap, config, &dn).await;
    let _ = ldap.unbind().await;

    // 若组反查整体失败（warnings 含 ERROR 前缀），按错误上抛——角色是
    // 授权输入，静默空列表会造成"看起来登录成功但啥权限都没有"的隐性故障。
    if let Some(err) = warnings.iter().find(|w| w.starts_with("ERROR:")) {
        return Err(LdapError::Search(
            err.trim_start_matches("ERROR:").to_string(),
        ));
    }

    let display_name = extract_cn_from_dn(&dn).unwrap_or_else(|| username.to_string());
    Ok(LdapAuthResult {
        identity: Identity {
            subject: dn.clone(),
            display_name,
            roles: normalize_roles(&raw_groups),
            source: AuthnSource::Ldap {
                server: config.url.clone(),
                user_dn: dn.clone(),
            },
        },
        user_dn: dn,
        raw_groups,
        warnings,
    })
}

/// 组提取。内部约定：warnings 里 `ERROR:` 前缀 = 致命（上抛）；
/// 其余 = 告警（继续、诚实呈现）。
async fn extract_groups(
    ldap: &mut ldap3::Ldap,
    config: &LdapConfig,
    user_dn: &str,
) -> (Vec<String>, Vec<String>) {
    match config.group_extraction {
        GroupExtraction::MemberOf => {
            let result = ldap
                .search(user_dn, Scope::Base, "(objectClass=*)", &["memberOf"])
                .await;
            match result {
                Ok(search) => match search.success() {
                    Ok((entries, _)) => {
                        let mut groups = Vec::new();
                        for entry in entries {
                            let e = SearchEntry::construct(entry);
                            if let Some(list) = e.attrs.get("memberOf") {
                                groups.extend(list.clone());
                            }
                        }
                        (groups, Vec::new())
                    }
                    Err(e) => (Vec::new(), vec![format!("ERROR: memberOf 读取失败: {e}")]),
                },
                Err(e) => (
                    Vec::new(),
                    vec![format!("ERROR: memberOf search 失败: {e}")],
                ),
            }
        }
        GroupExtraction::MemberFilter => {
            // member 值是完整 DN，按 filter 语法转义（*, (, ), \ 是控制字符）
            let filter = format!(
                "(&(objectClass={})({}={}))",
                config.group_object_class,
                config.group_member_attr,
                escape_filter_value(user_dn)
            );
            let result = ldap
                .search(
                    config.base_dn.as_str(),
                    Scope::Subtree,
                    filter.as_str(),
                    &["cn"],
                )
                .await;
            match result {
                Ok(search) => match search.success() {
                    Ok((entries, _)) => {
                        let mut groups = Vec::new();
                        for entry in entries {
                            let e = SearchEntry::construct(entry);
                            if let Some(cns) = e.attrs.get("cn") {
                                groups.extend(cns.clone());
                            }
                        }
                        (groups, Vec::new())
                    }
                    Err(e) => (Vec::new(), vec![format!("ERROR: 组反查失败: {e}")]),
                },
                Err(e) => (Vec::new(), vec![format!("ERROR: 组反查 search 失败: {e}")]),
            }
        }
    }
}

/// 组标识 → 角色名：MemberOf 模式拿到的是 DN，抽取其 CN 段。
pub fn normalize_roles(raw_groups: &[String]) -> Vec<String> {
    let mut roles: Vec<String> = raw_groups
        .iter()
        .map(|g| extract_cn_from_dn(g).unwrap_or_else(|| g.clone()))
        .collect();
    roles.sort();
    roles.dedup();
    roles
}

/// 从 DN 里抽第一段 `CN=` 值（处理 `\,` 等转义）。找不到返回 None。
pub fn extract_cn_from_dn(dn: &str) -> Option<String> {
    for part in split_dn_unescaped_comma(dn) {
        let part = part.trim();
        if let Some(rest) = part
            .strip_prefix("CN=")
            .or_else(|| part.strip_prefix("cn="))
        {
            return Some(unescape_dn_value(rest));
        }
    }
    None
}

/// 按"未被反斜杠转义的逗号"切分 DN（naive `split(',')` 会把 `\,` 切碎）。
fn split_dn_unescaped_comma(dn: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for ch in dn.chars() {
        if escaped {
            cur.push('\\');
            cur.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ',' {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    parts.push(cur);
    parts
}

/// DN 值反转义：`\X` → `X`（与 [`escape_dn_value`] 的成对转义互逆；
/// 本 crate 不产生 RFC 4514 十六进制转义，故无需处理 `\2c` 形态）。
fn unescape_dn_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(n) => out.push(n),
                None => {} // 尾随裸反斜杠，丢弃
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// RFC 4514 DN 值转义——用户名进 DN 模板前必过（防 DN 注入）。
pub fn escape_dn_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (i, ch) in value.char_indices() {
        match ch {
            '\\' | ',' | '+' | '"' | '<' | '>' | ';' => {
                out.push('\\');
                out.push(ch);
            }
            '#' if i == 0 => {
                out.push('\\');
                out.push(ch);
            }
            ' ' if i == 0 || i + ch.len_utf8() == value.len() => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// RFC 4514 filter 值转义——DN/用户名进 search filter 前必过（防 filter 注入）。
pub fn escape_filter_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\5c"),
            '*' => out.push_str("\\2a"),
            '(' => out.push_str("\\28"),
            ')' => out.push_str("\\29"),
            '\0' => out.push_str("\\00"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests;
