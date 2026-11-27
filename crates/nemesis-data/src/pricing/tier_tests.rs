//! （2026-10-12 内部专项）prompt-length 阶梯价测试：`price_tiers`
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
// 选档口径（三轮复评回归锁，原诊断探针翻转转正）：选档看
// 完整 prompt 长度（非缓存输入+缓存创建+缓存读取，[`tier_selection_tokens`]），
// 计费按三类各自分列。此前选档只吃非缓存输入——大量缓存读的长会话永远
// 命中短上下文基础价。
// ---------------------------------------------------------------------------

#[test]
fn cached_read_crosses_threshold_selects_tier() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 200_000,
        input_cost_per_million: Some(3.0),
        output_cost_per_million: Some(9.0),
        cache_read_cost_per_million: Some(0.2),
        cache_creation_cost_per_million: None,
    }]);
    // 纯输入 10k 远低于阈值，但缓存读 240k 是真实上下文 → 完整长度 250k
    // 过档：10k*3 + 1k*9 + 0 + 240k*0.2 = 30k+9k+48k = 87k / 1e6 = 0.087。
    // （基础价误档得 0.036——旧 BUG 形态，探针时代断言的就是它。）
    let total = cost_from_pricing(&p, 10_000, 1_000, 0, 240_000);
    assert!(
        approx(total, 0.087),
        "cache read must trigger tier selection, got {total}"
    );
    let b = cost_breakdown_from_pricing(&p, 10_000, 1_000, 0, 240_000);
    assert!(
        approx(b.total_cost_usd, 0.087),
        "breakdown must agree: {}",
        b.total_cost_usd
    );
    assert!(
        approx(
            b.total_cost_usd,
            b.input_cost_usd
                + b.output_cost_usd
                + b.cache_creation_cost_usd
                + b.cache_read_cost_usd
        ),
        "columns must sum to total"
    );
}

#[test]
fn cache_creation_crosses_threshold_selects_tier() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 200_000,
        input_cost_per_million: Some(3.0),
        output_cost_per_million: Some(9.0),
        cache_read_cost_per_million: Some(0.2),
        cache_creation_cost_per_million: Some(2.0),
    }]);
    // 150k 纯输入未过档，+100k 缓存写 = 250k 过档：
    // 150k*3 + 1k*9 + 100k*2 + 0 = 659k / 1e6 = 0.659。
    let total = cost_from_pricing(&p, 150_000, 1_000, 100_000, 0);
    assert!(
        approx(total, 0.659),
        "cache creation must trigger tier selection, got {total}"
    );
}

#[test]
fn cache_in_total_below_threshold_stays_base() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 200_000,
        input_cost_per_million: Some(3.0),
        output_cost_per_million: Some(9.0),
        cache_read_cost_per_million: Some(0.2),
        cache_creation_cost_per_million: Some(2.0),
    }]);
    // 100k 纯输入 + 30k 写 + 50k 读 = 180k < 200k → 基础价：
    // 100k*1 + 10k*2 + 30k*1.25 + 50k*0.1 = 162.5k / 1e6 = 0.1625。
    let total = cost_from_pricing(&p, 100_000, 10_000, 30_000, 50_000);
    assert!(
        approx(total, 0.1625),
        "below-threshold total must stay base, got {total}"
    );
}

