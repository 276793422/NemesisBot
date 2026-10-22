// 循环流程策略槽测试（WASM 样板工程批 1）。
//
// 覆盖面：
// ① 默认槽 = 零逻辑哨兵——三策略裁决恒 `Builtin`（行为 = 无策略槽，
//    与批 1 改造前的内置路径等价；行为零变化由既有全量套件兜底）；
// ② 槽替换语义——setter 整槽替换 + 传回哨兵即恢复内置（镜像
//    `set_extra_hidden_tools` 全量替换）；
// ③ ValidationPolicy 裁决映射——Valid/Fixed/Reject 到 `Outcome` 的宿主
//    映射（Reject 按 schema violation 类 class "B"）；
// ④ 标题回调槽 `Option` 语义——默认 None（内置小模型路径）、可装可清。
//
// 自带迷你 Mock（兄弟模块私有类型不共享，同 g1 形状）。

use super::*;

/// 恒定回复的迷你 provider。
struct PolicyMockProvider;

#[async_trait]
impl LlmProvider for PolicyMockProvider {
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

fn policy_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(PolicyMockProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("You are a test assistant.".to_string()),
            max_turns: 5,
            tools: Vec::new(),
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    )
}

fn judge_input() -> policy::FinalJudgeInput {
    policy::FinalJudgeInput {
        session_key: "test:1".to_string(),
        content: "final answer".to_string(),
        turns_used: 3,
        validation_failures: 0,
    }
}

fn trigger_input() -> policy::CompactTriggerInput {
    policy::CompactTriggerInput {
        session_key: "test:1".to_string(),
        tail_tokens: 100,
        provider_signal: 0,
        signal: 100,
        threshold: 1000,
        soft: 666,
        window: 4096,
        covers_up_to: 0,
        new_boundary: 5,
    }
}

// ① 默认槽 = 零逻辑哨兵
#[tokio::test]
async fn default_slots_are_builtin_sentinels() {
    let agent_loop = policy_loop();

    let slot = agent_loop.final_judge_policy.read().clone();
    assert!(matches!(
        slot.judge(&judge_input()).await,
        policy::FinalJudgeVerdict::Builtin
    ));

    let slot = agent_loop.compact_policy.read().clone();
    assert!(matches!(
        slot.should_compact(&trigger_input()).await,
        policy::CompactTriggerVerdict::Builtin
    ));
    let exec_input = policy::CompactExecInput {
        session_key: "test:1",
        prefix: &[],
        existing_summary: "",
        window: 4096,
        model: "test-model",
    };
    assert!(matches!(
        slot.generate_summary(&exec_input).await,
        policy::CompactExecVerdict::Builtin
    ));

    let slot = agent_loop.validation_policy.read().clone();
    assert!(matches!(
        slot.validate(&policy::ValidationInput {
            tool: "echo".to_string(),
            schema: None,
            args_json: "{}".to_string(),
        })
        .await,
        policy::ValidationVerdict::Builtin
    ));

    // ④ 标题槽默认 None = 内置小模型路径。
    assert!(agent_loop.title_callback.read().is_none());
}

// ② 槽替换语义：整槽替换 + 哨兵恢复
#[tokio::test]
async fn compact_slot_override_and_restore() {
    struct ForceCompact;
    #[async_trait]
    impl policy::CompactPolicy for ForceCompact {
        async fn should_compact(
            &self,
            _input: &policy::CompactTriggerInput,
        ) -> policy::CompactTriggerVerdict {
            policy::CompactTriggerVerdict::Compact
        }

        async fn generate_summary(
            &self,
            _input: &policy::CompactExecInput<'_>,
        ) -> policy::CompactExecVerdict {
            policy::CompactExecVerdict::Summary("custom summary".to_string())
        }
    }

    let agent_loop = policy_loop();
    agent_loop.set_compact_policy(std::sync::Arc::new(ForceCompact));
    let slot = agent_loop.compact_policy.read().clone();
    assert!(matches!(
        slot.should_compact(&trigger_input()).await,
        policy::CompactTriggerVerdict::Compact
    ));

    // 传回哨兵 = 恢复内置（桥故障回退路径）。
    agent_loop.set_compact_policy(std::sync::Arc::new(policy::BuiltinCompactPolicy));
    let slot = agent_loop.compact_policy.read().clone();
    assert!(matches!(
        slot.should_compact(&trigger_input()).await,
        policy::CompactTriggerVerdict::Builtin
    ));
}

