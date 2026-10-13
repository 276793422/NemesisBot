//! N5（2026-10-13 机会项）：交付物登记的 loop 侧测试。
//!
//! - **dispatch 瀑布入桶**：present_files 工具真实走 `handle_tool_call`，
//!   交付物按 session_key 分桶落入 `turn_deliverables`（工具登记、loop
//!   drain 双侧同一 Arc 的接缝验证）。
//! - **drain 语义**：`drain_turn_deliverables` 取走即清（第二次 drain 为
//!   空）+ 去重走 `chat_log::dedup_presented_files`（title/note 取最后
//!   登记）。
//! - **emit 事件**：`emit_deliverables_registered` 经 agent_event_tx 广播
//!   `DeliverablesRegistered`（web pump → SSE/WS 的上游），字段与访问器
//!   对齐（round_text/f1 先例）；空清单不发布；通道未装配 no-op。
//! - 落盘行形状归 chat_log n5_tests（同一投影，两端各钉一头）；
//!   `run_agent_loop_internal` 内 drain→落盘→emit 的调用次序由同一 fn
//!   内的语句序保证（d3 先例：入口行编译保证 + 单元件各测）。

use super::*;

// —— 空 provider：dispatch 测试不触发 LLM 调用 ——

struct N5NoopProvider;

#[async_trait]
impl LlmProvider for N5NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            finish_reason: None,
            escalated_to: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn n5_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec!["present_files".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn n5_tc(name: &str, args: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: "call_n5".to_string(),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

/// 临时工作区（每测试唯一；断言不依赖目录消失时序）。
fn n5_temp_ws(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "n5_loop_{}_{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp workspace");
    dir
}

/// 装配带 present_files 的 loop（临时工作区 + 共享桶），返回 (loop, ws)。
fn n5_loop_with_tool(tag: &str) -> (AgentLoop, std::path::PathBuf) {
    let ws = n5_temp_ws(tag);
    let mut al = AgentLoop::new(Box::new(N5NoopProvider), n5_config());
    al.register_tool(
        "present_files".to_string(),
        Box::new(crate::loop_tools::PresentFilesTool::new(
            ws.to_string_lossy().to_string(),
            al.turn_deliverables_bucket(),
        )),
    );
    (al, ws)
}

fn drain_events(
    rx: &mut tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
) -> Vec<nemesis_types::agent::AgentEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

/// dispatch 真实走瀑布：交付物入桶（session_key 分桶）；桶与 loop 共享
/// 同一 Arc（agent_factory 接缝的回归钉）。
#[tokio::test]
async fn n5_dispatch_collects_into_shared_bucket() {
    let (al, ws) = n5_loop_with_tool("dispatch");
    std::fs::write(ws.join("r.md"), "# r").expect("seed deliverable");

    let ctx = RequestContext::new("web", "chat1", "agent", "sess:n5:a");
    let out = al
        .handle_tool_call(
            &n5_tc(
                "present_files",
                r#"{"files": [{"path": "r.md", "note": "结果"}]}"#,
            ),
            &ctx,
        )
        .await;
    assert!(out.contains("1 deliverable"), "工具执行不回归: {out}");

    let buckets = al.turn_deliverables.lock();
    let v = buckets.get("sess:n5:a").expect("session 桶存在");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].path, "r.md");
    assert_eq!(v[0].note, "结果");
    let _ = std::fs::remove_dir_all(&ws);
}

