//! N1（devtool-upgrade 阶段 1）：三级 context_window 解析链测试。
//!
//! L1 config 显式 > L2 价目表 catalog > L3 fallback-128k，外加 instance
//! 默认值同源断言与「L3 猜大 → provider 溢出 → 应急压缩 → 重试成功」
//! 端到端回归锁（溢出应急链的机制测试本就存在于 tests.rs 的
//! `test_run_with_context_error_and_retry_success`——这里补 L3 场景版）。

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::{ChatOptions, ToolDefinition};

/// 最小 config（与 tests.rs 的 test_config 同形；helper 私有故本地复制）。
fn local_test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

/// 空 provider——解析链测试不发 LLM 请求，占位即可。
struct NoopProvider;
#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("noop".to_string())
    }
}

fn temp_dir_unique(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("nb-ctx-window-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// config.json 落盘（含 context_window 的条目可自由拼）。
fn write_config(dir: &std::path::Path, model_list: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("config.json");
    std::fs::write(&path, format!(r#"{{"model_list": [{model_list}]}}"#)).unwrap();
    path
}

fn ctx_entry(name: &str, full: &str, context_window: &str) -> String {
    format!(r#"{{"model_name": "{name}", "model": "{full}", "context_window": {context_window}}}"#)
}

fn plain_entry(name: &str, full: &str) -> String {
    format!(r#"{{"model_name": "{name}", "model": "{full}"}}"#)
}

/// 播种下载层价目条目（max_input_tokens 可选）。
fn pricing_entry(id: &str, max_input: Option<i64>) -> nemesis_data::ModelPricing {
    nemesis_data::ModelPricing {
        model_id: id.to_string(),
        display_name: "test".to_string(),
        input_cost_per_million: 1.0,
        output_cost_per_million: 2.0,
        cache_read_cost_per_million: 0.0,
        cache_creation_cost_per_million: 0.0,
        max_input_tokens: max_input,
        max_output_tokens: None,
        aliases: Vec::new(),
    }
}

#[test]
fn l1_config_explicit_wins() {
    let dir = temp_dir_unique("l1");
    let cfg_path = write_config(&dir, &ctx_entry("opus", "x/opus", "200000"));
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();

    let (window, source) = resolve_context_window_tiered(Some(&cfg), "opus", None);
    assert_eq!(window, Some(200000));
    assert_eq!(source, "config");
}

#[test]
fn l2_catalog_hit_including_bare_suffix() {
    let dir = temp_dir_unique("l2");
    let store = std::sync::Arc::new(nemesis_data::PricingStore::open(&dir).unwrap());
    store
        .replace_downloaded(
            vec![pricing_entry("glm-4.7", Some(128_000))],
            nemesis_data::PricingMeta::default(),
        )
        .unwrap();

    // 无 config（standalone 形态）+ 带厂商前缀的别名 → bare-suffix 命中。
    let (window, source) =
        resolve_context_window_tiered(None, "zhipu/glm-4.7", Some(store.as_ref()));
    assert_eq!(window, Some(128_000));
    assert_eq!(source, "catalog");

    // config 有该条目但没配 context_window → 仍走 catalog。
    let cfg: serde_json::Value = serde_json::from_str(&format!(
        "{{\"model_list\": [{}]}}",
        plain_entry("glm", "zhipu/glm-4.7")
    ))
    .unwrap();
    let (window, source) = resolve_context_window_tiered(Some(&cfg), "glm", Some(store.as_ref()));
    assert_eq!(window, Some(128_000), "catalog 供给生效窗口");
    assert_eq!(source, "catalog");
}

#[test]
fn l3_fallback_when_neither_hits() {
    // 无 config、无价目表注入。
    let (window, source) = resolve_context_window_tiered(None, "obscure/local-7b", None);
    assert_eq!(window, None);
    assert_eq!(source, "fallback-128k");

    // 价目表完全未知的模型也一样（lookup 返回 None）。
    let dir = temp_dir_unique("l3");
    let store = nemesis_data::PricingStore::open(&dir).unwrap();
    let (window, source) = resolve_context_window_tiered(None, "obscure/local-7b", Some(&store));
    assert_eq!(window, None);
    assert_eq!(source, "fallback-128k");

    assert_eq!(FALLBACK_CONTEXT_WINDOW, 128_000, "L3 常量 = 128k");
}

#[test]
fn instance_default_matches_fallback() {
    // instance.rs 的默认值必须与 FALLBACK_CONTEXT_WINDOW 同源（N1 前是
    // 各写各的 32000）。
    let instance = AgentInstance::new(local_test_config());
    assert_eq!(instance.context_window(), FALLBACK_CONTEXT_WINDOW);
    assert_eq!(instance.context_window(), 128_000);
}

#[tokio::test]
async fn agent_loop_resolves_all_three_tiers() {
    // AgentLoop 路径的三级：同一 loop 换 config / pricing 观察解析结果。
    let dir = temp_dir_unique("loop-tiers");
    let cfg_path = write_config(&dir, &plain_entry("local", "obscure/local-7b"));
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), local_test_config());
    agent_loop.set_config_path(cfg_path);
    agent_loop.set_active_model("local");

    // L3：config 条目存在但无 context_window、无 pricing。
    let (w, source) = agent_loop.current_context_window_with_source();
    assert_eq!(w, None);
    assert_eq!(source, "fallback-128k");

    // L2：注入价目表后同一 loop 命中 catalog。
    let pricing_dir = temp_dir_unique("loop-pricing");
    let store = std::sync::Arc::new(nemesis_data::PricingStore::open(&pricing_dir).unwrap());
    store
        .replace_downloaded(
            vec![pricing_entry("local-7b", Some(96_000))],
            nemesis_data::PricingMeta::default(),
        )
        .unwrap();
    agent_loop.set_pricing_store(store);
    let (w, source) = agent_loop.current_context_window_with_source();
    assert_eq!(w, Some(96_000));
    assert_eq!(source, "catalog");

    // L1：config 显式 context_window 压过价目表。
    let cfg_path = write_config(&dir, &ctx_entry("local", "obscure/local-7b", "64000"));
    agent_loop.set_config_path(cfg_path);
    let (w, source) = agent_loop.current_context_window_with_source();
    assert_eq!(w, Some(64_000));
    assert_eq!(source, "config");

    // 兼容封装：current_context_window 只回窗口。
    assert_eq!(agent_loop.current_context_window(), Some(64_000));
}

/// N1 回归锁：L3 猜大（128k）但真实模型更小 → provider 报溢出 → 应急
/// 压缩（force_compression）→ 重试成功。机制与
/// `test_run_with_context_error_and_retry_success` 相同，差异在配置侧
/// **明确走 L3**（无 context_window 键、无价目表注入）。
#[tokio::test]
async fn l3_guess_too_big_recovers_via_emergency_compression() {
    struct ContextErrorThenSuccessProvider {
        call_count: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl crate::r#loop::LlmProvider for ContextErrorThenSuccessProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<ChatOptions>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if count == 0 {
                Err(
                    "context_length_exceeded: this model's maximum context length is 32768 tokens"
                        .to_string(),
                )
            } else {
                Ok(LlmResponse {
                    content: "Recovered after compression!".to_string(),
                    tool_calls: Vec::new(),
                    finished: true,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                })
            }
        }
    }

    // config 条目无 context_window、loop 无 pricing_store → L3 128k 生效。
    let dir = temp_dir_unique("l3-overflow");
    let cfg_path = write_config(&dir, &plain_entry("local", "obscure/local-7b"));
    let agent_loop = AgentLoop::new(
        Box::new(ContextErrorThenSuccessProvider {
            call_count: AtomicUsize::new(0),
        }),
        local_test_config(),
    );
    agent_loop.set_config_path(cfg_path);
    agent_loop.set_active_model("local");
    let (w, source) = agent_loop.current_context_window_with_source();
    assert_eq!(
        (w, source),
        (None, "fallback-128k"),
        "前置：本测试确实走 L3"
    );

    let instance = AgentInstance::new(local_test_config());
    let context = crate::context::RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "Hello", &context).await;

    let done: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            // AgentEvent 经 loop.rs 的 use 从 crate::types 引入，super::* 可见。
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(done, vec!["Recovered after compression!".to_string()]);
}
