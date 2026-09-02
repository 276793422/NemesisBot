//! Pricing table + cost-formula tests (kept out of production files).

use crate::models::ModelPricing;
use crate::pricing::{PricingTable, compute_cost_usd, cost_from_pricing, lookup_pricing};
use std::collections::HashMap;

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn embedded_table_loads_and_is_sane() {
    let table = PricingTable::embedded();
    assert!(table.entries().len() >= 30, "expected ~36 entries");
    // model_id unique.
    let mut ids: Vec<&str> = table
        .entries()
        .iter()
        .map(|e| e.model_id.as_str())
        .collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(before, ids.len(), "duplicate model_id in embedded table");
    // No negative prices anywhere.
    for e in table.entries() {
        assert!(e.input_cost_per_million >= 0.0);
        assert!(e.output_cost_per_million >= 0.0);
        assert!(e.cache_read_cost_per_million >= 0.0);
        assert!(e.cache_creation_cost_per_million >= 0.0);
    }
}

#[test]
fn lookup_exact_and_bare_suffix() {
    // Exact model_id.
    assert!(lookup_pricing("gpt-4o").is_some());
    // Provider-qualified config name resolves via bare suffix.
    assert!(lookup_pricing("openai/gpt-4o").is_some());
    assert!(lookup_pricing("zhipu/glm-4.7").is_some());
    assert!(lookup_pricing("deepseek/deepseek-chat").is_some());
    // Deep slashes (openrouter style) still strip to the last segment.
    assert!(lookup_pricing("deepseek/deepseek-reasoner").is_some());
}

#[test]
fn lookup_alias_paths() {
    // deepseek/ prefixed alias defined on the deepseek-chat entry.
    let hit = lookup_pricing("deepseek/deepseek-chat").expect("alias hit");
    assert_eq!(hit.model_id, "deepseek-chat");
    // Fireworks GLM alias resolves to glm-4.7.
    let hit = lookup_pricing("fireworks_ai/glm-4p7").expect("glm alias hit");
    assert_eq!(hit.model_id, "glm-4.7");
    // Alias matched via bare suffix too.
    let hit = lookup_pricing("zhipu/glm-4.6");
    assert!(hit.is_some());
}

#[test]
fn lookup_unknown_and_empty() {
    // TestAIServer models are not in the table.
    assert!(lookup_pricing("testai-1.1").is_none());
    assert!(lookup_pricing("test/testai-4.2").is_none());
    assert!(lookup_pricing("").is_none());
    assert!(lookup_pricing("   ").is_none());
    assert!(lookup_pricing("totally-made-up-model").is_none());
}

#[test]
fn cost_gpt4o_plain_math() {
    // gpt-4o: $2.5/M input, $10/M output.
    let cost = compute_cost_usd("gpt-4o", 1_000_000, 500_000, 0, 0);
    assert!(approx(cost, 2.5 + 5.0), "got {cost}");
    // Provider-qualified name hits the same entry.
    let cost = compute_cost_usd("openai/gpt-4o", 1_000_000, 0, 0, 0);
    assert!(approx(cost, 2.5), "got {cost}");
}

#[test]
fn cost_deepseek_cache_split() {
    // deepseek-chat: 0.28 in / 0.42 out / 0.03 cache-read.
    let cost = compute_cost_usd("deepseek/deepseek-chat", 1_000_000, 100_000, 0, 600_000);
    // plain 400k*0.28 + out 100k*0.42 + read 600k*0.03, all /1e6
    let expected = (400_000.0 * 0.28 + 100_000.0 * 0.42 + 600_000.0 * 0.03) / 1_000_000.0;
    assert!(approx(cost, expected), "got {cost} want {expected}");
}

#[test]
fn cost_claude_cache_creation() {
    // claude-opus-4-5: 5 in / 25 out / 0.5 read / 6.25 write.
    let cost = compute_cost_usd("claude-opus-4-5", 1_000_000, 50_000, 200_000, 300_000);
    let expected =
        (500_000.0 * 5.0 + 50_000.0 * 25.0 + 200_000.0 * 6.25 + 300_000.0 * 0.5) / 1_000_000.0;
    assert!(approx(cost, expected), "got {cost} want {expected}");
}

#[test]
fn cost_clamps_negative_plain_input() {
    let p = lookup_pricing("gpt-4o").unwrap();
    // Cache tokens exceed reported prompt tokens (separate-report convention)
    // → plain input clamps to 0, cache portion still billed.
    let cost = cost_from_pricing(p, 100, 0, 0, 500);
    let expected = 500.0 * p.cache_read_cost_per_million / 1_000_000.0;
    assert!(approx(cost, expected), "got {cost} want {expected}");
    assert!(cost >= 0.0);
}

#[test]
fn cost_unknown_model_degrades_to_zero() {
    assert_eq!(
        compute_cost_usd("testai-1.1", 1_000_000, 1_000_000, 0, 0),
        0.0
    );
    assert_eq!(compute_cost_usd("", 1000, 1000, 0, 0), 0.0);
}

#[test]
fn cost_from_pricing_synthetic_entry() {
    let p = ModelPricing {
        model_id: "synthetic".into(),
        display_name: "Synthetic".into(),
        input_cost_per_million: 1.0,
        output_cost_per_million: 2.0,
        cache_read_cost_per_million: 0.1,
        cache_creation_cost_per_million: 1.25,
        max_input_tokens: None,
        max_output_tokens: None,
        aliases: vec!["vendor/synthetic".into()],
    };
    // plain 100k*1 + out 200k*2 + write 50k*1.25 + read 150k*0.1 = 100k+400k+62.5k+15k = 577.5k /1e6
    let cost = cost_from_pricing(&p, 300_000, 200_000, 50_000, 150_000);
    assert!(approx(cost, 0.5775), "got {cost}");

    // The synthetic alias resolves through the table API when registered.
    let mut table = PricingTable {
        entries: Vec::new(),
        by_id: HashMap::new(),
        by_alias: HashMap::new(),
    };
    table.entries.push(p.clone());
    table.by_id.insert(p.model_id.clone(), 0);
    for a in &p.aliases {
        table.by_alias.insert(a.clone(), 0);
    }
    assert!(table.lookup("synthetic").is_some());
    assert!(table.lookup("vendor/synthetic").is_some());
    assert_eq!(
        table.lookup("vendor/synthetic").unwrap().model_id,
        "synthetic"
    );
}

#[test]
fn embedded_aliases_roundtrip() {
    let table = PricingTable::embedded();
    // glm-4.7 carries the fireworks provider-qualified alias.
    let glm = table.lookup("fireworks_ai/glm-4p7").expect("glm alias");
    assert_eq!(glm.model_id, "glm-4.7");
    assert!(glm.aliases.iter().any(|a| a == "fireworks_ai/glm-4p7"));
    // Entries expose their aliases for frontend matching.
    // (OpenAI entries carry no aliases — `openai/gpt-4o` resolves via the
    // bare-suffix fallback; deepseek defines one explicitly.)
    let ds = table.lookup("deepseek-chat").unwrap();
    assert!(ds.aliases.iter().any(|a| a == "deepseek/deepseek-chat"));
}
