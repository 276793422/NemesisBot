use super::*;

#[test]
fn test_default_aliases() {
    let config = RouterConfig::default();
    assert_eq!(config.aliases.get("fast").unwrap(), "groq/llama-3");
    assert_eq!(config.aliases.get("smart").unwrap(), "anthropic/claude-sonnet-4-6");
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
    });
    router.add_candidate(Candidate {
        provider: "azure".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.03,
        quality_score: 0.9,
        priority: 2,
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
    });
    router.add_candidate(Candidate {
        provider: "deepseek".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.7,
        priority: 1,
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
        timestamp: chrono::Utc::now(),
    });
    collector.record(Metric {
        provider: "openai".to_string(),
        latency_ms: 200,
        success: false,
        tokens_used: 0,
        cost: 0.0,
        timestamp: chrono::Utc::now(),
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
            timestamp: chrono::Utc::now(),
        });
    }
    let metrics = collector.get_metrics("test");
    assert_eq!(metrics.total_requests, 3); // Only last 3 kept
}

#[test]
fn test_default_aliases_function() {
    let aliases = default_aliases();
    assert_eq!(aliases.get("fast").unwrap(), "groq/llama-3.3-70b-versatile");
    assert_eq!(aliases.get("smart").unwrap(), "anthropic/claude-sonnet-4-20250514");
    assert_eq!(aliases.get("cheap").unwrap(), "deepseek/deepseek-chat");
    assert_eq!(aliases.get("local").unwrap(), "ollama/llama3.3");
}

#[test]
fn test_resolve_alias_function() {
    let aliases = default_aliases();
    assert_eq!(resolve_alias(&aliases, "fast"), Some("groq/llama-3.3-70b-versatile".to_string()));
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
    assert_eq!(merged.get("smart").unwrap(), "anthropic/claude-sonnet-4-20250514");
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
        timestamp: chrono::Utc::now(),
    });
    collector.record(Metric {
        provider: "anthropic".to_string(),
        latency_ms: 200,
        success: true,
        tokens_used: 300,
        cost: 0.01,
        timestamp: chrono::Utc::now(),
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
        timestamp: chrono::Utc::now(),
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
        timestamp: chrono::Utc::now() - chrono::Duration::hours(2),
    });
    // Recent sample
    collector.record(Metric {
        provider: "test".to_string(),
        latency_ms: 50,
        success: true,
        tokens_used: 200,
        cost: 0.005,
        timestamp: chrono::Utc::now(),
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
            priority: i as i32,
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
            timestamp: chrono::Utc::now(),
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
    });
    router.add_candidate(Candidate {
        provider: "deepseek".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.7,
        priority: 2,
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
    });
    router.add_candidate(Candidate {
        provider: "slow-provider".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.7,
        priority: 2,
    });

    // Record metrics for the slow provider (higher latency)
    router.metrics().record(Metric {
        provider: "fast-provider".to_string(),
        latency_ms: 50,
        success: true,
        tokens_used: 100,
        cost: 0.001,
        timestamp: chrono::Utc::now(),
    });
    router.metrics().record(Metric {
        provider: "slow-provider".to_string(),
        latency_ms: 500,
        success: true,
        tokens_used: 100,
        cost: 0.001,
        timestamp: chrono::Utc::now(),
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
    });
    router.add_candidate(Candidate {
        provider: "provider-b".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.01,
        quality_score: 0.7,
        priority: 2,
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
    });
    router.add_candidate(Candidate {
        provider: "expensive".to_string(),
        model: "gpt-4".to_string(),
        cost_per_1k: 0.10,
        quality_score: 0.9,
        priority: 2,
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
        timestamp: chrono::Utc::now(),
    });
    collector.record(Metric {
        provider: "test".to_string(),
        latency_ms: 200,
        success: true,
        tokens_used: 200,
        cost: 0.02,
        timestamp: chrono::Utc::now(),
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
        timestamp: chrono::Utc::now(),
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
        timestamp: chrono::Utc::now(),
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
    assert_eq!(serde_json::to_string(&Policy::Quality).unwrap(), "\"quality\"");
    assert_eq!(serde_json::to_string(&Policy::Latency).unwrap(), "\"latency\"");
    assert_eq!(serde_json::to_string(&Policy::RoundRobin).unwrap(), "\"round_robin\"");
    assert_eq!(serde_json::to_string(&Policy::Fallback).unwrap(), "\"fallback\"");
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
        async fn chat(&self, _: &[Message], _: &[ToolDefinition], _: &str, _: &ChatOptions) -> Result<LLMResponse, FailoverError> {
            Ok(LLMResponse { content: "mock".into(), tool_calls: vec![], finish_reason: "stop".into(), usage: None, reasoning_content: None, extra: HashMap::new() })
        }
        fn default_model(&self) -> &str { "mock-model" }
        fn name(&self) -> &str { "mock" }
    }

    let router = Router::new(RouterConfig::default());
    router.register_provider("mock", Arc::new(MockProvider));
    router.add_candidate(Candidate {
        provider: "mock".to_string(),
        model: "mock-model".to_string(),
        cost_per_1k: 0.0,
        quality_score: 0.5,
        priority: 1,
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
        timestamp: chrono::Utc::now(),
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
