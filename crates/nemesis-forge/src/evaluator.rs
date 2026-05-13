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
mod tests {
    use super::*;

    #[test]
    fn test_evaluator_config_default() {
        let config = EvaluatorConfig::default();
        assert_eq!(config.pass_threshold, 60);
    }

    #[test]
    fn test_evaluator_config_serialization() {
        let config = EvaluatorConfig {
            pass_threshold: 80,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: EvaluatorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pass_threshold, 80);
    }

    #[tokio::test]
    async fn test_evaluate_no_provider() {
        let evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;

        assert!(result.passed);
        assert_eq!(result.score, 70);
        assert!(result.details.contains("default"));
        assert_eq!(result.dimensions.len(), 4);
        assert_eq!(result.dimensions.get("correctness"), Some(&70));
        assert_eq!(result.dimensions.get("security"), Some(&75));
    }

    #[tokio::test]
    async fn test_evaluate_no_provider_high_threshold() {
        let config = EvaluatorConfig {
            pass_threshold: 80,
        };
        let evaluator = QualityEvaluator::new(config);
        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;

        // Default score 70 < threshold 80
        assert!(!result.passed);
        assert_eq!(result.score, 70);
    }

    #[tokio::test]
    async fn test_evaluate_with_provider() {
        use async_trait::async_trait;

        struct MockLLM;

        #[async_trait]
        impl LLMCaller for MockLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Ok(r#"{"correctness": 85, "quality": 80, "security": 90, "reusability": 75, "notes": "Good quality artifact"}"#.to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(MockLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "skill content").await;

        assert!(result.passed);
        assert!(result.score > 0);
        assert!(result.details.contains("Good quality"));
        assert_eq!(result.dimensions.len(), 4);
        assert_eq!(result.dimensions.get("correctness"), Some(&85));

        // Verify weighted score: 85*0.4 + 80*0.2 + 90*0.2 + 75*0.2
        // = 34 + 16 + 18 + 15 = 83
        assert_eq!(result.score, 83);
    }

    #[tokio::test]
    async fn test_evaluate_provider_failure() {
        use async_trait::async_trait;

        struct FailLLM;

        #[async_trait]
        impl LLMCaller for FailLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Err("service unavailable".to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(FailLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;

        assert!(!result.passed);
        assert_eq!(result.score, 0);
        assert!(result.details.contains("service unavailable"));
    }

    #[tokio::test]
    async fn test_evaluate_invalid_json_response() {
        use async_trait::async_trait;

        struct GarbageLLM;

        #[async_trait]
        impl LLMCaller for GarbageLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Ok("This is not valid JSON at all".to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(GarbageLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;

        assert!(!result.passed);
        assert_eq!(result.score, 0);
        assert!(result.details.contains("Failed to parse"));
    }

    #[test]
    fn test_evaluate_sync_no_provider() {
        let evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        let result = evaluator.evaluate_sync("skill", "test", "1.0", "content");

        assert!(result.passed);
        assert_eq!(result.score, 70);
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_evaluator_config_default_pass_threshold() {
        assert_eq!(default_pass_threshold(), 60);
    }

    #[test]
    fn test_evaluator_with_forge_config() {
        let config = EvaluatorConfig::default();
        let forge_config = ForgeConfig::default();
        let evaluator = QualityEvaluator::with_forge_config(config, forge_config);
        assert!(evaluator.provider.is_none());
        assert!(evaluator.forge_config.is_some());
    }

    #[test]
    fn test_quality_evaluation_result_serialization() {
        let mut dims = HashMap::new();
        dims.insert("correctness".to_string(), 85);
        dims.insert("quality".to_string(), 80);
        let result = QualityEvaluationResult {
            passed: true,
            score: 83,
            details: "Good quality".to_string(),
            dimensions: dims,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: QualityEvaluationResult = serde_json::from_str(&json).unwrap();
        assert!(back.passed);
        assert_eq!(back.score, 83);
        assert_eq!(back.details, "Good quality");
    }

    #[tokio::test]
    async fn test_evaluate_with_provider_float_scores() {
        use async_trait::async_trait;

        struct FloatLLM;

        #[async_trait]
        impl LLMCaller for FloatLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Ok(r#"{"correctness": 85.7, "quality": 80.3, "security": 90.1, "reusability": 75.9, "notes": "Float scores"}"#.to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(FloatLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;

        assert!(result.passed);
        assert!(result.score > 0);
        // Float values should be truncated to u32
        assert_eq!(result.dimensions.get("correctness"), Some(&85));
    }

    #[tokio::test]
    async fn test_evaluate_with_provider_partial_scores() {
        use async_trait::async_trait;

        struct PartialLLM;

        #[async_trait]
        impl LLMCaller for PartialLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                // Only correctness and quality, missing security and reusability
                Ok(r#"{"correctness": 90, "quality": 80, "notes": "Partial"}"#.to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(PartialLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;

        assert!(!result.passed); // Score 52 < threshold 60
        assert_eq!(result.dimensions.len(), 2);
        // Score: 90*40/100 + 80*20/100 = 36 + 16 = 52
        assert_eq!(result.score, 52);
    }

    #[tokio::test]
    async fn test_evaluate_with_provider_no_notes() {
        use async_trait::async_trait;

        struct NoNotesLLM;

        #[async_trait]
        impl LLMCaller for NoNotesLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Ok(r#"{"correctness": 100, "quality": 100, "security": 100, "reusability": 100}"#.to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(NoNotesLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;

        assert!(result.passed);
        assert_eq!(result.score, 100);
        assert_eq!(result.details, "");
    }

    #[tokio::test]
    async fn test_evaluate_with_provider_below_threshold() {
        use async_trait::async_trait;

        struct LowScoreLLM;

        #[async_trait]
        impl LLMCaller for LowScoreLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Ok(r#"{"correctness": 10, "quality": 10, "security": 10, "reusability": 10, "notes": "Bad"}"#.to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(LowScoreLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;

        assert!(!result.passed);
        // Score: 10*40/100 + 10*20/100 + 10*20/100 + 10*20/100 = 4+2+2+2 = 10
        assert_eq!(result.score, 10);
    }

    #[test]
    fn test_evaluate_sync_with_provider() {
        use async_trait::async_trait;

        struct MockLLM;

        #[async_trait]
        impl LLMCaller for MockLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Ok(r#"{"correctness": 90, "quality": 85, "security": 80, "reusability": 75, "notes": "Sync test"}"#.to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(MockLLM));

        let result = evaluator.evaluate_sync("skill", "test", "1.0", "content");
        assert!(result.passed);
        assert!(result.score > 0);
    }

    #[tokio::test]
    async fn test_evaluate_with_forge_config_max_tokens() {
        use async_trait::async_trait;

        struct TokenCheckLLM {
            max_tokens_received: std::sync::Mutex<Option<i64>>,
        }

        #[async_trait]
        impl LLMCaller for TokenCheckLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                max_tokens: Option<i64>,
            ) -> Result<String, String> {
                *self.max_tokens_received.lock().unwrap() = max_tokens;
                Ok(r#"{"correctness": 80, "quality": 80, "security": 80, "reusability": 80, "notes": "ok"}"#.to_string())
            }
        }

        let mut forge_config = ForgeConfig::default();
        forge_config.validation.llm_max_tokens = 500;

        let config = EvaluatorConfig::default();
        let mut evaluator = QualityEvaluator::with_forge_config(config, forge_config);
        let llm = TokenCheckLLM {
            max_tokens_received: std::sync::Mutex::new(None),
        };
        let tokens_received = llm.max_tokens_received.lock().unwrap().clone();
        evaluator.set_provider(Box::new(llm));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;
        assert!(result.passed);
    }

    #[test]
    fn test_evaluator_config_deserialize_default() {
        let json = "{}";
        let config: EvaluatorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.pass_threshold, 60);
    }

    #[tokio::test]
    async fn test_evaluate_with_provider_empty_notes_field() {
        use async_trait::async_trait;

        struct EmptyNotesLLM;

        #[async_trait]
        impl LLMCaller for EmptyNotesLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Ok(r#"{"correctness": 80, "quality": 80, "security": 80, "reusability": 80, "notes": ""}"#.to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(EmptyNotesLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;
        assert!(result.passed);
        assert_eq!(result.details, "");
    }

    #[tokio::test]
    async fn test_evaluate_with_provider_notes_not_string() {
        use async_trait::async_trait;

        struct NotesNotStringLLM;

        #[async_trait]
        impl LLMCaller for NotesNotStringLLM {
            async fn chat(
                &self,
                _system_prompt: &str,
                _user_prompt: &str,
                _max_tokens: Option<i64>,
            ) -> Result<String, String> {
                Ok(r#"{"correctness": 80, "quality": 80, "security": 80, "reusability": 80, "notes": 42}"#.to_string())
            }
        }

        let mut evaluator = QualityEvaluator::new(EvaluatorConfig::default());
        evaluator.set_provider(Box::new(NotesNotStringLLM));

        let result = evaluator.evaluate("skill", "test", "1.0", "content").await;
        assert!(result.passed);
        // Notes is not a string, so should default to ""
        assert_eq!(result.details, "");
    }
}
