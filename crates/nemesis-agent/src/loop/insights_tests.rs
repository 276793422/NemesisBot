//! 自学习铺路批次一（2026-11-24）：active 洞察注入段测试。
//!
//! 覆盖：闭包注入出现 / 无闭包与 None 返回 = 段缺席零字节 /
//! 注入位置在 merged snapshot 内（system-reminder 包裹）/ 闭包字节
//! 稳定 → 同 build 输出字节一致。台账渲染本身的确定性（排序/cap/
//! 词表）归 nemesis-forge insights 测试；本文件只测 loop 侧接线。

// Minimal no-op provider for build_messages construction (chat is never
// called by these tests).
struct NoopProvider;
#[async_trait::async_trait]
impl crate::r#loop::LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<crate::r#loop::LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        Err("noop".to_string())
    }
}

fn make_instance_with_user_turn() -> crate::instance::AgentInstance {
    let instance = crate::instance::AgentInstance::new(crate::types::AgentConfig {
        system_prompt: Some("SYS".to_string()),
        system_prompt_split: None,
        ..Default::default()
    });
    instance.set_history(vec![
        crate::types::ConversationTurn {
            role: "system".to_string(),
            content: "SYS".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
        crate::types::ConversationTurn {
            role: "user".to_string(),
            content: "hello there".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
    ]);
    instance
}

fn insights_message(m: &[crate::r#loop::LlmMessage]) -> Option<String> {
    m.iter()
        .find(|msg| msg.content.contains("# Active Insights"))
        .map(|msg| msg.content.clone())
}

#[test]
fn test_insights_section_injected_via_closure() {
    let agent_loop =
        crate::r#loop::AgentLoop::new(Box::new(NoopProvider), crate::types::AgentConfig::default());
    agent_loop.set_insights_section_source(std::sync::Arc::new(|| {
        Some(
            "# Active Insights\n- [process_constraint] 先尝试自行解决，确实需要授权再询问\n\n(测试注记)".to_string(),
        )
    }));
    let instance = make_instance_with_user_turn();
    let m1 = agent_loop.build_messages(&instance);
    let section = insights_message(&m1).expect("闭包注入后 merged snapshot 应含洞察段");
    assert!(
        section.contains("system-reminder"),
        "洞察段随 merged snapshot 走 system-reminder 包裹"
    );
    assert!(section.contains("先尝试自行解决"));
}

#[test]
fn test_insights_section_absent_without_source() {
    let agent_loop =
        crate::r#loop::AgentLoop::new(Box::new(NoopProvider), crate::types::AgentConfig::default());
    let instance = make_instance_with_user_turn();
    let m = agent_loop.build_messages(&instance);
    assert!(
        insights_message(&m).is_none(),
        "未注入闭包 = 段缺席零字节（headless/裁剪装配形态）"
    );
}

#[test]
fn test_insights_section_absent_when_closure_returns_none() {
    let agent_loop =
        crate::r#loop::AgentLoop::new(Box::new(NoopProvider), crate::types::AgentConfig::default());
    // 空台账/损坏 fail-closed → render_section None → 段缺席
    agent_loop.set_insights_section_source(std::sync::Arc::new(|| None));
    let instance = make_instance_with_user_turn();
    let m = agent_loop.build_messages(&instance);
    assert!(insights_message(&m).is_none(), "None 返回 = 段缺席零字节");
}

#[test]
fn test_insights_section_byte_stable_across_builds() {
    let agent_loop =
        crate::r#loop::AgentLoop::new(Box::new(NoopProvider), crate::types::AgentConfig::default());
    // 模拟台账文件不变：闭包每次返回同字节（真实形态 = 从盘重渲且盘面
    // 未变；渲染确定性归 forge 测试）。
    agent_loop.set_insights_section_source(std::sync::Arc::new(|| {
        Some("# Active Insights\n- [style] 回复保持简洁\n".to_string())
    }));
    let instance = make_instance_with_user_turn();
    let s1 = insights_message(&agent_loop.build_messages(&instance));
    let s2 = insights_message(&agent_loop.build_messages(&instance));
    assert_eq!(s1, s2, "两次 build 洞察段字节一致（provider prefix 保住）");
    assert!(s1.is_some());
}

#[test]
fn test_insights_section_update_picked_up_next_build() {
    let agent_loop =
        crate::r#loop::AgentLoop::new(Box::new(NoopProvider), crate::types::AgentConfig::default());
    // 闭包读共享状态（真实形态 = 从盘重读）：盘面变化下一 build 即生效。
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c2 = counter.clone();
    agent_loop.set_insights_section_source(std::sync::Arc::new(move || {
        if c2.load(std::sync::atomic::Ordering::Relaxed) {
            Some("# Active Insights\n- [x] 新准则\n".to_string())
        } else {
            None
        }
    }));
    let instance = make_instance_with_user_turn();
    assert!(insights_message(&agent_loop.build_messages(&instance)).is_none());
    counter.store(true, std::sync::atomic::Ordering::Relaxed);
    let section = insights_message(&agent_loop.build_messages(&instance))
        .expect("盘面变化后下一 build 注入新段");
    assert!(section.contains("新准则"));
}
