//! [`super`]（pricing_filter）纯函数测试：过滤规则、extras 合并幂等、
//! 条目数校验阈值、宽容 i64 反序列化（上游浮点 token 数实证家族）。

use super::*;

/// 构造一个最小合法条目。
fn chat_entry(input: f64, output: f64) -> LiteLLMEntry {
    LiteLLMEntry {
        mode: Some("chat".into()),
        input_cost_per_token: Some(input),
        output_cost_per_token: Some(output),
        ..Default::default()
    }
}

#[test]
fn filter_keeps_only_chat_with_both_prices() {
    let raw = r#"{
        "good": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06},
        "completion_ok": {"mode": "completion", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06},
        "embedding_skipped": {"mode": "embedding", "input_cost_per_token": 1e-07, "output_cost_per_token": 1e-07},
        "no_mode_skipped": {"input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06},
        "half_priced_skipped": {"mode": "chat", "output_cost_per_token": 2e-06},
        "junk_meta_skipped": {"sample_spec": true}
    }"#;
    let out = filter_litellm_table(raw).unwrap();
    let map: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&out).unwrap();
    assert_eq!(map.len(), 2, "chat + completion only: {map:?}");
    assert!(map.contains_key("good"));
    assert!(map.contains_key("completion_ok"));
}

#[test]
fn filter_drops_unknown_fields_and_empty_table_errors() {
    let raw = r#"{
        "m": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06,
               "supports_function_calling": true, "deprecation_date": "2030-01-01"}
    }"#;
    let out = filter_litellm_table(raw).unwrap();
    // 重序列化只保留 LiteLLMEntry 已知字段（内嵌体积压缩来源）。
    assert!(!out.contains("supports_function_calling"));
    assert!(!out.contains("deprecation_date"));

    // 0 条收录 = 形状不对 → 报错（调用方降级）。
    assert!(filter_litellm_table(r#"{"a": {"mode": "embedding"}}"#).is_err());
    assert!(filter_litellm_table("not json at all").is_err());
}

#[test]
fn merge_extras_only_fills_missing_keys_and_is_idempotent() {
    let filtered = r#"{
        "upstream-model": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06}
    }"#;
    let extras = r#"{
        "upstream-model": {"mode": "chat", "input_cost_per_token": 9e-06, "output_cost_per_token": 9e-06},
        "glm-extra": {"mode": "chat", "input_cost_per_token": 6e-07, "output_cost_per_token": 2.2e-06,
                      "litellm_provider": "zhipu", "aliases": ["glm-4-7-251222"]}
    }"#;
    let merged = merge_extras(filtered, extras).unwrap();
    // 幂等：对已合并结果再跑一遍字节不变。
    let merged2 = merge_extras(&merged, extras).unwrap();
    assert_eq!(merged, merged2);

    let map: std::collections::BTreeMap<String, LiteLLMEntry> =
        serde_json::from_str(&merged).unwrap();
    // 上游已有的条目以上游为权威，extras 不覆盖。
    assert_eq!(map["upstream-model"].input_cost_per_token, Some(1e-06));
    // 缺失键被补上，aliases 保留。
    assert_eq!(map["glm-extra"].litellm_provider.as_deref(), Some("zhipu"));
    assert_eq!(
        map["glm-extra"].aliases,
        Some(vec!["glm-4-7-251222".to_string()])
    );
}

#[test]
fn validate_threshold_rejects_junk_pages() {
    // 生成 MIN_FILTERED_ENTRIES 条合法条目 → 过线。
    let mut map = serde_json::Map::new();
    for i in 0..MIN_FILTERED_ENTRIES {
        let e = chat_entry(1e-06, 2e-06);
        map.insert(format!("m{i}"), serde_json::to_value(e).unwrap());
    }
    let big = serde_json::Value::Object(map).to_string();
    assert_eq!(validate_filtered_table(&big).unwrap(), MIN_FILTERED_ENTRIES);

    // 少于阈值（代理劫持页/截断 payload 形态）→ Err。
    assert!(validate_filtered_table(r#"{"a": {"mode": "chat"}}"#).is_err());
}

#[test]
fn lenient_i64_accepts_float_int_null_and_garbage() {
    // 上游实证：2M 窗口模型带浮点 token 数（2000000.0）——严格 i64 会
    // 炸整表（编译期与运行时同源家族 bug）。
    let raw = r#"{
        "float_tokens": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06,
                         "max_input_tokens": 2000000.0, "max_tokens": 65536.5},
        "int_tokens": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06,
                       "max_input_tokens": 128000, "max_tokens": null},
        "garbage_tokens": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06,
                           "max_input_tokens": "big"}
    }"#;
    let out = filter_litellm_table(raw).unwrap();
    let map: std::collections::BTreeMap<String, LiteLLMEntry> = serde_json::from_str(&out).unwrap();
    let f = &map["float_tokens"];
    assert_eq!(f.max_input_tokens, Some(2_000_000));
    assert_eq!(f.max_tokens, Some(65_536), "float truncates");
    let i = &map["int_tokens"];
    assert_eq!(i.max_input_tokens, Some(128_000));
    assert_eq!(i.max_tokens, None, "explicit null = missing");
    let g = &map["garbage_tokens"];
    assert_eq!(g.max_input_tokens, None, "non-numeric degrades to missing");
}

#[test]
fn mirror_chain_starts_with_official_raw() {
    // 镜像链首条必须是官方 raw 地址（真实来源优先，镜像只是兜底）。
    assert_eq!(PRICE_MIRROR_URLS[0], LITELLM_PRICE_URL);
    assert!(PRICE_MIRROR_URLS.len() >= 2, "at least one mirror");
}
