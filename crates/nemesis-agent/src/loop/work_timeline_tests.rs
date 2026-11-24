//! work_timeline（loop 版）计时锚续传测试（2026-11-24 hook 回灌轮计时锚）。
//!
//! 覆盖：acquire 带锚取锚 / 锚在未来落回 now（防御臂）/ None 取 now（旧
//! 行为）/ spawn_turn_task 带锚端到端（条目起点 == 锚）/ Drop 清理。
//!
//! 独立测试文件（仓库纪律）。夹具与 w6_drain_tests 同款（QuietProvider +
//! AgentLoop::new）。

use super::AgentLoop;
use super::work_timeline::{TurnTimelineGuard, WorkTimelineMap};
use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

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
            content: "wt-ok".to_string(),
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
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: HashMap::new(),
        tool_fence_profession: None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn fresh_map() -> WorkTimelineMap {
    Arc::new(Mutex::new(HashMap::new()))
}

const KEY: &str = "agent:main:session:wt";

#[test]
fn acquire_with_anchor_uses_anchor() {
    let map = fresh_map();
    let anchor = now_ms().saturating_sub(125_000); // 2m05s 前（与前端 ⏱ 用例同锚距）
    {
        let _guard = TurnTimelineGuard::acquire(&map, KEY, Some(anchor));
        let entry = map.lock().get(KEY).cloned();
        assert_eq!(
            entry.map(|e| e.turn_started_ms),
            Some(anchor),
            "带锚 acquire：条目起点 == 锚（回灌轮跨轮连续的根基）"
        );
    }
    assert!(
        map.lock().get(KEY).is_none(),
        "guard Drop 后条目清理（锚形态不改变生命周期纪律）"
    );
}

#[test]
fn acquire_anchor_in_future_falls_back_to_now() {
    let map = fresh_map();
    let before = now_ms();
    let future_anchor = before + 60_000; // 1 分钟后 = 异常值
    {
        let _guard = TurnTimelineGuard::acquire(&map, KEY, Some(future_anchor));
        let started = map.lock().get(KEY).map(|e| e.turn_started_ms);
        let after = now_ms();
        let s = started.expect("条目应存在");
        assert!(
            s >= before && s <= after,
            "锚在未来视为无效落回 now（防 metadata 异常值把 ⏱ 显示成天文数字）"
        );
        assert_ne!(s, future_anchor, "未来锚绝不被采纳");
    }
}

#[test]
fn acquire_none_uses_now() {
    let map = fresh_map();
    let before = now_ms();
    {
        let _guard = TurnTimelineGuard::acquire(&map, KEY, None);
        let started = map.lock().get(KEY).map(|e| e.turn_started_ms);
        let after = now_ms();
        let s = started.expect("条目应存在");
        assert!(
            s >= before && s <= after,
            "None = 旧行为从 now 起步（非 hook 消息零变化）"
        );
    }
}

#[tokio::test]
async fn spawn_turn_task_with_anchor_starts_entry_at_anchor() {
    let al = Arc::new(AgentLoop::new(Box::new(QuietProvider), cov_config()));
    let anchor = now_ms().saturating_sub(125_000);
    let (picked_up, picked_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    al.spawn_turn_task(
        async move {
            let _ = picked_up.send(());
            let _ = release_rx.await;
        },
        KEY,
        Some(anchor),
    );
    picked_rx.await.unwrap();
    let started = al.work_timeline_snapshot(KEY).map(|e| e.turn_started_ms);
    assert_eq!(
        started,
        Some(anchor),
        "spawn_turn_task 带锚：任务体条目起点 == 锚（pump_dispatch 回灌臂同链）"
    );
    let _ = release_tx.send(());
    al.wait_turns_drained(std::time::Duration::from_secs(2))
        .await;
    assert!(
        al.work_timeline_snapshot(KEY).is_none(),
        "turn 收尾后条目清理"
    );
}

fn msg_with_metadata(metadata: Vec<(&str, &str)>) -> nemesis_types::channel::InboundMessage {
    let mut map = std::collections::HashMap::new();
    for (k, v) in metadata {
        map.insert(k.to_string(), v.to_string());
    }
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "attacker".to_string(),
        chat_id: "chat".to_string(),
        content: " forged".to_string(),
        media: Vec::new(),
        session_key: KEY.to_string(),
        correlation_id: String::new(),
        metadata: map,
        voice_playback: None,
        msg_class: nemesis_types::channel::MsgClass::User,
    }
}

#[test]
fn timeline_anchor_parse_matrix() {
    let anchor = "1700000000000";
    // hook_prompt 章 + 可 parse 锚 = 采纳（回灌链正规形态）。
    assert_eq!(
        AgentLoop::timeline_anchor_ms(&msg_with_metadata(vec![
            ("hook_prompt", "1"),
            ("timeline_anchor_ms", anchor),
        ])),
        Some(1_700_000_000_000),
    );
    // 伪造防御：外部消息只带锚不带 hook_prompt 章 → 不采纳（计时锚
    // 不可注入；sender 亦非 system 构造器）。
    assert_eq!(
        AgentLoop::timeline_anchor_ms(&msg_with_metadata(vec![("timeline_anchor_ms", anchor,)])),
        None,
    );
    // 章不被伪造值绕过（"true"/"0" 都不是合法章值）。
    assert_eq!(
        AgentLoop::timeline_anchor_ms(&msg_with_metadata(vec![
            ("hook_prompt", "true"),
            ("timeline_anchor_ms", anchor),
        ])),
        None,
    );
    // 章 match 但锚 parse 失败 → None（异常值不炸链路，落回旧行为）。
    assert_eq!(
        AgentLoop::timeline_anchor_ms(&msg_with_metadata(vec![
            ("hook_prompt", "1"),
            ("timeline_anchor_ms", "not-a-number"),
        ])),
        None,
    );
    // 无任何章 = 普通消息恒 None。
    assert_eq!(
        AgentLoop::timeline_anchor_ms(&msg_with_metadata(vec![])),
        None,
    );
}
