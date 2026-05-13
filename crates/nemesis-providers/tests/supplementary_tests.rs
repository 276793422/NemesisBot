//! Supplementary tests for nemesis-providers crate.
//!
//! Covers types, model_ref, failover, cooldown, error_classifier, factory,
//! router, fallback_provider, tool_call_extract, and openai_compat modules.

// ---------------------------------------------------------------------------
// types.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod types_extra {
    use nemesis_providers::types::*;
    use std::collections::HashMap;

    #[test]
    fn test_message_role_system() {
        let msg = Message {
            role: "system".to_string(),
            content: "You are helpful".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"system\""));
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "system");
    }

    #[test]
    fn test_message_role_user() {
        let msg = Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "user");
    }

    #[test]
    fn test_message_role_assistant() {
        let msg = Message {
            role: "assistant".to_string(),
            content: "Hi there".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "assistant");
    }

    #[test]
    fn test_message_role_tool() {
        let msg = Message {
            role: "tool".to_string(),
            content: "result data".to_string(),
            tool_calls: vec![],
            tool_call_id: Some("call_1".to_string()),
            timestamp: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "tool");
        assert_eq!(back.tool_call_id, Some("call_1".to_string()));
    }

    #[test]
    fn test_tool_call_minimal_fields() {
        let tc = ToolCall {
            id: "c1".to_string(),
            call_type: None,
            function: None,
            name: None,
            arguments: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "c1");
        assert!(back.call_type.is_none());
        assert!(back.function.is_none());
    }

    #[test]
    fn test_function_call_arguments_json() {
        let fc = FunctionCall {
            name: "exec".to_string(),
            arguments: r#"{"cmd":"ls","args":["-la"]}"#.to_string(),
        };
        let json = serde_json::to_string(&fc).unwrap();
        let back: FunctionCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "exec");
        assert!(back.arguments.contains("ls"));
    }

    #[test]
    fn test_llm_response_stop_reason() {
        let resp = LLMResponse {
            content: "done".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
        };
        assert_eq!(resp.finish_reason, "stop");
    }

    #[test]
    fn test_llm_response_tool_calls_reason() {
        let resp = LLMResponse {
            content: "".to_string(),
            tool_calls: vec![ToolCall {
                id: "c1".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "test".to_string(),
                    arguments: "{}".to_string(),
                }),
                name: None,
                arguments: None,
            }],
            finish_reason: "tool_calls".to_string(),
            usage: None,
        };
        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.tool_calls.len(), 1);
    }

    #[test]
    fn test_usage_info_total_calculation() {
        let usage = UsageInfo {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        assert_eq!(usage.prompt_tokens + usage.completion_tokens, usage.total_tokens);
    }

    #[test]
    fn test_usage_info_zero() {
        let usage = UsageInfo {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        };
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn test_tool_definition_type_default() {
        let json = r#"{"function":{"name":"t","description":"d","parameters":{}}}"#;
        let td: ToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(td.tool_type, "function");
    }

    #[test]
    fn test_tool_function_definition_params() {
        let tfd = ToolFunctionDefinition {
            name: "search".to_string(),
            description: "Search files".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "max_results": {"type": "integer"}
                },
                "required": ["query"]
            }),
        };
        let json = serde_json::to_string(&tfd).unwrap();
        let back: ToolFunctionDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "search");
    }

    #[test]
    fn test_chat_options_all_fields() {
        let mut extra = HashMap::new();
        extra.insert("stream".to_string(), serde_json::json!(true));
        let opts = ChatOptions {
            temperature: Some(0.5),
            max_tokens: Some(2048),
            top_p: Some(0.95),
            stop: Some(vec!["STOP".to_string(), "END".to_string()]),
            extra,
        };
        assert_eq!(opts.temperature.unwrap(), 0.5);
        assert_eq!(opts.max_tokens.unwrap(), 2048);
        assert_eq!(opts.top_p.unwrap(), 0.95);
        assert_eq!(opts.stop.unwrap().len(), 2);
        assert_eq!(opts.extra.len(), 1);
    }

    #[test]
    fn test_provider_model_config_empty_primary() {
        let cfg = ProviderModelConfig::new("");
        assert_eq!(cfg.primary, "");
        assert!(!cfg.has_fallbacks());
    }

    #[test]
    fn test_provider_model_config_many_fallbacks() {
        let cfg = ProviderModelConfig::with_fallbacks("p", &["a", "b", "c", "d", "e"]);
        assert!(cfg.has_fallbacks());
        assert_eq!(cfg.all_models().len(), 6);
        assert_eq!(cfg.all_models()[0], "p");
    }

    #[test]
    fn test_token_source_type_variants() {
        let ts = TokenSourceType::Static;
        let json = serde_json::to_string(&ts).unwrap();
        let back: TokenSourceType = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, TokenSourceType::Static));

        let ts = TokenSourceType::OAuth;
        let json = serde_json::to_string(&ts).unwrap();
        let back: TokenSourceType = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, TokenSourceType::OAuth));

        let ts = TokenSourceType::CliCredentials;
        let json = serde_json::to_string(&ts).unwrap();
        let back: TokenSourceType = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, TokenSourceType::CliCredentials));
    }

    #[test]
    fn test_llm_response_empty_tool_calls_not_serialized() {
        let resp = LLMResponse {
            content: "test".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("tool_calls"));
    }

    #[test]
    fn test_llm_response_with_usage_serialized() {
        let resp = LLMResponse {
            content: "test".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: Some(UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("usage"));
        assert!(json.contains("prompt_tokens"));
    }
}

