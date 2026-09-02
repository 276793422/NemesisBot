use super::*;

#[test]
fn test_default_aliases() {
    let config = RouterConfig::default();
    assert_eq!(config.aliases.get("fast").unwrap(), "groq/llama-3");
    assert_eq!(
        config.aliases.get("smart").unwrap(),
        "anthropic/claude-sonnet-4-6"
    );
}

#[test]
fn test_resolve_alias() {
    let router = Router::new(RouterConfig::default());
    assert_eq!(router.resolve_alias("fast"), "groq/llama-3");
    assert_eq!(router.resolve_alias("gpt-4"), "gpt-4");
}

#[test]
fn test_select_fallback() {
    let router = Router::new(RouterConfig {
        default_policy: Policy::Fallback,
        ..Default::default()
    });
    router.add_candidate(Candidate {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.03,
        quality_score: 0.9,
        priority: 1,
        semantic_description: String::new(),
    });
    router.add_candidate(Candidate {
        provider: "azure".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.03,
        quality_score: 0.9,
        priority: 2,
        semantic_description: String::new(),
    });

    let selected = router.select("gpt-4").unwrap();
    assert_eq!(selected.provider, "azure"); // Higher priority
}

#[test]
fn test_select_cost() {
    let router = Router::new(RouterConfig {
        default_policy: Policy::Cost,
        ..Default::default()
    });
    router.add_candidate(Candidate {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.03,
        quality_score: 0.9,
        priority: 1,
        semantic_description: String::new(),
    });
    router.add_candidate(Candidate {
        provider: "deepseek".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.7,
        priority: 1,
        semantic_description: String::new(),
    });

    let selected = router.select("gpt-4").unwrap();
    assert_eq!(selected.provider, "deepseek"); // Cheaper
}

#[test]
fn test_metrics_collector() {
    let collector = MetricsCollector::new(100);
    collector.record(Metric {
        provider: "openai".to_string(),
        latency_ms: 100,
        success: true,
        tokens_used: 500,
        cost: 0.015,
        timestamp: chrono::Local::now(),
    });
    collector.record(Metric {
        provider: "openai".to_string(),
        latency_ms: 200,
        success: false,
        tokens_used: 0,
        cost: 0.0,
        timestamp: chrono::Local::now(),
    });

    let metrics = collector.get_metrics("openai");
    assert_eq!(metrics.total_requests, 2);
    assert_eq!(metrics.total_failures, 1);
    assert!((metrics.success_rate - 0.5).abs() < 0.01);
}

#[test]
fn test_metrics_ring_buffer() {
    let collector = MetricsCollector::new(3);
    for i in 0..5 {
        collector.record(Metric {
            provider: "test".to_string(),
            latency_ms: i * 100,
            success: true,
            tokens_used: 100,
            cost: 0.01,
            timestamp: chrono::Local::now(),
        });
    }
    let metrics = collector.get_metrics("test");
    assert_eq!(metrics.total_requests, 3); // Only last 3 kept
}

#[test]
fn test_default_aliases_function() {
    let aliases = default_aliases();
    assert_eq!(aliases.get("fast").unwrap(), "groq/llama-3.3-70b-versatile");
    assert_eq!(
        aliases.get("smart").unwrap(),
        "anthropic/claude-sonnet-4-20250514"
    );
    assert_eq!(aliases.get("cheap").unwrap(), "deepseek/deepseek-chat");
    assert_eq!(aliases.get("local").unwrap(), "ollama/llama3.3");
}

#[test]
fn test_resolve_alias_function() {
    let aliases = default_aliases();
    assert_eq!(
        resolve_alias(&aliases, "fast"),
        Some("groq/llama-3.3-70b-versatile".to_string())
    );
    assert_eq!(resolve_alias(&aliases, "gpt-4"), None);
}

#[test]
fn test_merge_aliases_custom_overrides() {
    let defaults = default_aliases();
    let mut custom = HashMap::new();
    custom.insert("fast".to_string(), "custom/fast-model".to_string());
    custom.insert("my-custom".to_string(), "custom/model".to_string());

    let merged = merge_aliases(&defaults, &custom);
    assert_eq!(merged.get("fast").unwrap(), "custom/fast-model");
    assert_eq!(merged.get("my-custom").unwrap(), "custom/model");
    // Default still present
    assert_eq!(
        merged.get("smart").unwrap(),
        "anthropic/claude-sonnet-4-20250514"
    );
}

#[test]
fn test_get_policy_known() {
    let p = get_policy("fast");
    assert_eq!(p.policy, Policy::Latency);
    assert_eq!(p.name, "fast");
}

