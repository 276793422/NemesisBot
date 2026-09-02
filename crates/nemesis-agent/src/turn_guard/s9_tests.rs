//! S9 覆盖率批次：turn_guard.rs 剩余未覆盖行。
//! - 357：canonical_args 非 JSON 回退（whitespace 折叠）。
//! - 367/371：similarity 的 both-empty → 1.0 / union==0 防御分支。
//! - 260：文本重复 nudge 的 format! 参数行（tracing 之外，需真实进入分支）。

use super::*;

/// 非 JSON args 走 whitespace-collapse 回退（357）。
#[test]
fn canonical_args_non_json_collapses_whitespace() {
    assert_eq!(canonical_args("a   b\nc  d"), "a b c d");
    // JSON 路径对照：键排序 + 紧凑化
    assert_eq!(canonical_args(r#"{"b":1,"a":"x"}"#), r#"{"a":"x","b":1}"#);
}

/// 双空串相似度 = 1.0（367）。
#[test]
fn similarity_both_empty_is_one() {
    assert!((similarity("", "") - 1.0).abs() < 1e-9);
    // 不相交短串（shingle 集为空时走 both-empty；非空集走正常并交集）
    let s = similarity("abcd", "wxyz");
    assert!((0.0..=1.0).contains(&s));
}

/// union==0 的防御分支（371）：shingle 集合都为空但串非空（长度 < 4 的串
/// 产生空 shingle 集，此时 both-empty 已拦截返回 1.0；union==0 只有在
/// 两集合皆空之外再无来源——防御性不可达，这里钉住两短串的行为契约）。
#[test]
fn similarity_short_strings_do_not_panic() {
    // 长度 <4 的串 shingle 集为空 → both-empty 分支 → 1.0
    assert!((similarity("ab", "ab") - 1.0).abs() < 1e-9);
    // 一空一非空：sa 空并 sb 非空 → union>0，inter=0 → 0.0
    assert!((similarity("", "abcd") - 0.0).abs() < 1e-9);
}

/// 260：连续两轮近似文本 → nudge 生成（format! 参数行求值）。
#[test]
fn text_repetition_nudge_formats_on_second_round() {
    let mut guard = crate::turn_guard::TurnGuard::new();
    let text = "这是一段几乎不会变化的较长回复内容，用于触发文本重复检测逻辑。";
    let first = guard.check_text_repetition(text);
    assert!(first.is_none(), "第一轮只记录基线，不 nudge");
    let second = guard.check_text_repetition(text);
    assert!(second.is_some(), "第二轮连续相似必须 nudge");
    let nudge = second.unwrap();
    assert!(nudge.contains("几乎相同"), "nudge 内容: {}", nudge);
}
