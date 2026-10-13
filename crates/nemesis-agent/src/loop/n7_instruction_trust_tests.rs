//! N7（2026-10-13 机会项）：工作区指令信任档测试。
//!
//! 三档语义（build_messages 消费面）：trusted = verbatim 注入（主循环
//! 行为，缺省）；annotate = 注入但前置不可信来源横幅；isolated = 指令链
//! 区块与惰性发现区块整段跳过（缓冲照常排空，内容不进上下文）。另钉
//! 运行中热更新语义（区块逐 build 重渲染，set 下一轮即生效、无残留）。
//!
//! 形状同既有约定：恒定回复的迷你 provider（不真正跑 loop，只构造
//! AgentLoop）、临时目录工作区。

use super::*;

/// 恒定回复的迷你 provider（同 messages_cov_tests 形状）。
struct N7MockProvider;

#[async_trait]
impl LlmProvider for N7MockProvider {
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

fn n7_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

/// 工作区夹具：root AGENTS.md（根链常驻段的内容源）+ src/AGENTS.md
/// （惰性发现段的内容源）。
fn n7_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "ROOT RULES: n7 chain marker.").unwrap();
    std::fs::write(
        dir.path().join("src/AGENTS.md"),
        "SRC RULES: n7 lazy marker.",
    )
    .unwrap();
    dir
}

fn n7_loop(ws_root: std::path::PathBuf) -> AgentLoop {
    let agent_loop = AgentLoop::new(Box::new(N7MockProvider), n7_config());
    agent_loop.set_workspace_root(ws_root);
    agent_loop
}

fn merged_of(msgs: &[LlmMessage]) -> &str {
    msgs.iter()
        .find(|m| m.content.contains("system-reminder"))
        .map(|m| m.content.as_str())
        .expect("merged context message must exist")
}

/// 缺省 = Trusted：根链 verbatim 注入，无横幅。
#[test]
fn default_trust_injects_chain_verbatim() {
    let ws = n7_workspace();
    let al = n7_loop(ws.path().to_path_buf());
    assert_eq!(
        al.instruction_trust(),
        crate::workspace_instructions::InstructionTrust::Trusted,
        "缺省档 = trusted（主循环行为）"
    );
    let inst = AgentInstance::new(n7_config());
    inst.add_user_message("hi");
    let msgs = al.build_messages(&inst);
    let merged = merged_of(&msgs);
    assert!(merged.contains("# Workspace Instructions"));
    assert!(merged.contains("ROOT RULES: n7 chain marker."));
    assert!(!merged.contains("Untrusted Workspace Instructions"));
}

/// annotate：横幅前置 + 区块照常注入。
#[test]
fn annotate_prepends_banner_and_keeps_section() {
    let ws = n7_workspace();
    let al = n7_loop(ws.path().to_path_buf());
    al.set_instruction_trust(crate::workspace_instructions::InstructionTrust::Annotate);
    let inst = AgentInstance::new(n7_config());
    inst.add_user_message("hi");
    let msgs = al.build_messages(&inst);
    let merged = merged_of(&msgs);
    assert!(merged.contains("Untrusted Workspace Instructions"));
    // 横幅在区块标题之前（第一眼看到来源属性）。
    let banner = merged.find("Untrusted Workspace Instructions").unwrap();
    let section = merged.find("# Workspace Instructions").unwrap();
    assert!(banner < section);
    assert!(merged.contains("ROOT RULES: n7 chain marker."));
}

/// isolated：根链区块整段缺席（时间/环境快照段照常——合并消息仍在）。
#[test]
fn isolated_skips_chain_section() {
    let ws = n7_workspace();
    let al = n7_loop(ws.path().to_path_buf());
    al.set_instruction_trust(crate::workspace_instructions::InstructionTrust::Isolated);
    let inst = AgentInstance::new(n7_config());
    inst.add_user_message("hi");
    let msgs = al.build_messages(&inst);
    let merged = merged_of(&msgs);
    assert!(!merged.contains("# Workspace Instructions"));
    assert!(!merged.contains("ROOT RULES"));
    // 合并消息本身仍在（快照段恒在）——isolated 只裁指令面。
    assert!(merged.contains("# Current Time"));
}

/// isolated：惰性发现段同样缺席，但缓冲照常排空（不无限积压）。
#[test]
fn isolated_gates_lazy_discovery_but_drains() {
    let ws = n7_workspace();
    let al = n7_loop(ws.path().to_path_buf());
    al.set_instruction_trust(crate::workspace_instructions::InstructionTrust::Isolated);
    let inst = AgentInstance::new(n7_config());
    inst.add_user_message("read src/a.rs");
    al.note_read_for_instructions(&inst, "src/a.rs");

    let msgs = al.build_messages(&inst);
    let merged = merged_of(&msgs);
    assert!(
        !merged.contains("(newly discovered)"),
        "isolated 下惰性段不渲染: {merged}"
    );
    assert!(!merged.contains("SRC RULES"));
    // 排空已发生（第二次 build 也无残留可渲染）。
    assert!(inst.drain_pending_instructions().is_empty());
}

/// annotate：惰性发现段携带同款横幅。
#[test]
fn annotate_banners_lazy_discovery_section() {
    let ws = n7_workspace();
    let al = n7_loop(ws.path().to_path_buf());
    al.set_instruction_trust(crate::workspace_instructions::InstructionTrust::Annotate);
    let inst = AgentInstance::new(n7_config());
    inst.add_user_message("read src/a.rs");
    al.note_read_for_instructions(&inst, "src/a.rs");

    let msgs = al.build_messages(&inst);
    let merged = merged_of(&msgs);
    assert!(merged.contains("# Workspace Instructions (newly discovered)"));
    assert!(merged.contains("SRC RULES: n7 lazy marker."));
    let banner = merged.find("Untrusted Workspace Instructions").unwrap();
    let lazy = merged.find("(newly discovered)").unwrap();
    assert!(banner < lazy, "横幅前置于惰性段");
}

/// 热更新：trusted 构建后切 isolated，下一轮即生效、无残留。
#[test]
fn hot_apply_takes_effect_next_build() {
    let ws = n7_workspace();
    let al = n7_loop(ws.path().to_path_buf());
    let inst = AgentInstance::new(n7_config());
    inst.add_user_message("hi");

    let msgs = al.build_messages(&inst);
    let merged1 = merged_of(&msgs);
    assert!(merged1.contains("ROOT RULES"));

    al.set_instruction_trust(crate::workspace_instructions::InstructionTrust::Isolated);
    let msgs = al.build_messages(&inst);
    let merged2 = merged_of(&msgs);
    assert!(!merged2.contains("ROOT RULES"), "切档下一轮即生效");

    // 切回 trusted 同样即时恢复（无状态残留——区块逐 build 重渲染）。
    al.set_instruction_trust(crate::workspace_instructions::InstructionTrust::Trusted);
    let msgs = al.build_messages(&inst);
    let merged3 = merged_of(&msgs);
    assert!(merged3.contains("ROOT RULES"));
}
