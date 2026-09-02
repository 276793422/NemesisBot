//! [`super::PricingStore`] 分层价目表测试：临时目录落盘真实文件——
//! 原子写 / 损坏降级 / 三层优先级都按真实磁盘行为验证。

use super::*;
use crate::models::ModelPricing;

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nb-pricing-store-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn entry(id: &str, input: f64, output: f64) -> ModelPricing {
    ModelPricing {
        model_id: id.to_string(),
        display_name: "test".to_string(),
        input_cost_per_million: input,
        output_cost_per_million: output,
        cache_read_cost_per_million: 0.0,
        cache_creation_cost_per_million: 0.0,
        max_input_tokens: None,
        max_output_tokens: None,
        aliases: Vec::new(),
    }
}

fn fresh_store(name: &str) -> (PathBuf, PricingStore) {
    let dir = tmp_dir(name);
    let store = PricingStore::open(&dir).unwrap();
    (dir, store)
}

#[test]
fn empty_store_falls_through_to_embedded() {
    let (_dir, store) = fresh_store("empty");
    // 内置表含 gpt-4o（离线兜底）；自定义/下载层为空。
    let p = store.lookup("gpt-4o").expect("embedded fallback");
    assert_eq!(p.model_id, "gpt-4o");
    assert!(store.list_downloaded().is_none());
    assert!(store.list_custom().is_empty());
}

#[test]
fn custom_overrides_downloaded_and_embedded() {
    let (_dir, store) = fresh_store("priority");
    // 下载层有 gpt-4o（来自"下载"）；自定义层覆盖它。
    store
        .replace_downloaded(
            vec![entry("gpt-4o", 99.0, 99.0), entry("glm-4.7", 8.0, 8.0)],
            PricingMeta {
                entry_count: 2,
                ..Default::default()
            },
        )
        .unwrap();
    let got = store.lookup("gpt-4o").unwrap();
    assert_eq!(got.input_cost_per_million, 99.0, "downloaded layer visible");

    let mut custom = entry("gpt-4o", 1.5, 7.5);
    custom.aliases = vec!["my-gpt4o".to_string()];
    store.upsert_custom(custom).unwrap();

    let got = store.lookup("gpt-4o").unwrap();
    assert_eq!(got.input_cost_per_million, 1.5, "custom > downloaded");
    // 自定义别名也命中。
    assert_eq!(store.lookup("my-gpt4o").unwrap().model_id, "gpt-4o");
    // 下载层条目仍可达。
    assert_eq!(store.lookup("glm-4.7").unwrap().input_cost_per_million, 8.0);
    // 内置兜底仍可达（claude-sonnet-4-5 在内置 36 表内）。
    assert_eq!(
        store.lookup("claude-sonnet-4-5").unwrap().model_id,
        "claude-sonnet-4-5"
    );
}

#[test]
fn provider_qualified_name_hits_bare_suffix() {
    let (_dir, store) = fresh_store("suffix");
    store
        .replace_downloaded(
            vec![entry("deepseek-chat", 0.27, 1.1)],
            PricingMeta::default(),
        )
        .unwrap();
    // 我们的配置名是 provider/model 形状。
    let p = store.lookup("deepseek/deepseek-chat").unwrap();
    assert_eq!(p.model_id, "deepseek-chat");
}

#[test]
fn cost_math_matches_cost_from_pricing() {
    let (_dir, store) = fresh_store("cost");
    store
        .replace_downloaded(vec![entry("m1", 2.5, 10.0)], PricingMeta::default())
        .unwrap();
    // (1e6 - 2e5 - 3e5) * 2.5 + 2e5 * 10 / 1e6 … 全部 per-million /1e6。
    let cost = store.compute_cost_usd("m1", 1_000_000, 200_000, 200_000, 300_000);
    let expect = crate::cost_from_pricing(
        &entry("m1", 2.5, 10.0),
        1_000_000,
        200_000,
        200_000,
        300_000,
    );
    assert!((cost - expect).abs() < 1e-12);
    assert_eq!(store.compute_cost_usd("unknown-model", 100, 100, 0, 0), 0.0);
}

#[test]
fn corrupt_custom_file_degrades_and_survives() {
    let (dir, store) = fresh_store("corrupt");
    store.upsert_custom(entry("keep", 3.0, 3.0)).unwrap();
    // 写坏自定义文件后重开 → 自定义层丢弃（降级），下载/内置层继续。
    std::fs::write(dir.join(CUSTOM_FILE), "{not json").unwrap();
    let reopened = PricingStore::open(&dir).unwrap();
    assert!(
        reopened.lookup("keep").is_none(),
        "corrupt custom layer dropped"
    );
    assert!(reopened.lookup("gpt-4o").is_some(), "embedded still works");
}

#[test]
fn upsert_is_idempotent_and_remove_reports_missing() {
    let (_dir, store) = fresh_store("upsert");
    store.upsert_custom(entry("x", 1.0, 1.0)).unwrap();
    store.upsert_custom(entry("x", 2.0, 2.0)).unwrap();
    assert_eq!(store.list_custom().len(), 1);
    assert_eq!(store.list_custom()[0].input_cost_per_million, 2.0);
    assert!(store.remove_custom("x").unwrap());
    assert!(
        !store.remove_custom("x").unwrap(),
        "second remove = not found"
    );
}

#[test]
fn persistence_across_reopen() {
    let (dir, store) = fresh_store("persist");
    store
        .replace_downloaded(
            vec![entry("dl-model", 5.0, 5.0)],
            PricingMeta {
                etag: Some("\"abc\"".to_string()),
                fetched_at: Some(1_700_000_000),
                source_url: Some("https://example.test/table.json".to_string()),
                entry_count: 1,
            },
        )
        .unwrap();
    store.upsert_custom(entry("cust-model", 7.0, 7.0)).unwrap();

    let reopened = PricingStore::open(&dir).unwrap();
    assert_eq!(
        reopened.lookup("dl-model").unwrap().input_cost_per_million,
        5.0
    );
    assert_eq!(
        reopened
            .lookup("cust-model")
            .unwrap()
            .input_cost_per_million,
        7.0
    );
    let meta = reopened.meta();
    assert_eq!(meta.etag.as_deref(), Some("\"abc\""));
    assert_eq!(meta.entry_count, 1);
    assert_eq!(reopened.list_downloaded().unwrap().len(), 1);
}

#[test]
fn failed_fetch_updates_meta_but_keeps_table() {
    let (_dir, store) = fresh_store("failfetch");
    store
        .replace_downloaded(vec![entry("dl-model", 5.0, 5.0)], PricingMeta::default())
        .unwrap();
    store
        .record_failed_fetch("https://mirror.test/table.json")
        .unwrap();
    // 表保留 + meta 刷新。
    assert_eq!(
        store.lookup("dl-model").unwrap().input_cost_per_million,
        5.0
    );
    assert_eq!(
        store.meta().source_url.as_deref(),
        Some("https://mirror.test/table.json")
    );
    assert!(store.meta().fetched_at.is_some());
}
