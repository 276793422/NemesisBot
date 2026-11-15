//! 凭据守卫支撑件（计划 §5）。
//!
//! 别名命名空间 `vault:browser/<域名>/<name>`：type 动作文本槽接受 vault
//! 别名，execute 期三守卫（origin 域名绑定 / 密码框元素校验 / 拒错不回显
//! 明文）——LLM 拿到的是**使用权**（何时何地用，被域名绑定+凭据闸圈死），
//! 不是**读取权**（永远说不出密码内容）。
//!
//! 守卫实施点在 nemesisbot 侧 host_browser 工具 execute（注入层把别名改写
//! 明文后、CDP 打字前）；本模块只放无状态纯函数与 JS 片段。

use crate::cdp::js_str;
use nemesis_config::vault_ref::VAULT_PREFIX;

/// 别名命名空间段（`vault:browser/...` 的 `browser`）。
pub const NAMESPACE: &str = "browser";

/// 从完整别名值解析浏览器凭据绑定域名。
///
/// 输入形态 `vault:browser/gitee.com/login`（注入前原始参数值 / 脚本步骤
/// 自解析场景）；返回 `Some("gitee.com")`。非 `vault:browser/` 前缀 = None
/// （其他命名空间的 vault 别名不经浏览器守卫，由既有机制管辖）。
pub fn browser_alias_domain(value: &str) -> Option<String> {
    let rest = value.strip_prefix(VAULT_PREFIX)?.strip_prefix(NAMESPACE)?;
    let rest = rest.strip_prefix('/')?;
    let domain = rest.split('/').next()?;
    let domain = domain.trim();
    if domain.is_empty() {
        return None;
    }
    Some(domain.to_ascii_lowercase())
}

/// origin 域名匹配：host == domain 或 host 以 `.domain` 结尾（子域放行，
/// 大小写不敏感；端口在 origin 提取层剥掉）。
pub fn origin_matches_host(origin_host: &str, domain: &str) -> bool {
    let host = origin_host.trim().to_ascii_lowercase();
    let domain = domain.trim().to_ascii_lowercase();
    if host.is_empty() || domain.is_empty() {
        return false;
    }
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// 从 `location.origin` 值（`https://gitee.com:443` 形态）提取 host。
/// 无 scheme = 整串按 host 处理（防御形态）。
pub fn origin_host(origin: &str) -> String {
    let s = origin.trim();
    let after_scheme = match s.split_once("://") {
        Some((_, rest)) => rest,
        None => s,
    };
    // path/query 剥离
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    // port 剥离（IPv6 [::1]:8080 形态：] 之后才是端口）
    let host = match authority.rfind(']') {
        Some(i) if authority.starts_with('[') => &authority[..=i],
        _ => match authority.rfind(':') {
            Some(i) => &authority[..i],
            None => authority,
        },
    };
    host.trim_matches(['[', ']']).to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// JS 片段（Runtime.evaluate 用；selector 经 js_str 引号安全嵌入）
// ---------------------------------------------------------------------------

/// 读当前页面 origin（守卫①数据源）。
pub const JS_ORIGIN: &str = "location.origin";

/// 守卫②：目标元素是否密码型输入框。只认 `input[type=password]`——严格
/// 匹配，不做 name/id 含 "password" 的启发式放宽（守卫宁紧勿松）。
/// 元素缺失/非密码框都返回 false（调用方按 false 拒绝）。
pub fn js_is_password_input(selector: &str) -> String {
    let sel = js_str(selector);
    format!(
        "(() => {{ const el = document.querySelector({sel}); \
         return !!el && el.tagName === 'INPUT' && String(el.type || '').toLowerCase() === 'password'; \
         }})()"
    )
}

#[cfg(test)]
mod tests;