#[test]
fn tier_selection_clamps_negatives_and_saturates() {
    // 负分量钳 0（不参与、不抵扣其他分量）；饱和加法不 panic。
    assert_eq!(super::tier_selection_tokens(-5, -3, -1), 0);
    assert_eq!(super::tier_selection_tokens(10, -20, 5), 15);
    assert_eq!(
        super::tier_selection_tokens(i64::MAX, 1, 0),
        i64::MAX,
        "saturating add must not overflow"
    );
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
    // 纯输入 10k（input 参数 = 非缓存纯输入，账本不变量），缓存写 10k +
    // 读 80k 单列：10k*1(基础 in) + 40k*2(基础 out) + 10k*1.25(基础 write)
    // + 80k*0.5(档 read) = 10k + 80k + 12.5k + 40k = 142.5k / 1e6 = 0.1425
    let cost = cost_from_pricing(&p, 10_000, 40_000, 10_000, 80_000);
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
    // input 参数 = 非缓存纯输入：120k 纯输入，写 20k + 读 60k 单列。
    let b = cost_breakdown_from_pricing(&p, 120_000, 50_000, 20_000, 60_000);
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
    let t = cost_from_pricing(&p, 120_000, 50_000, 20_000, 60_000);
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

// ---------------------------------------------------------------------------
// 负价防御钳（BUG-20261115-020）：价格列负值在计价出口钳 0——CLI 入口
// loud 拒之外的纵深防御（下载表/import 畸形条目、手改 custom JSON）。
// 负价无合法计费语义（成本是可观测性数据，不存在返佣结算），钳 0 =
// 免费模型语义；token 列钳 0 的同族不变量。
// ---------------------------------------------------------------------------

#[test]
fn negative_base_prices_clamped_to_zero() {
    let mut p = base_entry();
    p.input_cost_per_million = -1.0;
    p.output_cost_per_million = -2.0;
    p.cache_read_cost_per_million = -0.1;
    p.cache_creation_cost_per_million = -1.25;
    // 正 token × 负价 = 负成本——钳后四列全 0。
    let total = cost_from_pricing(&p, 100_000, 50_000, 10_000, 20_000);
    assert_eq!(
        total, 0.0,
        "negative prices must not produce negative cost, got {total}"
    );
    let b = cost_breakdown_from_pricing(&p, 100_000, 50_000, 10_000, 20_000);
    assert_eq!(b.input_cost_usd, 0.0);
    assert_eq!(b.output_cost_usd, 0.0);
    assert_eq!(b.cache_creation_cost_usd, 0.0);
    assert_eq!(b.cache_read_cost_usd, 0.0);
    assert_eq!(b.total_cost_usd, 0.0);
}

#[test]
fn negative_price_mixed_with_positive_clamps_only_negative_column() {
    let mut p = base_entry();
    p.output_cost_per_million = -2.0; // 只有 output 负
    // input 照常计价，output 列钳 0：100k*1 + 10k*0 = 0.1。
    let total = cost_from_pricing(&p, 100_000, 10_000, 0, 0);
    assert!(
        approx(total, 0.1),
        "only the negative column clamps, got {total}"
    );
    let b = cost_breakdown_from_pricing(&p, 100_000, 10_000, 0, 0);
    assert!(approx(b.input_cost_usd, 0.1), "input: {}", b.input_cost_usd);
    assert_eq!(b.output_cost_usd, 0.0);
    assert!(approx(b.total_cost_usd, 0.1), "total: {}", b.total_cost_usd);
    // 分项和恒等于 total 的不变量在混合钳位下保持。
    assert!(approx(
        b.total_cost_usd,
        b.input_cost_usd + b.output_cost_usd + b.cache_creation_cost_usd + b.cache_read_cost_usd
    ));
}

#[test]
fn negative_tier_prices_clamped_to_zero() {
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 200_000,
        input_cost_per_million: Some(-3.0),
        output_cost_per_million: Some(-9.0),
        cache_read_cost_per_million: Some(-0.2),
        cache_creation_cost_per_million: Some(-2.0),
    }]);
    // 过档（250k ≥ 200k）：阶梯负价 → 全列钳 0。
    let total = cost_from_pricing(&p, 10_000, 1_000, 0, 240_000);
    assert_eq!(total, 0.0, "negative tier prices must clamp, got {total}");
}

#[test]
fn negative_tier_price_clamps_while_none_inherits_base() {
    // 档内部分 None（继承基础价）：显式负阶梯列钳 0，None 继承的非负
    // 基础价照常计价——钳位发生在继承回填之后的出口，不影响继承本身。
    let mut p = base_entry();
    p.price_tiers = Some(vec![PriceTier {
        min_prompt_tokens: 200_000,
        input_cost_per_million: Some(-3.0),
        output_cost_per_million: None,
        cache_read_cost_per_million: None,
        cache_creation_cost_per_million: None,
    }]);
    // 250k 过档：input=Some(-3.0)→0、output=None→2.0、cache_read=None→0.1、
    // cache_creation=None→1.25。计费 = 10k*0 + 1k*2 + 0 + 240k*0.1
    // = 2k + 24k = 26k / 1e6 = 0.026。
    let total = cost_from_pricing(&p, 10_000, 1_000, 0, 240_000);
    assert!(
        approx(total, 0.026),
        "inherit path must price non-negative base, got {total}"
    );
}

