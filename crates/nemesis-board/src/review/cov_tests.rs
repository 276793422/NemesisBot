// review.rs 覆盖率补充测试（ReviewParseError Display 147-149 / JSON 提取
// 的 `}` 在前 `{` 在后死形态 183 / truncate_bytes 的 rune 边界回退臂
// 238-239）。

use super::*;

/// Display 直接透出 message（147-149）。
#[test]
fn parse_error_display_returns_message() {
    let e = ReviewParseError {
        message: "缺少 verdict 字段".to_string(),
    };
    assert_eq!(format!("{e}"), "缺少 verdict 字段");
}

/// `}` 出现在 `{` 之前 → 提取失败 → 解析错误（183 的 return None）。
#[test]
fn reversed_braces_fall_to_parse_error() {
    let err = parse_review("这是说明文本 } { 而已").unwrap_err();
    assert!(
        err.message.contains("找不到 JSON 对象"),
        "实际文案：{}",
        err.message
    );
}

/// 线程评论截断落在多字节字符内部 → rune 边界回退循环（238-239），
/// 截断产物带省略号注记且不含半个字符。
#[test]
fn thread_comment_truncation_walks_back_char_boundary() {
    // 2047 字节 'x' + 3 字节 '好'：len=2050 > 2048，cut=2048 落在 '好' 中间。
    let content = format!("{}好", "x".repeat(2047));
    let prompt = build_review_user_prompt(
        "NB-9",
        "标题",
        "背景",
        None,
        "worker 汇报正文",
        &[("节点甲".to_string(), content)],
    );
    assert!(prompt.contains("（已截断）"), "必须带截断注记");
    assert!(!prompt.contains('\u{FFFD}'), "不得产生替换符（半个字符）");
}

// ===========================================================================
// wave6 追加：ReviewVerdict::as_str 三臂（75-81）。
// ===========================================================================

/// as_str 与 verdict 词表逐一对应（77-79）。
#[test]
fn w6_review_verdict_as_str_all_arms() {
    assert_eq!(ReviewVerdict::Pass.as_str(), "PASS");
    assert_eq!(ReviewVerdict::Fail.as_str(), "FAIL");
    assert_eq!(ReviewVerdict::Unsure.as_str(), "UNSURE");
}
