//! codec 多态分派单测（M5 补测，quality-hardening goal 2026-08-25）：
//! 魔数探测（detect_format）、codec 选取（detect_codec）、RawCodec 语义。

use super::*;

#[test]
fn detect_format_by_magic() {
    assert_eq!(detect_format(b"MZ\x90\x00"), FORMAT_TAG_PE);
    assert_eq!(detect_format(b"\x7fELF\x02\x01"), FORMAT_TAG_ELF);
    assert_eq!(detect_format(b"#!/bin/sh\n"), FORMAT_TAG_RAW);
    assert_eq!(detect_format(b""), FORMAT_TAG_RAW, "空串兜底 Raw");
}

#[test]
fn detect_codec_dispatches_by_behavior() {
    // MZ → PeCodec（用错误形状区分：PE 解析错误 vs Raw 的 Ok(None)）
    let mz_garbage = b"MZ".as_slice(); // 截断 PE → Err(Truncated)
    let pe_codec = detect_codec(mz_garbage);
    assert!(
        pe_codec.compute_l(mz_garbage).is_err(),
        "MZ dispatches to PeCodec"
    );
    // PE32+ 变体的 MZ 同样进 PE codec
    let mz2 = b"MZ\x00\x00\x00\x00".as_slice();
    assert!(detect_codec(mz2).compute_l(mz2).is_err());

    // \x7fELF → ElfCodec（同样用错误形状区分）
    let elf_garbage = b"\x7fELF".as_slice(); // 截断 → Err(Truncated)
    let elf_codec = detect_codec(elf_garbage);
    assert!(
        elf_codec.compute_l(elf_garbage).is_err(),
        "ELF dispatches to ElfCodec"
    );

    // 其他 → RawCodec（compute_l 恒 Ok(None)——无结构化长度标记）
    let raw = b"plain payload bytes".as_slice();
    let raw_codec = detect_codec(raw);
    assert_eq!(raw_codec.compute_l(raw).unwrap(), None);
}

#[test]
fn raw_codec_hashes_prefix_and_bounds_checks() {
    let data = b"0123456789abcdef".as_slice();
    // 全长
    let full = RawCodec.content_hash(data, data.len()).unwrap();
    let expected: [u8; 32] = Sha256::digest(data).into();
    assert_eq!(full, expected);
    // 前缀
    let half = RawCodec.content_hash(data, 8).unwrap();
    let expected_half: [u8; 32] = Sha256::digest(&data[..8]).into();
    assert_eq!(half, expected_half);
    // 越界拒绝
    let err = RawCodec.content_hash(data, data.len() + 1);
    assert!(matches!(err, Err(CodecError::Malformed(_))));
    // Raw 无 overlay 排除区（trait 默认空实现）
    assert!(RawCodec.overlay_excludes(data).is_empty());
}

#[test]
fn codec_error_display_is_debug_string() {
    // Display 直接转发 Debug（锚定当前语义：错误信息 = 变体 Debug 形态）。
    assert_eq!(CodecError::NotAnExecutable.to_string(), "NotAnExecutable");
    assert_eq!(CodecError::Truncated.to_string(), "Truncated");
    assert_eq!(
        CodecError::FieldOutOfBounds("e_phoff").to_string(),
        r#"FieldOutOfBounds("e_phoff")"#
    );
    assert_eq!(
        CodecError::UnknownOptionalHeaderMagic(0x10b).to_string(),
        "UnknownOptionalHeaderMagic(267)"
    );
    assert_eq!(
        CodecError::Malformed("ELF L 5 > file len 4".into()).to_string(),
        r#"Malformed("ELF L 5 > file len 4")"#
    );
    // 实现 std::error::Error（可装箱进 anyhow）
    let _: &dyn std::error::Error = &CodecError::Truncated;
}
