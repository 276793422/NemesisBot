// L5（2026-10-12 内部专项 T0）：续行回灌路径的输出凭据复扫测试。
//
// 工具输出回灌有三条路：①直接回灌（tool_dispatch 主线，output_rescan_tests
// 已覆盖）；②cluster 续行追加；③子代理后台返回（`subagent_continuation:`
// 前缀与 ② 走同一 dispatch_continuation）。本文件覆盖 ②③——经
// dispatch_continuation 的端到端：带假凭据的 task_response 进续行 LLM 的
// tool result 必须是脱敏版（inline 与 spawned 两臂都断言）；另加一例无安全
// 插件时明文直通（接线不得过度脱敏）。
//
// 自带迷你 provider（记录每次 chat 收到的 messages），不与 s9/g4 共享私有
// 类型——本仓测试文件独立惯例。

use super::*;

/// AWS 示例键：默认凭据扫描表命中样本（与 output_rescan_tests 同源）。
const AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// 记录每次 chat 收到的 messages 的 mock provider（断言续行 LLM 实际看到的
/// 上下文，而非只看出站回复）。
struct CapturingProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<LlmMessage>>>>,
}

impl CapturingProvider {
    fn new(
        responses: Vec<LlmResponse>,
        seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<LlmMessage>>>>,
    ) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            seen,
        }
    }
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.seen.lock().unwrap().push(messages);
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "l5 exhausted".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                finish_reason: None,
                escalated_to: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

fn l5_resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        finish_reason: None,
        escalated_to: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn l5_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn l5_msg(
    channel: &str,
    sender_id: &str,
    chat_id: &str,
    session_key: &str,
    content: &str,
    metadata: std::collections::HashMap<String, String>,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

/// 带 credential 扫描的安全插件（生产 agent_factory 注入同源；
/// dlp 关——本组测试只钉凭据路，DLP 语义归 output_rescan_tests）。
fn l5_security_plugin() -> std::sync::Arc<nemesis_security::pipeline::SecurityPlugin> {
    std::sync::Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig {
            credential_enabled: true,
            dlp_enabled: false,
            ..Default::default()
        },
    ))
}

/// 种一个最小续行快照（user 消息 + 待应答 tool_call）。
async fn seed_continuation(mgr: &crate::loop_continuation::ContinuationManager, task_id: &str) {
    mgr.save_continuation(
        task_id,
        vec![
            LlmMessage {
                role: "user".to_string(),
                content: "run the remote task".to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            },
            LlmMessage {
                role: "assistant".to_string(),
                content: String::new(),
                tool_calls: Some(vec![crate::types::ToolCallInfo {
                    id: format!("tc_{task_id}"),
                    name: "cluster_rpc".to_string(),
                    arguments: "{}".to_string(),
                }]),
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            },
        ],
        &format!("tc_{task_id}"),
        "web",
        "chatL5",
        "l5sess",
        "",
    )
    .await;
}

