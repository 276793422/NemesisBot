//! 回归钉：`push_chunk_incremental` 增量 UTF-8 解码。
//!
//! 旧实现逐 chunk 独立 `from_utf8_lossy`：TCP 分片把多字节字符劈开时，
//! 前半字节在上一个 chunk 被替换成 U+FFFD——长 CJK 回复几乎每个 chunk
//! 边界都烂字。修复 = 尾部不完整序列 holdback 到下一 chunk 拼接后解码。

use super::next_event_boundary;
use super::push_chunk_incremental;

/// 回归钉：`next_event_boundary` 双形态事件边界。
/// 此前 anthropic/http_provider 两 lane 只扫 `\n\n`——CRLF-only 上游
/// （SSE 规范行终止三态 LF/CR/CRLF）零事件解析，流式回复静默变空。

#[test]
fn boundary_lf_form() {
    assert_eq!(next_event_boundary("data: a\n\ndata: b"), Some((7, 2)));
    // 多边界取第一个。
    assert_eq!(next_event_boundary("a\n\nb\n\nc"), Some((1, 2)));
}

#[test]
fn boundary_crlf_form() {
    // CRLF-only 上游此前完全解析不出事件（find("\n\n") 永不命中）。
    assert_eq!(next_event_boundary("data: a\r\n\r\ndata: b"), Some((7, 4)));
}

#[test]
fn boundary_prefers_earlier_separator() {
    // LF 边界在前 → 取 LF。
    assert_eq!(next_event_boundary("a\n\nb\r\n\r\nc"), Some((1, 2)));
    // CRLF 边界在前 → 取 CRLF（旧 codex 内联语义：a<=b 取 LF，否则 CRLF）。
    assert_eq!(next_event_boundary("a\r\n\r\nb\n\nc"), Some((1, 4)));
    // 混合行尾（\r\n 行 + \n 空行）：CRLF 完整形态在场时优先整段消费。
    assert_eq!(next_event_boundary("data: x\r\n\r\n"), Some((7, 4)));
}

#[test]
fn boundary_none_when_no_separator() {
    assert_eq!(next_event_boundary("data: {\"a\":1}\n"), None);
    assert_eq!(next_event_boundary(""), None);
    // 半包：分隔符劈在 chunk 边界上（\r\n / \r\n 各半）不可误判。
    assert_eq!(next_event_boundary("data: x\r\n\r"), None);
}

#[test]
fn boundary_cr_only_form() {
    // CR-only 上游（规范行终止第三态）此前零事件解析——与 CRLF-only
    // 同一威胁面（流式回复静默变空）。
    assert_eq!(next_event_boundary("data: a\r\rdata: b"), Some((7, 2)));
}

#[test]
fn boundary_mixed_line_endings() {
    // SSE 规范允许逐行独立选终止符——数据行与空行可不同尾。
    // 数据行 LF + 空行 CRLF。
    assert_eq!(next_event_boundary("data: a\n\r\ndata: b"), Some((7, 3)));
    // 数据行 CRLF + 空行 LF。
    assert_eq!(next_event_boundary("data: a\r\n\ndata: b"), Some((7, 3)));
    // 数据行 CR + 空行 CRLF。
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
    // 再补 \r 仍是半包（\r\n / \r\n 各半，既有钉）。
    assert_eq!(next_event_boundary("data: x\r\n\r"), None);
    assert_eq!(next_event_boundary("data: x\r\n\r\n"), Some((7, 4)));
}

#[test]
fn boundary_spec_forms_stream_end_to_end_split() {
    // 全形态整流事件切分：三态行尾的合法流与 LF 流切分结果逐块一致。
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
    // LF-only。
    assert_eq!(
        split("data: {\"a\":1}\n\ndata: {\"b\":2}\n\n"),
        expect.clone()
    );
    // CRLF-only（既有钉覆盖单边界，这里钉全流）。
    assert_eq!(
        split("data: {\"a\":1}\r\n\r\ndata: {\"b\":2}\r\n\r\n"),
        expect.clone()
    );
    // 混合行尾（LF+CRLF / CRLF+LF）。
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

fn feed(chunks: &[&[u8]]) -> String {
    let mut buf = String::new();
    let mut pending = Vec::new();
    for c in chunks {
        push_chunk_incremental(&mut buf, &mut pending, c);
    }
    buf
}

#[test]
fn cjk_char_split_across_three_chunks_survives() {
    // "中" = AD，一字节一个 chunk（最恶劣劈法）。
    let out = feed(&[&[0xE4], &[0xB8], &[0xAD]]);
    assert_eq!(out, "中");
}

#[test]
fn emoji_four_byte_split_across_two_chunks_survives() {
    // "😀" = 9F 98 80，2+2 劈开。
    let out = feed(&[&[0xF0, 0x9F], &[0x98, 0x80]]);
    assert_eq!(out, "😀");
}

#[test]
fn mixed_ascii_and_cjk_split_mid_word() {
    // 真实形态：SSE 行内 ASCII 与 CJK 混排，chunk 边界落在字符中间。
    let full = "回答：好的，这是结果OK";
    let bytes = full.as_bytes();
    for split in 1..bytes.len() {
        let out = feed(&[&bytes[..split], &bytes[split..]]);
        assert_eq!(out, full, "split at byte {split}");
    }
}

#[test]
fn ascii_multichunk_passthrough_unchanged() {
    let out = feed(&[b"data: {\"a\":1}\n", b"\ndata: {\"b\":2}\n\n"]);
    assert_eq!(out, "data: {\"a\":1}\n\ndata: {\"b\":2}\n\n");
}

#[test]
fn genuinely_invalid_byte_replaced_inline() {
    // 真坏字节（0xFF）当场 lossy 替换，不悬挂、不影响后续解码。
    let out = feed(&[&[0x41, 0xFF, 0x42]]);
    assert_eq!(out, "A\u{FFFD}B");
}

#[test]
fn invalid_byte_then_split_char_mix() {
    // 坏字节与劈开字符同 chunk 混合：坏字节立即替换，残尾照常 holdback。
    let mut buf = String::new();
    let mut pending = Vec::new();
    push_chunk_incremental(&mut buf, &mut pending, &[0xFF, 0xE4]);
    push_chunk_incremental(&mut buf, &mut pending, &[0xB8, 0xAD]);
    assert_eq!(buf, "\u{FFFD}中");
    assert!(pending.is_empty());
}

#[test]
fn pending_stays_bounded_after_every_chunk() {
    // holdback 上界 3 字节（UTF-8 最长前缀残尾）。连续裸 lead byte 是坏
    // 字节流 → lossy 现场替换（每次 2×U+FFFD），pending 不无限累积。
    let mut buf = String::new();
    let mut pending = Vec::new();
    for _ in 0..64 {
        push_chunk_incremental(&mut buf, &mut pending, &[0xE4]);
        assert!(pending.len() <= 3, "pending = {pending:?}");
    }
    assert!(buf.contains('\u{FFFD}'), "坏序列必须现场替换不悬挂");
    // 坏流之后恢复完整序列照常解码。
    push_chunk_incremental(&mut buf, &mut pending, "中".as_bytes());
    push_chunk_incremental(&mut buf, &mut pending, b"!");
    assert!(buf.ends_with("中!"), "got: {buf:?}");
    assert!(pending.is_empty());
}
