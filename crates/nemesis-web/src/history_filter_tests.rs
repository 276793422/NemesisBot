//! [`HistoryFilter`] 行为测试：Pass/拦截/应答内容/坏请求兜底。
//!
//! chat_log 走全局 path manager（既有测试同款约定：唯一键 + 测试后清理）。

use std::sync::Arc;

use nemesis_bus::{Filter, FilterChain, FilterDecision};
use nemesis_types::channel::InboundMessage;

use crate::history_filter::HistoryFilter;

struct BusFixture {
    bus: Arc<nemesis_bus::MessageBus>,
}

impl BusFixture {
    fn new() -> Self {
        Self {
            bus: Arc::new(nemesis_bus::MessageBus::new()),
        }
    }

    fn filter(&self) -> HistoryFilter {
        HistoryFilter::new(self.bus.clone())
    }
}

fn unique_session_key() -> String {
    format!(
        "agent:main:session:hf-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn history_msg(session_key: &str, content: String) -> InboundMessage {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("request_type".to_string(), "history".to_string());
    metadata.insert("session_id".to_string(), "hf-sid-x".to_string());
    InboundMessage {
        channel: "web".to_string(),
        sender_id: "tester".to_string(),
        chat_id: "conn-7".to_string(),
        content,
        media: Vec::new(),
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

#[tokio::test]
async fn non_history_message_passes_without_outbound() {
    let fx = BusFixture::new();
    let mut out_rx = fx.bus.subscribe_outbound();
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    chain.attach(Arc::new(fx.filter()));

    let mut msg = history_msg("agent:main:session:whatever", r#"{"text":1}"#.to_string());
    msg.metadata.remove("request_type"); // 普通 chat.send 形态
    assert_eq!(chain.run(&msg).await, FilterDecision::Pass);
    assert!(out_rx.try_recv().is_err(), "Pass 不得产生任何出站");
}

#[tokio::test]
async fn history_request_intercepted_and_served_from_chat_log() {
    let fx = BusFixture::new();
    let mut out_rx = fx.bus.subscribe_outbound();

    let session_key = unique_session_key();
    // 预置主 workspace chat_log（与生产同一条落盘路径）。
    nemesis_agent::chat_log::append_chat_log(&session_key, "user", "历史问题甲");
    nemesis_agent::chat_log::append_chat_log(&session_key, "assistant", "历史回答乙");
    nemesis_agent::chat_log::append_chat_log(&session_key, "user", "历史问题丙");

    let chain: FilterChain<InboundMessage> = FilterChain::new();
    chain.attach(Arc::new(fx.filter()));

    let payload = serde_json::json!({"request_id": "rq-1", "limit": 20});
    let msg = history_msg(&session_key, payload.to_string());
    assert_eq!(chain.run(&msg).await, FilterDecision::Intercepted);

    let out = out_rx.try_recv().expect("拦截后必须产生出站帧");
    assert_eq!(out.channel, "web");
    assert_eq!(out.chat_id, "conn-7");
    assert_eq!(out.message_type, "history");

    let v: serde_json::Value = serde_json::from_str(&out.content).expect("合法 JSON");
    assert_eq!(v["request_id"], "rq-1");
    // HD：会话归属回显。
    assert_eq!(v["session_id"], "hf-sid-x");
    // 三条预置行全部可见（含项目会话场景下「loop 不存在也能读」的核心断言）。
    assert_eq!(v["total_count"], 3);
    let msgs = v["messages"].as_array().expect("messages array");
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0]["content"], "历史问题甲");
    assert_eq!(msgs[2]["content"], "历史问题丙");
    // 测试环境环空 → last_seq 省略。
    assert!(v.get("last_seq").is_none());

    nemesis_agent::chat_log::delete_chat_log(&session_key);
}

#[tokio::test]
async fn garbage_payload_gets_empty_page_response_not_silence() {
    let fx = BusFixture::new();
    let mut out_rx = fx.bus.subscribe_outbound();
    let filter = fx.filter();

    let msg = history_msg("agent:main:session:whatever", "not json".to_string());
    assert_eq!(filter.inspect(&msg).await, FilterDecision::Intercepted);

    let out = out_rx.try_recv().expect("解析失败也必须诚实应答");
    assert_eq!(out.message_type, "history");
    let v: serde_json::Value = serde_json::from_str(&out.content).expect("合法 JSON");
    // 与 loop 内失败路径同形态：request_id=""、空页、无归属。
    assert_eq!(v["request_id"], "");
    assert_eq!(v["messages"], serde_json::json!([]));
    assert_eq!(v["total_count"], 0);
    assert_eq!(v["session_id"], "");
}

#[tokio::test]
async fn empty_session_key_falls_back_to_metadata_derivation() {
    let fx = BusFixture::new();
    let mut out_rx = fx.bus.subscribe_outbound();

    // 非 web 假想发布方：session_key 空，仅带 metadata.session_id——
    // 回落 loop 同款推导（agent:main:session:{sid}）。
    let session_key = unique_session_key();
    let sid = session_key
        .strip_prefix("agent:main:session:")
        .unwrap()
        .to_string();
    nemesis_agent::chat_log::append_chat_log(&session_key, "user", "回落推导可见");

    let filter = fx.filter();
    let mut msg = history_msg("", serde_json::json!({"request_id": "rq-2"}).to_string());
    msg.metadata.insert("session_id".to_string(), sid.clone());
    assert_eq!(filter.inspect(&msg).await, FilterDecision::Intercepted);

    let out = out_rx.try_recv().expect("必须产生出站帧");
    let v: serde_json::Value = serde_json::from_str(&out.content).expect("合法 JSON");
    assert_eq!(v["total_count"], 1);
    assert_eq!(v["messages"][0]["content"], "回落推导可见");

    nemesis_agent::chat_log::delete_chat_log(&session_key);
}
