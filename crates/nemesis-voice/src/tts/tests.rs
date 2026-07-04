use super::*;

/// 回归：用户报告 "【**2026年7月3日 周五 下午两点四十**！】" 无法播放。
/// 根因——CJK 之间的 ASCII 空格让 Kokoro 抛 "Unknown token"（issue 2223），
/// 此前 normalize 只删 emoji 后的孤儿空格，漏了中文之间手打的空格。
#[test]
fn normalize_user_regression_no_cjk_adjacent_space() {
    let out = normalize_tts_text("【**2026年7月3日 周五 下午两点四十**！】");
    assert_eq!(out, "2026年7月3日周五下午两点四十!");
    assert!(
        !out.contains(' '),
        "normalized text must contain no ASCII space: {out:?}"
    );
}

#[test]
fn normalize_drops_space_between_cjk() {
    assert_eq!(normalize_tts_text("你 好"), "你好");
    assert_eq!(normalize_tts_text("3日 周五"), "3日周五");
    // 多个连续 CJK 间空格全去
    assert_eq!(normalize_tts_text("你 好 吗"), "你好吗");
    // CJK 与 ASCII 数字之间也去（前是 CJK）
    assert_eq!(normalize_tts_text("第3 章"), "第3章");
}

#[test]
fn normalize_keeps_english_spaces() {
    // 英文单词之间的空格必须保留（Kokoro 英文路径需要）
    assert_eq!(normalize_tts_text("hello world"), "hello world");
    assert_eq!(normalize_tts_text("it is 2026"), "it is 2026");
}

#[test]
fn normalize_drops_orphan_space_after_stripped() {
    // emoji 被剥后紧跟的空格也要去（原有逻辑不能回归）
    assert_eq!(normalize_tts_text("你好😀 世界"), "你好世界");
}

#[test]
fn normalize_collapses_adjacent_punct() {
    // 连续/相邻标点会触发 C++ 异常，只保留一个
    assert_eq!(normalize_tts_text("你好。。世界"), "你好.世界");
    assert_eq!(normalize_tts_text("真的吗？？"), "真的吗?");
}

#[test]
fn normalize_maps_fullwidth_punct() {
    assert_eq!(normalize_tts_text("你好，世界。"), "你好,世界.");
}

#[test]
fn normalize_strips_unsafe_symbols() {
    // 【】、markdown 星号、书名号等都不在白名单，必须剥掉（不能漏到 Kokoro）
    let out = normalize_tts_text("《**测试**》");
    assert_eq!(out, "测试");
}
