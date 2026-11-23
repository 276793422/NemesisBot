//! 快照复活封堵（2026-11-24）死会话臂 bus 面测试。
//!
//! 非空 session_key 快照 + jsonl 缺失/空 → `run_continuation_turn` 诚实
//! 拒绝：provider 零调用 / 注记出站（含结果文本）/ 快照回收。存活回归：
//! jsonl 有行（生产不变量——快照保存时 user 行恒已在轮首落盘）→ 正常
//! 续行执行。

use super::AgentLoop;
use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use async_trait::async_trait;
use nemesis_types::channel::InboundMessage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn unique_key(tag: &str) -> String {
    format!(
        "test:contdrop:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// 记次 provider：死会话臂必须零调用；存活臂恰好一次。
#[derive(Clone)]
struct CountingProvider {
    calls: Arc<AtomicUsize>,
    final_content: String,
    seen: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
}

impl CountingProvider {
    fn new(final_content: &str) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            final_content: final_content.to_string(),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl LlmProvider for CountingProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(messages);
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: self.final_content.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn drop_config() -> crate::types::AgentConfig {
    crate::types::AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: HashMap::new(),
        tool_fence_profession: None,
    }
}

fn cont_msg(task_id: &str, chat_id: &str, content: &str) -> InboundMessage {
    // 生产 G5 恢复/回调发布形态：闸门按 sender 内容前缀路由 Continuation
    // （msg_class 化只改分类，路由仍用前缀——s9_msg 同款构造）。
    InboundMessage::user_ingress(
        "system".to_string(),
        format!(
            "{}{task_id}",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        chat_id.to_string(),
        content.to_string(),
    )
}

fn user_msg(text: &str) -> LlmMessage {
    LlmMessage {
        role: "user".to_string(),
        content: text.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

/// 死会话臂：快照带非空 session_key 但 jsonl 缺失（保存后删除/清空）→
/// 诚实拒绝。provider 零调用；注记出站（含任务结果文本）；快照被回收。
#[tokio::test]
async fn dead_session_snapshot_is_dropped_with_honest_notice() {
    let _logs = crate::test_support::capture_logs();
    let key = unique_key("dead"); // 无 jsonl —— 判死
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let provider = CountingProvider::new("SHOULD_NOT_APPEAR");
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider.clone()),
        drop_config(),
        out_tx,
        crate::r#loop::ConcurrentMode::Reject,
        8,
        0,
    );
    let mgr = Arc::new(crate::loop_continuation::ContinuationManager::new());
    mgr.save_continuation(
        "dropgate1",
        vec![user_msg("go")],
        "tc1",
        "web",
        "chatDG",
        &key,
        "",
    )
    .await;
    agent_loop.set_continuation_manager(mgr.clone());

    in_tx
        .send(cont_msg("dropgate1", "chatDG", "the task result body"))
        .await
        .unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    // 注记出站：含拒绝原因 + 任务结果文本（worker 已产出，用户必须可见）。
    let out = out_rx.recv().await.expect("死会话臂注记必须出站");
    assert!(
        out.content.contains("上下文已被删除"),
        "注记须含删除原因，got: {}",
        out.content
    );
    assert!(
        out.content.contains("the task result body"),
        "注记须含任务结果文本，got: {}",
        out.content
    );
    assert_eq!(
        out.meta.task_id.as_deref(),
        Some("dropgate1"),
        "出站带 task_id 记账"
    );
    // provider 零调用（续行未执行——已删内容不回流）。
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        0,
        "死会话臂不得调用 provider"
    );
    // 快照被回收（finish_handling）。
    assert!(
        mgr.load_continuation("dropgate1").await.is_none(),
        "快照必须被回收"
    );
}

/// 存活回归：jsonl 有行（生产不变量）→ 不触发死会话臂，正常续行执行。
#[tokio::test]
async fn alive_session_snapshot_continues_normally() {
    let _logs = crate::test_support::capture_logs();
    let key = unique_key("alive");
    crate::chat_log::append_chat_log(&key, "user", "hi"); // 生产不变量：轮首已落盘
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let provider = CountingProvider::new("cont alive final");
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider.clone()),
        drop_config(),
        out_tx,
        crate::r#loop::ConcurrentMode::Reject,
        8,
        0,
    );
    let mgr = Arc::new(crate::loop_continuation::ContinuationManager::new());
    mgr.save_continuation(
        "dropgate2",
        vec![user_msg("go")],
        "tc2",
        "web",
        "chatDG",
        &key,
        "",
    )
    .await;
    agent_loop.set_continuation_manager(mgr.clone());

    in_tx
        .send(cont_msg("dropgate2", "chatDG", "the task result body"))
        .await
        .unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("存活续行必须出站最终回复");
    assert!(
        out.content.contains("cont alive final"),
        "got: {}",
        out.content
    );
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "存活续行恰好一次 LLM 调用"
    );
    assert!(
        mgr.load_continuation("dropgate2").await.is_none(),
        "快照执行后被回收"
    );
    crate::chat_log::delete_chat_log(&key);
}

/// jsonl 存在但零行（chat.clear 形态）→ 同样判死（truncate 后文件在，
/// 内容空 = 上下文已清空）。
#[tokio::test]
async fn cleared_session_snapshot_is_dropped_too() {
    let _logs = crate::test_support::capture_logs();
    let key = unique_key("cleared");
    crate::chat_log::append_chat_log(&key, "user", "hi");
    crate::chat_log::clear_chat_log(&key).expect("clear 落空文件"); // truncate 落空文件
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let provider = CountingProvider::new("SHOULD_NOT_APPEAR");
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider.clone()),
        drop_config(),
        out_tx,
        crate::r#loop::ConcurrentMode::Reject,
        8,
        0,
    );
    let mgr = Arc::new(crate::loop_continuation::ContinuationManager::new());
    mgr.save_continuation(
        "dropgate3",
        vec![user_msg("go")],
        "tc3",
        "web",
        "chatDG",
        &key,
        "",
    )
    .await;
    agent_loop.set_continuation_manager(mgr.clone());

    in_tx
        .send(cont_msg("dropgate3", "chatDG", "result text"))
        .await
        .unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("清空会话臂注记必须出站");
    assert!(
        out.content.contains("上下文已被删除"),
        "got: {}",
        out.content
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(mgr.load_continuation("dropgate3").await.is_none());
    crate::chat_log::delete_chat_log(&key);
}
