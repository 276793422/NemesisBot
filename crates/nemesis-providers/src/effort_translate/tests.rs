//! effort_translate 单测：厂商 × 档位矩阵 + extra_body 优先级 + 零注入边界。

use super::*;
use serde_json::json;

/// 空请求体起点（不含 model/messages——翻译只补丁顶层键）。
fn base_body() -> serde_json::Value {
    json!({ "model": "x", "messages": [] })
}

// ---- 未配置（空 effort）零注入 ----

#[test]
fn empty_effort_injects_nothing() {
    let mut b = base_body();
    apply_effort_to_body(
        &mut b,
        "glm-4.7",
        "https://open.bigmodel.cn/api/paas/v4",
        "",
        true,
        &serde_json::Value::Null,
    );
    assert!(b.get("thinking").is_none());
    assert!(b.get("reasoning_effort").is_none());
    assert!(b.get("enable_thinking").is_none());
}

#[test]
fn whitespace_effort_injects_nothing() {
    let mut b = base_body();
    apply_effort_to_body(&mut b, "glm-4.7", "", "  ", true, &serde_json::Value::Null);
    assert!(b.get("thinking").is_none());
}

// ---- GLM 厂商 ----

#[test]
fn glm_off_disables_thinking() {
    let mut b = base_body();
    apply_effort_to_body(
        &mut b,
        "glm-4.7",
        "https://open.bigmodel.cn/api/paas/v4",
        "off",
        true,
        &serde_json::Value::Null,
    );
    assert_eq!(b["thinking"], json!({ "type": "disabled" }));
}

#[test]
fn glm_high_enables_thinking() {
    let mut b = base_body();
    apply_effort_to_body(
        &mut b,
        "glm-4.7",
        "",
        "high",
        true,
        &serde_json::Value::Null,
    );
    assert_eq!(b["thinking"], json!({ "type": "enabled" }));
}

#[test]
fn glm_medium_and_low_both_enabled_no_fine_grain() {
    // glm-4.x 可开关族无细档：low/medium/high 同为 enabled，诚实不细分。
    for tier in ["low", "medium", "high"] {
        let mut b = base_body();
        apply_effort_to_body(
            &mut b,
            "zhipu/glm-4.7",
            "",
            tier,
            false,
            &serde_json::Value::Null,
        );
        assert_eq!(b["thinking"], json!({ "type": "enabled" }), "tier={tier}");
    }
}

// ---- GLM-5.x 恒思考族（2026-11-09 实机探测：disabled 报 1210）----

#[test]
fn glm5_off_skips_honestly_no_key_sent() {
    // 恒思考模型关不掉：off 不发任何键（DeepSeek-R1 同款诚实跳过）。
    for model in ["glm-5.3", "glm-5.3-flash", "zhipu/glm-5.3-flash"] {
        let mut b = base_body();
        apply_effort_to_body(&mut b, model, "", "off", true, &serde_json::Value::Null);
        assert!(b.get("thinking").is_none(), "model={model}");
        assert!(b.get("reasoning_effort").is_none(), "model={model}");
    }
}

#[test]
fn glm5_tiers_collapse_monotonically_to_vendor_low_high_max() {
    // vendor 词表 {low, high, max}（2026-11-09 实机校准 reasoning_effort
    // 标准键）：七档 ceil 合流，档位递增发出的值永不回退；弃用的私有键
    // thinking 不再出现。
    let cases = [
        ("minimal", "low"),
        ("low", "low"),
        ("medium", "high"),
        ("high", "high"),
        ("xhigh", "max"),
        ("max", "max"),
    ];
    for (tier, wire) in cases {
        let mut b = base_body();
        apply_effort_to_body(
            &mut b,
            "glm-5.3-flash",
            "https://open.bigmodel.cn/api/paas/v4",
            tier,
            true,
            &serde_json::Value::Null,
        );
        assert_eq!(b["reasoning_effort"], json!(wire), "tier={tier}");
        assert!(b.get("thinking").is_none(), "tier={tier}");
    }
}

#[test]
fn glm5_detected_by_model_name_alone() {
    // glm-5 前缀识别不依赖 base（z.ai / 中转站同享）。
    let mut b = base_body();
    apply_effort_to_body(
        &mut b,
        "glm-5.3",
        "",
        "high",
        true,
        &serde_json::Value::Null,
    );
    assert_eq!(b["reasoning_effort"], json!("high"));
}

