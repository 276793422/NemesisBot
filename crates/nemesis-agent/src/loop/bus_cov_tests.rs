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
            finish_reason: None,
            escalated_to: None,
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
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: HashMap::new(),
        tool_fence_profession: None,
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
    al.finish_message(
        &msg,
        "should be skipped".to_string(),
        None,
        true,
        false,
        None,
        None,
    )
    .await;
    assert!(
        orx.try_recv().is_err(),
        "sent_in_round must suppress publish"
    );

    // 标记已按会话清除：下一次发布照常。
    al.finish_message(
        &msg,
        "goes through".to_string(),
        None,
        true,
        false,
        None,
        None,
    )
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

/// 泵 slash 改写回归（真模型验证发现的接线缺失）：生产泵路径
/// 必须在 gate 前完成四段解析链改写——内置深度模板（/security-review）
/// 以模板展开形态进 LLM（含 $ARGUMENTS 注入），而不是原样透传斜杠文本。
#[tokio::test]
async fn pump_expands_builtin_template_before_gate() {
    use std::sync::{Arc, Mutex};

    struct CapturingProvider(Arc<Mutex<Vec<String>>>);

    #[async_trait]
    impl LlmProvider for CapturingProvider {
        async fn chat(
            &self,
            _model: &str,
            messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            self.0
                .lock()
                .unwrap()
                .extend(messages.iter().map(|m| m.content.clone()));
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
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

    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut al = AgentLoop::new(Box::new(CapturingProvider(captured.clone())), cov_config());
    let (otx, _orx) = tokio::sync::mpsc::channel(8);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    let (itx, irx) = tokio::sync::mpsc::channel(4);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    itx.send(inbound(
        "/security-review eval_test.txt",
        "web",
        "covuser",
        "agent:main:session:covslash",
    ))
    .await
    .unwrap();
    drop(itx);
    pump.await.unwrap();

    let got = captured.lock().unwrap();
    // LLM 收到的必须是展开后的模板正文（含安全评审纪律 + 参数注入），
    // 不再是原始斜杠文本。
    assert!(
        got.iter().any(|c| c.contains("安全评审专家")),
        "expanded template body expected in LLM messages: {:?}",
        got.iter()
            .map(|c| c.chars().take(60).collect::<String>())
            .collect::<Vec<_>>()
    );
    assert!(
        got.iter()
            .any(|c| c.contains("eval_test.txt") && !c.starts_with('/')),
        "$ARGUMENTS must be substituted with the args: {:?}",
        got.iter()
            .map(|c| c.chars().take(60).collect::<String>())
            .collect::<Vec<_>>()
    );
}

/// 红/绿证：回合体 panic 跳过尾巴 release_session
/// → 会话永久 busy（Reject 全拒 / Queue/Steer 排队到满，直到 agent 重启
/// ——clear_session_busy 只挂在停机路径）。工具执行已被 tool_dispatch 的
/// catch_unwind 单独包住，但回合体其余部分（provider 响应解析/预处理/
/// 落盘/封印）没有——provider chat panic 直穿 process_admitted。修复 =
/// 任务体 RAII 守卫，panic 展开时 Drop 兜底释放；断言 = panic 落定后会话
/// 必须已释放。
#[tokio::test]
async fn panic_mid_turn_releases_session_busy() {
    struct PanickingProvider;
    #[async_trait]
    impl LlmProvider for PanickingProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            panic!("provider exploded mid-turn");
        }
    }

    let mut al = AgentLoop::new(Box::new(PanickingProvider), cov_config());
    let (otx, _orx) = tokio::sync::mpsc::channel(8);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    let (itx, irx) = tokio::sync::mpsc::channel(4);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    let key = "agent:main:session:covpanic";
    itx.send(inbound("会炸的消息", "web", "covuser", key))
        .await
        .unwrap();
    drop(itx);
    pump.await.unwrap(); // 泵退出；spawn 的 turn 任务独立继续跑

    // 等 turn 任务展开完成（InFlightTurnGuard 的 Drop 在 panic 展开路径
    // 减账——；in_flight_turns 归零 = 任务体已落定）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while al.in_flight_turns() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "panicked turn task did not unwind in time"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        !al.is_session_busy(key),
        "panic between gate acquire and tail release must not brick the session (busy leak)"
    );
}
/// 行号锚漂移根修（2026-11-11）：gate 两臂的 meta.occupies 落盘真相透传
/// ——维护 ⏳/✓ 回执（瞬态不落盘）恒 Some(false)；Admitted 终局回复
/// None（旧规则 user/assistant 占行）。前端按帧键计行号锚，rewind/
/// regenerate 的 message_index 不再双向漂移。
#[tokio::test]
async fn pump_stamps_occupies_false_on_maintenance_frames() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(16);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    let (itx, irx) = tokio::sync::mpsc::channel(4);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    itx.send(inbound(
        "/compact",
        "web",
        "covuser",
        "agent:main:session:covocc-m",
    ))
    .await
    .unwrap();
    drop(itx);
    pump.await.unwrap();

    let mut got = Vec::new();
    while let Ok(o) = orx.try_recv() {
        got.push(o);
    }
    // ⏳ 前置回执：receipt=true + occupies=false（瞬态不落盘）。
    let pending = got
        .iter()
        .find(|o| o.meta.receipt && o.content.contains("正在压缩会话"))
        .expect("maintenance ⏳ receipt expected");
    assert_eq!(
        pending.meta.occupies,
        Some(false),
        "维护 ⏳ 回执瞬态不落盘 → occupies=false（修复前无键，前端按 assistant 误计锚）"
    );
    // ✓ 压缩结果帧：receipt=false（LLM 摘要）但同样瞬态 → Some(false)。
    let done = got
        .iter()
        .find(|o| !o.meta.receipt)
        .expect("maintenance ✓ frame expected");
    assert_eq!(
        done.meta.occupies,
        Some(false),
        "维护 ✓ 帧落盘只有 user 行 → occupies=false 不占锚"
    );
}

