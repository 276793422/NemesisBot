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
    }
}

#[test]
fn glm5_levels_map_monotonically_to_vendor_low_high_max() {
    // vendor 无 medium：low→low、medium→high、high→max（保序满射）。
    let cases = [("low", "low"), ("medium", "high"), ("high", "max")];
    for (tier, level) in cases {
        let mut b = base_body();
        apply_effort_to_body(
            &mut b,
            "glm-5.3-flash",
            "https://open.bigmodel.cn/api/paas/v4",
            tier,
            true,
            &serde_json::Value::Null,
        );
        assert_eq!(
            b["thinking"],
            json!({ "type": "enabled", "level": level }),
            "tier={tier}"
        );
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
    assert_eq!(b["thinking"], json!({ "type": "enabled", "level": "max" }));
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
fn unknown_vendor_high_passes_reasoning_effort() {
    for tier in ["low", "medium", "high"] {
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
    // 词表解析统一在 Effort::parse（大小写不敏感）；未知值 None。
    assert_eq!(Effort::parse("OFF"), Some(Effort::Off));
    assert_eq!(Effort::parse("High"), Some(Effort::High));
    assert_eq!(Effort::parse("medium"), Some(Effort::Medium));
    assert_eq!(Effort::parse("extreme"), None);
    assert_eq!(Effort::parse(""), None);
}