#[test]
fn test_get_policy_unknown_returns_balanced() {
    let p = get_policy("nonexistent");
    assert_eq!(p.name, "balanced");
}

#[test]
fn test_all_policies() {
    let policies = all_policies();
    assert!(policies.contains_key("fast"));
    assert!(policies.contains_key("balanced"));
    assert!(policies.contains_key("cheap"));
    assert!(policies.contains_key("best"));
    assert_eq!(policies.len(), 4);
}

#[test]
fn test_policy_names() {
    let names = policy_names();
    assert_eq!(names.len(), 4);
    assert!(names.contains(&"fast".to_string()));
}

#[test]
fn test_get_all_metrics() {
    let collector = MetricsCollector::new(100);
    collector.record(Metric {
        provider: "openai".to_string(),
        latency_ms: 100,
        success: true,
        tokens_used: 500,
        cost: 0.015,
        timestamp: chrono::Local::now(),
    });
    collector.record(Metric {
        provider: "anthropic".to_string(),
        latency_ms: 200,
        success: true,
        tokens_used: 300,
        cost: 0.01,
        timestamp: chrono::Local::now(),
    });

    let all = collector.get_all_metrics();
    assert_eq!(all.len(), 2);
    assert!(all.contains_key("openai"));
    assert!(all.contains_key("anthropic"));
}

#[test]
fn test_reset_metrics() {
    let collector = MetricsCollector::new(100);
    collector.record(Metric {
        provider: "openai".to_string(),
        latency_ms: 100,
        success: true,
        tokens_used: 500,
        cost: 0.015,
        timestamp: chrono::Local::now(),
    });
    assert_eq!(collector.get_metrics("openai").total_requests, 1);
    collector.reset("openai");
    assert_eq!(collector.get_metrics("openai").total_requests, 0);
}

#[test]
fn test_prune_old_samples() {
    let collector = MetricsCollector::new(100);
    // Old sample (1 hour ago)
    collector.record(Metric {
        provider: "test".to_string(),
        latency_ms: 100,
        success: true,
        tokens_used: 500,
        cost: 0.01,
        timestamp: chrono::Local::now() - chrono::Duration::hours(2),
    });
    // Recent sample
    collector.record(Metric {
        provider: "test".to_string(),
        latency_ms: 50,
        success: true,
        tokens_used: 200,
        cost: 0.005,
        timestamp: chrono::Local::now(),
    });

    assert_eq!(collector.get_metrics("test").total_requests, 2);

    // Prune samples older than 1 hour
    collector.prune(std::time::Duration::from_secs(3600));

    let metrics = collector.get_metrics("test");
    assert_eq!(metrics.total_requests, 1);
}

// --- Benchmark-style throughput tests ---

#[test]
fn test_router_select_throughput() {
    let router = Router::new(RouterConfig::default());

    // Register candidates
    for i in 0..10 {
        router.add_candidate(Candidate {
            provider: format!("provider-{}", i),
            model: format!("model-{}", i),
            cost_per_1k: 0.01,
            quality_score: 0.9,
            priority: i,
            semantic_description: String::new(),
        });
    }

    let start = std::time::Instant::now();
    let iterations = 10_000;
    for i in 0..iterations {
        let _ = router.select(&format!("model-{}", i % 10));
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "Router select too slow: {:?}",
        elapsed
    );
}

