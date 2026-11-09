//! 回归钉：`next_event_boundary` 全形态事件边界。
//!
//! 旧实现（nemesis-providers/nemesis-mcp 各自内联）只扫 `\n\n` 与
//! `\r\n\r\n` 两形态——CR-only 与混合行尾（规范合法）零事件解析。
//! 修复 = 行导向扫描：三态行终止逐行独立 + 贪婪 CRLF + 尾随 `\r`
//! 半包等待。

use super::next_event_boundary;

// ---- 既有两形态（自 stream_util 迁移的钉，行为不变）----

#[test]
fn boundary_lf_form() {
    assert_eq!(next_event_boundary("data: a\n\ndata: b"), Some((7, 2)));
    // 多边界取第一个。
    assert_eq!(next_event_boundary("a\n\nb\n\nc"), Some((1, 2)));
}

#[test]
fn boundary_crlf_form() {
    assert_eq!(next_event_boundary("data: a\r\n\r\ndata: b"), Some((7, 4)));
}

#[test]
fn boundary_prefers_earlier_separator() {
    // LF 边界在前 → 取 LF。
    assert_eq!(next_event_boundary("a\n\nb\r\n\r\nc"), Some((1, 2)));
    // CRLF 边界在前 → 取 CRLF。
    assert_eq!(next_event_boundary("a\r\n\r\nb\n\nc"), Some((1, 4)));
    assert_eq!(next_event_boundary("data: x\r\n\r\n"), Some((7, 4)));
}

#[test]
fn boundary_none_when_no_separator() {
    assert_eq!(next_event_boundary("data: {\"a\":1}\n"), None);
    assert_eq!(next_event_boundary(""), None);
    // 半包：分隔符劈在 chunk 边界上（\r\n / \r\n 各半）不可误判。
    assert_eq!(next_event_boundary("data: x\r\n\r"), None);
}

// ---- 新形态（本轮修复面）----

#[test]
fn boundary_cr_only_form() {
    // CR-only 上游（规范行终止第三态）此前零事件解析。
    assert_eq!(next_event_boundary("data: a\r\rdata: b"), Some((7, 2)));
}

#[test]
fn boundary_mixed_line_endings() {
    // SSE 规范允许逐行独立选终止符——数据行与空行可不同尾。
    assert_eq!(next_event_boundary("data: a\n\r\ndata: b"), Some((7, 3)));
    assert_eq!(next_event_boundary("data: a\r\n\ndata: b"), Some((7, 3)));
    assert_eq!(next_event_boundary("data: a\r\r\ndata: b"), Some((7, 3)));
}

#[test]
fn boundary_trailing_cr_holds_back_for_all_forms() {
    // 半包纪律对 CR 形态同样成立：尾随 \r 可能是 CR 终止符也可能是
    // CRLF 前半，须等下一字节定谳（WHATWG 参考解析器同语义）。
    assert_eq!(next_event_boundary("data: x\r"), None);
    // 第二个 \r 仍不可判定（CR 边界 or CRLF 前半）——继续等待。
    assert_eq!(next_event_boundary("data: x\r\r"), None);
    // 第三字节非 \n → CR-only 边界确认。
    assert_eq!(next_event_boundary("data: x\r\rd"), Some((7, 2)));
    // 第三字节 \n → 实为 CR + CRLF 混合形态（数据行 CR + 空行 CRLF）。
    assert_eq!(next_event_boundary("data: x\r\r\n"), Some((7, 3)));
    // 补 \n → 实为单行 CRLF（非边界）。
    assert_eq!(next_event_boundary("data: x\r\n"), None);
    assert_eq!(next_event_boundary("data: x\r\n\r"), None);
    assert_eq!(next_event_boundary("data: x\r\n\r\n"), Some((7, 4)));
}