// ③ ValidationPolicy 裁决映射（schema 缺席路径：Builtin → Valid）
#[tokio::test]
async fn validation_slot_verdicts_map_to_outcomes() {
    struct FixedVerdictPolicy(policy::ValidationVerdict);
    #[async_trait]
    impl policy::ValidationPolicy for FixedVerdictPolicy {
        async fn validate(&self, _input: &policy::ValidationInput) -> policy::ValidationVerdict {
            match &self.0 {
                policy::ValidationVerdict::Valid => policy::ValidationVerdict::Valid,
                policy::ValidationVerdict::Fixed(f) => policy::ValidationVerdict::Fixed(f.clone()),
                policy::ValidationVerdict::Reject { message } => {
                    policy::ValidationVerdict::Reject {
                        message: message.clone(),
                    }
                }
                policy::ValidationVerdict::Builtin => policy::ValidationVerdict::Builtin,
            }
        }
    }

    let tc = crate::types::ToolCallInfo {
        id: "c1".to_string(),
        name: "echo".to_string(),
        arguments: "{}".to_string(),
    };

    // Valid → 直通（schema 缺席也放行——组件显式放行优先于内置 None→Valid）。
    let agent_loop = policy_loop();
    agent_loop.set_validation_policy(std::sync::Arc::new(FixedVerdictPolicy(
        policy::ValidationVerdict::Valid,
    )));
    assert!(matches!(
        agent_loop.check_tool_args(&tc).await,
        crate::args_validator::Outcome::Valid
    ));

    // Fixed → 携带修复后的 args。
    agent_loop.set_validation_policy(std::sync::Arc::new(FixedVerdictPolicy(
        policy::ValidationVerdict::Fixed(r#"{"path":"a"}"#.to_string()),
    )));
    match agent_loop.check_tool_args(&tc).await {
        crate::args_validator::Outcome::Fixed(fixed) => assert_eq!(fixed, r#"{"path":"a"}"#),
        other => panic!("expected Fixed, got {other:?}"),
    }

    // Reject → Invalid{class:"B"}（schema violation 类，计 failure 预算）。
    agent_loop.set_validation_policy(std::sync::Arc::new(FixedVerdictPolicy(
        policy::ValidationVerdict::Reject {
            message: "policy says no".to_string(),
        },
    )));
    match agent_loop.check_tool_args(&tc).await {
        crate::args_validator::Outcome::Invalid { message, class } => {
            assert_eq!(message, "policy says no");
            assert_eq!(class, "B");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }

    // Builtin → schema 缺席 = Valid（内置原语义）。
    agent_loop.set_validation_policy(std::sync::Arc::new(FixedVerdictPolicy(
        policy::ValidationVerdict::Builtin,
    )));
    assert!(matches!(
        agent_loop.check_tool_args(&tc).await,
        crate::args_validator::Outcome::Valid
    ));
}

// ④ 标题回调槽：可装可清
#[tokio::test]
async fn title_callback_slot_install_and_clear() {
    struct FixedTitle;
    #[async_trait]
    impl policy::TitleCallback for FixedTitle {
        async fn generate_title(&self, _input: &policy::TitleInput) -> Option<String> {
            Some("固定标题".to_string())
        }
    }

    let agent_loop = policy_loop();
    assert!(agent_loop.title_callback.read().is_none());

    agent_loop.set_title_callback(Some(std::sync::Arc::new(FixedTitle)));
    assert!(agent_loop.title_callback.read().is_some());

    agent_loop.set_title_callback(None);
    assert!(agent_loop.title_callback.read().is_none());
}