#[test]
fn test_metrics_collector_record_throughput() {
    let collector = MetricsCollector::new(1000);

    let start = std::time::Instant::now();
    let iterations = 10_000;
    for i in 0..iterations {
        collector.record(Metric {
            provider: format!("provider-{}", i % 5),
            latency_ms: 100 + (i % 50) as u64,
            success: i % 10 != 0,
            tokens_used: 100,
            cost: 0.001,
            timestamp: chrono::Local::now(),
        });
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "MetricsCollector record too slow: {:?}",
        elapsed
    );
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_policy_default_is_fallback() {
    assert_eq!(Policy::default(), Policy::Fallback);
}

#[test]
fn test_router_config_default() {
    let config = RouterConfig::default();
    assert_eq!(config.default_policy, Policy::Fallback);
    assert!(!config.aliases.is_empty());
}

#[test]
fn test_policy_weights_default() {
    let weights = PolicyWeights::default();
    assert!((weights.cost - 0.33).abs() < 0.01);
    assert!((weights.quality - 0.33).abs() < 0.01);
    assert!((weights.latency - 0.34).abs() < 0.01);
}

#[test]
fn test_select_quality_policy() {
    let router = Router::new(RouterConfig {
        default_policy: Policy::Quality,
        ..Default::default()
    });
    router.add_candidate(Candidate {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.03,
        quality_score: 0.9,
        priority: 1,
        semantic_description: String::new(),
    });
    router.add_candidate(Candidate {
        provider: "deepseek".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.7,
        priority: 2,
        semantic_description: String::new(),
    });

    let selected = router.select("gpt-4").unwrap();
    assert_eq!(selected.provider, "openai"); // Higher quality
}

#[test]
fn test_select_latency_policy() {
    let router = Router::new(RouterConfig {
        default_policy: Policy::Latency,
        ..Default::default()
    });
    router.add_candidate(Candidate {
        provider: "fast-provider".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.03,
        quality_score: 0.9,
        priority: 1,
        semantic_description: String::new(),
    });
    router.add_candidate(Candidate {
        provider: "slow-provider".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.7,
        priority: 2,
        semantic_description: String::new(),
    });

    // Record metrics for the slow provider (higher latency)
    router.metrics().record(Metric {
        provider: "fast-provider".to_string(),
        latency_ms: 50,
        success: true,
        tokens_used: 100,
        cost: 0.001,
        timestamp: chrono::Local::now(),
    });
    router.metrics().record(Metric {
        provider: "slow-provider".to_string(),
        latency_ms: 500,
        success: true,
        tokens_used: 100,
        cost: 0.001,
        timestamp: chrono::Local::now(),
    });

    let selected = router.select("gpt-4").unwrap();
    assert_eq!(selected.provider, "fast-provider");
}

#[test]
fn test_select_round_robin() {
    let router = Router::new(RouterConfig {
        default_policy: Policy::RoundRobin,
        ..Default::default()
    });
    router.add_candidate(Candidate {
        provider: "provider-a".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.03,
        quality_score: 0.9,
        priority: 1,
        semantic_description: String::new(),
    });
    router.add_candidate(Candidate {
        provider: "provider-b".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.7,
        priority: 2,
        semantic_description: String::new(),
    });

    let first = router.select("gpt-4").unwrap();
    let second = router.select("gpt-4").unwrap();
    // Round-robin should alternate
    assert_ne!(first.provider, second.provider);
}

#[test]
fn test_select_no_matching_returns_first() {
    let router = Router::new(RouterConfig::default());
    router.add_candidate(Candidate {
        provider: "default".to_string(),
        model: "default-model".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.5,
        priority: 1,
        semantic_description: String::new(),
    });

    let selected = router.select("nonexistent-model");
    assert!(selected.is_some());
    assert_eq!(selected.unwrap().model, "default-model");
}

#[test]
fn test_select_empty_candidates() {
    let router = Router::new(RouterConfig::default());
    let selected = router.select("anything");
    assert!(selected.is_none());
}

#[test]
fn test_select_with_policy_override() {
    let router = Router::new(RouterConfig {
        default_policy: Policy::Fallback,
        ..Default::default()
    });
    router.add_candidate(Candidate {
        provider: "cheap".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.5,
        priority: 1,
        semantic_description: String::new(),
    });
    router.add_candidate(Candidate {
        provider: "expensive".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.10,
        quality_score: 0.9,
        priority: 2,
        semantic_description: String::new(),
    });

    // Default is Fallback (priority-based)
    let fb = router.select("gpt-4").unwrap();
    assert_eq!(fb.provider, "expensive");

    // Override with Quality
    let q = router.select_with_policy(Policy::Quality, "gpt-4").unwrap();
    assert_eq!(q.provider, "expensive");

    // Override with Cost
    let c = router.select_with_policy(Policy::Cost, "gpt-4").unwrap();
    assert_eq!(c.provider, "cheap");
}

#[test]
fn test_set_and_get_policy() {
    let router = Router::new(RouterConfig::default());
    assert_eq!(router.get_policy(), Policy::Fallback);

    router.set_policy(Policy::Cost);
    assert_eq!(router.get_policy(), Policy::Cost);

    router.set_policy(Policy::Quality);
    assert_eq!(router.get_policy(), Policy::Quality);
}

#[test]
fn test_set_aliases() {
    let router = Router::new(RouterConfig::default());
    let mut new_aliases = HashMap::new();
    new_aliases.insert("custom".to_string(), "my/model".to_string());

    router.set_aliases(new_aliases);
    assert_eq!(router.resolve_alias("custom"), "my/model");
    // Old aliases should be gone
    assert_eq!(router.resolve_alias("fast"), "fast"); // No longer aliased
}

#[test]
fn test_metrics_no_samples() {
    let collector = MetricsCollector::new(100);
    let metrics = collector.get_metrics("nonexistent");
    assert_eq!(metrics.total_requests, 0);
    assert_eq!(metrics.avg_latency_ms, 0.0);
    assert_eq!(metrics.success_rate, 0.0);
}

#[test]
fn test_metrics_avg_latency() {
    let collector = MetricsCollector::new(100);
    collector.record(Metric {
        provider: "test".to_string(),
        latency_ms: 100,
        success: true,
        tokens_used: 100,
        cost: 0.01,
        timestamp: chrono::Local::now(),
    });
    collector.record(Metric {
        provider: "test".to_string(),
        latency_ms: 200,
        success: true,
        tokens_used: 200,
        cost: 0.02,
        timestamp: chrono::Local::now(),
    });

    let metrics = collector.get_metrics("test");
    assert!((metrics.avg_latency_ms - 150.0).abs() < 0.01);
}

#[test]
fn test_metrics_avg_cost_per_1k() {
    let collector = MetricsCollector::new(100);
    collector.record(Metric {
        provider: "test".to_string(),
        latency_ms: 100,
        success: true,
        tokens_used: 1000,
        cost: 0.05,
        timestamp: chrono::Local::now(),
    });

    let metrics = collector.get_metrics("test");
    assert!((metrics.avg_cost_per_1k - 0.05).abs() < 0.001);
}

#[test]
fn test_metrics_zero_tokens_cost() {
    let collector = MetricsCollector::new(100);
    collector.record(Metric {
        provider: "test".to_string(),
        latency_ms: 100,
        success: true,
        tokens_used: 0,
        cost: 0.0,
        timestamp: chrono::Local::now(),
    });

    let metrics = collector.get_metrics("test");
    assert_eq!(metrics.avg_cost_per_1k, 0.0);
}

#[test]
fn test_candidate_serialization() {
    let c = Candidate {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.03,
        quality_score: 0.9,
        priority: 1,
        semantic_description: String::new(),
    };
    let json = serde_json::to_string(&c).unwrap();
    let deserialized: Candidate = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.provider, "openai");
    assert_eq!(deserialized.model, "gpt-4");
    assert!((deserialized.cost_per_1k - 0.03).abs() < f64::EPSILON);
}

