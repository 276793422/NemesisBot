//! [`FilterChain`] 框架契约测试：排序、终止语义、透传、泛型性。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{Filter, FilterChain, FilterDecision};
use crate::MessageBus;
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

/// 记录调用序 + 可编程决策的探针过滤器。
struct Probe {
    tag: &'static str,
    prio: i32,
    decision: FilterDecision,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl<T: Send + Sync> Filter<T> for Probe {
    fn name(&self) -> &'static str {
        self.tag
    }
    fn priority(&self) -> i32 {
        self.prio
    }
    async fn inspect(&self, _msg: &T) -> FilterDecision {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.decision.clone()
    }
}

#[tokio::test]
async fn empty_chain_passes() {
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    assert_eq!(chain.run(&dummy_msg("x")).await, FilterDecision::Pass);
    assert!(chain.list().is_empty());
}

#[tokio::test]
async fn single_filter_intercepts() {
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    let calls = Arc::new(AtomicUsize::new(0));
    chain.attach(Arc::new(Probe {
        tag: "only",
        prio: 100,
        decision: FilterDecision::Intercepted,
        calls: calls.clone(),
    }));
    assert_eq!(
        chain.run(&dummy_msg("x")).await,
        FilterDecision::Intercepted
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn attach_sorts_by_priority_not_registration_order() {
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    // 乱序注册：200 先挂、100 后挂。
    chain.attach(Arc::new(Probe {
        tag: "high-prio-number",
        prio: 200,
        decision: FilterDecision::Pass,
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    chain.attach(Arc::new(Probe {
        tag: "low-prio-number",
        prio: 100,
        decision: FilterDecision::Pass,
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    assert_eq!(
        chain.list(),
        vec![("low-prio-number", 100), ("high-prio-number", 200)]
    );
}

#[tokio::test]
async fn first_non_pass_terminates_rest_untouched() {
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    let later_calls = Arc::new(AtomicUsize::new(0));
    chain.attach(Arc::new(Probe {
        tag: "first",
        prio: 100,
        decision: FilterDecision::Intercepted,
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    chain.attach(Arc::new(Probe {
        tag: "second-never-called",
        prio: 200,
        decision: FilterDecision::Pass,
        calls: later_calls.clone(),
    }));
    assert_eq!(
        chain.run(&dummy_msg("x")).await,
        FilterDecision::Intercepted
    );
    assert_eq!(later_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn equal_priority_keeps_registration_order() {
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    chain.attach(Arc::new(Probe {
        tag: "alpha",
        prio: 100,
        decision: FilterDecision::Pass,
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    chain.attach(Arc::new(Probe {
        tag: "beta",
        prio: 100,
        decision: FilterDecision::Pass,
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    assert_eq!(chain.list(), vec![("alpha", 100), ("beta", 100)]);
}

#[tokio::test]
async fn rejected_terminates_with_reason() {
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    chain.attach(Arc::new(Probe {
        tag: "strict",
        prio: 50,
        decision: FilterDecision::Rejected("不合规".to_string()),
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    assert_eq!(
        chain.run(&dummy_msg("x")).await,
        FilterDecision::Rejected("不合规".to_string())
    );
}

/// 泛型性：链对任意消息类型可用（框架不绑定 InboundMessage）。
#[tokio::test]
async fn chain_is_generic_over_payload_type() {
    struct Ping;
    let chain: FilterChain<Ping> = FilterChain::new();
    chain.attach(Arc::new(Probe {
        tag: "pinger",
        prio: 1,
        decision: FilterDecision::Intercepted,
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    assert_eq!(chain.run(&Ping).await, FilterDecision::Intercepted);
}

/// 挂载点语义演练：Intercepted 的消息不进扇出（用真 bus 验证订阅方零收到）。
struct ContentGate {
    intercept_tag: &'static str,
}

#[async_trait::async_trait]
impl Filter<InboundMessage> for ContentGate {
    fn name(&self) -> &'static str {
        "content-gate"
    }
    fn priority(&self) -> i32 {
        100
    }
    async fn inspect(&self, msg: &InboundMessage) -> FilterDecision {
        if msg.content == self.intercept_tag {
            FilterDecision::Intercepted
        } else {
            FilterDecision::Pass
        }
    }
}

#[tokio::test]
async fn intercepted_message_never_reaches_bus_fanout() {
    let bus = Arc::new(MessageBus::new());
    let mut rx = bus.subscribe_inbound();
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    chain.attach(Arc::new(ContentGate {
        intercept_tag: "hooked",
    }));

    for content in ["hooked", "normal"] {
        let msg = dummy_msg(content);
        if chain.run(&msg).await == FilterDecision::Pass {
            bus.publish_inbound(msg);
        }
    }

    // 只有 Pass 的那条到达订阅者。
    let got = rx.try_recv().expect("passthrough message should arrive");
    assert_eq!(got.content, "normal");
    assert!(
        rx.try_recv().is_err(),
        "intercepted message must not fan out"
    );
}
