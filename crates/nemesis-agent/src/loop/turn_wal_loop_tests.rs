// L8（2026-10-13 内部专项 T3）：turn WAL 循环级测试——dispatch 三态落盘
// （admitted/started/finished + ok 布尔）、开关关闭零落盘、总线级断头续轮
// （恢复消息进 LLM + 回复出站 + 非 agent 键诚实跳过）。WAL 文件级生命周期
// （TTL 清扫/轮转/坏行/幂等）归 turn_wal/tests.rs，本文件不重复。
//
// 自带迷你 provider/工具（与 s9/l5 同构，模块私有不可共享——本仓测试文件
// 独立惯例）。

use super::*;
use crate::test_support::capture_logs;
use async_trait::async_trait;
use std::io::{BufRead, BufReader};

use crate::turn_wal::{
    STATE_ADMITTED, STATE_FINISHED, STATE_INTERRUPTED, STATE_STARTED, TurnWal, WalCall,
};

// ---------------------------------------------------------------------------
// 迷你 fixture
// ---------------------------------------------------------------------------

/// 直调 handle_tool_call 的用例不触 LLM——触达即测试设计错误。
struct NoLlmProvider;

#[async_trait]
impl LlmProvider for NoLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        panic!("本组用例不应触达 LLM");
    }
}

/// 记录每次 chat 收到的 messages（续轮用例断言 LLM 实际看到的上下文）。
struct CapturingProvider {
    seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<LlmMessage>>>>,
    reply: &'static str,
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
        Ok(LlmResponse {
            content: self.reply.to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 固定结果工具（Ok/Err/错误形态文本三形态）。
#[derive(Debug)]
struct StubTool {
    result: Result<String, String>,
}

#[async_trait]
impl Tool for StubTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        self.result.clone()
    }
}

fn wal_cfg() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
    }
}

fn wal_ctx() -> RequestContext {
    RequestContext::new("web", "chat-wal", "waluser", "agent:main:session:s-wal")
}

fn read_wal(p: &std::path::Path) -> Vec<crate::turn_wal::WalRecord> {
    match std::fs::File::open(p) {
        Ok(f) => BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn bus_msg(
    channel: &str,
    sender_id: &str,
    chat_id: &str,
    session_key: &str,
    content: &str,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
    }
}

// ---------------------------------------------------------------------------
// ① dispatch 三态落盘
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_writes_three_states_for_successful_tool() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let mut al = AgentLoop::new(Box::new(NoLlmProvider), wal_cfg());
    al.set_turn_wal(std::sync::Arc::new(
        TurnWal::open(dir.path()).expect("open wal"),
    ));
    al.register_tool(
        "walstub".to_string(),
        Box::new(StubTool {
            result: Ok("stub done".to_string()),
        }),
    );

    let out = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "wal-1".into(),
                name: "walstub".into(),
                arguments: r#"{"x":1}"#.into(),
            },
            &wal_ctx(),
        )
        .await;
    assert_eq!(out, "stub done");

    let recs = read_wal(&dir.path().join("wal.jsonl"));
    assert_eq!(recs.len(), 3, "三态各一行: {recs:?}");
    assert_eq!(recs[0].state, STATE_ADMITTED);
    assert_eq!(recs[1].state, STATE_STARTED);
    assert_eq!(recs[2].state, STATE_FINISHED);
    assert_eq!(recs[2].ok, Some(true));
    assert!(recs.iter().all(|r| r.call_id == "wal-1"));
    assert!(
        recs.iter()
            .all(|r| r.session_key == "agent:main:session:s-wal")
    );
    assert!(recs.iter().all(|r| r.tool == "walstub"));
    assert!(recs.iter().all(|r| r.args_preview == r#"{"x":1}"#));
}

/// 错误两形态（Err 包装 + Ok 里的错误形态文本）finished 都必须 ok=false
/// ——ok 是「调用效果成功」的判定，不是「没 panic」。
#[tokio::test]
async fn dispatch_marks_failure_on_error_results() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let mut al = AgentLoop::new(Box::new(NoLlmProvider), wal_cfg());
    al.set_turn_wal(std::sync::Arc::new(
        TurnWal::open(dir.path()).expect("open wal"),
    ));
    al.register_tool(
        "walfail".to_string(),
        Box::new(StubTool {
            result: Err("boom-wal".to_string()),
        }),
    );
    al.register_tool(
        "walerrtext".to_string(),
        Box::new(StubTool {
            result: Ok("Error: old_text not found".to_string()),
        }),
    );

    let out1 = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "wal-f1".into(),
                name: "walfail".into(),
                arguments: "{}".into(),
            },
            &wal_ctx(),
        )
        .await;
    assert!(
        out1.contains("Tool error:") && out1.contains("boom-wal"),
        "got: {out1}"
    );
    let out2 = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "wal-f2".into(),
                name: "walerrtext".into(),
                arguments: "{}".into(),
            },
            &wal_ctx(),
        )
        .await;
    assert_eq!(out2, "Error: old_text not found");

    let recs = read_wal(&dir.path().join("wal.jsonl"));
    assert_eq!(recs.len(), 6, "{recs:?}");
    for (i, chunk) in recs.chunks(3).enumerate() {
        assert_eq!(chunk[2].state, STATE_FINISHED, "chunk {i}");
        assert_eq!(
            chunk[2].ok,
            Some(false),
            "错误形态调用 finished 必须 ok=false (chunk {i})"
        );
    }
}

