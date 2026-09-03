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
fn resolve_max_output_tokens_from_config() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model": "qwen/qwen3-30b-a3b", "model_name": "qwen3-30b-a3b"},
            {"model": "anthropic/claude-sonnet-4", "model_name": "claude-sonnet-4",
             "max_output_tokens": 16384}
        ]
    });
    // Declared cap returned verbatim.
    assert_eq!(
        resolve_max_output_tokens(&cfg, "claude-sonnet-4"),
        Some(16384)
    );
    // Match by full `model` id also works.
    assert_eq!(
        resolve_max_output_tokens(&cfg, "anthropic/claude-sonnet-4"),
        Some(16384)
    );
    // Field absent on the entry → None (caller falls back to the default).
    assert_eq!(resolve_max_output_tokens(&cfg, "qwen3-30b-a3b"), None);
    // Unknown alias → None.
    assert_eq!(resolve_max_output_tokens(&cfg, "nonexistent"), None);
    // No model_list at all → None.
    assert_eq!(
        resolve_max_output_tokens(&serde_json::json!({}), "deepseek-v4-flash"),
        None
    );
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
    assert_eq!(
        resolve_display_model(&cfg, "qwen3-30b-a3b"),
        "qwen/qwen3-30b-a3b"
    );
    assert_eq!(
        resolve_display_model(&cfg, "claude-sonnet-4"),
        "anthropic/claude-sonnet-4"
    );
    // Match by full `model` id also works.
    assert_eq!(
        resolve_display_model(&cfg, "qwen/qwen3-30b-a3b"),
        "qwen/qwen3-30b-a3b"
    );
    // Unknown alias → falls back to the alias itself.
    assert_eq!(resolve_display_model(&cfg, "nonexistent"), "nonexistent");
    // No model_list at all → fallback.
    assert_eq!(
        resolve_display_model(&serde_json::json!({}), "deepseek-v4-flash"),
        "deepseek-v4-flash"
    );
}

#[test]
fn display_lowercase() {
    assert_eq!(ModelTier::Mini.to_string(), "mini");
    assert_eq!(ModelTier::Auto.to_string(), "auto");
}

// U16 (sixth batch): per-model context_window resolution.
#[test]
fn resolve_context_window_per_model() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "opus", "model": "anthropic/claude-opus-4.7-fast",
             "context_window": 1000000},
            {"model_name": "small", "model": "x/small", "context_window": 0},
            {"model_name": "plain", "model": "x/plain"}
        ]
    });
    assert_eq!(
        resolve_context_window(&cfg, "opus"),
        Some(1000000),
        "match by alias"
    );
    assert_eq!(
        resolve_context_window(&cfg, "anthropic/claude-opus-4.7-fast"),
        Some(1000000),
        "match by full id"
    );
    assert_eq!(
        resolve_context_window(&cfg, "small"),
        None,
        "zero window filtered out"
    );
    assert_eq!(resolve_context_window(&cfg, "plain"), None, "field absent");
    assert_eq!(
        resolve_context_window(&serde_json::json!({}), "anything"),
        None,
        "no model_list"
    );
}

// Sixth-batch sweep: delegation tools are Normal-tier and above.
#[test]
fn tier_lists_delegation_tools() {
    let normal = tier_allowed_tools(ModelTier::Normal);
    assert!(normal.contains(&"claude_code"), "claude_code in Normal");
    assert!(
        normal.contains(&"codex_delegate"),
        "codex_delegate in Normal"
    );
    let mini = tier_allowed_tools(ModelTier::Mini);
    assert!(!mini.contains(&"claude_code"), "Mini excluded by design");
    assert!(!mini.contains(&"codex_delegate"), "Mini excluded by design");
}

