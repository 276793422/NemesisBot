//! header_guard 单元测试（BUG-20261122-038）。
use super::*;

#[test]
fn credential_clean_value_passes_and_trims() {
    // 常规 key + 粘贴尾随换行/空格 → trim 后放行（真实触发形态）。
    assert_eq!(
        header_safe_credential("api_key", "sk-test-123\n").unwrap(),
        "sk-test-123"
    );
    assert_eq!(
        header_safe_credential("api_key", "  sk-test  ").unwrap(),
        "sk-test"
    );
    assert_eq!(header_safe_credential("api_key", "sk-1").unwrap(), "sk-1");
}

#[test]
fn credential_empty_value_allowed() {
    // 空 key 放行（保留现网 wire 行为；HttpCompat 空 key 由
    // resolve_provider_selection 早臂 loud Err，anthropic 空值头照旧）。
    assert_eq!(header_safe_credential("api_key", "").unwrap(), "");
    assert_eq!(header_safe_credential("api_key", "  \n").unwrap(), "");
}

#[test]
fn credential_rejects_control_chars_without_value_leak() {
    // 尾随/内嵌换行、\r、\0、DEL、内嵌 tab 之后的控制字符 → 拒绝。
    assert!(header_safe_credential("api_key", "sk-1\nsk-2").is_err());
    assert!(header_safe_credential("api_key", "sk\r1").is_err());
    assert!(header_safe_credential("api_key", "sk\u{0}1").is_err());
    assert!(header_safe_credential("api_key", "sk\u{7f}1").is_err());
    // 提示只落 byte 码与字段名，不携带字段内容（SEC-002：可能是凭据）。
    // 用内嵌控制字符（尾随换行是 trim 防线的放行形态，见 credential_clean）。
    let err = header_safe_credential("api_key", "SECRET\u{1}").unwrap_err();
    assert!(err.contains("api_key"), "提示须含字段名: {err}");
    assert!(err.contains("0x01"), "提示须含违规 byte 码: {err}");
    assert!(!err.contains("SECRET"), "提示不得泄漏字段内容: {err}");
}

#[test]
fn credential_tab_allowed() {
    // \t 是 HeaderValue 允许字符（http crate 语义）——组合值仍合法。
    assert_eq!(
        header_safe_credential("api_key", "sk-\t1").unwrap(),
        "sk-\t1"
    );
}

#[cfg(feature = "http")]
mod http_gated {
    use super::filter_valid_header_pairs;

    #[test]
    fn filter_drops_invalid_name_and_value() {
        let mut map = std::collections::HashMap::new();
        map.insert("X-Valid".to_string(), "v1".to_string());
        map.insert("bad name".to_string(), "v2".to_string()); // 头名含空格
        map.insert("X-Multi".to_string(), "a\nb".to_string()); // 值含换行
        map.insert("X-Ctl".to_string(), "a\u{1}b".to_string()); // 值含控制字符
        let out = filter_valid_header_pairs("test-lane", &map);
        assert_eq!(out.len(), 1, "只应保留合法对: {out:?}");
        assert_eq!(out.get("X-Valid").map(String::as_str), Some("v1"));
    }

    #[test]
    fn filter_keeps_tab_value_and_unicode_obs_text() {
        let mut map = std::collections::HashMap::new();
        map.insert("X-Tab".to_string(), "a\tb".to_string());
        map.insert("X-Obs".to_string(), "caf\u{e9}".to_string()); // obs-text 合法
        let out = filter_valid_header_pairs("test-lane", &map);
        assert_eq!(out.len(), 2, "HeaderValue 允许 tab 与 obs-text: {out:?}");
    }

    #[test]
    fn filter_empty_map_noop() {
        let out = filter_valid_header_pairs("test-lane", &Default::default());
        assert!(out.is_empty());
    }
}