#[tokio::test]
async fn pump_final_reply_has_no_occupies_key() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(8);
    al.outbound_tx = Some(otx);
    let al = std::sync::Arc::new(al);

    let (itx, irx) = tokio::sync::mpsc::channel(4);
    let pump = tokio::spawn(al.clone().run_bus_arc(irx));
    itx.send(inbound(
        "普通任务",
        "web",
        "covuser",
        "agent:main:session:covocc-f",
    ))
    .await
    .unwrap();
    drop(itx);
    pump.await.unwrap();

    let mut got = Vec::new();
    while let Ok(o) = orx.try_recv() {
        got.push(o);
    }
    let finals: Vec<_> = got.iter().filter(|o| !o.meta.receipt).collect();
    assert_eq!(finals.len(), 1, "exactly one final reply expected: {got:?}");
    assert_eq!(
        finals[0].meta.occupies, None,
        "终局回复已由 turn 内部落盘 → 无键（旧规则 user/assistant 占行）"
    );
}

// --- 降级注记槽（fallback 用户面标注）---

/// 降级注记 provider 桩：chat 恒成功；take_response_degradation 恒返回
/// 配置注记（桩不清——取件清的职责在 adapter 实现，桩只测 loop 侧
/// last-call 覆盖语义）。
struct DegradingNoteProvider {
    note: Option<String>,
}

#[async_trait]
impl LlmProvider for DegradingNoteProvider {
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
            content: "note-ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }

    fn take_response_degradation(&self) -> Option<String> {
        self.note.clone()
    }
}

/// 主路径成功调用后统一取件按会话键入表；干净调用（None）覆盖清本键——
/// 注记按会话键隔离（BUG-20261120-013），跨会话并行 turn 各写各键。
#[tokio::test]
async fn degradation_note_taken_on_success_and_cleared_by_clean_call() {
    let key = "agent:main:session:covdegr-set";
    let al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let degrading: std::sync::Arc<dyn LlmProvider> = std::sync::Arc::new(DegradingNoteProvider {
        note: Some("主模型x → http-compat/备选y".to_string()),
    });

    al.chat_call_escalating(0, &degrading, "m", Vec::new(), None, Vec::new(), key)
        .await
        .unwrap();
    assert_eq!(
        al.response_degradation.lock().get(key).map(|s| s.as_str()),
        Some("主模型x → http-compat/备选y"),
        "成功调用后必须取件入本会话键"
    );

    // 干净调用 → last-call 覆盖清本键。
    let clean: std::sync::Arc<dyn LlmProvider> = std::sync::Arc::new(QuietProvider);
    al.chat_call_escalating(0, &clean, "m", Vec::new(), None, Vec::new(), key)
        .await
        .unwrap();
    assert!(
        !al.response_degradation.lock().contains_key(key),
        "干净调用清本会话键"
    );
}

