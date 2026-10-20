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
    // P2C（2026-09-12 NB-15）：Big 1→2——并行工具批首个坏调用即烧光
    // budget=1，模型没机会看到回灌错误自纠；2 给一次下轮自纠机会。
    assert_eq!(ModelTier::Big.validation_retry_budget(), 2);
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

// 2026-10-20 压缩点一等公民化：per-model 绝对压缩点键。
#[test]
fn resolve_compact_context_tokens_per_model() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "big", "model": "x/big", "compact_context_tokens": 200000},
            {"model_name": "zero", "model": "x/zero", "compact_context_tokens": 0},
            {"model_name": "plain", "model": "x/plain"}
        ]
    });
    assert_eq!(
        resolve_compact_context_tokens(&cfg, "big"),
        Some(200000),
        "match by alias"
    );
    assert_eq!(
        resolve_compact_context_tokens(&cfg, "x/big"),
        Some(200000),
        "match by full id"
    );
    assert_eq!(
        resolve_compact_context_tokens(&cfg, "zero"),
        None,
        "zero filtered out (= unset)"
    );
    assert_eq!(
        resolve_compact_context_tokens(&cfg, "plain"),
        None,
        "field absent = 75% chain fallback"
    );
    assert_eq!(
        resolve_compact_context_tokens(&serde_json::json!({}), "anything"),
        None,
        "no model_list"
    );
}

// 2026-10-20 真浏览器 E2E 实证回归锁：活 gateway 的 active_model 是剥前缀
// 裸名（`resolve_from_model_config` 切 '/' 取尾段；CLI `model add --default`
// 写 agents.defaults.llm 同为裸名），自定义别名条目（model_name ≠ 裸名）
// 在纯精确匹配下全族失联——context_window=131072 被无视、压缩点落价目表。
// find_model_entry 的 bare-suffix 回落修复；此处钉核心语义。
#[test]
fn find_model_entry_bare_suffix_fallback() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "glm-test", "model": "zhipu/glm-4.7", "context_window": 131072}
        ]
    });
    // 裸名（活 gateway active_model 形态）经 bare-suffix 命中——修复前 None。
    let entry = find_model_entry(&cfg, "glm-4.7").expect("bare suffix must resolve");
    assert_eq!(
        entry.get("context_window").and_then(|v| v.as_i64()),
        Some(131072)
    );
    // 精确 name / full 优先级不变。
    assert!(find_model_entry(&cfg, "glm-test").is_some());
    assert!(find_model_entry(&cfg, "zhipu/glm-4.7").is_some());
    assert!(find_model_entry(&cfg, "unrelated").is_none(), "no match");
    // active 带 vendor 前缀时末段参与 bare-suffix（`openai/gpt-4o` ↔
    // `azure/gpt-4o` 形态）。
    let cfg2 = serde_json::json!({
        "model_list": [
            {"model_name": "aliased", "model": "azure/gpt-4o", "max_output_tokens": 8192}
        ]
    });
    assert!(find_model_entry(&cfg2, "openai/gpt-4o").is_some());
}

#[test]
fn find_model_entry_exact_beats_bare_suffix() {
    // 精确匹配跨数组序压过 bare-suffix：bare 候选在前、精确候选在后，
    // 仍必须返回精确条目（find(exact) 先于 or_else(find(bare))）。
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "other", "model": "z/glm-4.7", "context_window": 1},
            {"model_name": "glm-4.7", "model": "zhipu/glm-4.7", "context_window": 131072}
        ]
    });
    let entry = find_model_entry(&cfg, "glm-4.7").expect("exact name must win");
    assert_eq!(
        entry.get("context_window").and_then(|v| v.as_i64()),
        Some(131072),
        "exact entry wins over earlier bare-suffix candidate"
    );
}

#[test]
fn resolve_context_window_bare_name_reaches_custom_alias_entry() {
    // 活 gateway 场景端到端：alias=裸名 "glm-4.7" 必须读到自定义别名条目的
    // context_window（修复前落价目表/兜底——context_window 与压缩点同源同族）。
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "glm-test", "model": "zhipu/glm-4.7", "context_window": 131072}
        ]
    });
    assert_eq!(
        resolve_context_window(&cfg, "glm-4.7"),
        Some(131072),
        "context_window reachable via bare name"
    );
    assert_eq!(
        resolve_compact_context_tokens(&cfg, "glm-4.7"),
        None,
        "unset compact key stays None (= 75% chain)"
    );
}