#[test]
fn test_policy_serialization() {
    assert_eq!(serde_json::to_string(&Policy::Cost).unwrap(), "\"cost\"");
    assert_eq!(
        serde_json::to_string(&Policy::Quality).unwrap(),
        "\"quality\""
    );
    assert_eq!(
        serde_json::to_string(&Policy::Latency).unwrap(),
        "\"latency\""
    );
    assert_eq!(
        serde_json::to_string(&Policy::RoundRobin).unwrap(),
        "\"round_robin\""
    );
    assert_eq!(
        serde_json::to_string(&Policy::Fallback).unwrap(),
        "\"fallback\""
    );
}

#[test]
fn test_policy_deserialization() {
    let p: Policy = serde_json::from_str("\"cost\"").unwrap();
    assert_eq!(p, Policy::Cost);

    let p: Policy = serde_json::from_str("\"fallback\"").unwrap();
    assert_eq!(p, Policy::Fallback);
}

#[test]
fn test_router_register_and_use_provider() {
    struct MockProvider;
    #[async_trait::async_trait]
    impl LLMProvider for MockProvider {
        async fn chat(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: &ChatOptions,
        ) -> Result<LLMResponse, FailoverError> {
            Ok(LLMResponse {
                content: "mock".into(),
                tool_calls: vec![],
                finish_reason: "stop".into(),
                usage: None,
                reasoning_content: None,
                extra: HashMap::new(),
                raw_request_body: None,
                raw_response_body: None,
            })
        }
        fn default_model(&self) -> &str {
            "mock-model"
        }
        fn name(&self) -> &str {
            "mock"
        }
    }

    let router = Router::new(RouterConfig::default());
    router.register_provider("mock", Arc::new(MockProvider));
    router.add_candidate(Candidate {
        provider: "mock".to_string(),
        model: "mock-model".to_string(),
        cost_per_1k: 0.0,
        quality_score: 0.5,
        priority: 1,
        semantic_description: String::new(),
    });

    let selected = router.select("mock-model");
    assert!(selected.is_some());
    assert_eq!(selected.unwrap().provider, "mock");
}

