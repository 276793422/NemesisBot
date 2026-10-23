//! config_watch 纯逻辑测试：`current_max_tokens_override` 的 u32 值域防护。
//!
//! per-model `max_output_tokens` 是 JSON 直读 i64（resolver 家族契约：值
//! 原样透传），消费侧 `ChatOptions.max_tokens` 是 `Option<u32>`——裸 `as`
//! 会把 -1 回绕成 4294967291，每轮请求带巨值 max_tokens 打爆 provider
//! 400。修复后：值域外的坏值按缺席处理（None → 调用方回落 8192 默认）。
//!
//! 独立测试文件（生产文件只保留声明行，仓库 2026-08-25 纪律）；provider
//! 桩自建（loop/tests.rs 的辅助模块私有，与 length_escalation_tests 同先例）。

use super::*;
use async_trait::async_trait;

fn cw_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

struct NoopProvider;

#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("not used".to_string())
    }
}

/// 带 config.json 的 loop（TempDir 须与 loop 同寿命，返回持有）。
fn loop_with_config(cfg: serde_json::Value) -> (tempfile::TempDir, Arc<AgentLoop>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.json");
    std::fs::write(&path, serde_json::to_string(&cfg).unwrap()).unwrap();
    let loop_ = Arc::new(AgentLoop::new(Box::new(NoopProvider), cw_config()));
    loop_.set_config_path(path);
    (dir, loop_)
}

fn override_of(loop_: &AgentLoop, alias: &str) -> Option<u32> {
    loop_.current_max_tokens_override(Some(alias))
}

#[test]
fn valid_value_passthrough() {
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/ok", "model_name": "m-ok", "max_output_tokens": 1234}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-ok"), Some(1234));
}

#[test]
fn negative_value_treated_as_absent_not_wrapped() {
    // 修复前：-1i64 as u32 = 4294967291 → 每轮请求带巨值 max_tokens。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/neg", "model_name": "m-neg", "max_output_tokens": -1}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-neg"), None);
}

#[test]
fn over_u32_value_treated_as_absent_not_truncated() {
    // 修复前：99_999_999_999i64 as u32 高位截断成无意义小值。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/big", "model_name": "m-big",
             "max_output_tokens": 99_999_999_999i64}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-big"), None);
}

#[test]
fn absent_key_still_none() {
    // 对照：键缺席 = None（resolver 家族语义不受影响）。
    let (_dir, loop_) = loop_with_config(serde_json::json!({
        "model_list": [
            {"model": "m/plain", "model_name": "m-plain"}
        ]
    }));
    assert_eq!(override_of(&loop_, "m-plain"), None);
}
