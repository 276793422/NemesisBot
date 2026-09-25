// factory.rs 覆盖率补充测试（resolve_provider_selection 的 protocol 钉死
// 三臂空 api_base 默认 142-158、create_provider 的 HttpCompat 代理透传
// 270 经 opt_proxy）。
//
// 豁免：160 的 `unreachable!`（normalize_model_protocol 只返回 canonical
// 集合，非 canonical 在其内部已报错）——纯死臂，无输入可达。

use super::*;

fn cfg(llm_ref: &str, protocol: &str) -> FactoryConfig {
    FactoryConfig {
        llm_ref: llm_ref.to_string(),
        protocol: protocol.to_string(),
        ..Default::default()
    }
}

/// protocol 三 canonical 值的 api_base 默认填充（142-158 的三个空 base
/// 臂）：anthropic / openai（151）/ responses。
#[test]
fn protocol_pins_provider_type_and_default_base() {
    let sel = resolve_provider_selection(&cfg("any/model", "anthropic")).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Anthropic);
    assert_eq!(sel.api_base, "https://api.anthropic.com");

    let sel = resolve_provider_selection(&cfg("any/model", "openai")).unwrap();
    assert_eq!(sel.provider_type, ProviderType::HttpCompat);
    assert_eq!(sel.api_base, "https://api.openai.com/v1");

    let sel = resolve_provider_selection(&cfg("any/model", "responses")).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Codex);
    assert_eq!(sel.api_base, "https://chatgpt.com/backend-api/codex");

    // 非空 api_base 不被默认值覆盖（对照）。
    let mut explicit = cfg("any/model", "openai");
    explicit.api_base = "http://127.0.0.1:9/v1".to_string();
    let sel = resolve_provider_selection(&explicit).unwrap();
    assert_eq!(sel.api_base, "http://127.0.0.1:9/v1");
}

/// create_provider 的 HttpCompat lane 代理透传（270 经 opt_proxy 非空臂）；
/// 空代理 → None（对照）。
#[test]
fn create_provider_http_compat_proxy_passthrough() {
    let mut with_proxy = cfg("http-compat/glm-x", "openai");
    with_proxy.api_key = "k".to_string();
    with_proxy.proxy = "http://127.0.0.1:9".to_string();
    let provider = create_provider(&with_proxy).expect("带代理构造必须成功");
    assert!(!provider.name().is_empty());

    let plain = cfg("http-compat/glm-x", "openai");
    let provider = create_provider(&plain).expect("无代理构造必须成功");
    assert!(!provider.name().is_empty());
}

/// protocol 钉死路径下的 create_provider：responses → Codex lane、
/// anthropic → Anthropic lane（构造覆盖，不发生网络 IO）。
#[test]
fn create_provider_protocol_lanes_construct() {
    let codex = cfg("any/gpt-5.2", "responses");
    let provider = create_provider(&codex).expect("codex lane 必须可构造");
    assert_eq!(provider.name(), "codex");

    let anthropic = cfg("any/claude-x", "anthropic");
    let provider = create_provider(&anthropic).expect("anthropic lane 必须可构造");
    assert_eq!(provider.name(), "anthropic");
}

/// create_provider_or_null：成功 → 无告警；空 llm_ref → NullProvider +
/// 告警（278-283）。
#[test]
fn create_provider_or_null_both_arms() {
    let good = cfg("http-compat/glm-x", "openai");
    let (provider, warning) = create_provider_or_null(&good);
    assert!(warning.is_none());
    assert!(!provider.name().is_empty());

    let bad = FactoryConfig {
        llm_ref: String::new(),
        ..Default::default()
    };
    let (provider, warning) = create_provider_or_null(&bad);
    let warning = warning.expect("失败必须带装配告警");
    assert!(warning.contains("empty LLM reference"), "{warning}");
    assert_eq!(provider.name(), "null");
}