// ---------------------------------------------------------------------------
// ② 开关关闭：不挂 WAL → 零落盘
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wal_not_attached_no_file_created() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let wal_dir = dir.path().join("logs").join("turn_wal");
    let mut al = AgentLoop::new(Box::new(NoLlmProvider), wal_cfg());
    assert!(al.turn_wal().is_none(), "未装配 = None");
    al.register_tool(
        "walstub".to_string(),
        Box::new(StubTool {
            result: Ok("ok".to_string()),
        }),
    );

    let out = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "wal-off".into(),
                name: "walstub".into(),
                arguments: "{}".into(),
            },
            &wal_ctx(),
        )
        .await;
    assert_eq!(out, "ok");
    assert!(!wal_dir.exists(), "未装配 WAL 时目录都不得创建");
}

// ---------------------------------------------------------------------------
// ③ 总线级断头续轮
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bus_resume_replays_only_agent_keyed_interrupted_sessions() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let wal = TurnWal::open(dir.path()).expect("open wal");

    // 断头调用 A：agent 键（web 会话键，route_message 原样采纳）→ 应续轮。
    let call_a = WalCall {
        session_key: "agent:main:session:s-resume".into(),
        channel: "web".into(),
        chat_id: "chat-resume".into(),
        user: "alice".into(),
        call_id: "call-resume-1".into(),
        tool: "exec".into(),
    };
    wal.log(&call_a, STATE_ADMITTED, r#"{"cmd":"npm run build"}"#, None);
    wal.log(&call_a, STATE_STARTED, r#"{"cmd":"npm run build"}"#, None);
    // 断头调用 B：非 agent 键 → interrupted 留痕但不自动续轮。
    let call_b = WalCall {
        session_key: "web:skip-me".into(),
        channel: "web".into(),
        chat_id: "chat-skip".into(),
        user: "bob".into(),
        call_id: "call-skip-1".into(),
        tool: "write_file".into(),
    };
    wal.log(&call_b, STATE_STARTED, r#"{"path":"x.txt"}"#, None);

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(CapturingProvider {
            seen: std::sync::Arc::clone(&seen),
            reply: "WAL_RESUME_ACK",
        }),
        wal_cfg(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_turn_wal(std::sync::Arc::new(wal));

    // 恢复发生在 run_bus_impl 消费任何新消息**之前**：in_tx 直接关，
    // 泵先跑恢复，再因通道关闭退出。
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    // 恢复消息触达 LLM：recv 先等出站（spawned 臂在当前线程运行时上靠
    // await 让位被轮询；出站已发布 = chat 已返回 = seen 已落）。
    let out = out_rx.recv().await.expect("resumed reply published");
    assert!(
        out.content.contains("WAL_RESUME_ACK"),
        "got: {}",
        out.content
    );
    let captured = seen.lock().unwrap().clone();
    assert_eq!(captured.len(), 1, "只有 agent 键会话产生续轮 LLM 调用");
    let user_msgs: Vec<&LlmMessage> = captured[0].iter().filter(|m| m.role == "user").collect();
    assert!(
        user_msgs.iter().any(|m| {
            m.content.contains("⟳ 系统续轮")
                && m.content.contains("tool=exec")
                && m.content.contains("npm run build")
        }),
        "续轮消息必须携带断头现场: {:?}",
        user_msgs.iter().map(|m| &m.content).collect::<Vec<_>>()
    );

    // 两处断头都已补 interrupted 留痕（drain 后文件只含 interrupted 行）。
    let recs = read_wal(&dir.path().join("wal.jsonl"));
    assert_eq!(recs.len(), 2, "interrupted 留痕: {recs:?}");
    assert!(recs.iter().all(|r| r.state == STATE_INTERRUPTED));
}

/// WAL 干净（零候选）时恢复是 no-op：正常消息照常处理，无续轮注入。
#[tokio::test]
async fn bus_clean_wal_produces_no_resume() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let wal = TurnWal::open(dir.path()).expect("open wal");

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(CapturingProvider {
            seen: std::sync::Arc::clone(&seen),
            reply: "NORMAL_ACK",
        }),
        wal_cfg(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_turn_wal(std::sync::Arc::new(wal));

    in_tx
        .send(bus_msg(
            "web",
            "alice",
            "chat-1",
            "agent:main:session:s-normal",
            "hello normal",
        ))
        .await
        .unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    // recv 先等出站（spawned 臂靠 await 让位被轮询），再断言 LLM 侧。
    let out = out_rx.recv().await.expect("reply published");
    assert!(out.content.contains("NORMAL_ACK"), "got: {}", out.content);
    let captured = seen.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    assert!(
        captured[0]
            .iter()
            .all(|m| !m.content.contains("⟳ 系统续轮")),
        "干净 WAL 不得注入续轮消息"
    );
    let recs = read_wal(&dir.path().join("wal.jsonl"));
    assert!(recs.is_empty(), "零候选恢复后 drain 文件应为空: {recs:?}");
}
