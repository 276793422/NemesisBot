// tool_call_extract.rs 覆盖率补充测试（无标记文本早退 16）。

use super::*;

/// 文本不含 {"tool_calls" 标记 → 空表早退（16）。
#[test]
fn text_without_marker_yields_no_calls() {
    assert!(extract_tool_calls_from_text("plain answer, no tools here").is_empty());
}

/// 有标记但括号不配对 → 第二早退臂（16）。
#[test]
fn marker_without_matching_brace_yields_no_calls() {
    let text = "answer {\"tool_calls\": [{\"id\": \"a\"";
    assert!(extract_tool_calls_from_text(text).is_empty());
}