#[test]
fn boundary_greedy_crlf_not_split_into_cr_lf() {
    // "a\r\n\n"：\r\n 恒作 CRLF 消费（贪婪），空行是后面的 LF ——
    // 数据行 CRLF + 空行 LF 混合形态，不是「CR 边界 + 残留 \n」。
    assert_eq!(next_event_boundary("a\r\n\nb"), Some((1, 3)));
}

#[test]
fn boundary_leading_blank_line_progresses() {
    // 流头空行 = 空事件：块为空但必须推进（sep ≥ 1，消费方跳过空块）。
    assert_eq!(next_event_boundary("\n\ndata: b"), Some((0, 1)));
    assert_eq!(next_event_boundary("\r\n\r\ndata: b"), Some((0, 2)));
}

#[test]
fn boundary_spec_forms_stream_end_to_end_split() {
    // 全形态整流事件切分：三态行尾的合法流切分结果逐块一致。
    let split = |s: &str| {
        let mut buf = s.to_string();
        let mut out = Vec::new();
        while let Some((pos, sep)) = next_event_boundary(&buf) {
            out.push(buf[..pos].to_string());
            buf = buf[pos + sep..].to_string();
        }
        (out, buf)
    };
    let expect = (
        vec!["data: {\"a\":1}".to_string(), "data: {\"b\":2}".to_string()],
        String::new(),
    );
    assert_eq!(
        split("data: {\"a\":1}\n\ndata: {\"b\":2}\n\n"),
        expect.clone()
    );
    assert_eq!(
        split("data: {\"a\":1}\r\n\r\ndata: {\"b\":2}\r\n\r\n"),
        expect.clone()
    );
    assert_eq!(split("data: {\"a\":1}\n\r\ndata: {\"b\":2}\r\n\n"), expect);
    // CR-only：末事件尾随 \r\r 等下一字节定谳（流式半包语义，真实流
    // 总有后续 chunk 或 EOF flush）——前面的事件照常切出。
    assert_eq!(
        split("data: {\"a\":1}\r\rdata: {\"b\":2}\r\r"),
        (
            vec!["data: {\"a\":1}".to_string()],
            "data: {\"b\":2}\r\r".to_string()
        )
    );
}

#[test]
fn boundary_chunk_split_cr_separator_reassembles() {
    // CR 分隔符劈在 chunk 边界：尾随 \r 逐段等待，第三字节定谳成型。
    let mut acc = String::from("data: x\r");
    assert_eq!(next_event_boundary(&acc), None);
    acc.push('\r');
    assert_eq!(next_event_boundary(&acc), None);
    acc.push('d');
    assert_eq!(next_event_boundary(&acc), Some((7, 2)));
}

// ---------------------------------------------------------------------------
// push_chunk_incremental（BUG-20261109-007：nemesis-mcp 逐 chunk lossy 同族钉）
// ---------------------------------------------------------------------------

#[test]
fn push_chunk_cjk_split_across_chunks_survives() {
    // "中" = E4 B8 AD，一字节一个 chunk（最恶劣劈法）。
    let mut buf = String::new();
    let mut pending = Vec::new();
    super::push_chunk_incremental(&mut buf, &mut pending, &[0xE4]);
    super::push_chunk_incremental(&mut buf, &mut pending, &[0xB8]);
    super::push_chunk_incremental(&mut buf, &mut pending, &[0xAD]);
    assert_eq!(buf, "中");
    assert!(pending.is_empty());
}

#[test]
fn push_chunk_invalid_byte_replaced_then_split_char_completes() {
    // 真坏字节当场替换；紧随其后的劈开字符照常跨 chunk 补全。
    let mut buf = String::new();
    let mut pending = Vec::new();
    super::push_chunk_incremental(&mut buf, &mut pending, &[0xFF, 0xE4]);
    super::push_chunk_incremental(&mut buf, &mut pending, &[0xB8, 0xAD]);
    assert_eq!(buf, "\u{FFFD}中");
    assert!(pending.is_empty());
}