#[test]
fn test_merge_aliases_does_not_modify_inputs() {
    let defaults = default_aliases();
    let mut custom = HashMap::new();
    custom.insert("custom".to_string(), "custom/model".to_string());

    let merged = merge_aliases(&defaults, &custom);
    assert_eq!(merged.len(), defaults.len() + 1);

    // Originals should not be modified
    assert!(!defaults.contains_key("custom"));
}

#[test]
fn test_router_metrics_accessor() {
    let router = Router::new(RouterConfig::default());
    let metrics = router.metrics();
    metrics.record(Metric {
        provider: "test".to_string(),
        latency_ms: 100,
        success: true,
        tokens_used: 100,
        cost: 0.01,
        timestamp: chrono::Local::now(),
    });
    let m = metrics.get_metrics("test");
    assert_eq!(m.total_requests, 1);
}

#[test]
fn test_prune_no_samples() {
    let collector = MetricsCollector::new(100);
    // Should not panic with empty collector
    collector.prune(std::time::Duration::from_secs(3600));
}

#[test]
fn test_model_matches_full_name() {
    assert!(model_matches("deepseek-chat", "deepseek/deepseek-chat"));
    assert!(model_matches(
        "deepseek/deepseek-chat",
        "deepseek/deepseek-chat"
    ));
    assert!(model_matches("gpt-4", "gpt-4"));
}

#[test]
fn test_model_matches_bare_name_request() {
    // 请求是裸名 → 只有裸名候选命中（不能拿裸名去匹配别人的前缀形态，
    // "chat" 不命中 "deepseek/deepseek-chat"）。
    assert!(model_matches("deepseek-chat", "deepseek-chat"));
    assert!(!model_matches("deepseek/deepseek-chat", "deepseek-chat"));
    assert!(!model_matches("chat", "deepseek-chat"));
}

#[test]
fn test_model_matches_edge_forms() {
    // 尾斜杠/空段/多段前缀的边界形态。
    assert!(!model_matches("deepseek-chat", "deepseek/")); // 空 bare 段不匹配
    assert!(model_matches("a", "x/y/a")); // 多段取最后一段（罕见但无害）
    assert!(!model_matches("other", "deepseek/deepseek-chat"));
}

#[test]
fn test_select_by_prefixed_model_finds_bare_candidate() {
    // 修复前（恒等表达式）这条 select 走 fallback 分支返回第一个候选，
    // 这里钉死前缀请求能命中裸名候选。
    let router = Router::new(RouterConfig::default());
    router.add_candidate(Candidate {
        provider: "other".to_string(),
        model: "llama-3".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.5,
        priority: 1,
        semantic_description: String::new(),
    });
    router.add_candidate(Candidate {
        provider: "deepseek".to_string(),
        model: "deepseek-chat".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.5,
        priority: 2,
        semantic_description: String::new(),
    });
    let selected = router.select("deepseek/deepseek-chat").unwrap();
    assert_eq!(selected.provider, "deepseek");
    assert_eq!(selected.model, "deepseek-chat");
}

// ===========================================================================
// I5 (P3.4): semantic routing
// ===========================================================================

fn sem_candidate(provider: &str, model: &str, desc: &str, priority: i32) -> Candidate {
    Candidate {
        provider: provider.to_string(),
        model: model.to_string(),
        cost_per_1k: 0.0,
        quality_score: 0.5,
        priority,
        semantic_description: desc.to_string(),
    }
}

/// Fixed-vector mock embedder: keywords map to orthogonal unit vectors.
fn mock_embedder() -> SemanticEmbedder {
    std::sync::Arc::new(|text: &str| -> Option<Vec<f32>> {
        let t = text.to_lowercase();
        if t.contains("code") || t.contains("编程") {
            Some(vec![1.0, 0.0])
        } else if t.contains("chat") || t.contains("闲聊") {
            Some(vec![0.0, 1.0])
        } else {
            None // unknown domain = unavailable
        }
    })
}

#[test]
fn test_semantic_policy_selects_matching_model() {
    let router = Router::new(RouterConfig::default());
    router.set_semantic_embedder(mock_embedder());
    let code_model = sem_candidate("prov-a", "code-gen-model", "best at coding 编程 tasks", 1);
    let chat_model = sem_candidate(
        "prov-b",
        "chat-warm-model",
        "good at chat 闲聊 casual talk",
        1,
    );
    let matching = vec![code_model.clone(), chat_model];

    // Programming intent routes to the code model.
    let pick = router.select_with_semantic("帮我写段 code 修复这个 bug", &matching);
    assert_eq!(
        pick.as_ref().map(|c| c.model.as_str()),
        Some("code-gen-model")
    );

    // Chat intent routes to the chat model.
    let pick2 = router.select_with_semantic("陪我 chat 聊聊天", &matching);
    assert_eq!(
        pick2.as_ref().map(|c| c.model.as_str()),
        Some("chat-warm-model")
    );
}

