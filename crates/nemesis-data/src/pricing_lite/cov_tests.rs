//! parse_litellm_json 过滤分支补充测试（mode 过滤 / 缺价跳过 /
//! provider 投射）。
//!
//! 注：非有限价格（NaN/inf）防御分支在纯 JSON 输入下不可达——serde_json
//! 对 `1e400` 直接报 "number out of range"（解析层即拒绝，不会产出 inf
//! 浮点），该分支只对未来的解析器行为变化兜底。

use super::parse_litellm_json;

/// mode 非 chat/completion（如 embedding）→ 跳过。
#[test]
fn non_chat_mode_is_skipped() {
    let raw = r#"{
        "anchor-good": {
            "mode": "chat",
            "input_cost_per_token": 0.000001,
            "output_cost_per_token": 0.000002
        },
        "embed-model": {
            "mode": "embedding",
            "input_cost_per_token": 0.000001,
            "output_cost_per_token": 0.000002
        }
    }"#;
    let parsed = parse_litellm_json(raw).expect("parse");
    let ids: Vec<&str> = parsed.iter().map(|p| p.model_id.as_str()).collect();
    assert!(!ids.contains(&"embed-model"));
    assert!(ids.contains(&"anchor-good"));
}

/// 双价缺一（只有 input 无 output）→ 无法诚实计价，跳过。
#[test]
fn missing_output_price_is_skipped() {
    let raw = r#"{
        "anchor-good": {
            "mode": "chat",
            "input_cost_per_token": 0.000001,
            "output_cost_per_token": 0.000002
        },
        "half-priced": {
            "mode": "chat",
            "input_cost_per_token": 0.000001
        }
    }"#;
    let parsed = parse_litellm_json(raw).expect("parse");
    let ids: Vec<&str> = parsed.iter().map(|p| p.model_id.as_str()).collect();
    assert!(!ids.contains(&"half-priced"));
}

/// 正常条目的 per-token → per-million 换算 + provider 显示名投射。
#[test]
fn provider_name_projected_to_display_name() {
    let raw = r#"{
        "paid-model": {
            "mode": "completion",
            "litellm_provider": "openai",
            "input_cost_per_token": 0.000001,
            "output_cost_per_token": 0.000002
        }
    }"#;
    let parsed = parse_litellm_json(raw).expect("parse");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].model_id, "paid-model");
    assert_eq!(parsed[0].display_name, "openai");
    assert_eq!(parsed[0].input_cost_per_million, 1.0);
    assert_eq!(parsed[0].output_cost_per_million, 2.0);
}

/// 全部条目被过滤（0 条 chat）→ 报错（0 条 = 传错文件的契约）。
#[test]
fn zero_parsed_entries_is_an_error() {
    let raw = r#"{"embed-only": {"mode": "embedding"}}"#;
    let err = parse_litellm_json(raw).expect_err("0 entries must error");
    assert!(err.contains("LiteLLM price table parse failed") || !err.is_empty());
}