#[test]
fn test_resolve_reasoning_effort_tiers() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "a", "model": "x/a", "reasoning_effort": "high"},
            {"model_name": "b", "model": "x/b", "reasoning_effort": "off"},
            {"model_name": "c", "model": "x/c"},
            {"model_name": "d", "model": "x/d", "reasoning_effort": "weird"}
        ]
    });
    assert_eq!(resolve_reasoning_effort(&cfg, "a").as_deref(), Some("high"));
    // "off" → None (send nothing).
    assert_eq!(resolve_reasoning_effort(&cfg, "b"), None);
    // Missing field → None.
    assert_eq!(resolve_reasoning_effort(&cfg, "c"), None);
    // Unknown tier → None (no garbage on the wire).
    assert_eq!(resolve_reasoning_effort(&cfg, "d"), None);
}

// ============================================================================
// T10（多模态 goal）：vision 能力解析（解析序 User > Probe > Name > Default）
// ============================================================================

#[test]
fn vision_name_detection_boundaries() {
    // 组合规则：qwen/kimi 系 -VL 变体（归一化去点/连字符）。
    assert!(detect_vision_from_name("qwen2.5-vl"));
    assert!(detect_vision_from_name("Qwen2.5-VL-72B"));
    assert!(detect_vision_from_name("qwen/qwen2.5-vl-72b-instruct"));
    assert!(detect_vision_from_name("kimi-vl-a3b"));
    // 单标记家族。
    assert!(detect_vision_from_name("gpt-4o"));
    assert!(detect_vision_from_name("gpt-4.1"));
    assert!(detect_vision_from_name("o1-mini"));
    assert!(detect_vision_from_name("gemini-2.5-flash"));
    assert!(detect_vision_from_name("claude-sonnet-4"));
    assert!(detect_vision_from_name("llava-1.5"));
    assert!(detect_vision_from_name("InternVL2-8B"));
    assert!(detect_vision_from_name("glm-4.1v-thinking"));
    assert!(detect_vision_from_name("my-model-vision-addon"));
    // 纯文本模型不命中（false → 调用方落到名字/默认层）。
    assert!(!detect_vision_from_name("qwen2.5-72b"));
    assert!(!detect_vision_from_name("kimi-k2-instruct"));
    assert!(!detect_vision_from_name("deepseek-v4-flash"));
    assert!(!detect_vision_from_name("astron-code-latest"));
}

#[test]
fn vision_resolution_user_pin_wins() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "pinned-yes", "model": "x/pinned-yes", "vision": "yes"},
            {"model_name": "pinned-no", "model": "x/pinned-no", "vision": "no"},
            // 用户钉 no 压过名字命中 + 探针实测。
            {"model_name": "qwen2.5-vl", "model": "x/qwen2.5-vl",
             "vision": "no", "vision_probe": true},
            // 用户钉 yes 压过探针 false。
            {"model_name": "plain", "model": "x/plain",
             "vision": "yes", "vision_probe": false}
        ]
    });
    assert_eq!(
        resolve_active_vision(&cfg, "pinned-yes"),
        VisionResolution {
            supported: true,
            source: VisionSource::User
        }
    );
    assert_eq!(
        resolve_active_vision(&cfg, "pinned-no"),
        VisionResolution {
            supported: false,
            source: VisionSource::User
        }
    );
    assert_eq!(
        resolve_active_vision(&cfg, "qwen2.5-vl"),
        VisionResolution {
            supported: false,
            source: VisionSource::User
        },
        "用户钉死压过探针与名字"
    );
    assert_eq!(
        resolve_active_vision(&cfg, "plain"),
        VisionResolution {
            supported: true,
            source: VisionSource::User
        }
    );
}