#[test]
fn test_semantic_fallback_when_no_embedder() {
    let router = Router::new(RouterConfig::default()); // NO embedder injected
    let low_pri = sem_candidate("prov-a", "model-a", "code stuff", 1);
    let high_pri = sem_candidate("prov-b", "model-b", "chat stuff", 9);
    let matching = vec![low_pri, high_pri.clone()];
    // Degrades to priority order (fail-open, no panic).
    let pick = router.select_with_semantic("any intent", &matching);
    assert_eq!(pick.as_ref().map(|c| c.model.as_str()), Some("model-b"));

    // Embedder present but returns None for this text → also fallback.
    let router2 = Router::new(RouterConfig::default());
    router2.set_semantic_embedder(mock_embedder());
    let pick2 = router2.select_with_semantic("unknown domain text", &matching);
    assert_eq!(pick2.as_ref().map(|c| c.model.as_str()), Some("model-b"));
}

/// Policy enum accepts "semantic"; unknown strings keep the legacy
/// get_policy fallback behavior (unknown name → balanced).
#[test]
fn test_policy_semantic_serde_roundtrip() {
    let p: Policy = serde_json::from_str("\"semantic\"").unwrap();
    assert!(matches!(p, Policy::Semantic));
    let back = serde_json::to_string(&p).unwrap();
    assert_eq!(back, "\"semantic\"");
}

// ===========================================================================
// W4c 补测（2026-08-25）：Router::chat failover 全矩阵 + select 臂 + serde 默认
// ===========================================================================

/// 可编程 mock provider：按调用序返回预设结果，并记录收到的 model 名。
struct ScriptedProvider {
    name: String,
    default_model: String,
    script: parking_lot::Mutex<Vec<Result<LLMResponse, FailoverError>>>,
    seen_models: parking_lot::Mutex<Vec<String>>,
}