#[test]
fn glm4_switchable_family_unchanged_by_glm5_row() {
    // glm-5 行只拦 glm-5 前缀；glm-4.x 可开关族行为不变（off→disabled）。
    let mut b = base_body();
    apply_effort_to_body(&mut b, "glm-4.7", "", "off", true, &serde_json::Value::Null);
    assert_eq!(b["thinking"], json!({ "type": "disabled" }));
}

#[test]
fn glm_detected_by_base_url() {
    // model 名不含 glm，但 base 是 z.ai / bigmodel.cn → 识别为 GLM。
    for base in [
        "https://open.bigmodel.cn/api/paas/v4",
        "https://api.z.ai/api/paas/v4",
    ] {
        let mut b = base_body();
        apply_effort_to_body(
            &mut b,
            "some-alias",
            base,
            "off",
            true,
            &serde_json::Value::Null,
        );
        assert_eq!(b["thinking"], json!({ "type": "disabled" }), "base={base}");
    }
}

// ---- Qwen3 厂商 ----

#[test]
fn qwen3_off_disables_thinking() {
    let mut b = base_body();
    apply_effort_to_body(
        &mut b,
        "qwen3-235b-a22b",
        "",
        "off",
        false,
        &serde_json::Value::Null,
    );
    assert_eq!(b["enable_thinking"], json!(false));
}

#[test]
fn qwen3_high_streaming_enables_thinking() {
    let mut b = base_body();
    apply_effort_to_body(
        &mut b,
        "qwen3-30b",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "high",
        true,
        &serde_json::Value::Null,
    );
    assert_eq!(b["enable_thinking"], json!(true));
}

#[test]
fn qwen3_high_non_streaming_honest_skip() {
    // 非流式带 true 会被端点 400 —— 诚实跳过（维持端点默认）。
    let mut b = base_body();
    apply_effort_to_body(
        &mut b,
        "qwen3-30b",
        "",
        "high",
        false,
        &serde_json::Value::Null,
    );
    assert!(b.get("enable_thinking").is_none());
    assert!(b.get("reasoning_effort").is_none());
}

// ---- DeepSeek 推理系（恒思考，无开关）----

#[test]
fn deepseek_reasoner_skips_all_tiers() {
    for (model, tier) in [
        ("deepseek-reasoner", "off"),
        ("deepseek-reasoner", "high"),
        ("deepseek-r1-0528", "low"),
        ("deepseek-r1", "medium"),
    ] {
        let mut b = base_body();
        apply_effort_to_body(&mut b, model, "", tier, true, &serde_json::Value::Null);
        assert!(
            b.get("reasoning_effort").is_none(),
            "model={model} tier={tier}"
        );
        assert!(b.get("thinking").is_none(), "model={model} tier={tier}");
    }
}

// ---- 未知厂商（OpenAI 官方字段直传）----

#[test]
fn unknown_vendor_passes_all_non_off_tiers() {
    // OpenAI 官方枚举七档恒等直传（含 2026-11-09 新增 minimal/xhigh/max）；
    // off 非官方合法值不发（防 400）。端点不支持某档 = 400 带端点原文
    // 诚实透传（实机背书：gpt-5.6-sol 拒 minimal、收其余五档）。
    for tier in ["minimal", "low", "medium", "high", "xhigh", "max"] {
        let mut b = base_body();
        apply_effort_to_body(
            &mut b,
            "gpt-5.6",
            "https://api.openai.com/v1",
            tier,
            true,
            &serde_json::Value::Null,
        );
        assert_eq!(b["reasoning_effort"], json!(tier), "tier={tier}");
    }
}

#[test]
fn unknown_vendor_off_injects_nothing() {
    // off 非官方合法值，不发（防 400）。
    let mut b = base_body();
    apply_effort_to_body(&mut b, "gpt-5.6", "", "off", true, &serde_json::Value::Null);
    assert!(b.get("reasoning_effort").is_none());
}

// ---- extra_body 逃生舱 ----

#[test]
fn extra_body_null_no_merge() {
    let mut b = base_body();
    apply_effort_to_body(&mut b, "glm-4.7", "", "off", true, &serde_json::Value::Null);
    assert_eq!(b["thinking"], json!({ "type": "disabled" }));
}

