//! P30（WS14）：canvas 块检出 + JS 语法预检单测。
//!
//! 覆盖面：围栏检出（含嵌套围栏不误触发/未闭合/大小写/长围栏）、语法预检
//! 三要件（合法 / 括号不平衡 / 引号未闭合）+ 模板字面量/正则不误判/注释与
//! 字符串隔离、json 数据岛与外链 src 跳过、绝对行号换算、回灌文本形态。

use super::*;

// ---------------------------------------------------------------------------
// 围栏检出
// ---------------------------------------------------------------------------

#[test]
fn no_canvas_block_yields_empty_scan() {
    let scan = scan_canvas_blocks("普通回复。\n```html\n<p>x</p>\n```\n完。");
    assert!(scan.blocks.is_empty());
    assert!(scan.is_ok());
}

#[test]
fn simple_canvas_block_detected_with_content_and_start_line() {
    let content = "上图：\n```canvas\n<html><body><p>hi</p></body></html>\n```\n完。";
    let scan = scan_canvas_blocks(content);
    assert_eq!(scan.blocks.len(), 1);
    assert_eq!(scan.blocks[0].html, "<html><body><p>hi</p></body></html>");
    assert_eq!(scan.blocks[0].start_line, 3);
    assert!(scan.is_ok());
}

#[test]
fn multiple_canvas_blocks_detected() {
    let content = "```canvas\n<p>1</p>\n```\n中间文字\n```canvas\n<p>2</p>\n```";
    let scan = scan_canvas_blocks(content);
    assert_eq!(scan.blocks.len(), 2);
    assert_eq!(scan.blocks[0].html, "<p>1</p>");
    assert_eq!(scan.blocks[1].html, "<p>2</p>");
    assert_eq!(scan.blocks[1].start_line, 6);
}

#[test]
fn canvas_info_string_is_case_insensitive_and_first_token_only() {
    let scan = scan_canvas_blocks("```Canvas extra words\n<p>x</p>\n```");
    assert_eq!(scan.blocks.len(), 1);
}

#[test]
fn nested_fence_example_does_not_trigger_detection() {
    // 外层 ```markdown 围栏里的 ```canvas 示例是内容不是块——关闭围栏
    // 不允许携带 info 串，故第 2 行不会关闭外层，也不会开新块。
    let content = "```markdown\n```canvas\n<p>not real</p>\n```\n```";
    let scan = scan_canvas_blocks(content);
    assert!(scan.blocks.is_empty(), "blocks: {:?}", scan.blocks);
}

#[test]
fn longer_opening_fence_only_closes_on_matching_length() {
    // ````canvas 开启（4 反引号）：内部 ``` 行（3 反引号）是内容。
    let content = "````canvas\n<p>x</p>\n```\n````";
    let scan = scan_canvas_blocks(content);
    assert_eq!(scan.blocks.len(), 1);
    assert_eq!(scan.blocks[0].html, "<p>x</p>\n```");
}

#[test]
fn unclosed_canvas_fence_reported_at_opening_line() {
    let content = "文本\n```canvas\n<p>hi</p>\n（没有收尾围栏）";
    let scan = scan_canvas_blocks(content);
    assert_eq!(scan.blocks.len(), 1, "未闭合块也要入列（判定有 canvas）");
    assert!(!scan.is_ok());
    assert_eq!(scan.issues.len(), 1);
    assert_eq!(scan.issues[0].line, 2, "问题定位到开启围栏行");
    assert!(scan.issues[0].message.contains("未闭合"));
}

// ---------------------------------------------------------------------------
// JS 语法预检（三要件 + 启发式边界）
// ---------------------------------------------------------------------------

#[test]
fn valid_js_has_no_issues() {
    let js = "const x = (1 + 2) * 3;\ndocument.getElementById('out').textContent = 'ok';";
    assert!(check_js_syntax(js).is_empty(), "{:?}", check_js_syntax(js));
}

#[test]
fn unbalanced_brackets_reported_with_line_and_col() {
    let js = "function f() {\n  return (1 + 2;\n}";
    let issues = check_js_syntax(js);
    // 两个问题：第 3 行的 '}' 与第 2 行的 '(' 失配；EOF 时第 1 行的 '{' 未闭合。
    assert_eq!(issues.len(), 2, "{issues:?}");
    assert!(issues[0].message.contains("括号不匹配"), "{issues:?}");
    assert_eq!(issues[0].line, 3);
    assert_eq!(issues[0].col, 1);
    assert!(issues[0].message.contains("第 2 行第 10 列"), "{issues:?}");
    assert!(issues[1].message.contains("括号未闭合"), "{issues:?}");
    assert_eq!(issues[1].line, 1);
    assert_eq!(issues[1].col, 14);
}

#[test]
fn unclosed_quote_reported_with_line_and_col() {
    let js = "const s = 'abc;\nconst t = 1;";
    let issues = check_js_syntax(js);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].line, 1);
    assert_eq!(issues[0].col, 11);
    assert!(issues[0].message.contains("字符串引号未闭合"), "{issues:?}");
}

#[test]
fn unclosed_template_literal_reported() {
    let js = "const t = `hi ${name;";
    let issues = check_js_syntax(js);
    // 模板反引号未闭合报一次；${ 表达式帧同源不重复报。
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].line, 1);
    assert_eq!(issues[0].col, 11);
    assert!(issues[0].message.contains("模板字面量未闭合"), "{issues:?}");
}

#[test]
fn closed_template_expression_is_balanced() {
    let js = "const t = `hi ${name} bye`;\nel.textContent = `${a + b}`;";
    assert!(check_js_syntax(js).is_empty(), "{:?}", check_js_syntax(js));
}

