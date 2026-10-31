//! think_strip 全边界单测：非流式纯函数 + 流式状态机（chunk 边界切碎 /
//! 未闭合 / 畸形 / 多块 / 字段并存拼接 / 直通零影响）。

use super::*;

// ---------------------------------------------------------------------------
// strip_inline_thinking（非流式）
// ---------------------------------------------------------------------------

#[test]
fn no_tag_passthrough_byte_identical() {
    // 无标签：正文逐字节不变、思考 None（零影响直通）。
    let cases = [
        "plain answer",
        "  spaced  ",
        "a</think>b",        // 畸形：只有闭标签 → 不猜，原样直通
        "a<thinkx</think>y", // 伪标签（think 后跟非 >）→ 原样直通
        "",
    ];
    for c in cases {
        let (clean, thinking) = strip_inline_thinking(c);
        assert_eq!(clean, c, "content must be untouched: {c:?}");
        assert_eq!(thinking, None, "no thinking expected: {c:?}");
    }
}

#[test]
fn basic_strip() {
    let (clean, thinking) = strip_inline_thinking("<think>secret plan</think>answer");
    assert_eq!(clean, "answer");
    assert_eq!(thinking.as_deref(), Some("secret plan"));
}

#[test]
fn leading_whitespace_before_content_trimmed() {
    let (clean, thinking) = strip_inline_thinking("<think>x</think>\n\n  answer here");
    assert_eq!(clean, "answer here");
    assert_eq!(thinking.as_deref(), Some("x"));
}

#[test]
fn unclosed_block_all_goes_to_thinking() {
    // 无闭合（截断响应）：已进入部分全归 thinking，无正文。
    let (clean, thinking) = strip_inline_thinking("<think>partial thought");
    assert_eq!(clean, "");
    assert_eq!(thinking.as_deref(), Some("partial thought"));

    // 开标签前有正文：正文保留。
    let (clean, thinking) = strip_inline_thinking("pre<think>x");
    assert_eq!(clean, "pre");
    assert_eq!(thinking.as_deref(), Some("x"));
}

#[test]
fn multiple_blocks_joined_with_blank_line() {
    let (clean, thinking) =
        strip_inline_thinking("<think>first</think>mid<think>\nsecond\n</think>end");
    assert_eq!(clean, "midend");
    assert_eq!(thinking.as_deref(), Some("first\n\nsecond"));
}

#[test]
fn markup_inside_think_preserved_verbatim() {
    let inner = "has <b>bold</b> and {\"json\": true} inside";
    let (clean, thinking) = strip_inline_thinking(&format!("<think>{inner}</think>out"));
    assert_eq!(clean, "out");
    assert_eq!(thinking.as_deref(), Some(inner));
}

#[test]
fn empty_think_block_is_silent_shell() {
    // 剥空壳：无思考产出，正文不动。
    let (clean, thinking) = strip_inline_thinking("<think></think>answer");
    assert_eq!(clean, "answer");
    assert_eq!(thinking, None);
}

#[test]
fn adjacent_tags_no_content_between() {
    let (clean, thinking) = strip_inline_thinking("<think>a</think><think>b</think>");
    assert_eq!(clean, "");
    assert_eq!(thinking.as_deref(), Some("a\n\nb"));
}

// ---------------------------------------------------------------------------
// merge_reasoning（归一合并）
// ---------------------------------------------------------------------------

#[test]
fn merge_reasoning_all_arms() {
    assert_eq!(merge_reasoning(None, None), None);
    assert_eq!(merge_reasoning(Some("a".into()), None), Some("a".into()));
    assert_eq!(merge_reasoning(None, Some("b".into())), Some("b".into()));
    assert_eq!(
        merge_reasoning(Some("a".into()), Some("b".into())),
        Some("a\n\nb".into())
    );
}

// ---------------------------------------------------------------------------
// ThinkStripper（流式状态机）
// ---------------------------------------------------------------------------

#[test]
fn streamer_plain_multi_chunk_untouched() {
    let mut s = ThinkStripper::new();
    let mut out = String::new();
    for c in ["hel", "lo ", "wor", "ld"] {
        out.push_str(&s.feed(c));
        assert_eq!(s.take_thinking(), None);
    }
    assert_eq!(out, "hello world");
    assert_eq!(s.finish(&mut String::new()), "");
}