#[test]
fn extra_body_overrides_auto_translation() {
    // 显式键 > 自动翻译（用户意志最大）：GLM off 自动给 disabled，
    // 用户显式 thinking enabled 覆盖。
    let mut b = base_body();
    let extra = json!({ "thinking": { "type": "enabled" } });
    apply_effort_to_body(&mut b, "glm-4.7", "", "off", true, &extra);
    assert_eq!(b["thinking"], json!({ "type": "enabled" }));
}

#[test]
fn extra_body_adds_unrelated_keys() {
    // 逃生舱不止思考参数：任意厂商私有键浅合并到 body 顶层。
    let mut b = base_body();
    let extra = json!({ "top_k": 40, "custom_flag": true });
    apply_effort_to_body(&mut b, "gpt-5.6", "", "high", true, &extra);
    assert_eq!(b["top_k"], json!(40));
    assert_eq!(b["custom_flag"], json!(true));
    assert_eq!(b["reasoning_effort"], json!("high"));
}

#[test]
fn extra_body_non_object_ignored() {
    // 非对象形态（字符串/数组）诚实忽略，不炸不合并。
    let mut b = base_body();
    apply_effort_to_body(
        &mut b,
        "glm-4.7",
        "",
        "off",
        true,
        &json!("thinking=disabled"),
    );
    assert_eq!(b["thinking"], json!({ "type": "disabled" }));
    assert_eq!(b.as_object().unwrap().len(), 3); // model/messages/thinking，无杂键
}

// ---- 杂务闸语义（aux off → GLM disabled，DeepSeek 跳过）----

#[test]
fn aux_off_semantics_glm_disables() {
    // aux_chat_options 强制 off → GLM 请求带 thinking disabled =
    // 杂务不烧思考 token（B1 核心收益）。
    let mut b = base_body();
    apply_effort_to_body(&mut b, "glm-4.7", "", "OFF", true, &serde_json::Value::Null);
    assert_eq!(b["thinking"], json!({ "type": "disabled" }));
}

// ---- 注册表结构契约（2026-11-09 注册表化重构；厂商增删零改派发）----

use super::{Dialect, Effort, REGISTRY};

#[test]
fn registry_glm5_entry_precedes_glm_entry() {
    // 先专后通：glm-5 恒思考行必须排在 glm 可开关行之前——否则 glm 行会
    // 抢走 glm-5.x 流量（行为回归：level 细档退化成单开关）。
    let glm5 = REGISTRY
        .iter()
        .position(|d: &Dialect| d.name == "glm-5-always-thinking");
    let glm = REGISTRY.iter().position(|d| d.name == "glm-switchable");
    let (Some(g5), Some(g)) = (glm5, glm) else {
        panic!("registry entries missing: glm5={glm5:?} glm={glm:?}");
    };
    assert!(
        g5 < g,
        "glm-5 row (idx {g5}) must precede glm row (idx {g})"
    );
}

#[test]
fn registry_fallback_is_last_and_matches_all() {
    let Some(last) = REGISTRY.last() else {
        panic!("registry empty");
    };
    assert_eq!(last.name, "openai-official-fallback", "兜底恒在末位");
    assert!((last.matches)("anything", ""), "兜底谓词恒真");
}

#[test]
fn unknown_tier_is_honest_zero_injection() {
    // 未知档位（防御臂；上游 resolve_reasoning_effort 已限定值域）：
    // 不为垃圾值发明任何厂商键。所有已知厂商行统一零注入。
    for model in ["glm-5.3-flash", "glm-4.7", "qwen3-30b", "gpt-5.6"] {
        let mut b = base_body();
        apply_effort_to_body(&mut b, model, "", "extreme", true, &serde_json::Value::Null);
        assert!(
            b.get("thinking").is_none()
                && b.get("reasoning_effort").is_none()
                && b.get("enable_thinking").is_none(),
            "model={model}"
        );
    }
}

#[test]
fn effort_parse_is_case_insensitive_and_total_for_known_values() {
    // 词表解析统一在 Effort::parse（大小写不敏感 + trim）；未知值 None。
    assert_eq!(Effort::parse("OFF"), Some(Effort::Off));
    assert_eq!(Effort::parse("High"), Some(Effort::High));
    assert_eq!(Effort::parse("medium"), Some(Effort::Medium));
    assert_eq!(Effort::parse("extreme"), None);
    assert_eq!(Effort::parse(""), None);
}

