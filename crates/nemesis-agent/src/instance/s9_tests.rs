//! S9 覆盖率批次：instance.rs 剩余未覆盖行。
//! - 333：compress_history 的 debug! 参数行（需 subscriber + 历史 > 2 条）。
//! - 392/402：set_history 保留旧 system prompt（旧史有 system、新史无 → 回插 0 位）。

use super::*;
use crate::test_support::capture_logs;

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["search".to_string()],
        models: std::collections::HashMap::new(),
    }
}

/// compress_history 进入压缩分支（>2 条）且 debug! 参数被求值（333）。
/// 行为（镜像 Go forceCompression）：保 system + 压缩注记 + 非系统轮的后
/// 50%（4 条 → 保 2）= 1+1+2 = 4 条。
#[test]
fn compress_history_logs_debug_with_enough_turns() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("one");
    instance.add_assistant_message("two", Vec::new(), None);
    instance.add_user_message("three");
    instance.add_assistant_message("four", Vec::new(), None);
    assert_eq!(instance.get_history().len(), 5, "system + 4 turns");
    let _logs = capture_logs();
    instance.compress_history();
    let hist = instance.get_history();
    assert_eq!(hist.len(), 4, "system + compression note + 2 kept turns");
    assert_eq!(hist[0].role, "system", "system prompt preserved");
    assert!(
        hist[1].content.contains("[Session compressed at"),
        "compression note inserted at index 1, got: {}",
        hist[1].content
    );
}

/// set_history：旧史首条是 system、新史首条不是 → 旧 system 回插到 0 位
/// （392 debug 参数行 + 402 insert）。
#[test]
fn set_history_reinserts_system_prompt_when_new_history_lacks_one() {
    let instance = AgentInstance::new(test_config());
    instance.add_user_message("old");
    let old = instance.get_history();
    assert_eq!(old[0].role, "system");

    let _logs = capture_logs();
    instance.set_history(vec![
        ConversationTurn {
            role: "user".to_string(),
            content: "fresh from disk".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "T".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: "reply".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "T".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ]);
    let hist = instance.get_history();
    assert_eq!(hist[0].role, "system", "system prompt re-inserted at index 0");
    assert_eq!(hist[1].content, "fresh from disk");
    assert_eq!(hist.len(), 3);
}

/// 对照：新史自带 system → 不重复插入。
#[test]
fn set_history_no_double_system_when_new_history_has_one() {
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![
        ConversationTurn {
            role: "system".to_string(),
            content: "already here".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "T".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "T".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ]);
    let hist = instance.get_history();
    assert_eq!(hist.len(), 2, "no duplicate system inserted");
    assert_eq!(hist[0].content, "already here");
}
