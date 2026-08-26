//! S9 覆盖率批次：tool_doc_folding.rs 剩余未覆盖行。
//! - 74：句末 `.`（后随 EOF）按句终切断（`None => true` 臂）。
//! - 78：`None => false` 臂为防御性死区——输入已 `trim()`，`.` 后不可能只剩
//!   空白（见报告结构性豁免）。

use super::*;

#[test]
fn one_line_summary_period_at_end_of_text_cuts() {
    // 串以 '.' 结束：rest 为空 → next=None → ends_sentence=true（74 的
    // `None => true` 臂）→ 在句号后切断。
    assert_eq!(one_line_summary("Ends here."), "Ends here.");
    // '.' 后跟大写字母 → Some(c) 臂判为句终（对照）。
    assert_eq!(one_line_summary("First sentence.Next"), "First sentence.");
}