impl ScriptedProvider {
    fn new(
        name: &str,
        default_model: &str,
        script: Vec<Result<LLMResponse, FailoverError>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            default_model: default_model.to_string(),
            script: parking_lot::Mutex::new(script),
            seen_models: parking_lot::Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl LLMProvider for ScriptedProvider {
    async fn chat(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        model: &str,
        _: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        self.seen_models.lock().push(model.to_string());
        self.script
            .lock()
            .pop()
            .expect("scripted provider exhausted")
    }
    fn default_model(&self) -> &str {
        &self.default_model
    }
    fn name(&self) -> &str {
        &self.name
    }
}

fn ok_response(content: &str, total_tokens: i64) -> LLMResponse {
    LLMResponse {
        content: content.to_string(),
        tool_calls: vec![],
        finish_reason: "stop".to_string(),
        usage: Some(UsageInfo {
            prompt_tokens: 10,
            completion_tokens: total_tokens - 10,
            total_tokens,
            cached_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }),
        reasoning_content: None,
        extra: HashMap::new(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn plain_candidate(provider: &str, model: &str, priority: i32) -> Candidate {
    Candidate {
        provider: provider.to_string(),
        model: model.to_string(),
        cost_per_1k: 0.001,
        quality_score: 0.5,
        priority,
        semantic_description: String::new(),
    }
}

fn one_user_message() -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: "hi".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }]
}

#[tokio::test]
async fn test_w4c_router_chat_success_records_metrics() {
    let router = Router::new(RouterConfig::default());
    let prov = ScriptedProvider::new("p1", "m1", vec![Ok(ok_response("ok", 1000))]);
    router.register_provider("p1", prov);
    router.add_candidate(plain_candidate("p1", "m1", 1));

    let resp = router
        .chat(&one_user_message(), &[], "m1", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "ok");

    let m = router.metrics().get_metrics("p1");
    assert_eq!(m.total_requests, 1);
    assert_eq!(m.total_failures, 0);
    assert_eq!(m.success_rate, 1.0);
    // cost = cost_per_1k(0.001) * 1000 tokens / 1000
    assert!((m.avg_cost_per_1k - 0.001).abs() < 1e-9);
}

#[tokio::test]
async fn test_w4c_router_chat_retriable_error_falls_back_to_alt() {
    let router = Router::new(RouterConfig::default());
    let primary = ScriptedProvider::new(
        "p1",
        "m1",
        vec![Err(FailoverError::Timeout {
            provider: "p1".into(),
            model: "m1".into(),
        })],
    );
    let alt = ScriptedProvider::new("p2", "m1", vec![Ok(ok_response("from-alt", 10))]);
    router.register_provider("p1", primary);
    router.register_provider("p2", alt);
    // fallback policy：primary 优先级更高被先选
    router.add_candidate(plain_candidate("p1", "m1", 5));
    router.add_candidate(plain_candidate("p2", "m1", 1));

    let resp = router
        .chat(&one_user_message(), &[], "m1", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "from-alt");

    // 失败也进了 metrics
    let m = router.metrics().get_metrics("p1");
    assert_eq!(m.total_failures, 1);
    assert_eq!(m.success_rate, 0.0);
}

#[tokio::test]
async fn test_w4c_router_chat_retriable_all_fail_returns_last_error() {
    let router = Router::new(RouterConfig::default());
    let primary = ScriptedProvider::new(
        "p1",
        "m1",
        vec![Err(FailoverError::RateLimit {
            provider: "p1".into(),
            model: "m1".into(),
            retry_after: None,
        })],
    );
    let alt = ScriptedProvider::new(
        "p2",
        "m1",
        vec![Err(FailoverError::Overloaded {
            provider: "p2".into(),
        })],
    );
    router.register_provider("p1", primary);
    router.register_provider("p2", alt);
    router.add_candidate(plain_candidate("p1", "m1", 5));
    router.add_candidate(plain_candidate("p2", "m1", 1));

    let err = router
        .chat(&one_user_message(), &[], "m1", &ChatOptions::default())
        .await
        .unwrap_err();
    // fallback 全败后回传 primary 的原始错误
    assert!(matches!(err, FailoverError::RateLimit { .. }));
}

#[tokio::test]
async fn test_w4c_router_chat_non_retriable_returns_immediately() {
    let router = Router::new(RouterConfig::default());
    let primary = ScriptedProvider::new(
        "p1",
        "m1",
        vec![Err(FailoverError::Auth {
            provider: "p1".into(),
            model: "m1".into(),
            status: 401,
        })],
    );
    let alt = ScriptedProvider::new("p2", "m1", vec![Ok(ok_response("never", 10))]);
    router.register_provider("p1", primary);
    router.register_provider("p2", alt);
    router.add_candidate(plain_candidate("p1", "m1", 5));
    router.add_candidate(plain_candidate("p2", "m1", 1));

    let err = router
        .chat(&one_user_message(), &[], "m1", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FailoverError::Auth { status: 401, .. }));
}

#[tokio::test]
async fn test_w4c_router_chat_no_provider_for_model_returns_unknown() {
    let router = Router::new(RouterConfig::default());
    // 有候选但没注册任何 provider
    router.add_candidate(plain_candidate("ghost", "m1", 1));

    let err = router
        .chat(&one_user_message(), &[], "m1", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Unknown { provider, message } => {
            assert_eq!(provider, "router");
            assert!(message.contains("no provider available for model: m1"));
        }
        other => panic!("expected Unknown, got {:?}", other),
    }
}

#[tokio::test]
async fn test_w4c_router_chat_passes_candidate_model_to_provider() {
    // 前缀形态请求命中裸名候选：provider 收到候选的 model 而非请求原文。
    let router = Router::new(RouterConfig::default());
    let prov = ScriptedProvider::new("p1", "bare-model", vec![Ok(ok_response("ok", 5))]);
    router.register_provider("p1", prov.clone());
    router.add_candidate(plain_candidate("p1", "bare-model", 1));

    let _ = router
        .chat(
            &one_user_message(),
            &[],
            "prov/bare-model",
            &ChatOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(prov.seen_models.lock()[0], "bare-model");
}

#[test]
fn test_w4c_select_with_policy_empty_match_returns_first() {
    let router = Router::new(RouterConfig::default());
    router.add_candidate(plain_candidate("pa", "model-a", 1));
    router.add_candidate(plain_candidate("pb", "model-b", 2));
    // 请求的 model 谁都不匹配 → 返回第一个候选
    let pick = router
        .select_with_policy(Policy::Cost, "totally-unknown")
        .unwrap();
    assert_eq!(pick.model, "model-a");
}

#[test]
fn test_w4c_select_with_policy_single_match_short_circuits() {
    let router = Router::new(RouterConfig::default());
    router.add_candidate(plain_candidate("pa", "model-a", 1));
    router.add_candidate(plain_candidate("pb", "model-b", 9));
    // 唯一匹配的候选直接返回（不进入策略比较）
    let pick = router
        .select_with_policy(Policy::Quality, "model-b")
        .unwrap();
    assert_eq!(pick.model, "model-b");
}

#[test]
fn test_w4c_semantic_policy_via_select_degrades_to_priority() {
    // 单匹配短路不经策略臂
    let router = Router::new(RouterConfig {
        default_policy: Policy::Semantic,
        ..Default::default()
    });
    router.add_candidate(plain_candidate("pa", "model-a", 1));
    router.add_candidate(plain_candidate("pb", "model-b", 7));
    let pick = router.select("model-a").unwrap();
    assert_eq!(pick.model, "model-a");

    // 同 model 双候选 → 走 Semantic 策略臂（无 intent 载荷）→ 降级为优先级
    let router2 = Router::new(RouterConfig {
        default_policy: Policy::Semantic,
        ..Default::default()
    });
    router2.add_candidate(plain_candidate("pa", "m", 1));
    router2.add_candidate(plain_candidate("pb", "m", 7));
    let pick2 = router2.select("m").unwrap();
    assert_eq!(pick2.provider, "pb");
}

#[test]
fn test_w4c_semantic_all_descriptions_fail_embed_falls_back_priority() {
    // intent 可嵌入但所有候选描述都嵌不出 → 优先级兜底
    let router = Router::new(RouterConfig::default());
    router.set_semantic_embedder(Arc::new(|text: &str| -> Option<Vec<f32>> {
        if text.contains("intent-marker") {
            Some(vec![1.0, 0.0])
        } else {
            None
        }
    }));
    let low = sem_candidate("pa", "model-a", "desc-a", 1);
    let high = sem_candidate("pb", "model-b", "desc-b", 9);
    let pick = router.select_with_semantic("intent-marker query", &[low, high]);
    assert_eq!(pick.as_ref().map(|c| c.model.as_str()), Some("model-b"));
}

#[test]
fn test_w4c_semantic_partial_description_embed_skips_unembeddable() {
    // 一个候选的描述嵌不出（跳过），另一个能嵌 → 即便被跳过者优先级更高，
    // 也选唯一可比较的那个。
    let router = Router::new(RouterConfig::default());
    router.set_semantic_embedder(Arc::new(|text: &str| -> Option<Vec<f32>> {
        if text.contains("chat") {
            Some(vec![0.0, 1.0])
        } else {
            None
        }
    }));
    let unembeddable = sem_candidate("pa", "model-a", "unembeddable desc", 99);
    let embeddable = sem_candidate("pb", "model-b", "chat stuff", 1);
    let pick = router.select_with_semantic("chat intent", &[unembeddable, embeddable]);
    assert_eq!(pick.as_ref().map(|c| c.model.as_str()), Some("model-b"));
}

#[test]
fn test_w4c_policy_weights_serde_defaults() {
    // serde 缺字段走 default_weight()（0.33）
    let w: PolicyWeights = serde_json::from_str("{}").unwrap();
    assert!((w.cost - 0.33).abs() < 1e-9);
    assert!((w.quality - 0.33).abs() < 1e-9);
    assert!((w.latency - 0.33).abs() < 1e-9);
    let w2: PolicyWeights = serde_json::from_str(r#"{"cost": 1.0}"#).unwrap();
    assert!((w2.cost - 1.0).abs() < 1e-9);
    assert!((w2.quality - 0.33).abs() < 1e-9);
}

#[test]
fn test_w4c_metrics_get_metrics_empty_vec_after_prune() {
    // prune 清空样本后 get_metrics 命中 is_empty 臂 → 全默认值
    let collector = MetricsCollector::new(10);
    collector.record(Metric {
        provider: "p".to_string(),
        latency_ms: 5,
        success: true,
        tokens_used: 100,
        cost: 0.1,
        timestamp: chrono::Local::now() - chrono::Duration::minutes(10),
    });
    assert_eq!(collector.get_metrics("p").total_requests, 1);
    collector.prune(std::time::Duration::from_secs(60));
    let m = collector.get_metrics("p");
    assert_eq!(m.total_requests, 0);
    assert_eq!(m.success_rate, 0.0);
    assert_eq!(m.avg_latency_ms, 0.0);
}

#[test]
fn test_w4c_cosine_guard_arms() {
    // 空/维度不匹配 → 0.0（正交处理）
    assert_eq!(super::cosine(&[], &[1.0]), 0.0);
    assert_eq!(super::cosine(&[1.0, 2.0], &[1.0]), 0.0);
    // 同向 → 1.0；反向 → -1.0
    assert!((super::cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
    assert!((super::cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
}
