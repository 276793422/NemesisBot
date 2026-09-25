//! PricingStore 分层价目表覆盖率补充测试（自定义层 alias 索引 / 下载层
//! 损坏降级 / lookup 四级匹配）。

use std::path::PathBuf;

use crate::models::ModelPricing;

fn temp_dir(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nemesis_pricing_cov_{}_{}_{}",
        tag,
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn entry(id: &str, aliases: &[&str]) -> ModelPricing {
    ModelPricing {
        model_id: id.to_string(),
        display_name: id.to_string(),
        input_cost_per_million: 1.0,
        output_cost_per_million: 2.0,
        cache_read_cost_per_million: 0.0,
        cache_creation_cost_per_million: 0.0,
        max_input_tokens: None,
        max_output_tokens: None,
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
    }
}

/// 自定义层带 alias 的条目：alias 索引（custom_alias_idx）建立后，
/// alias 直接命中（含 `vendor/alias` 前缀形态的 bare-suffix 命中）。
#[test]
fn custom_alias_lookup_hits_custom_layer() {
    let dir = temp_dir("custom_alias");
    {
        let store = crate::PricingStore::open(&dir).expect("open");
        store
            .upsert_custom(entry("my-model", &["my-alias", "legacy-name"]))
            .expect("upsert custom");
    }
    let reopened = crate::PricingStore::open(&dir).expect("reopen");
    let list = reopened.list_custom();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].aliases.len(), 2);

    // alias 直接命中
    let hit = reopened.lookup("my-alias").expect("alias hit");
    assert_eq!(hit.model_id, "my-model");
    // provider/alias 形态：bare-suffix 落 alias 命中
    let hit2 = reopened.lookup("vendor/my-alias").expect("bare alias hit");
    assert_eq!(hit2.model_id, "my-model");
    // 自定义优先于内嵌层：同名遮蔽内嵌条目
    let mine = reopened.lookup("my-model").expect("id hit");
    assert_eq!(mine.input_cost_per_million, 1.0);
}

/// 下载层文件损坏（非法 JSON）→ 打开成功、下载层静默降级为空。
#[test]
fn corrupted_downloaded_file_degrades_to_none() {
    let dir = temp_dir("corrupt_downloaded");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("model_prices_downloaded.json"), "{not json").expect("write corrupt");

    let store = crate::PricingStore::open(&dir).expect("open with corrupt downloaded layer");
    assert!(store.list_downloaded().is_none(), "corrupt layer ignored");
    // 内嵌层仍可查（既有测试锚定的国内模型条目）。
    assert!(
        store.lookup("glm-4.7").is_some(),
        "embedded layer reachable"
    );
}

/// 下载层合法但为空数组（`Ok(_) => None` 分支）→ 同样视为无下载层。
#[test]
fn empty_downloaded_array_treated_as_absent() {
    let dir = temp_dir("empty_downloaded");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("model_prices_downloaded.json"), "[]").expect("write empty array");

    let store = crate::PricingStore::open(&dir).expect("open with empty downloaded layer");
    assert!(store.list_downloaded().is_none());
}

/// 下载层正常装载：lookup 命中下载层条目（自定义 > 下载 > 内嵌）。
#[test]
fn downloaded_layer_loads_and_lookups() {
    let dir = temp_dir("downloaded_ok");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let entries = vec![entry("dl-model", &["dl-alias"])];
    std::fs::write(
        dir.join("model_prices_downloaded.json"),
        serde_json::to_string(&entries).expect("serialize"),
    )
    .expect("write");

    let store = crate::PricingStore::open(&dir).expect("open");
    let dl = store.list_downloaded().expect("downloaded layer present");
    assert_eq!(dl.len(), 1);
    assert_eq!(store.lookup("dl-model").expect("hit").model_id, "dl-model");
}