#[test]
fn resolve_compact_context_tokens_bare_name_reaches_custom_alias_entry() {
    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "glm-test", "model": "zhipu/glm-4.7", "compact_context_tokens": 50000}
        ]
    });
    assert_eq!(
        resolve_compact_context_tokens(&cfg, "glm-4.7"),
        Some(50000),
        "user compact point reachable via bare name"
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

// B4 (2026-09-05): background-process trio is Normal-tier and above.
#[test]
fn tier_lists_background_tools() {
    let normal = tier_allowed_tools(ModelTier::Normal);
    assert!(
        normal.contains(&"background_start"),
        "background_start in Normal"
    );
    assert!(
        normal.contains(&"background_output"),
        "background_output in Normal"
    );
    assert!(
        normal.contains(&"background_kill"),
        "background_kill in Normal"
    );
    let mini = tier_allowed_tools(ModelTier::Mini);
    assert!(
        !mini.contains(&"background_start"),
        "Mini excluded by design (job_id bookkeeping burns turns)"
    );
    // Big/Auto see everything (empty slice = no filtering).
    assert!(tier_allowed_tools(ModelTier::Big).is_empty());
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

#[test]
fn protocol_normalization_canonical_set() {
    assert_eq!(normalize_model_protocol("anthropic").unwrap(), "anthropic");
    assert_eq!(
        normalize_model_protocol("claude").unwrap(),
        "anthropic",
        "claude 是用户侧别名"
    );
    assert_eq!(normalize_model_protocol("openai").unwrap(), "openai");
    assert_eq!(
        normalize_model_protocol("chat-completions").unwrap(),
        "openai",
        "chat-completions 是 openai 的显式别名"
    );
    assert_eq!(normalize_model_protocol("responses").unwrap(), "responses");
}

#[test]
fn protocol_normalization_empty_and_trim() {
    assert_eq!(
        normalize_model_protocol("").unwrap(),
        "",
        "空 = 清除（自动推断）"
    );
    assert_eq!(
        normalize_model_protocol("  ").unwrap(),
        "",
        "纯空白等价清除"
    );
    assert_eq!(
        normalize_model_protocol("  Claude \n").unwrap(),
        "anthropic",
        "trim + 大小写不敏感"
    );
}

#[test]
fn protocol_normalization_unknown_loud() {
    let err = normalize_model_protocol("grpc").unwrap_err();
    assert!(err.contains("grpc"));
    assert!(err.contains("anthropic | openai | responses"));
}

/// 集群专业职能框架 M4：images-openai 进归一值集（不在 factory 的对话
/// Provider 集内——构造路径 loud 拒绝），is_image_protocol 单一谓词。
#[test]
fn protocol_images_openai_and_predicate() {
    assert_eq!(
        normalize_model_protocol("images-openai").unwrap(),
        "images-openai"
    );
    assert_eq!(
        normalize_model_protocol(" Images-OpenAI ").unwrap(),
        "images-openai",
        "trim + 大小写不敏感"
    );
    assert!(is_image_protocol("images-openai"));
    assert!(is_image_protocol(" IMAGES-OPENAI "));
    assert!(!is_image_protocol(""));
    assert!(!is_image_protocol("openai"));
    assert!(!is_image_protocol("anthropic"));
    assert!(!is_image_protocol("responses"));
}

/// M6-1：`agents.defaults.max_tool_iterations` 解析——三态（在值/键缺席/
/// 0=不限透传）。
#[test]
fn resolve_max_tool_iterations_tri_state() {
    let with_key: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"max_tool_iterations": 42}}
    });
    assert_eq!(resolve_max_tool_iterations(&with_key), Some(42));

    let zero: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"max_tool_iterations": 0}}
    });
    assert_eq!(resolve_max_tool_iterations(&zero), Some(0), "0=不限照透传");

    let no_key: serde_json::Value = serde_json::json!({"model_list": []});
    assert_eq!(resolve_max_tool_iterations(&no_key), None);

    let no_section: serde_json::Value = serde_json::json!({});
    assert_eq!(resolve_max_tool_iterations(&no_section), None);

    // 负值照透传——消费侧（run_loop organ 1）<=0 统一映射不限。
    let negative: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"max_tool_iterations": -1}}
    });
    assert_eq!(resolve_max_tool_iterations(&negative), Some(-1));
}

