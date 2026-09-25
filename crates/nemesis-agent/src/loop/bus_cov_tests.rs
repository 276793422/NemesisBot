// loop/bus.rs 覆盖率补充测试（统一泵 gate 分臂 / spawn_turn_task 维护与
// 派发路径 / finish_message sent_in_round 跳过 / 续行无管理器诚实存活 /
// set_max_concurrent_turns 日志臂）。
//
// 泵经 run_bus_arc 驱动：inbound 发完即 drop sender → recv() None → 泵
// 退出，测试确定性收口（不依赖 stop 旗标竞态）。续行 inline/spawned 双臂
// 需要真实续行管理器与 ready 快照——已在 loop_continuation 覆盖（cov_tests
// 的 handle_cluster_continuation 直驱），泵层只补无管理器存活面，记豁免。

use super::AgentLoop;
use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use async_trait::async_trait;
use nemesis_types::channel::InboundMessage;
use std::collections::HashMap;

struct QuietProvider;

#[async_trait]
impl LlmProvider for QuietProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "cov-ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn cov_config() -> crate::types::AgentConfig {
    crate::types::AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: HashMap::new(),
    }
}

fn inbound(content: &str, channel: &str, sender_id: &str, key: &str) -> InboundMessage {
    InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: "covchat".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: key.to_string(),
        correlation_id: String::new(),
        metadata: HashMap::new(),
        voice_playback: None,
    }
}

/// 泵驱动维护命令与用户派发：两臂都以 spawn_turn_task 执行并发布回执，
/// 尾巴释放会话。
#[tokio::test]
async fn pump_runs_maintenance_and_user_dispatch_as_turn_tasks() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    al.set_max_concurrent_turns(0); // 日志 "unlimited" 臂
    al.set_max_concurrent_turns(2); // 日志数字臂 + turn_permits Some
    let (otx, mut orx) = tokio::sync::mpsc::channel(16);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    let (itx, irx) = tokio::sync::mpsc::channel(8);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    // 两个不同会话：同会话第二条会被 busy 闸拒绝（gate 忙分支已由别的
    // 测试钉住），这里让两臂各自成 turn。
    itx.send(inbound(
        "/compact",
        "web",
        "covuser",
        "agent:main:session:covpump-c",
    ))
    .await
    .unwrap();
    itx.send(inbound(
        "/build 修一下登录 repo:node-x",
        "web",
        "covuser",
        "agent:main:session:covpump-d",
    ))
    .await
    .unwrap();
    drop(itx);
    pump.await.unwrap();

    let mut got = Vec::new();
    while let Ok(o) = orx.try_recv() {
        got.push(o.content);
    }
    assert!(
        got.iter().any(|c| c.contains("正在压缩会话")),
        "compact receipt expected: {got:?}"
    );
    assert!(
        got.iter()
            .any(|c| c.contains("已把编码任务派发给节点 node-x")),
        "dispatch receipt expected: {got:?}"
    );
    // 派发无集群管理器 → handle_tool_call 同步错误编码在结果串直接回复。
    assert!(
        got.iter()
            .any(|c| c.contains("Error") || c.contains("error")),
        "dispatch sync error reply expected: {got:?}"
    );
    // 会话已释放（维护/派发尾巴 release_session）。
    assert!(!al.is_session_busy("agent:main:session:covpump-c"));
    assert!(!al.is_session_busy("agent:main:session:covpump-d"));
}

/// 普通消息走 Admitted 臂：完整 turn（quiet provider）→ finish_message
/// 发布响应（截断日志 + 非 rpc 分支）。
///
/// 注意：这里不能调 CaptureSink::init——OnceLock 先到先得，会毒化整个
/// 测试二进制的 capture 状态，破坏 session::tests 的「唯一 init 调用者」
/// 契约；capture 开或关对断言无影响。
#[tokio::test]
async fn pump_admits_plain_message_and_publishes_response() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(8);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    let (itx, irx) = tokio::sync::mpsc::channel(4);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    let key = "agent:main:session:covadmit";
    itx.send(inbound("普通消息", "web", "covuser", key))
        .await
        .unwrap();
    drop(itx);
    pump.await.unwrap();

    let mut got = Vec::new();
    while let Ok(o) = orx.try_recv() {
        got.push(o.content);
    }
    assert!(
        got.iter().any(|c| c.contains("cov-ok")),
        "plain turn response expected: {got:?}"
    );
    assert!(!al.is_session_busy(key));
}

/// finish_message：check_sent_in_round 命中 → 跳过发布（不占通道），
/// 随后按会话清除、发布恢复。
#[tokio::test]
async fn finish_message_skips_publish_when_already_sent() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(4);
    al.outbound_tx = Some(otx);
    let key = "agent:main:session:covsent";
    al.mark_sent_in_round(key);
    assert!(al.has_sent_in_round(key));

    let msg = inbound("hello", "web", "covuser", key);
    al.finish_message(&msg, "should be skipped".to_string(), None, true)
        .await;
    assert!(
        orx.try_recv().is_err(),
        "sent_in_round must suppress publish"
    );

    // 标记已按会话清除：下一次发布照常。
    al.finish_message(&msg, "goes through".to_string(), None, true)
        .await;
    let got = orx.recv().await.unwrap();
    assert_eq!(got.content, "goes through");
}

/// 续行臂：无管理器 → 诚实 warn（不 panic、不发布）。
#[tokio::test]
async fn pump_continuation_without_manager_warns_and_survives() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(4);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    let (itx, irx) = tokio::sync::mpsc::channel(4);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    itx.send(inbound(
        "task result",
        "system",
        "cluster_continuation:covnone",
        "",
    ))
    .await
    .unwrap();
    drop(itx);
    pump.await.unwrap();
    // 无管理器：无续行可处理，也无发布。
    assert!(orx.try_recv().is_err());
}