// ---- 值域扩档矩阵与真相源契约（2026-11-09：7 档 × 5 词条）----

#[test]
fn full_matrix_seven_tiers_x_five_dialects() {
    // 7 档 × 5 词条 = 35 组合逐格钉住（翻译表单一真相；流式恒 true，
    // qwen3 非流式跳过分支由专项用例覆盖）。每格断言：该写的键值 ==
    // 期望；不该出现的键缺席。
    let tiers = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    let reps: [(&str, &str, &str); 5] = [
        // (词条名, 代表模型, 代表 base)
        ("deepseek-reasoner", "deepseek-reasoner", ""),
        (
            "glm-5-always-thinking",
            "glm-5.3-flash",
            "https://open.bigmodel.cn/api/paas/v4",
        ),
        ("glm-switchable", "glm-4.7", ""),
        ("qwen3-hybrid-thinking", "qwen3-30b", ""),
        (
            "openai-official-fallback",
            "gpt-5.6",
            "https://api.openai.com/v1",
        ),
    ];
    let mut cells = 0;
    for tier in tiers {
        for (dialect, model, base) in reps {
            let mut b = base_body();
            apply_effort_to_body(&mut b, model, base, tier, true, &serde_json::Value::Null);
            let has = |k: &str| b.get(k).is_some();
            match (dialect, tier) {
                // 恒思考无开关族：全档零注入。
                ("deepseek-reasoner", _) => {
                    assert!(
                        !has("reasoning_effort") && !has("thinking") && !has("enable_thinking"),
                        "{dialect}/{tier}"
                    );
                }
                // GLM-5.x：off 诚实跳过；其余 ceil 合流 {low, high, max}。
                ("glm-5-always-thinking", "off") => {
                    assert!(!has("reasoning_effort") && !has("thinking"));
                }
                ("glm-5-always-thinking", t) => {
                    let want = if t == "minimal" || t == "low" {
                        "low"
                    } else if t == "medium" || t == "high" {
                        "high"
                    } else {
                        "max"
                    };
                    assert_eq!(b["reasoning_effort"], json!(want), "{dialect}/{t}");
                    assert!(!has("thinking"), "{dialect}/{t}（私有键已弃用）");
                }
                // GLM-4.x 可开关族：单开关，无细档。
                ("glm-switchable", "off") => {
                    assert_eq!(b["thinking"], json!({ "type": "disabled" }));
                }
                ("glm-switchable", _) => {
                    assert_eq!(b["thinking"], json!({ "type": "enabled" }));
                }
                // Qwen3：布尔（流式全档 true）。
                ("qwen3-hybrid-thinking", "off") => {
                    assert_eq!(b["enable_thinking"], json!(false));
                }
                ("qwen3-hybrid-thinking", _) => {
                    assert_eq!(b["enable_thinking"], json!(true));
                }
                // OpenAI 兜底：off 不发，其余恒等直传。
                ("openai-official-fallback", "off") => {
                    assert!(!has("reasoning_effort"));
                }
                ("openai-official-fallback", t) => {
                    assert_eq!(b["reasoning_effort"], json!(t));
                }
                _ => unreachable!("unknown dialect in reps: {dialect}"),
            }
            cells += 1;
        }
    }
    assert_eq!(cells, 35, "矩阵必须完整覆盖 7×5");
}

#[test]
fn effort_parse_matches_capability_truth_source() {
    // 词表单一真相源契约：nemesis-types::EFFORT_TIERS 每一项 parse 全收；
    // 非 off 档 as_str()（fallback 直传 wire 值）与配置值逐字节相等——
    // 两侧词表漂移在此钉死。解析容 trim + 大小写；词表外值不发明 wire 值。
    use nemesis_types::capability::EFFORT_TIERS;
    for t in EFFORT_TIERS {
        let e = Effort::parse(t).unwrap_or_else(|| panic!("EFFORT_TIERS member {t:?} must parse"));
        assert_eq!(e.as_str(), t);
        let upper = t.to_ascii_uppercase();
        assert_eq!(Effort::parse(&upper), Some(e), "case-insensitive: {t}");
    }
    assert_eq!(Effort::parse("  max  "), Some(Effort::Max), "trim 容忍");
    for bad in ["extreme", "none", "zzz-not-a-tier"] {
        assert_eq!(Effort::parse(bad), None, "garbage must not parse: {bad}");
    }
}