/// §6.2（2026-10-12 无人值守）：`agents.defaults.max_output_ceiling` 翻倍
/// 阶梯封顶解析——值生效 / 0 与负数与非数字回落默认 / 缺席回落默认。
/// 惯例与 `max_tool_iterations` 0=不限 相反：封顶没有不限形态。
#[test]
fn resolve_max_output_ceiling_defaulting() {
    let with_key: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"max_output_ceiling": 524288}}
    });
    assert_eq!(resolve_max_output_ceiling(&with_key), 524_288);

    let zero: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"max_output_ceiling": 0}}
    });
    assert_eq!(
        resolve_max_output_ceiling(&zero),
        MAX_OUTPUT_CEILING_DEFAULT,
        "0 不表示不限——封顶失守 = 成本无界，回落默认"
    );

    let negative: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"max_output_ceiling": -1}}
    });
    assert_eq!(
        resolve_max_output_ceiling(&negative),
        MAX_OUTPUT_CEILING_DEFAULT
    );

    let non_number: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"max_output_ceiling": "unlimited"}}
    });
    assert_eq!(
        resolve_max_output_ceiling(&non_number),
        MAX_OUTPUT_CEILING_DEFAULT
    );

    let no_key: serde_json::Value = serde_json::json!({"model_list": []});
    assert_eq!(
        resolve_max_output_ceiling(&no_key),
        MAX_OUTPUT_CEILING_DEFAULT
    );

    let no_section: serde_json::Value = serde_json::json!({});
    assert_eq!(
        resolve_max_output_ceiling(&no_section),
        MAX_OUTPUT_CEILING_DEFAULT
    );

    assert_eq!(MAX_OUTPUT_CEILING_DEFAULT, 262_144);
}

/// E5（2026-10-15 收尾保障）：`agents.defaults.pre_pause_probe` 解析——
/// 缺席 = 默认开 + 默认墙钟（用户拍板默认开）；只有显式 `enabled: false`
/// 关闭；timeout 0 / 负数 / 非数字 / 段缺席全部回落默认。
#[test]
fn resolve_pre_pause_probe_defaulting() {
    let dflt = PrePauseProbeSettings {
        enabled: true,
        timeout_secs: PRE_PAUSE_PROBE_DEFAULT_TIMEOUT_SECS,
    };

    // 全缺席（最常见形态）= 默认开。
    let empty: serde_json::Value = serde_json::json!({});
    assert_eq!(resolve_pre_pause_probe(&empty), dflt);

    // 空对象也 = 默认开（enabled 缺席不是关）。
    let bare: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"pre_pause_probe": {}}}
    });
    assert_eq!(resolve_pre_pause_probe(&bare), dflt);

    // 显式关闭。
    let off: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"pre_pause_probe": {"enabled": false}}}
    });
    assert_eq!(
        resolve_pre_pause_probe(&off),
        PrePauseProbeSettings {
            enabled: false,
            timeout_secs: PRE_PAUSE_PROBE_DEFAULT_TIMEOUT_SECS
        }
    );

    // timeout 覆盖生效。
    let custom: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"pre_pause_probe": {"enabled": true, "timeout_secs": 600}}}
    });
    assert_eq!(
        resolve_pre_pause_probe(&custom),
        PrePauseProbeSettings {
            enabled: true,
            timeout_secs: 600
        }
    );

    // timeout 0 / 负数 / 非数字 → 回落默认（探针必须中止有界，没有不限形态）。
    for bad in [0, -5] {
        let v: serde_json::Value = serde_json::json!({
            "agents": {"defaults": {"pre_pause_probe": {"timeout_secs": bad}}}
        });
        assert_eq!(
            resolve_pre_pause_probe(&v),
            dflt,
            "timeout={bad} 须回落默认"
        );
    }
    let non_number: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {"pre_pause_probe": {"timeout_secs": "none"}}}
    });
    assert_eq!(resolve_pre_pause_probe(&non_number), dflt);
}

/// N6（2026-10-13）：模型角色声明——canonical 值集 + 对话谓词。
/// 守卫语义：三个 default 槽写入点 + factory 装配点消费 is_conversation_role。
#[test]
fn model_role_normalize_and_conversation_predicate() {
    // 空与显式 conversation 都归 canonical "conversation"（缺省形态）。
    assert_eq!(normalize_model_role("").unwrap(), "conversation");
    assert_eq!(
        normalize_model_role("conversation").unwrap(),
        "conversation"
    );
    assert_eq!(
        normalize_model_role(" Conversation ").unwrap(),
        "conversation"
    );
    assert_eq!(normalize_model_role("utility").unwrap(), "utility");
    assert_eq!(normalize_model_role(" classifier ").unwrap(), "classifier");
    // 未知值 loud 拒绝（错别字不静默当 conversation）。
    let err = normalize_model_role("batch").unwrap_err();
    assert!(
        err.contains("unknown model role 'batch'")
            && err.contains("conversation | utility | classifier"),
        "错误信息带值集: {err}"
    );
    // 谓词：只有 conversation（含空缺省）坐主对话槽。
    assert!(is_conversation_role(""));
    assert!(is_conversation_role("conversation"));
    assert!(is_conversation_role(" CONVERSATION "));
    assert!(!is_conversation_role("utility"));
    assert!(!is_conversation_role("classifier"));
    assert!(!is_conversation_role("nonsense"));
}
