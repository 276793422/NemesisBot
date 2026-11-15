//! child_decode 单测（BUG-20261115-034）。
//!
//! 契约：UTF-8 严格优先字节精确（cargo/rustc/git/node 输出零变化）；
//! CJK OEM 码页族（936/932/949/950）GBK 等字节正确恢复；非 CJK 码页诚实
//! 回落 lossy。机器面包装只测机器无关断言（ASCII 直通）——OEMCP 随机器
//! 变化，不做平台相关断言。

use super::{decode_child_output, decode_with_codepage};

/// "你" = GBK C4E3，"好" = GBK BAC3。
const GBK_NIHAO: &[u8] = &[0xC4, 0xE3, 0xBA, 0xC3];

#[test]
fn ascii_passthrough_byte_exact() {
    let out = decode_with_codepage(b"total 42\r\ndocs  README.md", 936);
    assert_eq!(out, "total 42\r\ndocs  README.md");
}

#[test]
fn valid_utf8_chinese_takes_priority_over_codepage() {
    // UTF-8 合法字节在 CP936 参数下也必须原样（UTF-8 工具输出零变化）。
    let utf8 = "中文文件名.txt 与 English 混排".as_bytes();
    assert_eq!(
        decode_with_codepage(utf8, 936),
        "中文文件名.txt 与 English 混排"
    );
}

#[test]
fn gbk_bytes_recovered_under_cp936() {
    assert_eq!(decode_with_codepage(GBK_NIHAO, 936), "你好");
}

#[test]
fn gbk_bytes_with_ascii_prefix_recovered_under_cp936() {
    // dir 输出形态：ASCII 骨架 + GBK 文件名。GBK 内 ASCII 透明。
    // 注意：若骨架本身是 UTF-8 中文则整体合法 UTF-8 直通——这里是纯
    // ASCII 骨架 + GBK 文件名形态。
    let mut bytes = b"2026-11-15  10:00    <DIR>          ".to_vec();
    bytes.extend_from_slice(GBK_NIHAO);
    bytes.extend_from_slice(b".txt\r\n");
    let out = decode_with_codepage(&bytes, 936);
    assert!(out.contains("你好.txt"), "got: {out}");
    assert!(out.starts_with("2026-11-15"));
}

#[test]
fn non_cjk_codepage_honest_lossy_fallback() {
    // CP437（美式 OEM）无 CJK 解码器：GBK 字节诚实 lossy（现状语义），
    // 不猜不解。
    let out = decode_with_codepage(GBK_NIHAO, 437);
    assert!(out.contains('\u{FFFD}'));
}

#[test]
fn utf8_codepage_invalid_bytes_fallback_lossy() {
    // CP65001 传非法 UTF-8：无 65001 解码器条目 → lossy（不二次当 UTF-8）。
    let out = decode_with_codepage(GBK_NIHAO, 65001);
    assert!(out.contains('\u{FFFD}'));
}

#[test]
fn shift_jis_codepage_decodes() {
    // "こんにちは" Shift_JIS: こ=82B1 ん=82F1 に=82C9 ち=82BF は=82CD
    let sjis: &[u8] = &[0x82, 0xB1, 0x82, 0xF1, 0x82, 0xC9, 0x82, 0xBF, 0x82, 0xCD];
    assert_eq!(decode_with_codepage(sjis, 932), "こんにちは");
}

#[test]
fn empty_bytes_round_trip() {
    assert_eq!(decode_with_codepage(&[], 936), "");
    assert_eq!(decode_child_output(&[]), "");
}

#[test]
fn machine_wrapper_ascii_passthrough() {
    // 机器面包装：ASCII 在任何 OEMCP 下语义一致（机器无关断言）。
    assert_eq!(
        decode_child_output(b"plain ascii output"),
        "plain ascii output"
    );
}

#[test]
fn truncated_multibyte_tail_degrades_without_panic() {
    // 管道字节窗口截尾场景（exec 预览窗按字节截断）：GBK 尾字节被截断，
    // 解码降级不出 panic、出替换符。
    let truncated = &GBK_NIHAO[..3]; // 你 完整 + 好 的首字节
    let out = decode_with_codepage(truncated, 936);
    assert!(out.starts_with("你"));
}