// ---- wire 目标解析（二期 probe 消费面；marks 与探测数据对齐的裁决点）----

use super::{EffortWireTarget, effort_wire_target};

#[test]
fn wire_target_off_and_silent_dialects_are_not_sent() {
    let null = serde_json::Value::Null;
    // off = 不发键（任何方言）。
    for model in [
        "gpt-5.6",
        "glm-5.3-flash",
        "glm-4.7",
        "qwen3-30b",
        "deepseek-reasoner",
    ] {
        assert_eq!(
            effort_wire_target(model, "", "off", &null),
            EffortWireTarget::NotSent,
            "off must be NotSent: {model}"
        );
        assert_eq!(
            effort_wire_target(model, "", "", &null),
            EffortWireTarget::NotSent,
            "空串（未配置）= NotSent: {model}"
        );
    }
    // deepseek-reasoner 恒思考无开关：任何档位零注入。
    for tier in ["minimal", "low", "medium", "high", "xhigh", "max"] {
        assert_eq!(
            effort_wire_target("deepseek-reasoner", "", tier, &null),
            EffortWireTarget::NotSent,
            "deepseek 全档零注入: {tier}"
        );
    }
    // trim + 大小写防御。
    assert_eq!(
        effort_wire_target("gpt-5.6", "", "  OFF  ", &null),
        EffortWireTarget::NotSent
    );
}

#[test]
fn wire_target_fallback_is_identity() {
    // 兜底直传：6 个非 off 档位原样到 wire。
    let null = serde_json::Value::Null;
    for tier in ["minimal", "low", "medium", "high", "xhigh", "max"] {
        assert_eq!(
            effort_wire_target("gpt-5.6", "https://api.openai.com/v1", tier, &null),
            EffortWireTarget::ReasoningEffort(tier),
            "fallback identity: {tier}"
        );
    }
}

#[test]
fn wire_target_glm5_ceil_collapse() {
    // GLM 合流保序：minimal|low→low、medium|high→high、xhigh|max→max。
    let null = serde_json::Value::Null;
    let pairs = [
        ("minimal", "low"),
        ("low", "low"),
        ("medium", "high"),
        ("high", "high"),
        ("xhigh", "max"),
        ("max", "max"),
    ];
    for (tier, wire) in pairs {
        assert_eq!(
            effort_wire_target(
                "glm-5.3-flash",
                "https://open.bigmodel.cn/api/paas/v4",
                tier,
                &null
            ),
            EffortWireTarget::ReasoningEffort(wire),
            "glm5 ceil: {tier} → {wire}"
        );
    }
}

#[test]
fn wire_target_private_knob_dialects_are_not_probeable() {
    // qwen3（enable_thinking 布尔）与 glm 可开关（thinking 单开关）：发的是
    // 私有键，reasoning_effort 探测数据不适用 → ?（不猜）。
    let null = serde_json::Value::Null;
    for tier in ["low", "medium", "high", "xhigh", "max"] {
        assert_eq!(
            effort_wire_target("qwen3-30b", "", tier, &null),
            EffortWireTarget::NotProbeable,
            "qwen3: {tier}"
        );
        assert_eq!(
            effort_wire_target("glm-4.7", "", tier, &null),
            EffortWireTarget::NotProbeable,
            "glm-switchable: {tier}"
        );
    }
}

#[test]
fn wire_target_extra_body_pin_maps_all_tiers_to_pinned_value() {
    // 条目 extra_body 钉档：档位→值映射被钉死，但每档配置实际都发钉值
    // ——按钉值查判定表是诚实语义（全部档位同标记）。钉值合法时回查
    // parse 产静态串；钉值本身不合法（任意字符串）才落 NotProbeable。
    let pin_valid = json!({ "reasoning_effort": "xhigh" });
    assert_eq!(
        effort_wire_target("gpt-5.6", "", "low", &pin_valid),
        EffortWireTarget::ReasoningEffort("xhigh")
    );
    let pin_garbage = json!({ "reasoning_effort": "turbo" });
    assert_eq!(
        effort_wire_target("gpt-5.6", "", "low", &pin_garbage),
        EffortWireTarget::NotProbeable
    );
}