/// drain 即清 + 去重：同 session 两次声明同 path 投影为一条（title/note
/// 取最后）；第二次 drain 为空；不同 session 桶互不串扰。
#[tokio::test]
async fn n5_drain_dedups_and_clears_per_session() {
    let (al, ws) = n5_loop_with_tool("drain");
    std::fs::write(ws.join("a.md"), "a").expect("seed");

    al.turn_deliverables
        .lock()
        .entry("sess:a".to_string())
        .or_default()
        .extend(vec![
            crate::chat_log::PresentedFile {
                path: "a.md".to_string(),
                title: "v1".to_string(),
                note: String::new(),
            },
            crate::chat_log::PresentedFile {
                path: "a.md".to_string(),
                title: "v2".to_string(),
                note: "final".to_string(),
            },
            crate::chat_log::PresentedFile {
                path: "b.md".to_string(),
                title: "B".to_string(),
                note: String::new(),
            },
        ]);

    let drained_a = al.drain_turn_deliverables("sess:a");
    assert_eq!(drained_a.len(), 2, "同 path 去重后两条: {drained_a:?}");
    assert_eq!(drained_a[0].path, "a.md");
    assert_eq!(drained_a[0].title, "v2", "title 取最后登记");
    assert_eq!(drained_a[0].note, "final");
    assert_eq!(drained_a[1].path, "b.md");

    assert!(
        al.drain_turn_deliverables("sess:a").is_empty(),
        "drain 即清：第二次为空"
    );
    assert!(
        al.drain_turn_deliverables("sess:never").is_empty(),
        "无登记 session drain 为空"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// emit：登记 → drain → emit 广播 DeliverablesRegistered，字段与访问器
/// 对齐（web pump 按 session_key()/chat_id() 路由）。
#[tokio::test]
async fn n5_emit_publishes_deliverables_registered_event() {
    let (al, ws) = n5_loop_with_tool("emit");
    std::fs::write(ws.join("out.md"), "x").expect("seed");

    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    al.set_agent_event_tx(Some(tx));

    let ctx = RequestContext::new("web", "chat9", "agent", "agent:main:session:e1");
    al.handle_tool_call(
        &n5_tc(
            "present_files",
            r#"{"files": [{"path": "out.md", "title": "T"}]}"#,
        ),
        &ctx,
    )
    .await;

    let drained = al.drain_turn_deliverables("agent:main:session:e1");
    assert_eq!(drained.len(), 1);
    al.emit_deliverables_registered("agent:main:session:e1", "web:conn-1", &drained);

    let events = drain_events(&mut rx);
    assert_eq!(events.len(), 1, "恰好一条交付物事件: {events:?}");
    match &events[0] {
        nemesis_types::agent::AgentEvent::DeliverablesRegistered {
            session_key,
            chat_id,
            files,
        } => {
            assert_eq!(session_key, "agent:main:session:e1");
            assert_eq!(chat_id, "web:conn-1");
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].path, "out.md");
            assert_eq!(files[0].title, "T");
        }
        other => panic!("必须是 DeliverablesRegistered，实际 {other:?}"),
    }
    // 访问器对齐（round_text/f1 先例）——web pump 按此路由。
    assert_eq!(events[0].kind(), "DeliverablesRegistered");
    assert_eq!(events[0].chat_id(), "web:conn-1");
    assert_eq!(events[0].session_key(), Some("agent:main:session:e1"));
    let _ = std::fs::remove_dir_all(&ws);
}

/// 空清单不发布（emit 内部短路）；通道未装配为 no-op 不炸。
#[tokio::test]
async fn n5_empty_list_and_unwired_channel_are_noops() {
    let (al, ws) = n5_loop_with_tool("noop");
    let (tx, mut rx) = tokio::sync::broadcast::channel(16);
    al.set_agent_event_tx(Some(tx));

    // 空清单：不发布。
    al.emit_deliverables_registered("sess:e", "web:c", &[]);
    // 正常事件照常可收（证明不是通道坏了）。
    al.emit_deliverables_registered(
        "sess:e",
        "web:c",
        &[crate::chat_log::PresentedFile {
            path: "p.md".to_string(),
            title: String::new(),
            note: String::new(),
        }],
    );
    let events = drain_events(&mut rx);
    assert_eq!(events.len(), 1, "只有非空清单发布: {events:?}");

    // 通道未装配：no-op 不炸（CLI / B 端 worker 形态）。
    let bare = AgentLoop::new(Box::new(N5NoopProvider), n5_config());
    bare.emit_deliverables_registered(
        "sess:e",
        "web:c",
        &[crate::chat_log::PresentedFile {
            path: "p.md".to_string(),
            title: String::new(),
            note: String::new(),
        }],
    );
    let _ = std::fs::remove_dir_all(&ws);
}
