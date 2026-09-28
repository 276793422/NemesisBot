//! sentence.rs 纯函数测试（跨平台，常规 CI）。

use super::split_for_tts;
use super::split_sentences;

#[test]
fn splits_on_cjk_terminal_punctuation() {
    let out = split_sentences("今天天气不错。我们去公园吧！你呢？");
    assert_eq!(out, vec!["今天天气不错。", "我们去公园吧！", "你呢？"]);
}

#[test]
fn terminal_punctuation_stays_attached() {
    let out = split_sentences("第一句；第二句");
    assert_eq!(out, vec!["第一句；", "第二句"]);
}

#[test]
fn newline_is_a_splitter() {
    let out = split_sentences("第一行\n第二行\n\n第三行");
    assert_eq!(out, vec!["第一行", "第二行", "第三行"]);
}

#[test]
fn ascii_period_guards() {
    // 小数不切
    assert_eq!(
        split_sentences("价格为 3.5 元左右"),
        vec!["价格为 3.5 元左右"]
    );
    // 版本号不切
    assert_eq!(
        split_sentences("升级到 v1.2 版本"),
        vec!["升级到 v1.2 版本"]
    );
    // 后随空白且前非数字 → 切
    let out = split_sentences("This is done. Next step");
    assert_eq!(out, vec!["This is done.", "Next step"]);
}

#[test]
fn ellipsis_and_ascii_marks_split() {
    let out = split_sentences("等等…好吧!");
    assert_eq!(out.len(), 2);
    assert_eq!(out[0], "等等…");
    assert_eq!(out[1], "好吧!");
}

#[test]
fn empty_and_whitespace_input() {
    assert!(split_sentences("").is_empty());
    assert!(split_sentences("   \n  \n").is_empty());
}

#[test]
fn split_for_tts_soft_cuts_overlong() {
    // 一句超长中文（无终结标点），按逗号软切
    let long = (0..30).map(|_| "abcdefg，").collect::<String>();
    let out = split_for_tts(&long, 20);
    assert!(out.len() > 1, "expected soft splits, got {}", out.len());
    for piece in &out {
        assert!(
            piece.chars().count() <= 25,
            "piece too long: {} chars",
            piece.chars().count()
        );
    }
    // 内容不丢：拼接后去分隔符应等于原文去分隔符
    let strip = |v: &[String]| v.concat().replace(['，', ' '], "");
    assert_eq!(strip(&out), strip(std::slice::from_ref(&long)));
}

#[test]
fn split_for_tts_passthrough_short() {
    let out = split_for_tts("短句一。短句二。", 100);
    assert_eq!(out, vec!["短句一。", "短句二。"]);
}
