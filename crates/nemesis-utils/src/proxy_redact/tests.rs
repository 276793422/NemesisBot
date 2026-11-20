//! proxy_redact 单一真相源测试：同构位点统一走本模块的脱敏形态红线。

use super::redact_proxy_for_log;

/// userinfo 凭据 URL：只落 scheme，凭据段/主机段一个字符都不出现
///（SEC-002 凭据面红线——api_key 同判，BUG-20261111-068）。
#[test]
fn userinfo_credentials_never_leak() {
    let out = redact_proxy_for_log("http://alice:s3cret@proxy.corp:8080");
    assert_eq!(out, "http://<redacted>");
    assert!(!out.contains("alice"), "用户名泄漏: {out}");
    assert!(!out.contains("s3cret"), "口令泄漏: {out}");
    assert!(!out.contains("proxy.corp"), "主机段不落日志: {out}");
}

/// scheme 保留（代理 scheme 支持面：http/https/socks4/socks4a/socks5/socks5h
/// 与 proxy_file::scheme_supported 同清单）——scheme 是定位诊断必需的
/// 最小信息，凭据不必要。
#[test]
fn scheme_preserved_other_parts_masked() {
    assert_eq!(
        redact_proxy_for_log("socks5h://10.0.0.1:1080"),
        "socks5h://<redacted>"
    );
    assert_eq!(
        redact_proxy_for_log("https://host:443"),
        "https://<redacted>"
    );
}

/// 空值 / 纯空白 / `"://…"`（scheme 段为空）→ `<unknown-scheme>`
///（005 前例形态一致）。
#[test]
fn empty_and_schemeless_forms() {
    assert_eq!(redact_proxy_for_log(""), "<unknown-scheme>");
    assert_eq!(redact_proxy_for_log("   "), "<unknown-scheme>");
    assert_eq!(redact_proxy_for_log("://weird"), "<unknown-scheme>");
}

/// 非 URL 裸词按前例当 scheme 段输出（不猜意图，只保证 `"://"` 之后
/// 永不出现）。
#[test]
fn bare_word_is_treated_as_scheme_segment() {
    assert_eq!(
        redact_proxy_for_log("no-scheme-value"),
        "no-scheme-value://<redacted>"
    );
}
