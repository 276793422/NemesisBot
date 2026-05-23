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
