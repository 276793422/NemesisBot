// tool_call_repair.rs 覆盖率补充测试（JSON 解析三臂 113/120/132、嵌套
// function 命名、未配对花括号跳过 177、字符串内转义引号 216、XML invoke）。

use super::*;

/// fenced ```json 数组形态 → 数组臂（110-113）。
#[test]
fn fenced_json_array_of_calls() {
    let content = "前文\n```json\n\
        [{\"name\":\"exec\",\"arguments\":\"{\\\"cmd\\\":\\\"ls\\\"}\"}]\n\
        ```\n后文";
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].function.as_ref().unwrap().name, "exec");
    assert_eq!(calls[0].id, "repair_0");
    assert_eq!(calls[0].call_type.as_deref(), Some("function"));
}

/// 裸单对象 + arguments 为对象 → to_string 臂（135-136 侧道）。
#[test]
fn bare_single_object_with_arguments_object_form() {
    let calls =
        repair_tool_calls("{\"name\": \"write_file\", \"arguments\": {\"path\": \"a.txt\"}}");
    assert_eq!(calls.len(), 1, "{calls:?}");
    let f = calls[0].function.as_ref().unwrap();
    assert_eq!(f.name, "write_file");
    assert!(f.arguments.contains("a.txt"), "{}", f.arguments);
}

/// OpenAI 风格嵌套 function.name（128-132 的 or_else 臂）+ 缺 arguments
/// → "{}"。
#[test]
fn nested_function_name_and_missing_arguments() {
    let calls = repair_tool_calls("{\"function\": {\"name\": \"grep\"}}");
    assert_eq!(calls.len(), 1, "{calls:?}");
    let f = calls[0].function.as_ref().unwrap();
    assert_eq!(f.name, "grep");
    assert_eq!(f.arguments, "{}");
}

/// 未配对 '{'：match_balanced 失败 → span 跳过（177 的 matched=false
/// 路径），不 panic、不产出。
#[test]
fn unbalanced_brace_is_skipped_not_panicking() {
    let calls = repair_tool_calls("{\"name\": \"never_closed\"");
    assert!(calls.is_empty(), "{calls:?}");
}

/// JSON 字符串内的 \" 转义：match_balanced 的 esc 臂（216）不误判结束。
#[test]
fn escaped_quote_inside_json_string() {
    let content = r#"{"name": "exec", "arguments": "{\"cmd\": \"say \\\"hi\\\"\"}"}"#;
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].function.as_ref().unwrap().arguments.contains("hi"));
}

/// XML invoke 形态（JSON 不命中时回落）。
#[test]
fn xml_invoke_form() {
    let content = "<invoke name=\"browser\">\
                   <parameter name=\"url\">https://x</parameter>\
                   </invoke>";
    let calls = repair_tool_calls(content);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let f = calls[0].function.as_ref().unwrap();
    assert_eq!(f.name, "browser");
    assert!(f.arguments.contains("url"), "{}", f.arguments);
}

/// 数组元素带 name 字段但非字符串 → 数组臂产出空（113 回落），单对象臂
/// 同样无名（120 回落）→ 双回落空表。
#[test]
fn nameless_entries_fall_through_both_arms() {
    let calls = repair_tool_calls("[{\"name\": 1}]");
    assert!(calls.is_empty(), "{calls:?}");
}
