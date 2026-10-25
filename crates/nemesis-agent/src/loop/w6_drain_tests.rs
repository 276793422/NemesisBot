//! W6（2026-10-25 加固）：优雅停机
//! drain 测试——在途 turn gauge（spawn/inline 两臂 + guard 防漂移）、
//! drain 闸（外部通道诚实回执 / 内部通道静默丢 / 零处理早退）、有界等待
//! （归零 true / 超时 false）。
//!
//! 独立测试文件（仓库纪律）。夹具与 bus_cov_tests 同款（QuietProvider +
//! run_bus_arc 泵驱动）。

use super::AgentLoop;
use super::bus::DRAINING_RECEIPT;
use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use async_trait::async_trait;
use nemesis_types::channel::InboundMessage;
use std::collections::HashMap;
use std::time::Duration;

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
            finish_reason: None,
            escalated_to: None,
            content: "w6-ok".to_string(),
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
        tool_fence_profession: None,
    }
}

fn inbound(content: &str, channel: &str, key: &str) -> InboundMessage {
    InboundMessage {
        channel: channel.to_string(),
        sender_id: "w6user".to_string(),
        chat_id: "w6chat".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: key.to_string(),
        correlation_id: String::new(),
        metadata: HashMap::new(),
        voice_playback: None,
    }
}

#[tokio::test]
async fn in_flight_gauge_tracks_spawned_turn_and_guard_releases() {
    let al = std::sync::Arc::new(AgentLoop::new(Box::new(QuietProvider), cov_config()));
    assert_eq!(al.in_flight_turns(), 0, "空闲 gauge 为零");

    // 在途任务：持 gauge 等外部放行。
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let (picked_up, picked_rx) = tokio::sync::oneshot::channel::<()>();
    // spawn_turn_task 的任务体自带 guard（W6 接线本体）——测试不再手动
    // acquire（双 guard 会重复计数）。
    al.spawn_turn_task(async move {
        let _ = picked_up.send(());
        let _ = rx.await;
    });
    picked_rx.await.unwrap();
    assert_eq!(al.in_flight_turns(), 1, "任务体开始即计 1");

    // 有界等待：任务未完成 → 超时 false（150ms 预算内轮询不误报）。
    assert!(
        !al.wait_turns_drained(Duration::from_millis(150)).await,
        "在途任务未完成时不得报告已归零"
    );

    // 放行 → guard Drop 减账 → 归零 → 等待 true。
    let _ = tx.send(());
    assert!(
        al.wait_turns_drained(Duration::from_secs(2)).await,
        "任务完成后 drain 等待应返回 true"
    );
    assert_eq!(al.in_flight_turns(), 0, "guard Drop 已减账");
}

#[tokio::test]
async fn wait_turns_drained_true_immediately_when_idle() {
    let al = std::sync::Arc::new(AgentLoop::new(Box::new(QuietProvider), cov_config()));
    let t0 = std::time::Instant::now();
    assert!(al.wait_turns_drained(Duration::from_secs(5)).await);
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "空闲时立即返回，不吃满预算: {:?}",
        t0.elapsed()
    );
}

#[tokio::test]
async fn drain_gate_rejects_new_inbound_with_honest_receipt() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(8);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    // 先 drain 再起泵——闸在 rewrite/gate 之前，零处理早退。
    assert!(!al.is_draining());
    al.begin_shutdown_drain();
    assert!(al.is_draining(), "begin_shutdown_drain 置位");
    al.begin_shutdown_drain(); // 幂等臂
    assert!(al.is_draining());

    let (itx, irx) = tokio::sync::mpsc::channel(8);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    // slash 消息：正常路径会回「正在压缩会话」——drain 态必须是停机回执，
    // 证明闸在 rewrite/gate 之前短路（消息没进任何处理臂）。
    itx.send(inbound("/compact", "web", "agent:main:session:w6drain"))
        .await
        .unwrap();
    // 内部通道：静默丢（仅日志留痕），零出站。
    itx.send(inbound("内部消息", "system", "agent:main:session:w6sys"))
        .await
        .unwrap();
    drop(itx);
    pump.await.unwrap();

    let mut got = Vec::new();
    while let Ok(o) = orx.try_recv() {
        got.push(o.content);
    }
    assert_eq!(got.len(), 1, "恰好一条外部回执（内部通道静默）: {got:?}");
    assert!(
        got[0].contains(DRAINING_RECEIPT) || got[0].contains("正在停机"),
        "外部通道收到停机回执: {got:?}"
    );
    assert!(
        !got[0].contains("正在压缩会话"),
        "闸必须先于 rewrite/gate: {got:?}"
    );
    assert_eq!(al.in_flight_turns(), 0, "drain 态不产生新在途 turn");
}

#[tokio::test]
async fn spawned_turn_gauge_visible_via_wait_and_cleanup_after_finish() {
    // 泵路径的 gauge 接线（非直驱 guard）：Admitted 臂 spawn 的 turn 任务
    // 全程持 gauge——quiet provider 秒完成，泵收口后 gauge 必回零
    // （回归锁：spawn 臂忘挂 guard = 泵路径在途计数恒 0，drain 形同虚设）。
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(8);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    let (itx, irx) = tokio::sync::mpsc::channel(8);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    itx.send(inbound("hello", "web", "agent:main:session:w6gauge"))
        .await
        .unwrap();
    drop(itx);
    pump.await.unwrap();
    while orx.try_recv().is_ok() {}

    assert!(
        al.wait_turns_drained(Duration::from_secs(2)).await,
        "泵收口后在途归零"
    );
    assert_eq!(al.in_flight_turns(), 0);
}

// ---------------------------------------------------------------------------
// BUG-20261025-33：联合 drain 面必须覆盖全部常驻 loop——gateway 此前只等
// 主 loop，项目/G1 loop 在途时假报「已归零」（停机假完成日志的根因形态）。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wait_loops_drained_requires_all_loops_idle() {
    let a = std::sync::Arc::new(AgentLoop::new(Box::new(QuietProvider), cov_config()));
    let b = std::sync::Arc::new(AgentLoop::new(Box::new(QuietProvider), cov_config()));

    // 空闲两 loop：立即 true。
    assert!(
        AgentLoop::wait_loops_drained(&[a.clone(), b.clone()], Duration::from_millis(50)).await
    );

    // b 在途：联合等待必须超时 false（只看 a 会假绿——本缺陷的回归锁）。
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let (picked_up, picked_rx) = tokio::sync::oneshot::channel::<()>();
    b.spawn_turn_task(async move {
        let _ = picked_up.send(());
        let _ = release_rx.await;
    });
    picked_rx.await.unwrap();
    assert_eq!(b.in_flight_turns(), 1);
    assert!(
        !AgentLoop::wait_loops_drained(&[a.clone(), b.clone()], Duration::from_millis(150)).await,
        "任一 loop 在途 = 未排干"
    );

    // 放行 → guard Drop 减账 → 联合归零 true。
    let _ = release_tx.send(());
    assert!(AgentLoop::wait_loops_drained(&[a.clone(), b.clone()], Duration::from_secs(2)).await);
}