// ---------------------------------------------------------------------------
// model_ref.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod model_ref_extra {
    use nemesis_providers::model_ref::*;

    #[test]
    fn test_parse_with_provider_prefix() {
        let r = parse_model_ref("deepseek/deepseek-chat", "openai").unwrap();
        assert_eq!(r.provider, "deepseek");
        assert_eq!(r.model, "deepseek-chat");
    }

    #[test]
    fn test_parse_provider_normalization_gpt() {
        let r = parse_model_ref("gpt/gpt-4o", "openai").unwrap();
        assert_eq!(r.provider, "openai");
        assert_eq!(r.model, "gpt-4o");
    }

    #[test]
    fn test_parse_provider_normalization_claude() {
        let r = parse_model_ref("claude/claude-opus", "openai").unwrap();
        assert_eq!(r.provider, "anthropic");
        assert_eq!(r.model, "claude-opus");
    }

    #[test]
    fn test_parse_provider_normalization_glm() {
        let r = parse_model_ref("glm/glm-4", "openai").unwrap();
        assert_eq!(r.provider, "zhipu");
        assert_eq!(r.model, "glm-4");
    }

    #[test]
    fn test_parse_provider_normalization_google() {
        let r = parse_model_ref("google/gemini-pro", "openai").unwrap();
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.model, "gemini-pro");
    }

    #[test]
    fn test_parse_provider_normalization_qwen() {
        let r = parse_model_ref("qwen/qwen-max", "openai").unwrap();
        assert_eq!(r.provider, "qwen-portal");
        assert_eq!(r.model, "qwen-max");
    }

    #[test]
    fn test_parse_provider_normalization_kimi_code() {
        let r = parse_model_ref("kimi-code/kimi-k2", "openai").unwrap();
        assert_eq!(r.provider, "kimi-coding");
        assert_eq!(r.model, "kimi-k2");
    }

    #[test]
    fn test_parse_provider_normalization_z_ai() {
        let r = parse_model_ref("z.ai/model-v1", "openai").unwrap();
        assert_eq!(r.provider, "zai");
        assert_eq!(r.model, "model-v1");
    }

    #[test]
    fn test_parse_provider_normalization_z_ai_dash() {
        let r = parse_model_ref("z-ai/model-v1", "openai").unwrap();
        assert_eq!(r.provider, "zai");
        assert_eq!(r.model, "model-v1");
    }

    #[test]
    fn test_parse_provider_normalization_opencode_zen() {
        let r = parse_model_ref("opencode-zen/model-v1", "openai").unwrap();
        assert_eq!(r.provider, "opencode");
        assert_eq!(r.model, "model-v1");
    }

    #[test]
    fn test_normalize_provider_passthrough_unknown() {
        assert_eq!(normalize_provider("mistral"), "mistral");
        assert_eq!(normalize_provider("cohere"), "cohere");
        assert_eq!(normalize_provider("perplexity"), "perplexity");
    }

    #[test]
    fn test_model_key_format() {
        let key = model_key("deepseek", "chat-v3");
        assert_eq!(key, "deepseek/chat-v3");
    }

    #[test]
    fn test_model_key_deduplication() {
        let k1 = model_key("DeepSeek", "Chat-V3");
        let k2 = model_key("deepseek", "chat-v3");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_model_ref_clone() {
        let r1 = ModelRef { provider: "openai".to_string(), model: "gpt-4".to_string() };
        let r2 = r1.clone();
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_model_ref_serde_roundtrip() {
        let r = ModelRef { provider: "anthropic".to_string(), model: "claude-3-opus".to_string() };
        let json = serde_json::to_string(&r).unwrap();
        let back: ModelRef = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn test_parse_model_ref_only_spaces() {
        assert!(parse_model_ref("   ", "openai").is_none());
    }

    #[test]
    fn test_parse_model_ref_slash_only() {
        assert!(parse_model_ref("/", "openai").is_some());
    }
}

// ---------------------------------------------------------------------------
// failover.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod failover_extra {
    use nemesis_providers::failover::*;

    #[test]
    fn test_auth_error_display() {
        let err = FailoverError::Auth {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            status: 401,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("openai"));
        assert!(msg.contains("gpt-4"));
        assert!(msg.contains("401"));
    }

    #[test]
    fn test_rate_limit_error_display() {
        let err = FailoverError::RateLimit {
            provider: "anthropic".to_string(),
            model: "claude-3".to_string(),
            retry_after: Some(120),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("anthropic"));
        assert!(msg.contains("claude-3"));
    }

    #[test]
    fn test_billing_error_display() {
        let err = FailoverError::Billing { provider: "openai".to_string() };
        let msg = format!("{}", err);
        assert!(msg.contains("openai"));
    }

    #[test]
    fn test_timeout_error_display() {
        let err = FailoverError::Timeout {
            provider: "deepseek".to_string(),
            model: "chat".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("deepseek"));
        assert!(msg.contains("chat"));
    }

    #[test]
    fn test_format_error_display() {
        let err = FailoverError::Format {
            provider: "openai".to_string(),
            message: "bad json".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("openai"));
        assert!(msg.contains("bad json"));
    }

    #[test]
    fn test_overloaded_error_display() {
        let err = FailoverError::Overloaded { provider: "anthropic".to_string() };
        let msg = format!("{}", err);
        assert!(msg.contains("anthropic"));
    }

    #[test]
    fn test_unknown_error_display() {
        let err = FailoverError::Unknown {
            provider: "test".to_string(),
            message: "something broke".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("test"));
        assert!(msg.contains("something broke"));
    }

    #[test]
    fn test_from_status_all_codes() {
        // Auth codes
        let auth401 = FailoverError::from_status("p", "m", 401, "");
        assert!(matches!(auth401, FailoverError::Auth { .. }));
        let auth403 = FailoverError::from_status("p", "m", 403, "");
        assert!(matches!(auth403, FailoverError::Auth { .. }));

        // Rate limit
        let rl = FailoverError::from_status("p", "m", 429, "");
        assert!(matches!(rl, FailoverError::RateLimit { .. }));

        // Billing
        let bill = FailoverError::from_status("p", "m", 402, "");
        assert!(matches!(bill, FailoverError::Billing { .. }));

        // Overloaded
        let ol502 = FailoverError::from_status("p", "m", 502, "");
        assert!(matches!(ol502, FailoverError::Overloaded { .. }));
        let ol503 = FailoverError::from_status("p", "m", 503, "");
        assert!(matches!(ol503, FailoverError::Overloaded { .. }));

        // Unknown
        let unk400 = FailoverError::from_status("p", "m", 400, "bad");
        assert!(matches!(unk400, FailoverError::Unknown { .. }));
        let unk500 = FailoverError::from_status("p", "m", 500, "internal");
        assert!(matches!(unk500, FailoverError::Unknown { .. }));
    }

    #[test]
    fn test_from_status_truncates_long_body() {
        let long_body = "x".repeat(500);
        let err = FailoverError::from_status("p", "m", 400, &long_body);
        if let FailoverError::Unknown { message, .. } = err {
            assert!(message.len() < long_body.len());
        } else {
            panic!("expected Unknown variant");
        }
    }

    #[test]
    fn test_is_retriable_all_variants() {
        assert!(FailoverError::RateLimit { provider: "p".into(), model: "m".into(), retry_after: None }.is_retriable());
        assert!(FailoverError::Timeout { provider: "p".into(), model: "m".into() }.is_retriable());
        assert!(FailoverError::Overloaded { provider: "p".into() }.is_retriable());
        assert!(!FailoverError::Auth { provider: "p".into(), model: "m".into(), status: 401 }.is_retriable());
        assert!(!FailoverError::Billing { provider: "p".into() }.is_retriable());
        assert!(!FailoverError::Format { provider: "p".into(), message: "m".into() }.is_retriable());
        assert!(!FailoverError::Unknown { provider: "p".into(), message: "m".into() }.is_retriable());
    }

    #[test]
    fn test_reason_all_variants() {
        assert_eq!(FailoverError::Auth { provider: "p".into(), model: "m".into(), status: 0 }.reason(), FailoverReason::Auth);
        assert_eq!(FailoverError::RateLimit { provider: "p".into(), model: "m".into(), retry_after: None }.reason(), FailoverReason::RateLimit);
        assert_eq!(FailoverError::Billing { provider: "p".into() }.reason(), FailoverReason::Billing);
        assert_eq!(FailoverError::Timeout { provider: "p".into(), model: "m".into() }.reason(), FailoverReason::Timeout);
        assert_eq!(FailoverError::Format { provider: "p".into(), message: "m".into() }.reason(), FailoverReason::Format);
        assert_eq!(FailoverError::Overloaded { provider: "p".into() }.reason(), FailoverReason::Overloaded);
        assert_eq!(FailoverError::Unknown { provider: "p".into(), message: "m".into() }.reason(), FailoverReason::Unknown);
    }

    #[test]
    fn test_failover_reason_copy() {
        let r1 = FailoverReason::Auth;
        let r2 = r1;
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_failover_reason_equality() {
        assert_eq!(FailoverReason::Auth, FailoverReason::Auth);
        assert_ne!(FailoverReason::Auth, FailoverReason::RateLimit);
    }
}

// ---------------------------------------------------------------------------
// cooldown.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod cooldown_extra {
    use nemesis_providers::cooldown::*;
    use nemesis_providers::failover::FailoverReason;
    use std::time::Duration;

    #[test]
    fn test_calculate_standard_cooldown_values() {
        assert_eq!(calculate_standard_cooldown(1), Duration::from_secs(60));
        assert_eq!(calculate_standard_cooldown(2), Duration::from_secs(300));
        assert_eq!(calculate_standard_cooldown(3), Duration::from_secs(1500));
        assert_eq!(calculate_standard_cooldown(4), Duration::from_secs(3600));
    }

    #[test]
    fn test_calculate_billing_cooldown_values() {
        assert_eq!(calculate_billing_cooldown(1), Duration::from_secs(5 * 3600));
        assert_eq!(calculate_billing_cooldown(2), Duration::from_secs(10 * 3600));
        assert_eq!(calculate_billing_cooldown(3), Duration::from_secs(20 * 3600));
    }

    #[test]
    fn test_tracker_new_is_available() {
        let t = CooldownTracker::new();
        assert!(t.is_available("any-provider"));
    }

    #[test]
    fn test_tracker_default_is_available() {
        let t = CooldownTracker::default();
        assert!(t.is_available("any-provider"));
    }

    #[test]
    fn test_tracker_mark_failure_then_unavailable() {
        let t = CooldownTracker::new();
        t.mark_failure("p1", FailoverReason::RateLimit);
        assert!(!t.is_available("p1"));
    }

    #[test]
    fn test_tracker_mark_success_resets_all() {
        let t = CooldownTracker::new();
        t.mark_failure("p1", FailoverReason::RateLimit);
        t.mark_failure("p1", FailoverReason::Timeout);
        assert_eq!(t.error_count("p1"), 2);
        t.mark_success("p1");
        assert_eq!(t.error_count("p1"), 0);
        assert!(t.is_available("p1"));
    }

    #[test]
    fn test_tracker_failure_count_by_reason_multiple() {
        let t = CooldownTracker::new();
        t.mark_failure("p1", FailoverReason::RateLimit);
        t.mark_failure("p1", FailoverReason::RateLimit);
        t.mark_failure("p1", FailoverReason::Timeout);
        assert_eq!(t.failure_count("p1", FailoverReason::RateLimit), 2);
        assert_eq!(t.failure_count("p1", FailoverReason::Timeout), 1);
        assert_eq!(t.failure_count("p1", FailoverReason::Auth), 0);
    }

    #[test]
    fn test_tracker_independent_providers() {
        let t = CooldownTracker::new();
        t.mark_failure("p1", FailoverReason::RateLimit);
        assert!(!t.is_available("p1"));
        assert!(t.is_available("p2"));
    }

    #[test]
    fn test_tracker_billing_disable() {
        let t = CooldownTracker::new();
        t.mark_failure("p1", FailoverReason::Billing);
        assert!(!t.is_available("p1"));
    }

    #[test]
    fn test_tracker_cooldown_remaining_after_failure() {
        let t = CooldownTracker::new();
        t.mark_failure("p1", FailoverReason::RateLimit);
        let rem = t.cooldown_remaining("p1");
        assert!(rem.is_some());
        let dur = rem.unwrap();
        assert!(dur.as_secs() > 0);
        assert!(dur.as_secs() <= 60);
    }

    #[test]
    fn test_tracker_cooldown_remaining_no_failure() {
        let t = CooldownTracker::new();
        assert!(t.cooldown_remaining("unknown").is_none());
    }

    #[test]
    fn test_tracker_cooldown_remaining_after_success() {
        let t = CooldownTracker::new();
        t.mark_failure("p1", FailoverReason::RateLimit);
        t.mark_success("p1");
        assert!(t.cooldown_remaining("p1").is_none());
    }

    #[test]
    fn test_tracker_error_count_unknown() {
        let t = CooldownTracker::new();
        assert_eq!(t.error_count("unknown"), 0);
    }

    #[test]
    fn test_tracker_failure_count_unknown() {
        let t = CooldownTracker::new();
        assert_eq!(t.failure_count("unknown", FailoverReason::Auth), 0);
    }

    #[test]
    fn test_calculate_standard_cooldown_cap() {
        assert_eq!(calculate_standard_cooldown(100), Duration::from_secs(3600));
    }

    #[test]
    fn test_calculate_billing_cooldown_cap() {
        assert_eq!(calculate_billing_cooldown(100), Duration::from_secs(24 * 3600));
    }

    #[test]
    fn test_calculate_billing_cooldown_zero() {
        assert_eq!(calculate_billing_cooldown(0), Duration::from_secs(5 * 3600));
    }
}

// ---------------------------------------------------------------------------
// error_classifier.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod error_classifier_extra {
    use nemesis_providers::error_classifier::*;
    use nemesis_providers::failover::{FailoverError, FailoverReason};

    #[test]
    fn test_classify_rate_limit_rate_limit() {
        let err = classify_error("rate limit exceeded", "p", "m");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_rate_limit_resource_exhausted() {
        let err = classify_error("resource has been exhausted", "p", "m");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_rate_limit_quota_exceeded() {
        let err = classify_error("quota exceeded", "p", "m");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_overloaded() {
        let err = classify_error("overloaded_error", "p", "m");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_timeout() {
        let err = classify_error("request timeout", "p", "m");
        assert!(matches!(err, Some(FailoverError::Timeout { .. })));
    }

    #[test]
    fn test_classify_deadline_exceeded() {
        let err = classify_error("context deadline exceeded", "p", "m");
        assert!(matches!(err, Some(FailoverError::Timeout { .. })));
    }

    #[test]
    fn test_classify_billing_payment_required() {
        let err = classify_error("payment required", "p", "m");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_billing_insufficient_credits() {
        let err = classify_error("insufficient credits", "p", "m");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_billing_insufficient_balance() {
        let err = classify_error("insufficient balance", "p", "m");
        assert!(matches!(err, Some(FailoverError::Billing { .. })));
    }

    #[test]
    fn test_classify_auth_incorrect_api_key() {
        let err = classify_error("incorrect api key", "p", "m");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_unauthorized() {
        let err = classify_error("unauthorized", "p", "m");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_forbidden() {
        let err = classify_error("forbidden", "p", "m");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_auth_expired() {
        let err = classify_error("token has expired", "p", "m");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_format_invalid_request() {
        let err = classify_error("invalid request format", "p", "m");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_format_tool_use_id() {
        let err = classify_error("tool_use.id is invalid", "p", "m");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_image_dimension_error() {
        let err = classify_error("image dimensions exceed max 8000px", "p", "m");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_image_size_error() {
        let err = classify_error("image exceeds 20mb", "p", "m");
        assert!(matches!(err, Some(FailoverError::Format { .. })));
    }

    #[test]
    fn test_classify_unknown_returns_none() {
        let err = classify_error("a completely unrelated message", "p", "m");
        assert!(err.is_none());
    }

    #[test]
    fn test_classify_reason_rate_limit() {
        assert_eq!(classify_reason("rate limit"), Some(FailoverReason::RateLimit));
    }

    #[test]
    fn test_classify_reason_timeout() {
        assert_eq!(classify_reason("timeout"), Some(FailoverReason::Timeout));
    }

    #[test]
    fn test_classify_reason_billing() {
        assert_eq!(classify_reason("insufficient credits"), Some(FailoverReason::Billing));
    }

    #[test]
    fn test_classify_reason_auth() {
        assert_eq!(classify_reason("invalid token"), Some(FailoverReason::Auth));
    }

    #[test]
    fn test_classify_reason_format() {
        assert_eq!(classify_reason("invalid request format"), Some(FailoverReason::Format));
    }

    #[test]
    fn test_classify_reason_overloaded_as_rate_limit() {
        assert_eq!(classify_reason("overloaded"), Some(FailoverReason::RateLimit));
    }

    #[test]
    fn test_classify_reason_unknown() {
        assert_eq!(classify_reason("unrelated"), None);
    }

    #[test]
    fn test_classify_http_status_401() {
        let err = classify_error("status: 401 unauthorized", "p", "m");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_http_status_403() {
        let err = classify_error("status: 403 forbidden", "p", "m");
        assert!(matches!(err, Some(FailoverError::Auth { .. })));
    }

    #[test]
    fn test_classify_http_status_429() {
        let err = classify_error("status: 429 too many requests", "p", "m");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_http_status_500() {
        let err = classify_error("status: 500 internal server error", "p", "m");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_http_status_502() {
        let err = classify_error("status: 502 Bad Gateway", "p", "m");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_classify_http_status_503() {
        let err = classify_error("status: 503 Service Unavailable", "p", "m");
        assert!(matches!(err, Some(FailoverError::Overloaded { .. })));
    }

    #[test]
    fn test_is_image_dimension_error_true() {
        assert!(is_image_dimension_error("image dimensions exceed max 8000px"));
    }

    #[test]
    fn test_is_image_dimension_error_false() {
        assert!(!is_image_dimension_error("file not found"));
    }

    #[test]
    fn test_is_image_size_error_true() {
        assert!(is_image_size_error("image exceeds 20mb"));
    }

    #[test]
    fn test_is_image_size_error_false() {
        assert!(!is_image_size_error("image too small"));
    }

    #[test]
    fn test_classify_case_insensitive() {
        let err = classify_error("RATE LIMIT EXCEEDED", "p", "m");
        assert!(matches!(err, Some(FailoverError::RateLimit { .. })));
    }

    #[test]
    fn test_classify_mixed_case_timeout() {
        let err = classify_error("Request Timeout", "p", "m");
        assert!(matches!(err, Some(FailoverError::Timeout { .. })));
    }
}

// ---------------------------------------------------------------------------
// factory.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod factory_extra {
    use nemesis_providers::factory::*;

    #[test]
    fn test_factory_config_default_values() {
        let cfg = FactoryConfig::default();
        assert!(cfg.llm_ref.is_empty());
        assert!(cfg.api_key.is_empty());
        assert!(cfg.api_base.is_empty());
        assert!(cfg.workspace.is_empty());
        assert!(cfg.connect_mode.is_empty());
        assert!(cfg.account_id.is_empty());
        assert!(cfg.headers.is_empty());
    }

    #[test]
    fn test_resolve_anthropic_default_base() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude-sonnet".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.api_base, "https://api.anthropic.com");
        assert_eq!(sel.provider_type, ProviderType::Anthropic);
    }

    #[test]
    fn test_resolve_anthropic_custom_base() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude-sonnet".to_string(),
            api_key: "key".to_string(),
            api_base: "https://proxy.example.com".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.api_base, "https://proxy.example.com");
    }

    #[test]
    fn test_resolve_openai_default_base() {
        let cfg = FactoryConfig {
            llm_ref: "openai/gpt-4o".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.api_base, "https://chatgpt.com/backend-api/codex");
    }

    #[test]
    fn test_resolve_claude_cli_aliases() {
        for alias in &["claude-cli", "claude-code", "claudecodec"] {
            let cfg = FactoryConfig {
                llm_ref: format!("{}/model", alias),
                ..Default::default()
            };
            let sel = resolve_provider_selection(&cfg).unwrap();
            assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
        }
    }

    #[test]
    fn test_resolve_codex_cli_aliases() {
        for alias in &["codex-cli", "codex-code"] {
            let cfg = FactoryConfig {
                llm_ref: format!("{}/model", alias),
                ..Default::default()
            };
            let sel = resolve_provider_selection(&cfg).unwrap();
            assert_eq!(sel.provider_type, ProviderType::CodexCli);
        }
    }

    #[test]
    fn test_resolve_copilot_aliases() {
        for alias in &["copilot", "github_copilot"] {
            let cfg = FactoryConfig {
                llm_ref: format!("{}/gpt-4", alias),
                ..Default::default()
            };
            let sel = resolve_provider_selection(&cfg).unwrap();
            assert_eq!(sel.provider_type, ProviderType::GitHubCopilot);
        }
    }

    #[test]
    fn test_resolve_http_no_key_error() {
        let cfg = FactoryConfig {
            llm_ref: "deepseek/chat".to_string(),
            ..Default::default()
        };
        let result = resolve_provider_selection(&cfg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no API key"));
    }

    #[test]
    fn test_resolve_empty_ref_error() {
        let cfg = FactoryConfig {
            llm_ref: "".to_string(),
            ..Default::default()
        };
        assert!(resolve_provider_selection(&cfg).is_err());
    }

    #[test]
    fn test_resolve_workspace_default() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.workspace, ".");
    }

    #[test]
    fn test_resolve_workspace_custom() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude".to_string(),
            api_key: "key".to_string(),
            workspace: "/custom/path".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.workspace, "/custom/path");
    }

    #[test]
    fn test_provider_type_variants() {
        assert_eq!(ProviderType::HttpCompat, ProviderType::HttpCompat);
        assert_eq!(ProviderType::Anthropic, ProviderType::Anthropic);
        assert_eq!(ProviderType::Codex, ProviderType::Codex);
        assert_eq!(ProviderType::ClaudeCli, ProviderType::ClaudeCli);
        assert_eq!(ProviderType::CodexCli, ProviderType::CodexCli);
        assert_eq!(ProviderType::GitHubCopilot, ProviderType::GitHubCopilot);
        assert_ne!(ProviderType::HttpCompat, ProviderType::Anthropic);
    }

    #[test]
    fn test_create_provider_anthropic() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude-sonnet".to_string(),
            api_key: "test-key".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn test_create_provider_http() {
        let cfg = FactoryConfig {
            llm_ref: "deepseek/chat".to_string(),
            api_key: "test-key".to_string(),
            api_base: "https://api.deepseek.com/v1".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "http-compat");
    }

    #[test]
    fn test_create_provider_claude_cli() {
        let cfg = FactoryConfig {
            llm_ref: "claude-cli/claude-code".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "claude-cli");
    }

    #[test]
    fn test_create_provider_codex_cli() {
        let cfg = FactoryConfig {
            llm_ref: "codex-cli/default".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "codex-cli");
    }

    #[test]
    fn test_create_provider_copilot() {
        let cfg = FactoryConfig {
            llm_ref: "copilot/gpt-4.1".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "github-copilot");
    }

    #[test]
    fn test_create_provider_codex() {
        let cfg = FactoryConfig {
            llm_ref: "openai/gpt-4o".to_string(),
            api_key: "test-key".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "codex");
    }
}

// ---------------------------------------------------------------------------
// router.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod router_extra {
    use nemesis_providers::router::*;
    use std::collections::HashMap;

    #[test]
    fn test_default_aliases_contents() {
        let aliases = default_aliases();
        assert_eq!(aliases.get("fast").unwrap(), "groq/llama-3.3-70b-versatile");
        assert_eq!(aliases.get("smart").unwrap(), "anthropic/claude-sonnet-4-20250514");
        assert_eq!(aliases.get("cheap").unwrap(), "deepseek/deepseek-chat");
        assert_eq!(aliases.get("local").unwrap(), "ollama/llama3.3");
        assert_eq!(aliases.get("reasoning").unwrap(), "openai/o3-mini");
        assert_eq!(aliases.get("code").unwrap(), "anthropic/claude-sonnet-4-20250514");
    }

    #[test]
    fn test_resolve_alias_found() {
        let aliases = default_aliases();
        assert_eq!(resolve_alias(&aliases, "fast"), Some("groq/llama-3.3-70b-versatile".to_string()));
    }

    #[test]
    fn test_resolve_alias_not_found() {
        let aliases = default_aliases();
        assert_eq!(resolve_alias(&aliases, "gpt-4"), None);
    }

    #[test]
    fn test_merge_aliases_preserves_defaults() {
        let defaults = default_aliases();
        let mut custom = HashMap::new();
        custom.insert("custom".to_string(), "custom/model".to_string());
        let merged = merge_aliases(&defaults, &custom);
        assert!(merged.contains_key("fast"));
        assert!(merged.contains_key("custom"));
        assert_eq!(merged.len(), defaults.len() + 1);
    }

    #[test]
    fn test_merge_aliases_custom_overrides() {
        let defaults = default_aliases();
        let mut custom = HashMap::new();
        custom.insert("fast".to_string(), "new/fast-model".to_string());
        let merged = merge_aliases(&defaults, &custom);
        assert_eq!(merged.get("fast").unwrap(), "new/fast-model");
    }

    #[test]
    fn test_merge_aliases_empty_custom() {
        let defaults = default_aliases();
        let custom = HashMap::new();
        let merged = merge_aliases(&defaults, &custom);
        assert_eq!(merged.len(), defaults.len());
    }

    #[test]
    fn test_policy_default() {
        assert_eq!(Policy::default(), Policy::Fallback);
    }

    #[test]
    fn test_policy_serialization_all() {
        assert_eq!(serde_json::to_string(&Policy::Cost).unwrap(), "\"cost\"");
        assert_eq!(serde_json::to_string(&Policy::Quality).unwrap(), "\"quality\"");
        assert_eq!(serde_json::to_string(&Policy::Latency).unwrap(), "\"latency\"");
        assert_eq!(serde_json::to_string(&Policy::RoundRobin).unwrap(), "\"round_robin\"");
        assert_eq!(serde_json::to_string(&Policy::Fallback).unwrap(), "\"fallback\"");
    }

    #[test]
    fn test_policy_deserialization_all() {
        assert_eq!(serde_json::from_str::<Policy>("\"cost\"").unwrap(), Policy::Cost);
        assert_eq!(serde_json::from_str::<Policy>("\"quality\"").unwrap(), Policy::Quality);
        assert_eq!(serde_json::from_str::<Policy>("\"latency\"").unwrap(), Policy::Latency);
        assert_eq!(serde_json::from_str::<Policy>("\"round_robin\"").unwrap(), Policy::RoundRobin);
        assert_eq!(serde_json::from_str::<Policy>("\"fallback\"").unwrap(), Policy::Fallback);
    }

    #[test]
    fn test_policy_weights_default_values() {
        let w = PolicyWeights::default();
        assert!((w.cost - 0.33).abs() < 0.01);
        assert!((w.quality - 0.33).abs() < 0.01);
        assert!((w.latency - 0.34).abs() < 0.01);
    }

    #[test]
    fn test_router_config_default_policy() {
        let config = RouterConfig::default();
        assert_eq!(config.default_policy, Policy::Fallback);
        assert!(!config.aliases.is_empty());
    }

    #[test]
    fn test_candidate_serialization() {
        let c = Candidate {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            cost_per_1k: 0.03,
            quality_score: 0.9,
            priority: 1,
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: Candidate = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider, "openai");
        assert_eq!(back.model, "gpt-4");
        assert!((back.cost_per_1k - 0.03).abs() < f64::EPSILON);
        assert!((back.quality_score - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn test_candidate_default_quality() {
        let json = r#"{"provider":"p","model":"m","cost_per_1k":0.0,"priority":0}"#;
        let c: Candidate = serde_json::from_str(json).unwrap();
        assert!((c.quality_score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_metrics_collector_new() {
        let c = MetricsCollector::new(100);
        let m = c.get_metrics("nonexistent");
        assert_eq!(m.total_requests, 0);
    }

    #[test]
    fn test_metrics_collector_record_and_get() {
        let c = MetricsCollector::new(100);
        c.record(Metric {
            provider: "test".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 500,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        let m = c.get_metrics("test");
        assert_eq!(m.total_requests, 1);
        assert!((m.avg_latency_ms - 100.0).abs() < 0.01);
        assert!((m.success_rate - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_metrics_collector_ring_buffer_overflow() {
        let c = MetricsCollector::new(2);
        c.record(Metric {
            provider: "test".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        c.record(Metric {
            provider: "test".to_string(),
            latency_ms: 200,
            success: true,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        c.record(Metric {
            provider: "test".to_string(),
            latency_ms: 300,
            success: false,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        let m = c.get_metrics("test");
        assert_eq!(m.total_requests, 2); // Only last 2
    }

    #[test]
    fn test_metrics_collector_reset() {
        let c = MetricsCollector::new(100);
        c.record(Metric {
            provider: "test".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        assert_eq!(c.get_metrics("test").total_requests, 1);
        c.reset("test");
        assert_eq!(c.get_metrics("test").total_requests, 0);
    }

    #[test]
    fn test_metrics_collector_get_all() {
        let c = MetricsCollector::new(100);
        c.record(Metric {
            provider: "a".to_string(),
            latency_ms: 100,
            success: true,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        c.record(Metric {
            provider: "b".to_string(),
            latency_ms: 200,
            success: true,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Utc::now(),
        });
        let all = c.get_all_metrics();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_get_policy_fast() {
        let p = get_policy("fast");
        assert_eq!(p.policy, Policy::Latency);
        assert_eq!(p.name, "fast");
    }

    #[test]
    fn test_get_policy_balanced() {
        let p = get_policy("balanced");
        assert_eq!(p.policy, Policy::Quality);
        assert_eq!(p.name, "balanced");
    }

    #[test]
    fn test_get_policy_cheap() {
        let p = get_policy("cheap");
        assert_eq!(p.policy, Policy::Cost);
        assert_eq!(p.name, "cheap");
    }

    #[test]
    fn test_get_policy_best() {
        let p = get_policy("best");
        assert_eq!(p.policy, Policy::Quality);
        assert_eq!(p.name, "best");
    }

    #[test]
    fn test_get_policy_unknown() {
        let p = get_policy("nonexistent");
        assert_eq!(p.name, "balanced");
    }

    #[test]
    fn test_all_policies_count() {
        let policies = all_policies();
        assert_eq!(policies.len(), 4);
        assert!(policies.contains_key("fast"));
        assert!(policies.contains_key("balanced"));
        assert!(policies.contains_key("cheap"));
        assert!(policies.contains_key("best"));
    }

    #[test]
    fn test_policy_names_count() {
        let names = policy_names();
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn test_router_new() {
        let router = Router::new(RouterConfig::default());
        assert!(router.select("anything").is_none());
    }

    #[test]
    fn test_router_add_and_select() {
        let router = Router::new(RouterConfig::default());
        router.add_candidate(Candidate {
            provider: "p1".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.5,
            priority: 1,
        });
        let sel = router.select("m1").unwrap();
        assert_eq!(sel.provider, "p1");
    }

    #[test]
    fn test_router_set_get_policy() {
        let router = Router::new(RouterConfig::default());
        assert_eq!(router.get_policy(), Policy::Fallback);
        router.set_policy(Policy::Cost);
        assert_eq!(router.get_policy(), Policy::Cost);
    }

    #[test]
    fn test_router_set_aliases() {
        let router = Router::new(RouterConfig::default());
        let mut new_aliases = HashMap::new();
        new_aliases.insert("test".to_string(), "provider/model".to_string());
        router.set_aliases(new_aliases);
        assert_eq!(router.resolve_alias("test"), "provider/model");
        assert_eq!(router.resolve_alias("fast"), "fast"); // Old aliases gone
    }

    #[test]
    fn test_router_select_cost_policy() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Cost,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "expensive".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.10,
            quality_score: 0.9,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "cheap".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.5,
            priority: 2,
        });
        let sel = router.select("m1").unwrap();
        assert_eq!(sel.provider, "cheap");
    }

    #[test]
    fn test_router_select_quality_policy() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Quality,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "low-q".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.3,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "high-q".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.10,
            quality_score: 0.95,
            priority: 2,
        });
        let sel = router.select("m1").unwrap();
        assert_eq!(sel.provider, "high-q");
    }

    #[test]
    fn test_router_select_fallback_priority() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Fallback,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "low-pri".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.9,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "high-pri".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.10,
            quality_score: 0.3,
            priority: 10,
        });
        let sel = router.select("m1").unwrap();
        assert_eq!(sel.provider, "high-pri");
    }

    #[test]
    fn test_router_select_with_policy_override() {
        let router = Router::new(RouterConfig {
            default_policy: Policy::Fallback,
            ..Default::default()
        });
        router.add_candidate(Candidate {
            provider: "cheap".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.3,
            priority: 1,
        });
        router.add_candidate(Candidate {
            provider: "expensive".to_string(),
            model: "m1".to_string(),
            cost_per_1k: 0.10,
            quality_score: 0.9,
            priority: 2,
        });
        let cost_sel = router.select_with_policy(Policy::Cost, "m1").unwrap();
        assert_eq!(cost_sel.provider, "cheap");
        let qual_sel = router.select_with_policy(Policy::Quality, "m1").unwrap();
        assert_eq!(qual_sel.provider, "expensive");
    }

    #[test]
    fn test_router_select_no_match_returns_first() {
        let router = Router::new(RouterConfig::default());
        router.add_candidate(Candidate {
            provider: "default".to_string(),
            model: "default-model".to_string(),
            cost_per_1k: 0.01,
            quality_score: 0.5,
            priority: 1,
        });
        let sel = router.select("nonexistent").unwrap();
        assert_eq!(sel.model, "default-model");
    }

    #[test]
    fn test_router_select_empty_returns_none() {
        let router = Router::new(RouterConfig::default());
        assert!(router.select("anything").is_none());
    }

    #[test]
    fn test_provider_metrics_default() {
        let m = ProviderMetrics::default();
        assert!(m.provider.is_empty());
        assert_eq!(m.total_requests, 0);
        assert_eq!(m.total_failures, 0);
        assert!((m.avg_latency_ms - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_router_config_serialization() {
        let config = RouterConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let back: RouterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.default_policy, Policy::Fallback);
    }
}

// ---------------------------------------------------------------------------
// fallback_provider.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod fallback_provider_extra {
    use nemesis_providers::fallback_provider::*;
    use nemesis_providers::failover::FailoverError;
    use nemesis_providers::router::LLMProvider;
    use nemesis_providers::types::*;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockSuccessProvider {
        name: String,
        model: String,
    }

    #[async_trait]
    impl LLMProvider for MockSuccessProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _options: &ChatOptions,
        ) -> Result<LLMResponse, FailoverError> {
            Ok(LLMResponse {
                content: format!("response from {}", self.name),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                usage: None,
            })
        }

        fn default_model(&self) -> &str {
            &self.model
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    struct MockFailProvider {
        name: String,
        model: String,
        retriable: bool,
    }

    #[async_trait]
    impl LLMProvider for MockFailProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _model: &str,
            _options: &ChatOptions,
        ) -> Result<LLMResponse, FailoverError> {
            if self.retriable {
                Err(FailoverError::RateLimit {
                    provider: self.name.clone(),
                    model: self.model.clone(),
                    retry_after: None,
                })
            } else {
                Err(FailoverError::Auth {
                    provider: self.name.clone(),
                    model: self.model.clone(),
                    status: 401,
                })
            }
        }

        fn default_model(&self) -> &str {
            &self.model
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    #[test]
    fn test_fallback_exhausted_error_display() {
        let err = FallbackExhaustedError {
            chain_name: "test-chain".to_string(),
            providers_attempted: 3,
            total_providers: 5,
            errors: vec![
                ("p1".to_string(), "rate limit".to_string()),
                ("p2".to_string(), "timeout".to_string()),
                ("p3".to_string(), "auth error".to_string()),
            ],
        };
        let msg = format!("{}", err);
        assert!(msg.contains("test-chain"));
        assert!(msg.contains("3/5"));
        assert!(msg.contains("p1"));
        assert!(msg.contains("p2"));
        assert!(msg.contains("p3"));
    }

    #[test]
    fn test_fallback_attempt_success() {
        let attempt = FallbackAttempt {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            error: None,
            success: true,
        };
        assert!(attempt.success);
        assert!(attempt.error.is_none());
    }

    #[test]
    fn test_fallback_attempt_failure() {
        let attempt = FallbackAttempt {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            error: Some("rate limited".to_string()),
            success: false,
        };
        assert!(!attempt.success);
        assert!(attempt.error.is_some());
    }

    #[test]
    fn test_fallback_result_success() {
        let result = FallbackResult {
            response: Some(LLMResponse {
                content: "hello".to_string(),
                tool_calls: vec![],
                finish_reason: "stop".to_string(),
                usage: None,
            }),
            attempts: vec![FallbackAttempt {
                provider: "p1".to_string(),
                model: "m1".to_string(),
                error: None,
                success: true,
            }],
            exhausted_error: None,
        };
        assert!(result.response.is_some());
        assert!(result.exhausted_error.is_none());
        assert_eq!(result.attempts.len(), 1);
    }

    #[test]
    fn test_fallback_result_exhausted() {
        let result: FallbackResult = FallbackResult {
            response: None,
            attempts: vec![],
            exhausted_error: Some(FallbackExhaustedError {
                chain_name: "test".to_string(),
                providers_attempted: 0,
                total_providers: 0,
                errors: vec![],
            }),
        };
        assert!(result.response.is_none());
        assert!(result.exhausted_error.is_some());
    }

    #[test]
    fn test_fallback_provider_name() {
        let provider = FallbackProvider::new("my-chain", vec![]);
        assert_eq!(provider.name(), "my-chain");
    }

    #[test]
    fn test_fallback_provider_chain_len() {
        let provider = FallbackProvider::new("test", vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider { name: "p1".to_string(), model: "m1".to_string() }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider { name: "p2".to_string(), model: "m2".to_string() }),
                model: "m2".to_string(),
            },
        ]);
        assert_eq!(provider.chain_len(), 2);
    }

    #[test]
    fn test_fallback_provider_empty_chain_len() {
        let provider = FallbackProvider::new("test", vec![]);
        assert_eq!(provider.chain_len(), 0);
    }

    #[test]
    fn test_fallback_provider_cooldown_accessor() {
        let provider = FallbackProvider::new("test", vec![]);
        let cooldown = provider.cooldown();
        assert!(cooldown.is_available("any"));
    }

    #[test]
    fn test_resolve_candidates_dedup() {
        let p1 = Arc::new(MockSuccessProvider { name: "p1".to_string(), model: "m1".to_string() });
        let chain = vec![
            FallbackEntry { provider: p1.clone(), model: "m1".to_string() },
            FallbackEntry { provider: p1.clone(), model: "m1".to_string() },
            FallbackEntry { provider: Arc::new(MockSuccessProvider { name: "p2".to_string(), model: "m2".to_string() }), model: "m2".to_string() },
        ];
        let candidates = FallbackProvider::resolve_candidates(&chain, "");
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn test_resolve_candidates_all_unique() {
        let chain = vec![
            FallbackEntry { provider: Arc::new(MockSuccessProvider { name: "p1".to_string(), model: "m1".to_string() }), model: "m1".to_string() },
            FallbackEntry { provider: Arc::new(MockSuccessProvider { name: "p2".to_string(), model: "m2".to_string() }), model: "m2".to_string() },
        ];
        let candidates = FallbackProvider::resolve_candidates(&chain, "");
        assert_eq!(candidates.len(), 2);
    }

    #[tokio::test]
    async fn test_fallback_empty_chain_error() {
        let provider = FallbackProvider::new("test", vec![]);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let result = provider.chat(&msgs, &[], "", &ChatOptions::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fallback_first_provider_succeeds() {
        let provider = FallbackProvider::new("test", vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider { name: "p1".to_string(), model: "m1".to_string() }),
                model: "m1".to_string(),
            },
        ]);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let resp = provider.chat(&msgs, &[], "", &ChatOptions::default()).await.unwrap();
        assert_eq!(resp.content, "response from p1");
    }

    #[tokio::test]
    async fn test_fallback_fail_then_succeed() {
        let provider = FallbackProvider::new("test", vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider { name: "p1".to_string(), model: "m1".to_string(), retriable: true }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider { name: "p2".to_string(), model: "m2".to_string() }),
                model: "m2".to_string(),
            },
        ]);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let resp = provider.chat(&msgs, &[], "", &ChatOptions::default()).await.unwrap();
        assert_eq!(resp.content, "response from p2");
    }

    #[tokio::test]
    async fn test_fallback_non_retriable_stops_chain() {
        let provider = FallbackProvider::new("test", vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider { name: "p1".to_string(), model: "m1".to_string(), retriable: false }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider { name: "p2".to_string(), model: "m2".to_string() }),
                model: "m2".to_string(),
            },
        ]);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let result = provider.chat(&msgs, &[], "", &ChatOptions::default()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FailoverError::Auth { .. }));
    }

    #[tokio::test]
    async fn test_execute_detailed_success_tracking() {
        let provider = FallbackProvider::new("test", vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider { name: "p1".to_string(), model: "m1".to_string() }),
                model: "m1".to_string(),
            },
        ]);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let result = provider.execute_detailed(&msgs, &[], "", &ChatOptions::default()).await;
        assert!(result.response.is_some());
        assert!(result.exhausted_error.is_none());
        assert_eq!(result.attempts.len(), 1);
        assert!(result.attempts[0].success);
        assert!(result.attempts[0].error.is_none());
    }

    #[tokio::test]
    async fn test_execute_detailed_all_fail() {
        let provider = FallbackProvider::new("test", vec![
            FallbackEntry {
                provider: Arc::new(MockFailProvider { name: "p1".to_string(), model: "m1".to_string(), retriable: true }),
                model: "m1".to_string(),
            },
            FallbackEntry {
                provider: Arc::new(MockFailProvider { name: "p2".to_string(), model: "m2".to_string(), retriable: true }),
                model: "m2".to_string(),
            },
        ]);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let result = provider.execute_detailed(&msgs, &[], "", &ChatOptions::default()).await;
        assert!(result.response.is_none());
        let err = result.exhausted_error.unwrap();
        assert_eq!(err.providers_attempted, 2);
        assert_eq!(err.total_providers, 2);
        assert_eq!(result.attempts.len(), 2);
        assert!(!result.attempts[0].success);
        assert!(!result.attempts[1].success);
    }

    #[tokio::test]
    async fn test_execute_image_success() {
        let provider = FallbackProvider::new("test", vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider { name: "p1".to_string(), model: "m1".to_string() }),
                model: "m1".to_string(),
            },
        ]);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "describe this image".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let result = provider.execute_image(&msgs, &[], "m1", &ChatOptions::default()).await;
        assert!(result.response.is_some());
    }

    #[tokio::test]
    async fn test_execute_image_empty_chain() {
        let provider = FallbackProvider::new("test", vec![]);
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
        }];
        let result = provider.execute_image(&msgs, &[], "", &ChatOptions::default()).await;
        assert!(result.response.is_none());
        let err = result.exhausted_error.unwrap();
        assert_eq!(err.providers_attempted, 0);
        assert_eq!(err.total_providers, 0);
    }

    #[test]
    fn test_fallback_default_model() {
        let provider = FallbackProvider::new("test", vec![
            FallbackEntry {
                provider: Arc::new(MockSuccessProvider { name: "p1".to_string(), model: "default-model".to_string() }),
                model: "default-model".to_string(),
            },
        ]);
        assert_eq!(provider.default_model(), "default-model");
    }

    #[test]
    fn test_fallback_default_model_empty() {
        let provider = FallbackProvider::new("test", vec![]);
        assert_eq!(provider.default_model(), "");
    }
}

// ---------------------------------------------------------------------------
// tool_call_extract.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tool_call_extract_extra {
    use nemesis_providers::tool_call_extract::*;

    #[test]
    fn test_find_matching_brace_simple() {
        assert_eq!(find_matching_brace("{}", 0), Some(2));
    }

    #[test]
    fn test_find_matching_brace_nested() {
        assert_eq!(find_matching_brace("{\"a\":{}}", 0), Some(8));
    }

    #[test]
    fn test_find_matching_brace_not_at_brace() {
        assert_eq!(find_matching_brace("abc", 0), None);
    }

    #[test]
    fn test_find_matching_brace_out_of_bounds() {
        assert_eq!(find_matching_brace("{}", 5), None);
    }

    #[test]
    fn test_find_matching_brace_unmatched() {
        assert_eq!(find_matching_brace("{", 0), None);
    }

    #[test]
    fn test_find_matching_brace_with_string_braces() {
        let text = r#"{"key": "{"}"#;
        assert!(find_matching_brace(text, 0).is_some());
    }

    #[test]
    fn test_extract_tool_calls_basic() {
        let text = r#"{"tool_calls":[{"id":"c1","type":"function","function":{"name":"t","arguments":"{}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "c1");
    }

    #[test]
    fn test_extract_tool_calls_none() {
        let calls = extract_tool_calls_from_text("no tool calls here");
        assert!(calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_multiple() {
        let text = r#"{"tool_calls":[{"id":"c1","type":"function","function":{"name":"t1","arguments":"{}"}},{"id":"c2","type":"function","function":{"name":"t2","arguments":"{}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "c1");
        assert_eq!(calls[1].id, "c2");
    }

    #[test]
    fn test_extract_tool_calls_with_leading_text() {
        let text = r#"Here is my plan. {"tool_calls":[{"id":"c1","type":"function","function":{"name":"t","arguments":"{}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn test_extract_tool_calls_invalid_json() {
        let text = r#"{"tool_calls": not valid}"#;
        let calls = extract_tool_calls_from_text(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_with_arguments() {
        let text = r#"{"tool_calls":[{"id":"c1","type":"function","function":{"name":"exec","arguments":"{\"cmd\":\"ls\"}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].arguments.is_some());
        assert_eq!(calls[0].arguments.as_ref().unwrap().get("cmd").unwrap(), "ls");
    }

    #[test]
    fn test_strip_tool_calls_basic() {
        let text = r#"Some text {"tool_calls":[{"id":"c1","type":"function","function":{"name":"t","arguments":"{}"}}]} trailing"#;
        let stripped = strip_tool_calls_from_text(text);
        assert!(stripped.contains("Some text"));
        assert!(stripped.contains("trailing"));
        assert!(!stripped.contains("tool_calls"));
    }

    #[test]
    fn test_strip_tool_calls_none() {
        let text = "No tool calls here.";
        assert_eq!(strip_tool_calls_from_text(text), text);
    }

    #[test]
    fn test_strip_tool_calls_unmatched() {
        let text = r#"Text {"tool_calls":[{"id":"c1"}"#;
        assert_eq!(strip_tool_calls_from_text(text), text);
    }

    #[test]
    fn test_extract_tool_call_fields() {
        let text = r#"{"tool_calls":[{"id":"call_abc","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"/tmp\"}"}}]}"#;
        let calls = extract_tool_calls_from_text(text);
        assert_eq!(calls.len(), 1);
        let tc = &calls[0];
        assert_eq!(tc.id, "call_abc");
        assert_eq!(tc.call_type, Some("function".to_string()));
        assert_eq!(tc.name, Some("read_file".to_string()));
        assert!(tc.function.is_some());
        assert_eq!(tc.function.as_ref().unwrap().name, "read_file");
    }

    #[test]
    fn test_find_matching_brace_deeply_nested() {
        let text = r#"{"a":{"b":{"c":1}}}"#;
        assert_eq!(find_matching_brace(text, 0), Some(text.len()));
    }

    #[test]
    fn test_find_matching_brace_with_escapes() {
        let text = r#"{"msg": "say \"hello\""}"#;
        assert_eq!(find_matching_brace(text, 0), Some(text.len()));
    }

    #[test]
    fn test_find_matching_brace_with_array() {
        let text = r#"{"arr": [1, 2, 3]}"#;
        assert_eq!(find_matching_brace(text, 0), Some(text.len()));
    }
}

// ---------------------------------------------------------------------------
// openai_compat.rs supplementary tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod openai_compat_extra {
    use nemesis_providers::openai_compat::normalize_model;

    #[test]
    fn test_normalize_deepseek() {
        assert_eq!(normalize_model("deepseek/deepseek-chat", "https://api.deepseek.com"), "deepseek-chat");
    }

    #[test]
    fn test_normalize_groq() {
        assert_eq!(normalize_model("groq/llama3", "https://api.groq.com"), "llama3");
    }

    #[test]
    fn test_normalize_ollama() {
        assert_eq!(normalize_model("ollama/llama3", "http://localhost:11434"), "llama3");
    }

    #[test]
    fn test_normalize_zhipu() {
        assert_eq!(normalize_model("zhipu/glm-4", "https://open.bigmodel.cn"), "glm-4");
    }

    #[test]
    fn test_normalize_nvidia() {
        assert_eq!(normalize_model("nvidia/nemotron", "https://api.nvidia.com"), "nemotron");
    }

    #[test]
    fn test_normalize_moonshot() {
        assert_eq!(normalize_model("moonshot/moonshot-v1", "https://api.moonshot.cn"), "moonshot-v1");
    }

    #[test]
    fn test_normalize_google() {
        assert_eq!(normalize_model("google/gemini-pro", "https://generativelanguage.googleapis.com"), "gemini-pro");
    }

    #[test]
    fn test_normalize_openrouter_preserved() {
        assert_eq!(normalize_model("openai/gpt-4", "https://openrouter.ai/api/v1"), "openai/gpt-4");
    }

    #[test]
    fn test_normalize_no_slash() {
        assert_eq!(normalize_model("gpt-4", "https://api.openai.com"), "gpt-4");
    }

    #[test]
    fn test_normalize_unknown_prefix() {
        assert_eq!(normalize_model("myprovider/model-v1", "https://example.com"), "myprovider/model-v1");
    }

    #[test]
    fn test_normalize_case_insensitive_prefix() {
        assert_eq!(normalize_model("DeepSeek/chat", "https://api.deepseek.com"), "chat");
        assert_eq!(normalize_model("GROQ/llama3", "https://api.groq.com"), "llama3");
    }

    #[test]
    fn test_normalize_case_insensitive_api_base_openrouter() {
        assert_eq!(normalize_model("openai/gpt-4", "https://OPENROUTER.AI/api/v1"), "openai/gpt-4");
    }
}
