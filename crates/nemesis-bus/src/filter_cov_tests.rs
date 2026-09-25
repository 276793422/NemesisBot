//! FilterChain Default / Debug / Rejected 日志路径补充测试。

use std::sync::Arc;

use super::{Filter, FilterChain, FilterDecision};
use nemesis_types::channel::InboundMessage;

fn dummy_msg(tag: &str) -> InboundMessage {
    InboundMessage {
        channel: "test".to_string(),
        sender_id: "tester".to_string(),
        chat_id: "chat-1".to_string(),
        content: tag.to_string(),
        media: Vec::new(),
        session_key: "agent:main:session:s1".to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

/// Default impl 与 new 等价。
#[test]
fn default_chain_equals_new() {
    let chain: FilterChain<InboundMessage> = FilterChain::default();
    assert!(chain.list().is_empty());
}

/// Debug impl 免锁报规模（不触碰过滤器本体）。
#[test]
fn debug_reports_filter_count() {
    struct Noop;
    #[async_trait::async_trait]
    impl<T: Send + Sync> Filter<T> for Noop {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn priority(&self) -> i32 {
            0
        }
        async fn inspect(&self, _msg: &T) -> FilterDecision {
            FilterDecision::Pass
        }
    }
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    assert_eq!(format!("{chain:?}"), "FilterChain { filters: 0 }");
    chain.attach(Arc::new(Noop));
    assert_eq!(format!("{chain:?}"), "FilterChain { filters: 1 }");
}

/// Rejected 决策：链终止、拒绝原因透传（触发 reason 格式化 + info 日志
/// 路径）。
#[tokio::test]
async fn rejected_decision_carries_reason_and_stops_chain() {
    struct Reject;
    struct After;
    #[async_trait::async_trait]
    impl<T: Send + Sync> Filter<T> for Reject {
        fn name(&self) -> &'static str {
            "rejector"
        }
        fn priority(&self) -> i32 {
            0
        }
        async fn inspect(&self, _msg: &T) -> FilterDecision {
            FilterDecision::Rejected("history disabled".to_string())
        }
    }
    #[async_trait::async_trait]
    impl<T: Send + Sync> Filter<T> for After {
        fn name(&self) -> &'static str {
            "after"
        }
        fn priority(&self) -> i32 {
            10
        }
        async fn inspect(&self, _msg: &T) -> FilterDecision {
            FilterDecision::Pass
        }
    }

    let chain: FilterChain<InboundMessage> = FilterChain::new();
    chain.attach(Arc::new(After));
    chain.attach(Arc::new(Reject));
    let decision = chain.run(&dummy_msg("x")).await;
    assert_eq!(
        decision,
        FilterDecision::Rejected("history disabled".to_string())
    );
}
