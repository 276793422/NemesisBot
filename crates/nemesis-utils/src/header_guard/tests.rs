//! header_guard 单元测试（BUG-20261122-038；语义探针 BUG-20261123-013）。
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
    use super::validate_header_pair;

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

    #[test]
    fn validate_pair_names_key_not_value() {
        // BUG-20261123-013：单对校验（filter 的单一真相源）。
        assert!(validate_header_pair("X-Ok", "fine").is_ok());
        // 值非法：点名头名、不回显值（SEC-002）。
        let e = validate_header_pair("X-Multi", "a\nb").unwrap_err();
        assert!(e.contains("X-Multi"), "错误须点名头名: {e}");
        assert!(!e.contains("a\nb"), "错误不得回显值: {e}");
        // 名非法：点名头名。
        let e = validate_header_pair("bad name", "v").unwrap_err();
        assert!(e.contains("bad name"), "错误须点名头名: {e}");
        // HeaderValue 允许字符放行（与 filter 同语义）。
        assert!(validate_header_pair("X-Tab", "a\tb").is_ok());
        assert!(validate_header_pair("X-Obs", "caf\u{e9}").is_ok());
    }
}

/// BUG-20261123-013 语义探针（reqwest 真实行为实证钉，dev-dep reqwest
/// 无 http feature 照常编译——同 llm_error_class 先例）：reqwest 0.12.x
/// 对非法头名/头值**不是 panic**——builder poison，`send()` 立即返回
/// "builder error"，网络零触达。038 原记录的 panic 叙事由此证伪钉死。
/// 测试通过本身 = 无 panic 且错误形态可辨识（builder error）。
mod header_probe {
    // 拒连端口（loopback :1 无监听 = 立即 RST，无外呼、无端口占用）。
    const NEVER: &str = "http://127.0.0.1:1/never-reached";

    #[tokio::test]
    async fn invalid_header_value_is_builder_error_not_panic() {
        let client = reqwest::Client::new();
        let r = client.get(NEVER).header("X-Probe", "a\nb").send().await;
        let err = r.expect_err("非法头值必须 Err；测试走到这里本身就证明没有 panic");
        let msg = err.to_string();
        assert!(
            msg.contains("builder error"),
            "非法头值应产出 builder error 形态（预网络失败）: {msg}"
        );
    }

    #[tokio::test]
    async fn invalid_bearer_token_is_builder_error_not_panic() {
        // bearer_auth（038 记录的另一形态）：组合 "Bearer tok\nen" 同 poison。
        let client = reqwest::Client::new();
        let r = client.get(NEVER).bearer_auth("tok\nen").send().await;
        let err = r.expect_err("bearer_auth 非法 token 必须 Err 且不得 panic");
        assert!(
            err.to_string().contains("builder error"),
            "bearer_auth 非法 token 应产出 builder error: {err}"
        );
    }

    #[tokio::test]
    async fn valid_header_fails_at_network_not_builder() {
        // 阳性对照（防探针管道自证假绿）：合法头值不打 builder error——
        // 失败发生在网络层（拒连），证明 builder error 是可辨识的预网络
        // 失败类别，而非一切失败的通用形态。
        let client = reqwest::Client::new();
        let r = client
            .get(NEVER)
            .header("X-Probe", "fine-value")
            .send()
            .await;
        let err = r.expect_err("拒连端口必须 Err");
        assert!(
            !err.to_string().contains("builder error"),
            "合法头值不应产出 builder error（失败应发生在网络层）: {err}"
        );
    }
}
