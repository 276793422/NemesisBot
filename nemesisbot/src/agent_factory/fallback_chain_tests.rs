//! T2b（扩展计划 D4）：fallback 链装配单测（`wrap_fallback_chain`）。
//!
//! 断言面：`fallback_to` 缺失/全不可用 = 原样返回主 provider（`Arc::ptr_eq`，
//! 行为与现状字节一致）；有效别名 → 包装成 FallbackProvider（非 ptr_eq）；
//! 单级 resolve 失败 warn 跳过不阻断。提供者构造期无网络调用——api_base
//! 指向不存在的本地端口即可。

use std::collections::BTreeMap;
use std::sync::Arc;

use super::wrap_fallback_chain;

const DEAD_BASE: &str = "http://127.0.0.1:1/v1";

fn entry_with_extras(
    name: &str,
    extra: BTreeMap<String, serde_json::Value>,
) -> nemesis_config::ModelConfig {
    let mut m = nemesis_config::ModelConfig {
        model_name: name.to_string(),
        model: format!("test/{name}"),
        api_base: DEAD_BASE.to_string(),
        api_key: "test-key".to_string(),
        ..Default::default()
    };
    m.extra = extra;
    m
}

fn plain_entry(name: &str) -> nemesis_config::ModelConfig {
    entry_with_extras(name, BTreeMap::new())
}

fn cfg_with_model_list(entries: Vec<nemesis_config::ModelConfig>) -> nemesis_config::Config {
    let mut cfg = nemesis_config::Config::default();
    cfg.model_list = entries;
    cfg
}

fn fallback_to_extra(aliases: &[&str]) -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([("fallback_to".to_string(), serde_json::json!(aliases))])
}

fn dummy_provider() -> Arc<dyn nemesis_providers::router::LLMProvider> {
    Arc::new(nemesis_providers::null_provider::NullProvider::new(
        "test dummy".to_string(),
    ))
}

/// `fallback_to` 缺失 = 原样返回主 provider（Arc 同一指针，现状字节一致）。
#[test]
fn no_fallback_to_returns_primary() {
    let cfg = cfg_with_model_list(vec![plain_entry("m-a")]);
    let primary = dummy_provider();
    let out = wrap_fallback_chain(&cfg, "m-a", std::path::Path::new("."), primary.clone());
    assert!(Arc::ptr_eq(&primary, &out));
}

/// 两级可用别名 → FallbackProvider 包装（非同一指针）。
#[test]
fn two_level_chain_wraps_provider() {
    let cfg = cfg_with_model_list(vec![
        entry_with_extras("m-a", fallback_to_extra(&["m-b", "m-c"])),
        plain_entry("m-b"),
        plain_entry("m-c"),
    ]);
    let primary = dummy_provider();
    let out = wrap_fallback_chain(&cfg, "m-a", std::path::Path::new("."), primary.clone());
    assert!(
        !Arc::ptr_eq(&primary, &out),
        "expected FallbackProvider wrap"
    );
}

/// 别名不存在（resolve 失败）→ 跳过；全不可用 → 回落主 provider。
#[test]
fn unusable_alias_falls_back_to_primary() {
    let cfg = cfg_with_model_list(vec![entry_with_extras(
        "m-a",
        fallback_to_extra(&["ghost-model"]),
    )]);
    let primary = dummy_provider();
    let out = wrap_fallback_chain(&cfg, "m-a", std::path::Path::new("."), primary.clone());
    assert!(Arc::ptr_eq(&primary, &out));
}

/// 自引用别名跳过——链里出现自己只会空转冷却。
#[test]
fn self_reference_alias_skipped() {
    let cfg = cfg_with_model_list(vec![entry_with_extras("m-a", fallback_to_extra(&["m-a"]))]);
    let primary = dummy_provider();
    let out = wrap_fallback_chain(&cfg, "m-a", std::path::Path::new("."), primary.clone());
    assert!(Arc::ptr_eq(&primary, &out));
}

/// 混合：一个坏别名 + 一个好别名 → 仍然包装（好级别保住）。
#[test]
fn mixed_aliases_keep_usable_level() {
    let cfg = cfg_with_model_list(vec![
        entry_with_extras("m-a", fallback_to_extra(&["ghost-model", "m-b"])),
        plain_entry("m-b"),
    ]);
    let primary = dummy_provider();
    let out = wrap_fallback_chain(&cfg, "m-a", std::path::Path::new("."), primary.clone());
    assert!(
        !Arc::ptr_eq(&primary, &out),
        "usable level should still wrap"
    );
}
