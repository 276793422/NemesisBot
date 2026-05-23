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
mod tests;
