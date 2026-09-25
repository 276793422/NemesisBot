// loop/maintenance.rs 覆盖率补充测试（handle_maintenance 错误臂 /
// process_user_dispatch 无集群同步错误路径——收据先发 + user 行落盘 +
// 会话释放）。
//
// 形状同 e6_maintenance_tests（本文件私有迷你 provider/fixture，不跨文件
// 共享）。__ASYNC__ ACK 路径依赖集群 RPC 形态，由 cluster-uat 覆盖。

use super::*;

// ---------------------------------------------------------------------------
// 迷你 fixture（e6 同款形状）
// ---------------------------------------------------------------------------

struct CovLlmProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}

impl CovLlmProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmProvider for CovLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "cov final".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

fn cov_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

fn resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn cov_inbound(content: &str, session_key: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "covuser".to_string(),
        chat_id: "covchat".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

fn unique_key(tag: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("agent:main:session:covmt_{}_{}", tag, nanos)
}

// ---------------------------------------------------------------------------
// handle_maintenance 错误臂
// ---------------------------------------------------------------------------

/// Compact 错误臂：空会话 → `⚠ 压缩未完成：<原因>`（不走 ✓ 回执）。
#[tokio::test]
async fn handle_maintenance_compact_error_reports_failure() {
    let mut al = AgentLoop::new(Box::new(CovLlmProvider::new(vec![])), cov_config());
    al.set_session_store(std::sync::Arc::new(
        crate::session::SessionStore::new_in_memory(),
    ));
    let key = unique_key("compact_err");

    let reply = al
        .handle_maintenance(super::maintenance::SessionMaintenance::Compact, &key)
        .await;
    assert!(reply.contains("压缩未完成"), "got: {reply}");
    assert!(
        reply.contains("会话为空"),
        "root cause rides along: {reply}"
    );
}

/// Clear Ok 臂（对照锚）：`✓ 已清空会话历史`。
#[tokio::test]
async fn handle_maintenance_clear_ok_receipt() {
    let mut al = AgentLoop::new(
        Box::new(CovLlmProvider::new(vec![resp("unused")])),
        cov_config(),
    );
    al.set_session_store(std::sync::Arc::new(
        crate::session::SessionStore::new_in_memory(),
    ));
    let key = unique_key("clear_ok");
    crate::chat_log::append_chat_log(&key, "user", "to be cleared");

    let reply = al
        .handle_maintenance(super::maintenance::SessionMaintenance::Clear, &key)
        .await;
    assert_eq!(reply, "✓ 已清空会话历史");
    let (_, total, _, _) = crate::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(total, 0);
    crate::chat_log::delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// process_user_dispatch：无集群的同步错误路径（① 收据 ② 落盘 ⑤ 直复）
// ---------------------------------------------------------------------------

/// 集群未装配时 cluster_rpc 合成调用走同步错误 → 收据 + 错误两条出站、
/// user 原文落 chat_log 与 store、会话锁释放。
#[tokio::test]
async fn process_user_dispatch_without_cluster_replies_sync_error() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let mut al = AgentLoop::new(Box::new(CovLlmProvider::new(vec![])), cov_config());
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    al.set_session_store(store.clone());
    al.outbound_tx = Some(tx);

    let key = unique_key("dispatch");
    crate::chat_log::delete_chat_log(&key);
    // store.add_message 对未建条目的 key 静默丢弃（见 e6 populate 注），
    // 真实派发发生在既有会话上——先物化会话再派发。
    store.get_or_create(&key);
    let msg = cov_inbound("/build fix coverage repo:node-x", &key);

    al.process_user_dispatch(&msg, "fix coverage", "node-x", &key)
        .await;

    // ① 收据先发 + ⑤ 同步错误直复：出站共两条，顺序固定。
    let mut published = Vec::new();
    while let Ok(m) = rx.try_recv() {
        published.push(m);
    }
    assert_eq!(published.len(), 2, "published: {published:?}");
    assert!(
        published[0]
            .content
            .contains("⏳ 已把编码任务派发给节点 node-x"),
        "receipt first: {}",
        published[0].content
    );
    assert!(
        !published[1].content.is_empty() && published[1].content != published[0].content,
        "sync reply must differ from receipt: {}",
        published[1].content
    );

    // ② user 原文落 chat_log + store（会话一致性半边）。
    let (rows, total, _, _) = crate::chat_log::read_chat_log(&key, 10, None);
    assert!(total >= 1, "chat_log rows: {rows:?}");
    assert_eq!(rows[0]["role"].as_str(), Some("user"));
    assert_eq!(
        rows[0]["content"].as_str(),
        Some("/build fix coverage repo:node-x")
    );
    let history = store.get_history(&key);
    assert!(
        history
            .iter()
            .any(|m| m.role == "user" && m.content.contains("fix coverage"))
    );

    // 会话锁已释放（⑤ 尾巴 release_session）。
    assert!(al.try_acquire_session(&key), "session must be released");
    al.release_session(&key);
    crate::chat_log::delete_chat_log(&key);
}
