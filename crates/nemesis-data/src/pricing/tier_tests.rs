//! L3（2026-10-12 三方学习 T0）prompt-length 阶梯价测试：`price_tiers`
//! 解析语义（阈值命中/最高档胜出/None 继承）+ 两条计价口径同源
//! （`cost_from_pricing` 与 `cost_breakdown_from_pricing` 命中阶梯价时
//! 分项之和恒等于 total）+ serde 透传 round-trip。

use super::*;
use crate::models::PriceTier;

fn base_entry() -> ModelPricing {
    ModelPricing {
        model_id: "tiered".into(),
        display_name: "Tiered".into(),
        input_cost_per_million: 1.0,
        output_cost_per_million: 2.0,
        cache_read_cost_per_million: 0.1,
        cache_creation_cost_per_million: 1.25,
        max_input_tokens: None,
        max_output_tokens: None,
        aliases: Vec::new(),
        price_tiers: None,
    }
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

// ---------------------------------------------------------------------------
// 阈值语义：未达阈值 = 基础价；达到阈值 = 档价生效（覆盖输入与输出价）。
// ---------------------------------------------------------------------------

#[test]
fn tier_below_threshold_uses_base_price() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 200_000,
        input_cost_per_million: Some(3.0),
        output_cost_per_million: Some(9.0),
        cache_read_cost_per_million: None,
        cache_creation_cost_per_million: None,
    }]);
    // 100k < 200k 阈值 → 全按基础价：100k*1 + 50k*2 = 0.2
    let cost = cost_from_pricing(&p, 100_000, 50_000, 0, 0);
    assert!(
        approx(cost, 0.2),
        "below threshold must use base price, got {cost}"
    );
}

#[test]
fn tier_at_threshold_switches_to_tier_price() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 200_000,
        input_cost_per_million: Some(3.0),
        output_cost_per_million: Some(9.0),
        cache_read_cost_per_million: None,
        cache_creation_cost_per_million: None,
    }]);
    // 200k（恰好达阈值，>= 语义）→ 档价：200k*3 + 50k*9 = 1.05
    let cost = cost_from_pricing(&p, 200_000, 50_000, 0, 0);
    assert!(
        approx(cost, 1.05),
        "at threshold must use tier price, got {cost}"
    );
}

// ---------------------------------------------------------------------------
// 多档命中取阈值最大的一档（max_by_key）；None 字段继承基础价（部分覆盖）。
// ---------------------------------------------------------------------------

#[test]
fn tier_multi_hit_takes_highest_threshold() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![
        PriceTier {
            min_prompt_tokens: 100_000,
            input_cost_per_million: Some(2.0),
            output_cost_per_million: None, // 继承基础输出价 2.0
            cache_read_cost_per_million: None,
            cache_creation_cost_per_million: None,
        },
        PriceTier {
            min_prompt_tokens: 500_000,
            input_cost_per_million: Some(4.0),
            output_cost_per_million: Some(16.0),
            cache_read_cost_per_million: None,
            cache_creation_cost_per_million: None,
        },
    ]);
    // 300k：命中 100k 档（不命中 500k）→ in=2.0（档价）、out=2.0（继承）
    // 300k*2 + 50k*2 = 0.7
    let mid = cost_from_pricing(&p, 300_000, 50_000, 0, 0);
    assert!(
        approx(mid, 0.7),
        "mid band must use 100k tier + inherited output, got {mid}"
    );

    // 600k：两档全命中 → 取阈值最大的 500k 档：600k*4 + 50k*16 = 3.2
    let top = cost_from_pricing(&p, 600_000, 50_000, 0, 0);
    assert!(
        approx(top, 3.2),
        "top band must take highest-threshold tier, got {top}"
    );
}

#[test]
fn tier_partial_inheritance_covers_cache_prices() {
    let mut p = base_entry();
    // 只调缓存读价，其余全继承（None → 基础价）。
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 0,
        input_cost_per_million: None,
        output_cost_per_million: None,
        cache_read_cost_per_million: Some(0.5),
        cache_creation_cost_per_million: None,
    }]);
    // 100k 全量输入中 80k 缓存读、10k 缓存写、10k 明文：
    // 10k*1(基础 in) + 40k*2(基础 out) + 10k*1.25(基础 write) + 80k*0.5(档 read)
    // = 10k + 80k + 12.5k + 40k = 142.5k / 1e6 = 0.1425
    let cost = cost_from_pricing(&p, 100_000, 40_000, 10_000, 80_000);
    assert!(
        approx(cost, 0.1425),
        "cache-only tier override must inherit the rest, got {cost}"
    );
}

// ---------------------------------------------------------------------------
// 两条口径同源：分项版四列之和 == 总价版（命中阶梯价时也不分叉）。
// ---------------------------------------------------------------------------

#[test]
fn tier_breakdown_columns_sum_to_total() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 100_000,
        input_cost_per_million: Some(3.0),
        output_cost_per_million: Some(9.0),
        cache_read_cost_per_million: Some(0.2),
        cache_creation_cost_per_million: Some(2.0),
    }]);
    let b = cost_breakdown_from_pricing(&p, 200_000, 50_000, 20_000, 60_000);
    // 分项版：120k*3 + 50k*9 + 20k*2 + 60k*0.2 = 0.36+0.45+0.04+0.012 = 0.862
    assert!(
        approx(
            b.total_cost_usd,
            b.input_cost_usd
                + b.output_cost_usd
                + b.cache_creation_cost_usd
                + b.cache_read_cost_usd
        ),
        "breakdown columns must sum to total"
    );
    // 总价版同口径（同 effective_prices）。
    let t = cost_from_pricing(&p, 200_000, 50_000, 20_000, 60_000);
    assert!(
        approx(t, b.total_cost_usd),
        "total and breakdown must agree under tiers: {t} vs {}",
        b.total_cost_usd
    );
}

// ---------------------------------------------------------------------------
// serde 透传：price_tiers 在 JSON round-trip 中保留；缺省字段 = None（旧行
// 为零迁移）。
// ---------------------------------------------------------------------------

#[test]
fn tier_serde_roundtrip_and_default_absent() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 250_000,
        input_cost_per_million: Some(5.0),
        output_cost_per_million: None,
        cache_read_cost_per_million: None,
        cache_creation_cost_per_million: None,
    }]);
    let json = serde_json::to_string(&p).unwrap();
    assert!(json.contains("price_tiers"), "tiers must serialize");
    let back: ModelPricing = serde_json::from_str(&json).unwrap();
    let tiers = back.price_tiers.expect("tiers survive round-trip");
    assert_eq!(tiers.len(), 1);
    assert_eq!(tiers[0].min_prompt_tokens, 250_000);
    assert_eq!(tiers[0].input_cost_per_million, Some(5.0));
    assert_eq!(tiers[0].output_cost_per_million, None, "None stays None");

    // 无 price_tiers 的 JSON（旧数据/旧前端载荷）反序列化 = None；且序列化
    // 时字段省略（skip_serializing_if），对下游零噪音。
    let legacy: ModelPricing =
        serde_json::from_str(r#"{"model_id":"x","display_name":"x","input_cost_per_million":1.0,"output_cost_per_million":2.0,"cache_read_cost_per_million":0.0,"cache_creation_cost_per_million":0.0,"max_input_tokens":null,"max_output_tokens":null,"aliases":[]}"#)
            .expect("legacy payload must deserialize");
    assert!(legacy.price_tiers.is_none());
    let legacy_json = serde_json::to_string(&legacy).unwrap();
    assert!(
        !legacy_json.contains("price_tiers"),
        "None tiers must be omitted on serialize"
    );
}