// L8（2026-09-04 四轮盲审）：钉死值宽容解析——大写 / JSON 布尔 / true/false
// 变体都算用户钉死，不再静默忽略落回名字识别（用户明确表达了意图）。
#[test]
fn vision_resolution_user_pin_tolerant_forms() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "upper-no", "model": "x/u", "vision": "NO"},
            {"model_name": "mixed-yes", "model": "x/m", "vision": "Yes"},
            {"model_name": "bool-true", "model": "x/bt", "vision": true},
            {"model_name": "bool-false", "model": "x/bf", "vision": false},
            {"model_name": "str-true", "model": "x/st", "vision": "true"},
            {"model_name": "padded-no", "model": "x/pn", "vision": " no "}
        ]
    });
    let no = VisionResolution {
        supported: false,
        source: VisionSource::User,
    };
    let yes = VisionResolution {
        supported: true,
        source: VisionSource::User,
    };
    assert_eq!(resolve_active_vision(&cfg, "upper-no"), no, "大写 NO");
    assert_eq!(
        resolve_active_vision(&cfg, "mixed-yes"),
        yes,
        "混合大小写 Yes"
    );
    assert_eq!(
        resolve_active_vision(&cfg, "bool-true"),
        yes,
        "JSON 布尔 true"
    );
    assert_eq!(
        resolve_active_vision(&cfg, "bool-false"),
        no,
        "JSON 布尔 false"
    );
    assert_eq!(resolve_active_vision(&cfg, "str-true"), yes, "字符串 true");
    assert_eq!(resolve_active_vision(&cfg, "padded-no"), no, "首尾空白 no");
}

/// 垃圾钉死值（空串/乱写）不钉死 → 落回名字识别（qwen-vl 命中支持）。
#[test]
fn vision_resolution_garbage_pin_falls_through_to_name() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "qwen2.5-vl", "model": "x/q", "vision": "maybe"},
            {"model_name": "plain", "model": "x/p", "vision": ""}
        ]
    });
    assert_eq!(
        resolve_active_vision(&cfg, "qwen2.5-vl"),
        VisionResolution {
            supported: true,
            source: VisionSource::Name
        },
        "垃圾钉死值落回名字识别"
    );
    assert_eq!(
        resolve_active_vision(&cfg, "plain"),
        VisionResolution {
            supported: true,
            source: VisionSource::DefaultAllow
        },
        "空串钉死值落回默认放行"
    );
}

#[test]
fn vision_resolution_probe_overrides_name() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "gpt-4o", "model": "x/gpt-4o", "vision_probe": false},
            {"model_name": "plain", "model": "x/plain", "vision_probe": true}
        ]
    });
    // 探针实测 false 压过名字命中。
    assert_eq!(
        resolve_active_vision(&cfg, "gpt-4o"),
        VisionResolution {
            supported: false,
            source: VisionSource::Probe
        }
    );
    // 探针实测 true（名字认不出时也放行，来源标记为 probe）。
    assert_eq!(
        resolve_active_vision(&cfg, "plain"),
        VisionResolution {
            supported: true,
            source: VisionSource::Probe
        }
    );
}

#[test]
fn vision_resolution_name_and_default() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "qwen2.5-vl", "model": "vendor/qwen2.5-vl-72b"},
            // opaque 别名 + real_name 参与 名字识别。
            {"model_name": "astron-code-latest", "model": "vendor/astron-code-latest",
             "real_name": "Qwen2.5-VL-72B"},
            {"model_name": "plain", "model": "x/plain"}
        ]
    });
    // 名字命中。
    assert_eq!(
        resolve_active_vision(&cfg, "qwen2.5-vl"),
        VisionResolution {
            supported: true,
            source: VisionSource::Name
        }
    );
    // real_name 命中（opaque 别名场景）。
    assert_eq!(
        resolve_active_vision(&cfg, "astron-code-latest"),
        VisionResolution {
            supported: true,
            source: VisionSource::Name
        }
    );
    // 认不出 → 默认放行（不是拒绝）。
    assert_eq!(resolve_active_vision(&cfg, "plain"), vision_default_allow());
    // 条目缺失 / 无 model_list → 默认放行。
    assert_eq!(
        resolve_active_vision(&cfg, "nonexistent"),
        vision_default_allow()
    );
    assert_eq!(
        resolve_active_vision(&serde_json::json!({}), "anything"),
        vision_default_allow()
    );
}
