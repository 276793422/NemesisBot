//! creds 纯函数单测：别名解析 / origin 匹配 / 密码框 JS 形态。

use super::{browser_alias_domain, js_is_password_input, origin_host, origin_matches_host};

// --- browser_alias_domain ---

#[test]
fn alias_domain_parses() {
    assert_eq!(
        browser_alias_domain("vault:browser/gitee.com/login"),
        Some("gitee.com".to_string())
    );
    // 深路径：只取第一段。
    assert_eq!(
        browser_alias_domain("vault:browser/app.example.com/team/user"),
        Some("app.example.com".to_string())
    );
}

#[test]
fn alias_domain_case_insensitive() {
    assert_eq!(
        browser_alias_domain("vault:browser/GitHub.com/token"),
        Some("github.com".to_string())
    );
}

#[test]
fn alias_domain_non_browser_namespace_is_none() {
    // 其他 vault 命名空间不经浏览器守卫（既有机制管辖）。
    assert_eq!(browser_alias_domain("vault:gitea/token"), None);
    assert_eq!(browser_alias_domain("vault:browserx/host/x"), None);
}

#[test]
fn alias_domain_malformed_is_none() {
    assert_eq!(browser_alias_domain("vault:browser/"), None);
    assert_eq!(browser_alias_domain("plain-password"), None);
    assert_eq!(browser_alias_domain(""), None);
    assert_eq!(browser_alias_domain("vault:browser//name"), None);
}

// --- origin_matches_host ---

#[test]
fn origin_match_exact_and_subdomain() {
    assert!(origin_matches_host("gitee.com", "gitee.com"));
    assert!(origin_matches_host("www.gitee.com", "gitee.com"));
    assert!(origin_matches_host("GITEE.COM", "gitee.com"));
    assert!(origin_matches_host("app.GitHub.com", "github.com"));
}

#[test]
fn origin_match_rejects_suffix_spoof() {
    // 后缀伪造：evilsite.com ≠ site.com（必须有 . 边界）。
    assert!(!origin_matches_host("evilsite.com", "site.com"));
    assert!(!origin_matches_host("notgitee.com", "gitee.com"));
    assert!(!origin_matches_host("", "gitee.com"));
    assert!(!origin_matches_host("gitee.com", ""));
}

// --- origin_host ---

#[test]
fn origin_host_strips_scheme_port_path() {
    assert_eq!(origin_host("https://gitee.com:443"), "gitee.com");
    assert_eq!(origin_host("http://localhost:8123/"), "localhost");
    assert_eq!(
        origin_host("https://a.example.com/x/y?z=1"),
        "a.example.com"
    );
    assert_eq!(origin_host("gitee.com"), "gitee.com"); // 无 scheme 防御形态
}

#[test]
fn origin_host_ipv6() {
    assert_eq!(origin_host("http://[::1]:8080/"), "::1");
    assert_eq!(origin_host("http://[2001:db8::1]/"), "2001:db8::1");
}

// --- js_is_password_input ---

#[test]
fn password_js_strict_shape() {
    let js = js_is_password_input("#login-pass");
    // 严格 input[type=password]：不认 name/id 启发式（宁紧勿松）。
    assert!(js.contains("toLowerCase() === 'password'"));
    assert!(js.contains("el.tagName === 'INPUT'"));
    // selector 引号安全嵌入（js_str）。
    assert!(js.contains("\"#login-pass\""));
}
