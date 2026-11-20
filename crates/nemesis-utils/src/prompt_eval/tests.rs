//! #16 触发文本规范化回归测试：截断点多字节安全（中文人格文件是常态
//! 载荷，按字节切在 char 中间会产出非法 UTF-8 给子进程命令行）。

use super::{PROMPT_EVAL_MAX_CHARS, floor_char_boundary, normalize_trigger_text};

#[test]
fn short_text_passes_through_verbatim() {
    let s = "你是安全评审专家。";
    assert_eq!(normalize_trigger_text(s), s);
    assert_eq!(floor_char_boundary(s, s.len()), s.len());
}

#[test]
fn overlong_ascii_truncates_at_limit() {
    let s = "a".repeat(PROMPT_EVAL_MAX_CHARS + 100);
    let out = normalize_trigger_text(&s);
    assert_eq!(out.len(), PROMPT_EVAL_MAX_CHARS);
    assert_eq!(out.chars().count(), PROMPT_EVAL_MAX_CHARS);
}

#[test]
fn overlong_multibyte_truncates_on_char_boundary() {
    // 全中文（每字 3 字节）：limit 落点大概率在字符中间，必须向前回退。
    let s = "安".repeat(PROMPT_EVAL_MAX_CHARS / 3 + 50);
    let out = normalize_trigger_text(&s);
    assert!(out.len() <= PROMPT_EVAL_MAX_CHARS);
    // 截断产物必须仍是合法 UTF-8（切片本身保证）且无半个字符。
    assert!(out.chars().all(|c| c == '安'));
    assert_eq!(out.chars().count(), out.len() / 3);
    // 回退点以后补齐一个完整字符仍在原串内。
    let boundary = floor_char_boundary(&s, PROMPT_EVAL_MAX_CHARS);
    assert!(s.is_char_boundary(boundary));
}

#[test]
fn boundary_index_beyond_len_clamps() {
    let s = "abc";
    assert_eq!(floor_char_boundary(s, 100), 3);
    assert_eq!(floor_char_boundary(s, 2), 2);
    // index=1 落在 'a' 与 'b' 边界上（ASCII 处处是边界）→ 原样。
    assert_eq!(floor_char_boundary(s, 1), 1);
}

#[test]
fn mixed_script_boundary_walks_back_to_char_start() {
    // "安" (3B) + "a" (1B) 交错：从多字节字符中间回退必须落在字符头。
    let mut s = String::new();
    for _ in 0..100 {
        s.push('安');
        s.push('x');
    }
    for probe in [4, 5, 6, 7, PROMPT_EVAL_MAX_CHARS / 2] {
        let b = floor_char_boundary(&s, probe);
        assert!(s.is_char_boundary(b), "probe {probe} → {b} 必须是边界");
    }
}
