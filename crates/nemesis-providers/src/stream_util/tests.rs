//! BUG-HUNT R38 回归钉：`push_chunk_incremental` 增量 UTF-8 解码。
//!
//! 旧实现逐 chunk 独立 `from_utf8_lossy`：TCP 分片把多字节字符劈开时，
//! 前半字节在上一个 chunk 被替换成 U+FFFD——长 CJK 回复几乎每个 chunk
//! 边界都烂字。修复 = 尾部不完整序列 holdback 到下一 chunk 拼接后解码。

use super::push_chunk_incremental;

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
    // "中" = E4 B8 AD，一字节一个 chunk（最恶劣劈法）。
    let out = feed(&[&[0xE4], &[0xB8], &[0xAD]]);
    assert_eq!(out, "中");
}

#[test]
fn emoji_four_byte_split_across_two_chunks_survives() {
    // "😀" = F0 9F 98 80，2+2 劈开。
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
