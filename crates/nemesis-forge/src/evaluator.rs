//! Quality evaluator - Stage 3 quality evaluation using LLM-as-Judge.
//!
//! Evaluates forge artifacts on four dimensions:
//! - correctness (40%): Does the content correctly implement its stated purpose?
//! - quality (20%): Code/text quality, clarity, documentation
//! - security (20%): Security considerations, no dangerous patterns
//! - reusability (20%): Can this be reused in other contexts?
//!
//! When no LLM provider is available, returns a default score of 70.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::reflector_llm::LLMCaller;
use crate::config::ForgeConfig;

/// Configuration for the quality evaluator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluatorConfig {
    /// Minimum score (0-100) required to pass evaluation. Default: 60.
    #[serde(default = "default_pass_threshold")]
    pub pass_threshold: u32,
}

impl Default for EvaluatorConfig {
    fn default() -> Self {
        Self {
            pass_threshold: default_pass_threshold(),
        }
    }
}

fn default_pass_threshold() -> u32 {
    60
}

/// Result of a quality evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityEvaluationResult {
    /// Whether the artifact passed the quality threshold.
    pub passed: bool,
    /// Overall quality score (0-100), weighted across dimensions.
    pub score: u32,
    /// Human-readable evaluation details.
    pub details: String,
    /// Per-dimension scores.
    pub dimensions: HashMap<String, u32>,
}

/// Quality evaluator that uses LLM-as-Judge for artifact evaluation.
///
/// Mirrors Go's `QualityEvaluator`. When no LLM provider is configured,
/// returns a default score of 70 across all dimensions.
pub struct QualityEvaluator {
    provider: Option<Box<dyn LLMCaller>>,
    config: EvaluatorConfig,
    /// Optional forge config for additional settings (LLM max tokens).
    forge_config: Option<ForgeConfig>,
}

impl QualityEvaluator {
    /// Create a new evaluator with the given configuration.
    pub fn new(config: EvaluatorConfig) -> Self {
        Self {
            provider: None,
            config,
            forge_config: None,
        }
    }

    /// Create a new evaluator with forge config for additional settings.
    pub fn with_forge_config(config: EvaluatorConfig, forge_config: ForgeConfig) -> Self {
        Self {
            provider: None,
            config,
            forge_config: Some(forge_config),
        }
    }

    /// Set the LLM provider for evaluation.
    pub fn set_provider(&mut self, provider: Box<dyn LLMCaller>) {
        self.provider = Some(provider);
    }

    /// Evaluate an artifact's quality.
    ///
    /// When a provider is available, calls the LLM to score the artifact on
    /// four dimensions and computes a weighted score. Without a provider,
    /// returns a default score of 70.
    pub async fn evaluate(
        &self,
        kind: &str,
        name: &str,
        version: &str,
        content: &str,
    ) -> QualityEvaluationResult {
        match self.provider.as_ref() {
            Some(provider) => {
                let max_tokens = self.forge_config
                    .as_ref()
                    .map(|c| c.validation.llm_max_tokens as i64)
                    .unwrap_or(2000);

                let prompt = format!(
                    "Evaluate the following Forge artifact for quality.\n\n\
                    Type: {}\n\
                    Name: {}\n\
                    Version: {}\n\
                    \n\
                    Content:\n\
                    {}\n\
                    \n\
                    Score each dimension from 0-100:\n\
                    - correctness: Does the content correctly implement its stated purpose? (weight 40%%)\n\
                    - quality: Code/text quality, clarity, documentation (weight 20%%)\n\
                    - security: Security considerations, no dangerous patterns (weight 20%%)\n\
                    - reusability: Can this be reused in other contexts? (weight 20%%)\n\
                    \n\
                    Respond with ONLY a JSON object:\n\
                    {{\"correctness\": N, \"quality\": N, \"security\": N, \"reusability\": N, \"notes\": \"brief explanation\"}}",
                    kind, name, version, content
                );

                let system_prompt = "You are a code quality evaluator. Respond only with valid JSON.";

                match provider.chat(system_prompt, &prompt, Some(max_tokens)).await {
                    Ok(response) => {
                        match crate::reflector_llm::extract_json(&response) {
                            Some(parsed) => {
                                let mut dimensions = HashMap::new();

                                // Extract dimension scores
                                for key in &["correctness", "quality", "security", "reusability"] {
                                    if let Some(val) = parsed.get(*key) {
                                        if let Some(n) = val.as_u64() {
                                            dimensions.insert(key.to_string(), n as u32);
                                        } else if let Some(n) = val.as_f64() {
                                            dimensions.insert(key.to_string(), n as u32);
                                        }
                                    }
                                }

                                // Extract notes
                                let notes = parsed
                                    .get("notes")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                // Calculate weighted score:
                                // correctness(40%) + quality(20%) + security(20%) + reusability(20%)
                                let mut score: u32 = 0;
                                if let Some(&c) = dimensions.get("correctness") {
                                    score += c * 40 / 100;
                                }
                                if let Some(&q) = dimensions.get("quality") {
                                    score += q * 20 / 100;
                                }
                                if let Some(&s) = dimensions.get("security") {
                                    score += s * 20 / 100;
                                }
                                if let Some(&r) = dimensions.get("reusability") {
                                    score += r * 20 / 100;
                                }

                                let passed = score >= self.config.pass_threshold;

                                QualityEvaluationResult {
                                    passed,
                                    score,
                                    details: notes,
                                    dimensions,
                                }
                            }
                            None => {
                                // Could not parse JSON from LLM response
                                QualityEvaluationResult {
                                    passed: false,
                                    score: 0,
                                    details: "Failed to parse LLM response as JSON".to_string(),
                                    dimensions: HashMap::new(),
                                }
                            }
                        }
                    }
                    Err(e) => {
                        QualityEvaluationResult {
                            passed: false,
                            score: 0,
                            details: format!("LLM call failed: {}", e),
                            dimensions: HashMap::new(),
                        }
                    }
                }
            }
            None => {
                // No LLM provider - use default score (matches Go behavior)
                let mut dimensions = HashMap::new();
                dimensions.insert("correctness".to_string(), 70);
                dimensions.insert("quality".to_string(), 70);
                dimensions.insert("security".to_string(), 75);
                dimensions.insert("reusability".to_string(), 65);

                QualityEvaluationResult {
                    passed: 70 >= self.config.pass_threshold,
                    score: 70,
                    details: "No LLM Provider available, using default score".to_string(),
                    dimensions,
                }
            }
        }
    }

    /// Synchronous wrapper for evaluate that blocks on the async LLM call.
    pub fn evaluate_sync(
        &self,
        kind: &str,
        name: &str,
        version: &str,
        content: &str,
    ) -> QualityEvaluationResult {
        let future = self.evaluate(kind, name, version, content);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            Err(_) => {
                let rt = tokio::runtime::Runtime::new()
                    .expect("Failed to create tokio runtime for quality evaluation");
                rt.block_on(future)
            }
        }
    }
}

#[cfg(test)]
mod tests;