/// finish_message 终局回复：附加展示注记 + 取件即清（下一帧无注记）。
#[tokio::test]
async fn finish_message_appends_degradation_note_and_consumes_slot() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(4);
    al.outbound_tx = Some(otx);
    let msg = inbound("hello", "web", "covuser", "agent:main:session:covdegrade");
    al.response_degradation.lock().insert(
        msg.session_key.clone(),
        "glm-4.7 → http-compat/glm-5.3-flash".to_string(),
    );

    al.finish_message(&msg, "最终回复".to_string(), None, false, false, None, None)
        .await;
    let got = orx.recv().await.unwrap();
    assert!(
        got.content.contains("最终回复"),
        "原文保留: {}",
        got.content
    );
    assert!(
        got.content.contains("备选应答") && got.content.contains("glm-5.3-flash"),
        "注记必须随回复: {}",
        got.content
    );
    assert!(
        !al.response_degradation
            .lock()
            .contains_key(&msg.session_key),
        "取件即清"
    );

    // 表已清：下一帧不带注记。
    al.finish_message(&msg, "第二条".to_string(), None, false, false, None, None)
        .await;
    let got2 = orx.recv().await.unwrap();
    assert_eq!(got2.content, "第二条");
}

/// 回执帧（receipt=true）与错误文案（err=Some）既不背注记也不消费槽；
/// 随后终局回复正常消费。
#[tokio::test]
async fn finish_message_receipt_and_error_do_not_consume_note() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(4);
    al.outbound_tx = Some(otx);
    let msg = inbound("hello", "web", "covuser", "agent:main:session:covdegrade2");
    al.response_degradation
        .lock()
        .insert(msg.session_key.clone(), "a → b".to_string());

    // 回执帧：不背注记、不消费槽。
    al.finish_message(
        &msg,
        "⏳ 排队中".to_string(),
        None,
        false,
        true,
        Some(false),
        None,
    )
    .await;
    let got = orx.recv().await.unwrap();
    assert_eq!(got.content, "⏳ 排队中");
    assert!(
        al.response_degradation
            .lock()
            .contains_key(&msg.session_key),
        "回执不消费槽"
    );

    // 错误文案：不背注记、不消费槽。
    al.finish_message(
        &msg,
        String::new(),
        Some("boom".to_string()),
        true,
        false,
        None,
        None,
    )
    .await;
    let got = orx.recv().await.unwrap();
    assert!(
        got.content.contains("Error processing message"),
        "错误文案形态: {}",
        got.content
    );
    assert!(
        al.response_degradation
            .lock()
            .contains_key(&msg.session_key),
        "错误不消费槽"
    );

    // 随后终局回复正常消费。
    al.finish_message(&msg, "终局".to_string(), None, true, false, None, None)
        .await;
    let got = orx.recv().await.unwrap();
    assert!(
        got.content.contains("备选应答"),
        "终局注记: {}",
        got.content
    );
    assert!(
        !al.response_degradation
            .lock()
            .contains_key(&msg.session_key)
    );
}

/// BUG-20261120-013 红证：降级注记槽 loop 级单槽跨会话串台——A 会话的
/// 降级事实在槽（A 末次 LLM 调用降级后、尚未走到 finish_message 的窗口
/// 态），B 会话（从未降级）的终局先到把注记偷走（B 的回复谎称由备选
/// 应答），A 自己的终局反而裸奔。V5 统一泵跨会话并发是结构不变量
/// （spawn_turn_task 无条件派生，并发模式只管同会话忙处置），两会话
/// turn 重叠在默认配置即成立；finish_message 消费窗口（finalize 落盘
/// IO）内另一会话终局到达即可触发。注记必须按会话键隔离。
#[tokio::test]
async fn finish_message_degradation_note_is_session_keyed_not_global() {
    let mut al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    let (otx, mut orx) = tokio::sync::mpsc::channel(4);
    al.outbound_tx = Some(otx);
    // A 会话的降级事实在表（等价于 A 的末次 LLM 调用降级后、A 尚未走到
    // finish_message 的窗口态）。
    al.response_degradation.lock().insert(
        "agent:main:session:covsteal-a".to_string(),
        "a → b".to_string(),
    );

    // B 会话终局先到：从未降级，不得背注记。
    let msg_b = inbound("hello", "web", "covuser", "agent:main:session:covsteal-b");
    al.finish_message(
        &msg_b,
        "B的回复".to_string(),
        None,
        false,
        false,
        None,
        None,
    )
    .await;
    let got_b = orx.recv().await.unwrap();
    assert!(
        !got_b.content.contains("备选应答"),
        "B 从未降级，注记不得串台到 B: {}",
        got_b.content
    );

    // A 会话终局后到：注记必须落在真实降级方 A，披露不得丢失。
    let msg_a = inbound("hello", "web", "covuser", "agent:main:session:covsteal-a");
    al.finish_message(
        &msg_a,
        "A的回复".to_string(),
        None,
        false,
        false,
        None,
        None,
    )
    .await;
    let got_a = orx.recv().await.unwrap();
    assert!(
        got_a.content.contains("备选应答") && got_a.content.contains("a → b"),
        "A 的降级披露不得丢失: {}",
        got_a.content
    );
}
