//! [`super::pricing_lite`] 解析器测试：fixture 按 LiteLLM 真实字段名钉死
//! （受限网络下本机拉不到真表——fixture 即磁盘格式的证据快照，字段名与
//! LiteLLM 上游一致：per-token 计价 / mode 门 / max_tokens 是输出上限）。

use super::parse_litellm_json;

const FIXTURE: &str = r#"{
  "gpt-4o": {
    "max_tokens": 16384,
    "max_input_tokens": 128000,
    "input_cost_per_token": 2.5e-06,
    "output_cost_per_token": 1e-05,
    "cache_read_input_token_cost": 1.25e-06,
    "cache_creation_input_token_cost": 3.125e-06,
    "litellm_provider": "openai",
    "mode": "chat",
    "supports_function_calling": true,
    "supports_parallel_function_calling": false,
    "supports_vision": true
  },
  "glm-4.7": {
    "max_tokens": 8192,
    "max_input_tokens": 128000,
    "input_cost_per_token": 6e-07,
    "output_cost_per_token": 2.2e-06,
    "litellm_provider": "zhipu",
    "mode": "chat"
  },
  "text-embedding-3-small": {
    "input_cost_per_token": 2e-08,
    "output_cost_per_token": 0.0,
    "litellm_provider": "openai",
    "mode": "embedding"
  },
  "gpt-4o-audio-preview": {
    "input_cost_per_token": 2.5e-06,
    "output_cost_per_token": 1e-05,
    "litellm_provider": "openai",
    "mode": "audio_transcription"
  },
  "broken-model": {
    "mode": "chat",
    "input_cost_per_token": null,
    "output_cost_per_token": 1e-05,
    "litellm_provider": "openai"
  },
  "deepseek-chat": {
    "max_tokens": 8192,
    "max_input_tokens": 65536,
    "input_cost_per_token": 2.7e-07,
    "output_cost_per_token": 1.1e-06,
    "cache_read_input_token_cost": 7e-08,
    "litellm_provider": "deepseek",
    "mode": "chat"
  }
}"#;

#[test]
fn parses_chat_entries_with_per_token_to_per_million_conversion() {
    let out = parse_litellm_json(FIXTURE).unwrap();
    // chat×3（gpt-4o / glm-4.7 / deepseek-chat）；embedding、audio、缺价条目剔除。
    assert_eq!(out.len(), 3);

    let gpt4o = out.iter().find(|p| p.model_id == "gpt-4o").unwrap();
    // per-token ×1e6 = per-million：2.5e-06 → 2.5。
    assert!((gpt4o.input_cost_per_million - 2.5).abs() < 1e-9);
    assert!((gpt4o.output_cost_per_million - 10.0).abs() < 1e-9);
    assert!((gpt4o.cache_read_cost_per_million - 1.25).abs() < 1e-9);
    assert!((gpt4o.cache_creation_cost_per_million - 3.125).abs() < 1e-9);
    assert_eq!(gpt4o.max_input_tokens, Some(128000));
    // LiteLLM 的 max_tokens 是输出上限 → max_output_tokens。
    assert_eq!(gpt4o.max_output_tokens, Some(16384));
    assert_eq!(gpt4o.display_name, "openai");
}

#[test]
fn missing_cache_prices_default_to_zero() {
    let out = parse_litellm_json(FIXTURE).unwrap();
    let glm = out.iter().find(|p| p.model_id == "glm-4.7").unwrap();
    assert_eq!(glm.cache_read_cost_per_million, 0.0);
    assert_eq!(glm.cache_creation_cost_per_million, 0.0);
    // deepseek 只有 cache_read 没有 cache_creation → creation 归零。
    let ds = out.iter().find(|p| p.model_id == "deepseek-chat").unwrap();
    assert!((ds.cache_read_cost_per_million - 0.07).abs() < 1e-9);
    assert_eq!(ds.cache_creation_cost_per_million, 0.0);
}

#[test]
fn non_chat_and_priciless_entries_are_skipped() {
    let out = parse_litellm_json(FIXTURE).unwrap();
    assert!(out.iter().all(|p| p.model_id != "text-embedding-3-small"));
    assert!(out.iter().all(|p| p.model_id != "gpt-4o-audio-preview"));
    // input 价为 null（缺失）→ 无法诚实计价 → 跳过。
    assert!(out.iter().all(|p| p.model_id != "broken-model"));
}

#[test]
fn non_map_top_level_is_rejected() {
    // 误传我们自己 embed 的 {models:[...]} 形状 → 报错（调用方降级保留旧表）。
    assert!(parse_litellm_json(r#"{"models": []}"#).is_err());
    assert!(parse_litellm_json("not json at all").is_err());
}

#[test]
fn empty_tables_are_rejected_as_wrong_shape() {
    // 0 条 = 传错文件（真表数千条）→ Err，调用方降级保留旧表。
    assert!(parse_litellm_json("{}").is_err());
}