#[test]
fn regex_literal_not_misread_as_division() {
    // 字符类内的 / 不终止正则；除号链不误入正则模式。
    let js = "const re = /a[b/]c/g;\nconst q = x / y / 2;";
    assert!(check_js_syntax(js).is_empty(), "{:?}", check_js_syntax(js));
}

#[test]
fn regex_after_keyword_allowed_after_division_denied() {
    // return 后 / 是正则；标识符后 / 是除号。
    let js = "const v = flag ? a / b : 0;\nfunction f() { return /x/.test(s); }";
    assert!(check_js_syntax(js).is_empty(), "{:?}", check_js_syntax(js));
}

#[test]
fn comments_and_strings_do_not_confuse_balance() {
    let js = "let ok = true; // (fake (\n/* [也忽略 */\nlet s = \")({[\";\nlet more = (1 + 2);";
    assert!(check_js_syntax(js).is_empty(), "{:?}", check_js_syntax(js));
}

#[test]
fn escaped_quote_inside_string_is_fine() {
    // JS 源码：const s = 'a\'b'; const t = "x\"y";
    let js = "const s = 'a\\'b'; const t = \"x\\\"y\";";
    assert!(check_js_syntax(js).is_empty(), "{:?}", check_js_syntax(js));
}

#[test]
fn stray_closer_reported() {
    let js = "const a = 1;\n}";
    let issues = check_js_syntax(js);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].line, 2);
    assert!(issues[0].message.contains("多余的闭括号"), "{issues:?}");
}

#[test]
fn unclosed_block_comment_reported() {
    let js = "const a = 1;\n/* 未闭合注释 {";
    let issues = check_js_syntax(js);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].line, 2);
    assert!(issues[0].message.contains("块注释未闭合"), "{issues:?}");
}

#[test]
fn issue_cap_prevents_pathological_flood() {
    let js = ")".repeat(100);
    let issues = check_js_syntax(&js);
    assert_eq!(issues.len(), MAX_ISSUES + 1, "上限 + 截断提示各一条");
}

// ---------------------------------------------------------------------------
// 块级扫描：script 提取 / 数据岛跳过 / 绝对行号换算
// ---------------------------------------------------------------------------

#[test]
fn json_data_island_and_external_src_are_skipped() {
    let content = "```canvas\n\
<script type=\"application/json\">{\"x\": (坏括号也不检}</script>\n\
<script src=\"https://evil.example/x.js\">(</script>\n\
<script>document.title = 'ok';</script>\n\
```";
    let scan = scan_canvas_blocks(content);
    assert!(scan.is_ok(), "数据岛与外链都跳过: {:?}", scan.issues);
    // 数据岛原样保留在块内容里（前端 srcdoc 注入契约）。
    assert!(scan.blocks[0].html.contains("{\"x\": (坏括号也不检}"));
}

#[test]
fn scan_reports_absolute_line_numbers_across_block() {
    let content = "第一行\n```canvas\n<html><body>\n<script>\nconst a = (1;\n</script>\n</body></html>\n```\n尾行";
    let scan = scan_canvas_blocks(content);
    assert!(!scan.is_ok());
    assert_eq!(scan.issues.len(), 1, "{:?}", scan.issues);
    // JS 第 1 行的 '(' → 块内容首行是第 3 行，script 正文在回复第 5 行。
    assert_eq!(scan.issues[0].line, 5);
    assert_eq!(scan.issues[0].col, 11);
    assert!(
        scan.issues[0].message.contains("括号未闭合"),
        "{:?}",
        scan.issues[0].message
    );
}

#[test]
fn scan_of_multi_issue_block_keeps_all() {
    let content = "```canvas\n<script>\nconst s = 'abc;\nreturn (1;\n</script>\n```";
    let scan = scan_canvas_blocks(content);
    // 引号未闭合（第 3 行）+ '(' 未闭合（第 4 行）。
    assert_eq!(scan.issues.len(), 2, "{:?}", scan.issues);
    assert_eq!(scan.issues[0].line, 3);
    assert_eq!(scan.issues[1].line, 4);
}

#[test]
fn empty_canvas_block_is_valid() {
    let scan = scan_canvas_blocks("```canvas\n```\n完");
    assert!(scan.is_ok());
    assert_eq!(scan.blocks.len(), 1);
    assert_eq!(scan.blocks[0].html, "");
}

// ---------------------------------------------------------------------------
// 回灌文本
// ---------------------------------------------------------------------------

#[test]
fn feedback_text_carries_lines_retry_budget_and_disclaimer() {
    let content = "```canvas\n<script>\nconst s = 'abc;\n</script>\n```";
    let scan = scan_canvas_blocks(content);
    let text = scan.feedback_text(1, 2);
    assert!(text.contains("[canvas 预检]"), "{text}");
    assert!(text.contains("1/2"), "{text}");
    assert!(text.contains("第 3 行"), "{text}");
    assert!(
        text.contains("轻量平衡检查"),
        "诚实边界必须随文本告知: {text}"
    );
    assert!(text.contains("完整回复"), "要求整条重发防丢正文: {text}");
}

#[test]
fn feedback_text_truncates_long_issue_lists() {
    let scan = CanvasScan {
        blocks: vec![CanvasBlock {
            html: String::new(),
            start_line: 1,
        }],
        issues: (0..10)
            .map(|i| CanvasIssue {
                line: i + 1,
                col: 1,
                message: format!("问题{i}"),
            })
            .collect(),
    };
    let text = scan.feedback_text(2, 2);
    assert!(text.contains("问题7"));
    assert!(!text.contains("问题8"));
    assert!(text.contains("另有 2 条问题未列出"), "{text}");
}
