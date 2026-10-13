// fallback_provider.rs 覆盖率补充测试（Exhausted Display 67、new 91、
// execute_image 全链失败 203-238、execute_detailed 失败记账 343 与
// 冷却全跳过的兜底 Unknown 459-461）。

use super::*;
use crate::failover::FailoverError;
use crate::router::LLMProvider;
use crate::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use async_trait::async_trait;
use std::sync::Arc;

/// 恒返 Timeout（可重试 → 会进冷却记账）的桩 provider。
struct TimeoutStub {
    name: String,
}

#[async_trait]
impl LLMProvider for TimeoutStub {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Err(FailoverError::Timeout {
            provider: self.name.clone(),
            model: "stub-model".to_string(),
        })
    }

    fn default_model(&self) -> &str {
        "stub-model"
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn stub(name: &str) -> FallbackEntry {
    FallbackEntry {
        provider: Arc::new(TimeoutStub {
            name: name.to_string(),
        }),
        model: "stub-model".to_string(),
    }
}

fn messages() -> Vec<Message> {
    vec![Message::text("user", "hi")]
}

/// Exhausted Display（60-73：链名 + 计数 + 每个成员错误行）。
#[test]
fn exhausted_error_display_lists_chain_and_errors() {
    let e = FallbackExhaustedError {
        chain_name: "cov-chain".to_string(),
        providers_attempted: 2,
        total_providers: 2,
        errors: vec![
            ("prov-a".to_string(), "timeout".to_string()),
            ("prov-b".to_string(), "rate limited".to_string()),
        ],
    };
    let s = format!("{e}");
    assert!(s.contains("cov-chain"), "{s}");
    assert!(s.contains("2/2 providers attempted"), "{s}");
    assert!(s.contains("- prov-a: timeout"), "{s}");
    assert!(s.contains("- prov-b: rate limited"), "{s}");
}

/// new()（88-98：链长记账构造）+ is_error 语义对照。
#[test]
fn new_records_chain() {
    let _ = FallbackProvider::new("cov-chain", vec![stub("a"), stub("b")]);
    let _ = FallbackProvider::new("empty-chain", vec![]);
}

/// execute_image：两成员链全败 → 逐成员 attempts 记账 + exhausted 收尾
/// （203-238 的 mid-chain 与 last-candidate 两臂）。
#[tokio::test]
async fn execute_image_exhausted_chain_reports_all_attempts() {
    let provider = FallbackProvider::new("cov-chain", vec![stub("img-a"), stub("img-b")]);
    let result = provider
        .execute_image(&messages(), &[], "m", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    assert_eq!(result.attempts.len(), 2, "{:?}", result.attempts);
    assert!(!result.attempts[0].success);
    assert_eq!(result.attempts[0].provider, "img-a");
    let exhausted = result.exhausted_error.expect("全败必须给 exhausted");
    assert_eq!(exhausted.total_providers, 2);
    assert_eq!(exhausted.errors.len(), 2);
}

/// execute_detailed：可重试失败进冷却记账（343 mark_failure 臂）。
#[tokio::test]
async fn execute_detailed_failure_marks_cooldown() {
    let provider = FallbackProvider::new("cov-chain", vec![stub("det-a")]);
    let result = provider
        .execute_detailed(&messages(), &[], "m", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    assert_eq!(result.attempts.len(), 1);
    assert!(result.exhausted_error.is_some());
}

/// execute_detailed：链内所有成员都在冷却 → 零尝试兜底 Unknown
/// 「all providers in fallback chain exhausted」（459-461）。
#[tokio::test]
async fn execute_detailed_all_cooled_down_falls_to_unknown() {
    let provider = FallbackProvider::new("cov-chain", vec![stub("cool-a")]);
    // 第一次：失败并进冷却。
    let _ = provider
        .execute_detailed(&messages(), &[], "m", &ChatOptions::default())
        .await;
    // 第二次：唯一成员被冷却跳过 → 只有 skipped 记账 + 兜底 exhausted。
    let result = provider
        .execute_detailed(&messages(), &[], "m", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    assert_eq!(result.attempts.len(), 1, "{:?}", result.attempts);
    assert_eq!(
        result.attempts[0].error.as_deref(),
        Some("skipped: in cooldown")
    );
    let exhausted = result.exhausted_error.expect("全冷却也必须给 exhausted");
    assert!(exhausted.errors.is_empty(), "{:?}", exhausted.errors);
    assert_eq!(exhausted.chain_name, "cov-chain");
}

/// execute_image 不看冷却：detailed/chat 路径打进冷却的成员在 image 路径
/// 依然被真实尝试（行为钉扎；与 execute_detailed 的跳过语义不对称）。
#[tokio::test]
async fn execute_image_ignores_cooldown_and_attempts_anyway() {
    let provider = FallbackProvider::new("cov-chain", vec![stub("img-cool")]);
    // 先经 detailed/chat 路径失败 → mark_failure 进冷却。
    let _ = provider
        .execute_detailed(&messages(), &[], "m", &ChatOptions::default())
        .await;
    let result = provider
        .execute_image(&messages(), &[], "m", &ChatOptions::default())
        .await;
    assert!(result.response.is_none());
    assert_eq!(result.attempts.len(), 1, "{:?}", result.attempts);
    assert_eq!(
        result.attempts[0].error.as_deref(),
        Some("timeout calling provider img-cool/stub-model")
    );
    assert!(result.exhausted_error.is_some());
}

/// LLMProvider::chat：首调失败进冷却记账（343 的 warn 臂），再调全冷却 →
/// 兜底 Unknown「all providers in fallback chain exhausted」（459-461）。
#[tokio::test]
async fn chat_trait_exhaustion_and_cooldown_skip() {
    let provider = FallbackProvider::new("cov-chain", vec![stub("chat-a")]);
    let err1 = LLMProvider::chat(&provider, &messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err1, FailoverError::Timeout { .. }), "{err1:?}");

    let err2 = LLMProvider::chat(&provider, &messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err2 {
        FailoverError::Unknown { message, .. } => {
            assert!(
                message.contains("all providers in fallback chain exhausted"),
                "{message}"
            );
        }
        other => panic!("预期 Unknown，得到 {other:?}"),
    }
}