// ---------------------------------------------------------------------------
// 非有限价防御钳（BUG-20261126-137）：`f64::max(0.0)` 挡得住负价与 NaN
// （Rust max 是 NaN-aware），挡不住 **±inf 原样保留**。进价路径：下载表/
// import 的溢出字面量（serde_json 解析 `1e400` → inf 不报错；parse 层
// is_finite skip 只查 input/output，cache 两列缺席）→ 正 token × inf =
// inf 成本落账 → SUM 变 inf → usage.budget 费用闸 `inf >= limit` 恒真
// → reject 模式首个请求即误熔断停车。计费域无 inf 语义——非有限一律
// 钳 0（免费模型语义，与负价同族）。
// ---------------------------------------------------------------------------

#[test]
fn infinite_prices_clamped_to_zero() {
    let mut p = base_entry();
    p.input_cost_per_million = f64::INFINITY;
    p.output_cost_per_million = f64::NEG_INFINITY;
    p.cache_read_cost_per_million = f64::INFINITY;
    p.cache_creation_cost_per_million = f64::INFINITY;
    // 修复前形态：正 token × inf = inf（负 token 列已钳 0 不触发
    // -inf × 正 token = -inf 的另一路径），SUM 污染 + 预算闸恒熔断。
    let total = cost_from_pricing(&p, 100_000, 50_000, 10_000, 20_000);
    assert!(
        total.is_finite() && total == 0.0,
        "infinite prices must clamp to zero cost, got {total}"
    );
    let b = cost_breakdown_from_pricing(&p, 100_000, 50_000, 10_000, 20_000);
    assert!(b.input_cost_usd.is_finite() && b.input_cost_usd == 0.0);
    assert!(b.output_cost_usd.is_finite() && b.output_cost_usd == 0.0);
    assert!(b.cache_creation_cost_usd.is_finite() && b.cache_creation_cost_usd == 0.0);
    assert!(b.cache_read_cost_usd.is_finite() && b.cache_read_cost_usd == 0.0);
    assert!(b.total_cost_usd.is_finite() && b.total_cost_usd == 0.0);
}

#[test]
fn infinite_cache_price_with_cache_read_tokens_clamps() {
    // 实锤触发形态：仅 cache_read 价 inf + 带缓存读的请求（无缓存读时
    // token 钳 0 侧 0.0 * inf = NaN，同样被本钳收口——落 SQLite 会转
    // NULL 掩盖，但 NaN 账面已是错）。
    let mut p = base_entry();
    p.cache_read_cost_per_million = f64::INFINITY;
    let total = cost_from_pricing(&p, 100_000, 50_000, 0, 240_000);
    assert!(
        total.is_finite(),
        "inf cache price must not poison total, got {total}"
    );
    // cache 列钳 0 = 缓存免费语义；input/output 照常计价：
    // 100k*1 + 50k*2 + 0 = 0.2。
    assert!(approx(total, 0.2), "only inf column clamps, got {total}");
}

#[test]
fn nan_prices_clamped_to_zero_explicitly() {
    // NaN 已被 f64::max 隐式归 0（NaN-aware max 返回另一参数）——回归锁
    // 把这个隐式行为钉成显式契约，防未来改写钳位实现时丢失。
    let mut p = base_entry();
    p.input_cost_per_million = f64::NAN;
    p.cache_read_cost_per_million = f64::NAN;
    let total = cost_from_pricing(&p, 100_000, 50_000, 0, 20_000);
    assert!(
        total.is_finite() && approx(total, 0.1),
        "NaN prices must clamp to zero, only output prices: got {total}"
    );
}
