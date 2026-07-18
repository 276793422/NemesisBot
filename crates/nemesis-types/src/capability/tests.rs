    use super::*;

    #[test]
    fn size_buckets() {
        assert_eq!(tier_from_size_b(8), ModelTier::Mini);
        assert_eq!(tier_from_size_b(30), ModelTier::Mini);
        assert_eq!(tier_from_size_b(35), ModelTier::Mini);
        assert_eq!(tier_from_size_b(70), ModelTier::Normal);
        assert_eq!(tier_from_size_b(120), ModelTier::Normal);
        assert_eq!(tier_from_size_b(200), ModelTier::Big);
        assert_eq!(tier_from_size_b(405), ModelTier::Big);
    }

    #[test]
    fn parse_markers() {
        assert_eq!(parse_size_marker("qwen3-30b-a3b"), Some(30));
        assert_eq!(parse_size_marker("Llama-3-8B"), Some(8));
        assert_eq!(parse_size_marker("Qwen2.5-72B"), Some(72));
        assert_eq!(parse_size_marker("gpt-4"), None);
        assert_eq!(parse_size_marker("deepseek-v4-flash"), None);
        assert_eq!(parse_size_marker("1.5b"), Some(1));
    }

    #[test]
    fn detect_explicit_size_wins() {
        let h = TierHint {
            full_model: Some("vendor/anything".into()),
            real_name: None,
            size_b: Some(70),
        };
        assert_eq!(detect_tier(&h), ModelTier::Normal);
    }

    #[test]
    fn detect_name_marker() {
        let h = TierHint {
            full_model: Some("qwen/qwen3-30b-a3b".into()),
            real_name: None,
            size_b: None,
        };
        assert_eq!(detect_tier(&h), ModelTier::Mini);
    }

    #[test]
    fn detect_real_name_marker_overrides_opaque_alias() {
        // astron-code-latest is opaque, but the user gave a real_name.
        let h = TierHint {
            full_model: Some("vendor/astron-code-latest".into()),
            real_name: Some("Qwen3-30B".into()),
            size_b: None,
        };
        assert_eq!(detect_tier(&h), ModelTier::Mini);
    }

    #[test]
    fn detect_keyword_big() {
        let h = TierHint {
            full_model: Some("anthropic/claude-sonnet-4".into()),
            real_name: None,
            size_b: None,
        };
        assert_eq!(detect_tier(&h), ModelTier::Big);
    }

    #[test]
    fn detect_unknown_defaults_big() {
        // Opaque alias, no real_name, no size → safest default is Big (full).
        let h = TierHint {
            full_model: Some("vendor/astron-code-latest".into()),
            real_name: None,
            size_b: None,
        };
        assert_eq!(detect_tier(&h), ModelTier::Big);
    }

    #[test]
    fn resolve_passes_through_explicit() {
        let h = TierHint {
            full_model: Some("qwen/qwen3-30b".into()),
            real_name: None,
            size_b: None,
        };
        assert_eq!(ModelTier::Mini.resolve(&h), ModelTier::Mini);
        assert_eq!(ModelTier::Big.resolve(&h), ModelTier::Big);
    }

    #[test]
    fn resolve_auto_uses_hint() {
        let h = TierHint {
            full_model: Some("qwen/qwen3-30b".into()),
            real_name: None,
            size_b: None,
        };
        assert_eq!(ModelTier::Auto.resolve(&h), ModelTier::Mini);
    }

    #[test]
    fn serde_roundtrip() {
        let s = serde_json::to_string(&ModelTier::Mini).unwrap();
        assert_eq!(s, "\"mini\"");
        let back: ModelTier = serde_json::from_str("\"normal\"").unwrap();
        assert_eq!(back, ModelTier::Normal);
        let auto: ModelTier = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(auto, ModelTier::Auto);
    }

    #[test]
    fn retry_budget_per_tier() {
        assert_eq!(ModelTier::Mini.validation_retry_budget(), 3);
        assert_eq!(ModelTier::Normal.validation_retry_budget(), 2);
        assert_eq!(ModelTier::Big.validation_retry_budget(), 1);
    }

    #[test]
    fn resolve_active_tier_from_config() {
        let cfg = serde_json::json!({
            "model_list": [
                {"model": "qwen/qwen3-30b-a3b", "model_name": "qwen3-30b-a3b"},
                {"model": "anthropic/claude-sonnet-4", "model_name": "claude-sonnet-4",
                 "model_tier": "big"}
            ]
        });
        // Auto-detected from name.
        assert_eq!(resolve_active_tier(&cfg, "qwen3-30b-a3b"), ModelTier::Mini);
        // Explicit tier, passes through.
        assert_eq!(resolve_active_tier(&cfg, "claude-sonnet-4"), ModelTier::Big);
        // Unknown alias → Big (safest default).
        assert_eq!(resolve_active_tier(&cfg, "nonexistent"), ModelTier::Big);
    }

    #[test]
    fn resolve_display_model_basic() {
        let cfg = serde_json::json!({
            "model_list": [
                {"model": "qwen/qwen3-30b-a3b", "model_name": "qwen3-30b-a3b"},
                {"model": "anthropic/claude-sonnet-4", "model_name": "claude-sonnet-4"}
            ]
        });
        // Match by model_name → returns the `model` (provider/name) field.
        assert_eq!(resolve_display_model(&cfg, "qwen3-30b-a3b"), "qwen/qwen3-30b-a3b");
        assert_eq!(resolve_display_model(&cfg, "claude-sonnet-4"), "anthropic/claude-sonnet-4");
        // Match by full `model` id also works.
        assert_eq!(resolve_display_model(&cfg, "qwen/qwen3-30b-a3b"), "qwen/qwen3-30b-a3b");
        // Unknown alias → falls back to the alias itself.
        assert_eq!(resolve_display_model(&cfg, "nonexistent"), "nonexistent");
        // No model_list at all → fallback.
        assert_eq!(resolve_display_model(&serde_json::json!({}), "deepseek-v4-flash"), "deepseek-v4-flash");
    }

    #[test]
    fn display_lowercase() {
        assert_eq!(ModelTier::Mini.to_string(), "mini");
        assert_eq!(ModelTier::Auto.to_string(), "auto");
    }