#[test]
fn streamer_tag_split_across_chunks() {
    // <think> 与 </think> 都被 chunk 边界劈开。
    let mut s = ThinkStripper::new();
    let mut content = String::new();
    let mut reasoning = String::new();
    for c in ["<thi", "nk>sec", "ret</th", "ink>done"] {
        content.push_str(&s.feed(c));
        if let Some(th) = s.take_thinking() {
            reasoning.push_str(&th);
        }
    }
    assert_eq!(content, "done");
    assert_eq!(reasoning, "secret");
}

#[test]
fn streamer_holdback_released_when_not_a_tag() {
    // "<think" + "x..."：holdback 补齐后不是真标签 → 全部按正文放行。
    let mut s = ThinkStripper::new();
    let mut content = String::new();
    for c in ["a<think", "x</think>y"] {
        content.push_str(&s.feed(c));
        assert_eq!(s.take_thinking(), None);
    }
    assert_eq!(content, "a<thinkx</think>y");
    assert_eq!(s.finish(&mut String::new()), "");
}

#[test]
fn streamer_holdback_flushed_as_content_at_finish() {
    // 流在疑似标签前缀处结束：残留是正文（响应结束了，不是真标签）。
    let mut s = ThinkStripper::new();
    let mut content = String::new();
    content.push_str(&s.feed("abc<thi"));
    assert_eq!(s.take_thinking(), None);
    let mut reasoning = String::new();
    let flushed = s.finish(&mut reasoning);
    assert_eq!(format!("{content}{flushed}"), "abc<thi");
    assert_eq!(reasoning, "");
}

#[test]
fn streamer_unclosed_block_finish_goes_to_thinking() {
    // 无闭合（截断流）：InThink 残留（含 holdback 尾）finish 时归思考。
    let mut s = ThinkStripper::new();
    let mut content = String::new();
    for c in ["pre<think>hal", "f thought"] {
        content.push_str(&s.feed(c));
        assert_eq!(
            s.take_thinking(),
            None,
            "unclosed block must not drain early"
        );
    }
    assert_eq!(content, "pre");
    let mut reasoning = String::new();
    let flushed = s.finish(&mut reasoning);
    assert_eq!(flushed, "");
    assert_eq!(reasoning, "half thought");
}

#[test]
fn streamer_multiple_blocks_cross_chunk() {
    let mut s = ThinkStripper::new();
    let mut content = String::new();
    let mut reasoning = String::new();
    // 逐字节切碎——最严酷边界。
    let full = "<think>aa</think>x<think>bb</think>y";
    for ch in full.chars() {
        content.push_str(&s.feed(&ch.to_string()));
        if let Some(th) = s.take_thinking() {
            if !reasoning.is_empty() {
                reasoning.push_str("\n\n");
            }
            reasoning.push_str(&th);
        }
    }
    assert_eq!(content, "xy");
    assert_eq!(reasoning, "aa\n\nbb");
    assert_eq!(s.finish(&mut String::new()), "");
}

#[test]
fn streamer_empty_feed_noop() {
    let mut s = ThinkStripper::new();
    assert_eq!(s.feed(""), "");
    assert_eq!(s.take_thinking(), None);
    assert_eq!(s.finish(&mut String::new()), "");
}

#[test]
fn streamer_think_inside_think_stays_in_thinking() {
    // 思考内容里再出现 <think>：InThink 只认闭标签，嵌套文本原样归思考。
    let mut s = ThinkStripper::new();
    let mut content = String::new();
    let mut reasoning = String::new();
    for c in ["<think>a <think> nested", " </think>done"] {
        content.push_str(&s.feed(c));
        if let Some(th) = s.take_thinking() {
            reasoning.push_str(&th);
        }
    }
    assert_eq!(content, "done");
    assert_eq!(reasoning, "a <think> nested");
}

#[test]
fn streamer_finish_merges_into_existing_reasoning() {
    // finish 把未闭合块的残留思考并入已有 reasoning（字段并存拼接语义）；
    // 已被 take_thinking 取走的部分归调用方，finish 不重复并入。
    let mut s = ThinkStripper::new();
    let _ = s.feed("pre<think>residual");
    assert_eq!(
        s.take_thinking(),
        None,
        "unclosed block must not drain early"
    );
    let mut reasoning = "field".to_string();
    let flushed = s.finish(&mut reasoning);
    assert_eq!(flushed, "");
    assert_eq!(reasoning, "field\n\nresidual");
}
