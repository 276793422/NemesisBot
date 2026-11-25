// (devtool-upgrade)：打开文件上下文测试。
//
// 覆盖面：
// ① 渲染——无状态时 `# Open Files` 段缺席；有状态时合并 digest 携带路径列表
//    （顺序 = 上报顺序），且同状态两次 build 字节一致（多迭代稳定语义）；
// ② 生命周期——clear 后段消失，且全部消息字节回到从未 set 过的基线
//    （出轮清空 = 无跨轮陈旧泄漏）；
// ③ 解析联测——metadata 键经 nemesis-types::channel:open_files_from_metadata
//    单点清洗后与 AgentLoop 渲染端看到的一致（web 写入端共用 sanitize 单点，
//    端到端形状由 websocket_handler 测试覆盖）。
//
// set/clear 的生产配对点在 process_admitted（进轮 set / 出轮 clear，词法
// 相邻）；本文件直接操纵 pending_open_files 验证渲染与生命周期语义
// （同 直接操纵 drained 状态的先例——全 admitted-turn 编排不在此复刻）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享）。

use super::*;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 恒定回复的迷你 provider（同 i3_lazy_instructions_tests 形状； 全部测试
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
            finish_reason: None,
            escalated_to: None,
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
            system_prompt_split: None,
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),

            tool_fence_profession: None,
        },
    )
}

fn i5_instance() -> AgentInstance {
    let inst = AgentInstance::new(AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),

        tool_fence_profession: None,
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

    *agent_loop
        .pending_open_files
        .lock()
        .entry(String::new())
        .or_default() = vec!["/ws/src/main.rs".to_string(), "/ws/README.md".to_string()];

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

    // set（"" 键 = build_messages None 键路径读取的桶）→ 段在场。
    agent_loop
        .pending_open_files
        .lock()
        .insert(String::new(), vec!["/ws/src/main.rs".to_string()]);
    let with_files = agent_loop.build_messages(&inst);
    assert!(find_merged(&with_files).content.contains("# Open Files"));

    // clear（process_admitted 出轮语义：remove 本会话键）→ 段消失，全部消息回基线字节。
    agent_loop.pending_open_files.lock().remove("");
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
    agent_loop
        .pending_open_files
        .lock()
        .insert(String::new(), parsed);
    let msgs = agent_loop.build_messages(&inst);
    assert!(find_merged(&msgs).content.contains("- /ws/a.rs"));
}

// ---------------------------------------------------------------------------
// ③ 跨会话并发分桶（BUG-20261125-068 回归锁）：A/B 会话各写各键——
// A set 后 B set 不得覆写 A 的条目；A 出轮 remove 后 B 的条目仍在。
// 旧 loop 级单槽形态下：B set 覆写 A → A 的请求注入 B 的文件路径
// （跨会话串漏）；A clear 整槽 → B 的上下文丢失。
// ---------------------------------------------------------------------------

#[test]
fn per_session_buckets_isolate_concurrent_turns() {
    let agent_loop = i5_loop();
    let inst = i5_instance();

    // 模拟 process_admitted 顺序：A set → B set（并发窗口，B 后到）。
    agent_loop
        .pending_open_files
        .lock()
        .insert("sess-a".to_string(), vec!["/ws/a_only.rs".to_string()]);
    agent_loop
        .pending_open_files
        .lock()
        .insert("sess-b".to_string(), vec!["/ws/b_only.rs".to_string()]);

    // A 的构建只注入 A 自己的路径（旧单槽形态此处读到 B 的 b_only）。
    let (msgs_a, _) =
        agent_loop.build_messages_with_memory_annotated_for(&inst, None, Some("sess-a"), None);
    let merged_a = find_merged(&msgs_a);
    assert!(
        merged_a.content.contains("- /ws/a_only.rs"),
        "A must see its own entry, got: {}",
        merged_a.content
    );
    assert!(
        !merged_a.content.contains("/ws/b_only.rs"),
        "A must NOT see B's entry (cross-session bleed), got: {}",
        merged_a.content
    );

    // B 的构建只注入 B 自己的路径。
    let (msgs_b, _) =
        agent_loop.build_messages_with_memory_annotated_for(&inst, None, Some("sess-b"), None);
    let merged_b = find_merged(&msgs_b);
    assert!(merged_b.content.contains("- /ws/b_only.rs"));
    assert!(!merged_b.content.contains("/ws/a_only.rs"));

    // A 出轮 remove（process_admitted 收尾语义：只清本键）→ B 条目仍在。
    agent_loop.pending_open_files.lock().remove("sess-a");
    let (msgs_b2, _) =
        agent_loop.build_messages_with_memory_annotated_for(&inst, None, Some("sess-b"), None);
    assert!(
        find_merged(&msgs_b2).content.contains("- /ws/b_only.rs"),
        "A's turn-end clear must not wipe B's entry"
    );

    // workflow_edit 同款分桶：A set、B 不带（remove 本键）→ A 条目不受影响。
    agent_loop.pending_workflow_edit.lock().insert(
        "sess-a".to_string(),
        nemesis_types::channel::WorkflowEditTarget {
            workflow_name: Some("wf-a".to_string()),
        },
    );
    // B 轮不带 workflow_edit（process_admitted None 臂 = remove 本键）。
    agent_loop.pending_workflow_edit.lock().remove("sess-b");
    let (msgs_a2, _) =
        agent_loop.build_messages_with_memory_annotated_for(&inst, None, Some("sess-a"), None);
    assert!(
        find_merged(&msgs_a2).content.contains("wf-a"),
        "B's absent-workflow_edit turn must not clear A's target"
    );
}