/// 断言捕获的续行请求里凭据已脱敏（任一 tool 消息带遮蔽 token + 尾注，
/// 全文无明文键）。
fn assert_redacted_in_captured(seen: &[Vec<LlmMessage>]) {
    let all: Vec<LlmMessage> = seen.iter().flatten().cloned().collect();
    assert!(
        !all.iter().any(|m| m.content.contains(AWS_KEY)),
        "明文键不得进续行 LLM 上下文"
    );
    let tool_msgs: Vec<&LlmMessage> = all.iter().filter(|m| m.role == "tool").collect();
    assert!(
        tool_msgs
            .iter()
            .any(|m| m.content.contains("[REDACTED_CREDENTIAL]")),
        "tool result 应为脱敏版，实际 tool 消息：{:?}",
        tool_msgs.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    assert!(
        tool_msgs
            .iter()
            .any(|m| m.content.contains("[输出已脱敏: 检测到疑似凭据")),
        "脱敏尾注（诚实告知模型）应随 tool result 在场"
    );
}

// ---------------------------------------------------------------------------
// ② cluster 续行追加：inline 臂（permits=0）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cluster_continuation_inline_redacts_credentials() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(CapturingProvider::new(
            vec![l5_resp("resumed ok")],
            seen.clone(),
        )),
        l5_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_security_plugin(l5_security_plugin());
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "l5inline").await;
    agent_loop.set_continuation_manager(mgr);

    let task_response = format!("remote done; key={AWS_KEY} end");
    let msg = l5_msg(
        "system",
        &format!(
            "{}l5inline",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "chatL5",
        "",
        &task_response,
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("continuation final published");
    assert!(out.content.contains("resumed ok"), "got: {}", out.content);
    let captured = seen.lock().unwrap().clone();
    assert!(!captured.is_empty(), "续行 LLM 调用必须发生");
    assert_redacted_in_captured(&captured);
}

// ---------------------------------------------------------------------------
// ② cluster 续行追加：spawned 臂（permits=1，tokio::spawn + semaphore）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cluster_continuation_spawned_redacts_credentials() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(CapturingProvider::new(
            vec![l5_resp("resumed ok")],
            seen.clone(),
        )),
        l5_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        1,
    );
    agent_loop.set_security_plugin(l5_security_plugin());
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "l5spawn").await;
    agent_loop.set_continuation_manager(mgr);

    let task_response = format!("remote done; key={AWS_KEY} end");
    let msg = l5_msg(
        "system",
        &format!(
            "{}l5spawn",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "chatL5",
        "",
        &task_response,
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    // spawned 任务可能晚于 pump 结束 → 轮询等待（s9 spawned 同款）。
    let mut got = None;
    for _ in 0..40 {
        if let Ok(o) = out_rx.try_recv() {
            got = Some(o);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let out = got.expect("spawned continuation final published");
    assert!(out.content.contains("resumed ok"), "got: {}", out.content);
    // spawn 闭包里的捕获也要等 spawn 臂跑完。
    let mut captured = Vec::new();
    for _ in 0..40 {
        captured = seen.lock().unwrap().clone();
        if !captured.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!captured.is_empty(), "续行 LLM 调用必须发生");
    assert_redacted_in_captured(&captured);
}

// ---------------------------------------------------------------------------
// ③ 子代理后台返回：`subagent_continuation:` 前缀（同一 dispatch_continuation）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subagent_continuation_redacts_credentials() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(CapturingProvider::new(
            vec![l5_resp("bg resumed")],
            seen.clone(),
        )),
        l5_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_security_plugin(l5_security_plugin());
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "l5bg").await;
    agent_loop.set_continuation_manager(mgr);

    // agent_factory 后台子代理完成回灌的 bus 消息形状（g4 ⑤ 同款）。
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("status".to_string(), "ok".to_string());
    metadata.insert("source".to_string(), "background_subagent".to_string());
    let task_response = format!("sub-agent done; leaked {AWS_KEY}");
    let msg = l5_msg(
        "system",
        &format!(
            "{}l5bg",
            nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX
        ),
        "chatL5",
        "",
        &task_response,
        metadata,
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx
        .recv()
        .await
        .expect("bg continuation final published");
    assert!(out.content.contains("bg resumed"), "got: {}", out.content);
    let captured = seen.lock().unwrap().clone();
    assert!(!captured.is_empty(), "续行 LLM 调用必须发生");
    assert_redacted_in_captured(&captured);
}

// ---------------------------------------------------------------------------
// 无安全插件：接线不过度脱敏（明文直通，行为与历史一致）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_security_plugin_task_response_passes_through() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(CapturingProvider::new(
            vec![l5_resp("plain ok")],
            seen.clone(),
        )),
        l5_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    // 不 set_security_plugin —— 生产 security feature 在但未装配的形态。
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "l5plain").await;
    agent_loop.set_continuation_manager(mgr);

    let task_response = format!("remote done; key={AWS_KEY} end");
    let msg = l5_msg(
        "system",
        &format!(
            "{}l5plain",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "chatL5",
        "",
        &task_response,
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("continuation final published");
    assert!(out.content.contains("plain ok"), "got: {}", out.content);
    let captured = seen.lock().unwrap().clone();
    assert!(
        captured
            .iter()
            .flatten()
            .any(|m| m.role == "tool" && m.content.contains(AWS_KEY)),
        "无安全插件时 task_response 必须原样进 tool result（不得过度脱敏）"
    );
}
