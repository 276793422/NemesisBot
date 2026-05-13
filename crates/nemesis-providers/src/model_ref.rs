//! ModelRef parsing, provider aliases, model key normalization.

use serde::{Deserialize, Serialize};

/// A parsed model reference with provider and model name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

/// Parse "anthropic/claude-opus" into `ModelRef { provider: "anthropic", model: "claude-opus" }`.
/// If no slash present, uses `default_provider`.
/// Returns `None` for empty input.
pub fn parse_model_ref(raw: &str, default_provider: &str) -> Option<ModelRef> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    if let Some(idx) = raw.find('/') {
        if idx > 0 {
            let model = raw[idx + 1..].trim();
            if model.is_empty() {
                return None;
            }
            return Some(ModelRef {
                provider: normalize_provider(&raw[..idx]),
                model: model.to_string(),
            });
        }
    }

    Some(ModelRef {
        provider: normalize_provider(default_provider),
        model: raw.to_string(),
    })
}

/// Normalize provider identifiers to canonical form.
pub fn normalize_provider(provider: &str) -> String {
    let p = provider.trim().to_lowercase();
    match p.as_str() {
        "z.ai" | "z-ai" => "zai",
        "opencode-zen" => "opencode",
        "qwen" => "qwen-portal",
        "kimi-code" => "kimi-coding",
        "gpt" => "openai",
        "claude" => "anthropic",
        "glm" => "zhipu",
        "google" => "gemini",
        _ => &p,
    }
    .to_string()
}

/// Returns a canonical "provider/model" key for deduplication.
pub fn model_key(provider: &str, model: &str) -> String {
    format!(
        "{}/{}",
        normalize_provider(provider),
        model.trim().to_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_with_slash() {
        let r = parse_model_ref("anthropic/claude-opus", "openai").unwrap();
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.model, "claude-opus");
    }

    #[test]
    fn test_parse_without_slash() {
        let r = parse_model_ref("gpt-4", "openai").unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "gpt-4");
    }

    #[test]
    fn test_parse_empty() {
        assert!(parse_model_ref("", "openai").is_none());
        assert!(parse_model_ref("   ", "openai").is_none());
    }

    #[test]
    fn test_parse_trailing_slash() {
        assert!(parse_model_ref("anthropic/", "openai").is_none());
    }

    #[test]
    fn test_normalize_provider_aliases() {
        assert_eq!(normalize_provider("Claude"), "anthropic");
        assert_eq!(normalize_provider("GPT"), "openai");
        assert_eq!(normalize_provider("glm"), "zhipu");
        assert_eq!(normalize_provider("google"), "gemini");
        assert_eq!(normalize_provider("qwen"), "qwen-portal");
        assert_eq!(normalize_provider("kimi-code"), "kimi-coding");
        assert_eq!(normalize_provider("z.ai"), "zai");
        assert_eq!(normalize_provider("opencode-zen"), "opencode");
    }

    #[test]
    fn test_normalize_provider_passthrough() {
        assert_eq!(normalize_provider("anthropic"), "anthropic");
        assert_eq!(normalize_provider("deepseek"), "deepseek");
        assert_eq!(normalize_provider("ollama"), "ollama");
    }

    #[test]
    fn test_model_key() {
        let key = model_key("Claude", "GPT-4");
        assert_eq!(key, "anthropic/gpt-4");
    }

    #[test]
    fn test_model_key_normalizes() {
        let key = model_key("GPT", "gpt-4o");
        assert_eq!(key, "openai/gpt-4o");
    }

    #[test]
    fn test_parse_model_ref_whitespace() {
        let r = parse_model_ref("  anthropic / claude-opus  ", "openai").unwrap();
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.model, "claude-opus");
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_parse_leading_slash() {
        // Leading slash (provider is empty) falls through to default provider
        // The entire string becomes the model name
        let r = parse_model_ref("/gpt-4", "openai").unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "/gpt-4");
    }

    #[test]
    fn test_parse_provider_with_model() {
        let r = parse_model_ref("deepseek/deepseek-chat", "openai").unwrap();
        assert_eq!(r.provider, "deepseek");
        assert_eq!(r.model, "deepseek-chat");
    }

    #[test]
    fn test_parse_provider_normalization() {
        // Provider name should be normalized
        let r = parse_model_ref("Claude/claude-opus", "openai").unwrap();
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.model, "claude-opus");
    }

    #[test]
    fn test_parse_default_provider_applied() {
        let r = parse_model_ref("my-model", "deepseek").unwrap();
        assert_eq!(r.provider, "deepseek");
        assert_eq!(r.model, "my-model");
    }

    #[test]
    fn test_parse_model_with_slashes() {
        // Only the first slash is used to split provider/model
        let r = parse_model_ref("org/model-v2-beta", "openai").unwrap();
        assert_eq!(r.provider, "org");
        assert_eq!(r.model, "model-v2-beta");
    }

    #[test]
    fn test_model_ref_equality() {
        let r1 = ModelRef { provider: "openai".to_string(), model: "gpt-4".to_string() };
        let r2 = ModelRef { provider: "openai".to_string(), model: "gpt-4".to_string() };
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_model_ref_inequality() {
        let r1 = ModelRef { provider: "openai".to_string(), model: "gpt-4".to_string() };
        let r2 = ModelRef { provider: "anthropic".to_string(), model: "gpt-4".to_string() };
        assert_ne!(r1, r2);
    }

    #[test]
    fn test_normalize_provider_all_aliases() {
        assert_eq!(normalize_provider("z.ai"), "zai");
        assert_eq!(normalize_provider("z-ai"), "zai");
        assert_eq!(normalize_provider("opencode-zen"), "opencode");
        assert_eq!(normalize_provider("qwen"), "qwen-portal");
        assert_eq!(normalize_provider("kimi-code"), "kimi-coding");
        assert_eq!(normalize_provider("gpt"), "openai");
        assert_eq!(normalize_provider("claude"), "anthropic");
        assert_eq!(normalize_provider("glm"), "zhipu");
        assert_eq!(normalize_provider("google"), "gemini");
    }

    #[test]
    fn test_normalize_provider_case_insensitive() {
        assert_eq!(normalize_provider("CLAUDE"), "anthropic");
        assert_eq!(normalize_provider("GPT"), "openai");
        assert_eq!(normalize_provider("GLM"), "zhipu");
        assert_eq!(normalize_provider("Z.AI"), "zai");
    }

    #[test]
    fn test_normalize_provider_whitespace() {
        assert_eq!(normalize_provider("  anthropic  "), "anthropic");
    }

    #[test]
    fn test_model_key_different_models() {
        let k1 = model_key("openai", "gpt-4");
        let k2 = model_key("openai", "gpt-3.5");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_model_key_different_providers() {
        let k1 = model_key("openai", "gpt-4");
        let k2 = model_key("anthropic", "gpt-4");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_model_key_case_insensitive() {
        let k1 = model_key("OpenAI", "GPT-4");
        let k2 = model_key("openai", "gpt-4");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_model_ref_serialization() {
        let r = ModelRef { provider: "openai".to_string(), model: "gpt-4".to_string() };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("openai"));
        assert!(json.contains("gpt-4"));
    }

    #[test]
    fn test_model_ref_deserialization() {
        let json = r#"{"provider":"anthropic","model":"claude-3"}"#;
        let r: ModelRef = serde_json::from_str(json).unwrap();
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.model, "claude-3");
    }
}
