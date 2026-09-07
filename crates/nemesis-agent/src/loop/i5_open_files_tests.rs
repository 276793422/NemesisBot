// I5 (devtool-upgrade 阶段 7)：打开文件上下文测试。
//
// 覆盖面：
// ① 渲染——无状态时 `# Open Files` 段缺席；有状态时合并 digest 携带路径列表
//    （顺序 = 上报顺序），且同状态两次 build 字节一致（多迭代稳定语义）；
// ② 生命周期——clear 后段消失，且全部消息字节回到从未 set 过的基线
//    （出轮清空 = 无跨轮陈旧泄漏）；
// ③ 解析联测——metadata 键经 nemesis-types::channel::open_files_from_metadata
//    单点清洗后与 AgentLoop 渲染端看到的一致（web 写入端共用 sanitize 单点，
//    端到端形状由 websocket_handler 测试覆盖）。
//
// set/clear 的生产配对点在 process_admitted（进轮 set / 出轮 clear，词法
// 相邻）；本文件直接操纵 pending_open_files 验证渲染与生命周期语义
// （同 I3 直接操纵 drained 状态的先例——全 admitted-turn 编排不在此复刻）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享）。

use super::*;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 恒定回复的迷你 provider（同 i3_lazy_instructions_tests 形状；I5 全部测试
/// 不真正调 LLM，只需要构造 AgentLoop）。
struct I5MockProvider;

#[async_trait]
impl LlmProvider for I5MockProvider {
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
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn i5_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(I5MockProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("You are a test assistant.".to_string()),
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
        },
    )
}

fn i5_instance() -> AgentInstance {
    let inst = AgentInstance::new(AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    });
    inst.add_user_message("look at my open files");
    inst
}

fn find_merged(msgs: &[LlmMessage]) -> &LlmMessage {
    msgs.iter()
        .find(|m| m.content.contains("system-reminder"))
        .expect("merged context message must exist")
}

// ---------------------------------------------------------------------------
// ① 渲染：缺席 / 在场 + 字节稳定
// ---------------------------------------------------------------------------

#[test]
fn section_absent_when_no_open_files() {
    let agent_loop = i5_loop();
    let inst = i5_instance();

    let msgs = agent_loop.build_messages(&inst);
    for m in &msgs {
        assert!(
            !m.content.contains("# Open Files"),
            "no open-files section expected, got: {}",
            m.content
        );
    }
}

#[test]
fn section_present_in_report_order_and_byte_stable() {
    let agent_loop = i5_loop();
    let inst = i5_instance();

    *agent_loop.pending_open_files.write() =
        vec!["/ws/src/main.rs".to_string(), "/ws/README.md".to_string()];

    let msgs = agent_loop.build_messages(&inst);
    let merged = find_merged(&msgs);
    assert!(merged.content.contains("# Open Files (client-reported)"));
    let main_pos = merged
        .content
        .find("- /ws/src/main.rs")
        .expect("main.rs listed");
    let readme_pos = merged
        .content
        .find("- /ws/README.md")
        .expect("README listed");
    assert!(
        main_pos < readme_pos,
        "render order must follow report order"
    );
    assert!(merged.content.contains("读取文件仍受安全策略约束"));

    // 同状态两次 build：全消息字节一致（turn 内多迭代稳定语义）。
    let msgs2 = agent_loop.build_messages(&inst);
    assert_eq!(msgs.len(), msgs2.len());
    for (a, b) in msgs.iter().zip(msgs2.iter()) {
        assert_eq!(a.role, b.role);
        assert_eq!(a.content, b.content);
    }
}

// ---------------------------------------------------------------------------
// ② 生命周期：clear 后字节回基线
// ---------------------------------------------------------------------------

#[test]
fn clear_restores_baseline_bytes() {
    let agent_loop = i5_loop();
    let inst = i5_instance();

    // 基线：从未 set。
    let baseline = agent_loop.build_messages(&inst);

    // set → 段在场。
    *agent_loop.pending_open_files.write() = vec!["/ws/src/main.rs".to_string()];
    let with_files = agent_loop.build_messages(&inst);
    assert!(find_merged(&with_files).content.contains("# Open Files"));

    // clear（process_admitted 出轮语义）→ 段消失，全部消息回基线字节。
    agent_loop.pending_open_files.write().clear();
    let after_clear = agent_loop.build_messages(&inst);
    assert_eq!(baseline.len(), after_clear.len());
    for (a, b) in baseline.iter().zip(after_clear.iter()) {
        assert_eq!(a.role, b.role);
        assert_eq!(a.content, b.content);
    }
}

// ---------------------------------------------------------------------------
// ③ 解析联测：metadata 键 → 渲染端所见
// ---------------------------------------------------------------------------

#[test]
fn metadata_parse_feeds_render_state() {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "open_files".to_string(),
        r#"[" /ws/a.rs ", "/ws/a.rs", ""]"#.to_string(),
    );
    let parsed = nemesis_types::channel::open_files_from_metadata(&metadata);
    assert_eq!(parsed, vec!["/ws/a.rs"]);

    // 渲染端读同一份数据。
    let agent_loop = i5_loop();
    let inst = i5_instance();
    *agent_loop.pending_open_files.write() = parsed;
    let msgs = agent_loop.build_messages(&inst);
    assert!(find_merged(&msgs).content.contains("- /ws/a.rs"));
}
