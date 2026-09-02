//! S9 覆盖率批次：types.rs 剩余未覆盖行。
//! - 183-185：`default_tool_type` serde default（ToolDefinition 缺 "type" 字段反序列化）。
//! - 366：`repair_tool_message_pairs` 内部循环收尾（多 tool_calls 中部分已应答的混合情形）。

use super::*;

/// ToolDefinition 反序列化缺 "type" 字段 → serde default "function"。
#[test]
fn tool_definition_missing_type_field_uses_serde_default() {
    let json = serde_json::json!({
        "function": {
            "name": "read_file",
            "description": "Read a file",
            "parameters": {"type": "object", "properties": {}}
        }
    });
    let def: ToolDefinition =
        serde_json::from_value(json).expect("missing type field must deserialize via default");
    assert_eq!(def.tool_type, "function");
    assert_eq!(def.function.name, "read_file");
}

/// repair_tool_message_pairs：同一 assistant 轮带 2 个 tool_calls，其中一个
/// 已有 tool 结果、另一个没有 → 只有缺的那个得到合成结果（366 的循环体）。
#[test]
fn repair_pairs_partial_answered_calls_get_synthetic_only_for_missing() {
    let mk_calls = |ids: &[&str]| -> Vec<ToolCallInfo> {
        ids.iter()
            .map(|id| ToolCallInfo {
                id: id.to_string(),
                name: "exec".to_string(),
                arguments: "{}".to_string(),
            })
            .collect()
    };
    let mut messages = vec![
        ConversationTurn {
            role: "user".to_string(),
            content: "run two commands".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: mk_calls(&["call_a", "call_b"]),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        // call_a 已有结果；call_b 没有
        ConversationTurn {
            role: "tool".to_string(),
            content: "result of a".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_a".to_string()),
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ];
    repair_tool_message_pairs(&mut messages);
    // call_b 应获得一个合成 tool 结果（role=tool, tool_call_id=call_b）
    let synth: Vec<&ConversationTurn> = messages
        .iter()
        .filter(|m| m.tool_call_id.as_deref() == Some("call_b"))
        .collect();
    assert_eq!(synth.len(), 1, "exactly one synthetic result for call_b");
    // call_a 的原结果不被复制
    let a_results: Vec<&ConversationTurn> = messages
        .iter()
        .filter(|m| m.tool_call_id.as_deref() == Some("call_a"))
        .collect();
    assert_eq!(a_results.len(), 1, "call_a keeps its single real result");
    // 合成结果紧跟 assistant 轮之后（插入位置正确）
    let assistant_idx = messages.iter().position(|m| m.role == "assistant").unwrap();
    assert_eq!(
        messages[assistant_idx + 1].tool_call_id.as_deref(),
        Some("call_b"),
        "synthetic result inserted right after the assistant turn"
    );
}
