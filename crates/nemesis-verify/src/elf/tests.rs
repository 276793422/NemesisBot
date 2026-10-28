//! ELF codec 单测（M5 补测，quality-hardening goal 2026-08-25）。
//!
//! 与 pe/tests.rs 同理：此前 elf.rs 0 单测（基线 §9.3 可疑点）。手工构造
//! 最小 ELF 字节流（ELF64 LE / ELF32 BE 双轴），钉住 L 的两个来源
//! （PT_LOAD 段末尾 / section header table 末尾）、字节序处理、哈希
//! 前缀语义与全部错误路径。

use super::*;
use crate::codec::{CodecError, ElfCodec, ExecutableCodec};

fn put16(b: &mut [u8], off: usize, v: u16, le: bool) {
    let a = if le { v.to_le_bytes() } else { v.to_be_bytes() };
    b[off..off + 2].copy_from_slice(&a);
}
fn put32(b: &mut [u8], off: usize, v: u32, le: bool) {
    let a = if le { v.to_le_bytes() } else { v.to_be_bytes() };
    b[off..off + 4].copy_from_slice(&a);
}
fn put64(b: &mut [u8], off: usize, v: u64, le: bool) {
    let a = if le { v.to_le_bytes() } else { v.to_be_bytes() };
    b[off..off + 8].copy_from_slice(&a);
}

/// phdr 描述：(p_type, p_offset, p_filesz)。
struct ElfSpec {
    is64: bool,
    le: bool,
    phdrs: Vec<(u32, u64, u64)>,
    /// (e_shoff, e_shentsize, e_shnum)；None = shoff=0（无 section 表）。
    shdr: Option<(u64, u64, u64)>,
    /// 强制文件总长（模拟 overlay / 截断）；None = 按结构自然长度。
    force_len: Option<usize>,
}

/// 构造最小 ELF：e_ident（class/data 按 spec）+ 定长头字段 +
/// program header 数组 + 可选 section header 表占位。
fn build_elf(spec: &ElfSpec) -> Vec<u8> {
    let hdr_len = if spec.is64 { 64 } else { 52 };
    let phoff = hdr_len as u64;
    let phentsize: u64 = if spec.is64 { 56 } else { 32 };
    let shentsize = spec
        .shdr
        .map(|(_, se, _)| se)
        .unwrap_or(if spec.is64 { 64 } else { 40 });
    let shnum = spec.shdr.map(|(_, _, sn)| sn).unwrap_or(0);
    let shoff = spec.shdr.map(|(so, _, _)| so).unwrap_or(0);

    let mut len = (phoff as usize) + spec.phdrs.len() * phentsize as usize;
    for &(_, off, sz) in &spec.phdrs {
        len = len.max((off + sz) as usize);
    }
    if shoff > 0 {
        len = len.max((shoff + shentsize * shnum) as usize);
    }
    if let Some(fl) = spec.force_len {
        len = fl;
    }
    let mut b = vec![0u8; len.max(hdr_len)];
    b[0] = 0x7f;
    b[1] = b'E';
    b[2] = b'L';
    b[3] = b'F';
    b[4] = if spec.is64 { 2 } else { 1 };
    b[5] = if spec.le { 1 } else { 2 };
    let le = spec.le;
    if spec.is64 {
        put64(&mut b, 32, phoff, le); // e_phoff
        put64(&mut b, 40, shoff, le); // e_shoff
        put16(&mut b, 54, phentsize as u16, le);
        put16(&mut b, 56, spec.phdrs.len() as u16, le); // e_phnum
        put16(&mut b, 58, shentsize as u16, le);
        put16(&mut b, 60, shnum as u16, le);
    } else {
        put32(&mut b, 28, phoff as u32, le);
        put32(&mut b, 32, shoff as u32, le);
        put16(&mut b, 42, phentsize as u16, le);
        put16(&mut b, 44, spec.phdrs.len() as u16, le);
        put16(&mut b, 46, shentsize as u16, le);
        put16(&mut b, 48, shnum as u16, le);
    }
    for (i, &(ptype, off, sz)) in spec.phdrs.iter().enumerate() {
        let ph = phoff as usize + i * phentsize as usize;
        put32(&mut b, ph, ptype, le);
        if spec.is64 {
            put64(&mut b, ph + 8, off, le);
            put64(&mut b, ph + 32, sz, le);
        } else {
            put32(&mut b, ph + 4, off as u32, le);
            put32(&mut b, ph + 16, sz as u32, le);
        }
    }
    b
}

#[test]
fn compute_l_is_max_pt_load_end_elf64_le() {
    // 两个 PT_LOAD（ends 0x100 / 0x280）+ 一个 PT_NULL（跳过）→ L=0x280
    let elf = build_elf(&ElfSpec {
        is64: true,
        le: true,
        phdrs: vec![(1, 0, 0x100), (1, 0x200, 0x80), (0, 0x500, 0x100)],
        shdr: None,
        force_len: None,
    });
    assert_eq!(ElfCodec.compute_l(&elf).unwrap(), Some(0x280));
}

#[test]
fn compute_l_section_table_dominates_elf32_be() {
    // ELF32 大端（双轴覆盖）：PT_LOAD end 0x100，但 section header table
    // 末尾 0x180+2*40=0x1D0 更靠后 → L=0x1D0（多源综合取 max）
    let elf = build_elf(&ElfSpec {
        is64: false,
        le: false,
        phdrs: vec![(1, 0, 0x100)],
        shdr: Some((0x180, 40, 2)),
        force_len: None,
    });
    assert_eq!(ElfCodec.compute_l(&elf).unwrap(), Some(0x1D0));
}

#[test]
fn compute_l_zero_shoff_ignored_and_l_cannot_exceed_file() {
    // shoff=0 → section 表不参与
    let elf = build_elf(&ElfSpec {
        is64: true,
        le: true,
        phdrs: vec![(1, 0, 0x100)],
        shdr: None,
        force_len: None,
    });
    assert_eq!(ElfCodec.compute_l(&elf).unwrap(), Some(0x100));

    // L 超过文件长度 → Malformed（结构自相矛盾，不是静默截断）
    let over = build_elf(&ElfSpec {
        is64: true,
        le: true,
        phdrs: vec![(1, 0, 0x1000)],
        shdr: None,
        force_len: Some(0x100),
    });
    assert!(matches!(
        ElfCodec.compute_l(&over),
        Err(CodecError::Malformed(_))
    ));
}

#[test]
fn content_hash_is_sha256_of_prefix() {
    let elf = build_elf(&ElfSpec {
        is64: true,
        le: true,
        phdrs: vec![(1, 0, 0x100)],
        shdr: None,
        force_len: Some(0x180), // 带 overlay：L=0x100 < 文件长
    });
    let h = ElfCodec.content_hash(&elf, 0x100).unwrap();
    let expected: [u8; 32] = Sha256::digest(&elf[..0x100]).into();
    assert_eq!(h, expected, "无排除字段：整段 [0,L) SHA-256");

    assert!(matches!(
        ElfCodec.content_hash(&elf, elf.len() + 1),
        Err(CodecError::Malformed(_))
    ));
}

#[test]
fn compute_l_elf64_be_reads_u64_fields_big_endian() {
    // ELF64 大端：e_phoff / p_offset / p_filesz / e_shoff 全部走 u64 from_be
    // 臂（现有测试只覆盖 ELF64 LE 与 ELF32 BE，本轴补齐）。
    // PT_LOAD end = 0x300；section 表 0x100+1*64=0x140 → L=0x300。
    let elf = build_elf(&ElfSpec {
        is64: true,
        le: false,
        phdrs: vec![(1, 0x300, 0x00)],
        shdr: Some((0x100, 64, 1)),
        force_len: None,
    });
    assert_eq!(ElfCodec.compute_l(&elf).unwrap(), Some(0x300));
}

#[test]
fn elf_error_paths() {
    // < 0x40 → Truncated
    assert!(matches!(
        ElfCodec.compute_l(b"\x7fELF".as_slice()),
        Err(CodecError::Truncated)
    ));
    // 非 ELF 魔数（但 ≥0x40 字节）→ NotAnExecutable
    assert!(matches!(
        ElfCodec.compute_l(&[0u8; 0x80]),
        Err(CodecError::NotAnExecutable)
    ));
    // EI_CLASS 非法（3）→ UnsupportedElfClass
    let mut bad_class = build_elf(&ElfSpec {
        is64: true,
        le: true,
        phdrs: vec![],
        shdr: None,
        force_len: None,
    });
    bad_class[4] = 3;
    assert!(matches!(
        ElfCodec.compute_l(&bad_class),
        Err(CodecError::UnsupportedElfClass(3))
    ));
    // EI_DATA 非法（7）→ UnsupportedElfData
    let mut bad_data = build_elf(&ElfSpec {
        is64: true,
        le: true,
        phdrs: vec![],
        shdr: None,
        force_len: None,
    });
    bad_data[5] = 7;
    assert!(matches!(
        ElfCodec.compute_l(&bad_data),
        Err(CodecError::UnsupportedElfData(7))
    ));
}

// ---------------------------------------------------------------------------
// R196 红证（BUG-20261028-130）：L 计算三处裸加法在 u64 全域字段下溢出
// ——debug 构建 panic（`attempt to add with overflow`），release 回绕后
// 语义错（落点漂移 / L 谎报）。位点：phoff+i*phentsize、p_offset+p_filesz、
// shoff+shentsize*shnum，外加第四位点 = 读侧 u16/u32/u64 helper 自身的
// `off..off + N` range 构造（位点 1 红证的实际炸点：phoff=usize::MAX 时
// i=0 的 helper 在 off+4 处 debug panic，早于任何语义判断）。
// ---------------------------------------------------------------------------

/// 绿证（回归锁）：三处裸加法 checked 后诚实失败；读侧 helper 两段式 get
/// 零算术后巨偏移诚实 FieldOutOfBounds。
/// 可达性事实（红证复盘）：位点 2/3 的溢出真实可达（字段读成功后加法炸，
/// debug 构建旧代码 panic）；位点 1（phoff 落点）的 checked 溢出分支
/// **运行时不可达**——phoff 巨大时 i=0 的读侧 helper 先失败（修复前是
/// helper 内 off+4 debug panic，修复后诚实 FieldOutOfBounds），永远走不到
/// i=1 的落点加法。checked_add/mul 保留为 belt-and-braces（防未来循环
/// 重构改读序）。
#[test]
fn elf_header_arithmetic_overflow_honest_failure() {
    // 位点 1：e_phoff=usize::MAX + e_phnum=2 → i=0 读侧诚实 FieldOutOfBounds
    //（非溢出臂——溢出臂不可达，见上）
    let mut b1 = vec![0u8; 64];
    b1[0] = 0x7f;
    b1[1] = b'E';
    b1[2] = b'L';
    b1[3] = b'F';
    b1[4] = 2; // ELFCLASS64
    b1[5] = 1; // LE
    put64(&mut b1, 32, u64::MAX, true); // e_phoff
    put16(&mut b1, 54, 56, true); // e_phentsize
    put16(&mut b1, 56, 2, true); // e_phnum = 2
    assert!(matches!(
        ElfCodec.compute_l(&b1),
        Err(CodecError::FieldOutOfBounds(_))
    ));

    // 位点 2：PT_LOAD p_offset=u64::MAX + p_filesz=2 → 段末加法溢出 → Malformed
    let mut b2 = vec![0u8; 120];
    b2[0] = 0x7f;
    b2[1] = b'E';
    b2[2] = b'L';
    b2[3] = b'F';
    b2[4] = 2;
    b2[5] = 1;
    put64(&mut b2, 32, 64, true); // e_phoff = 64
    put16(&mut b2, 54, 56, true); // e_phentsize
    put16(&mut b2, 56, 1, true); // e_phnum = 1
    put32(&mut b2, 64, 1, true); // p_type = PT_LOAD
    put64(&mut b2, 72, u64::MAX, true); // p_offset
    put64(&mut b2, 96, 2, true); // p_filesz
    assert!(matches!(
        ElfCodec.compute_l(&b2),
        Err(CodecError::Malformed(_))
    ));

    // 位点 3：e_shoff=u64::MAX + 64*1 → 表末加法溢出 → Malformed
    let mut b3 = vec![0u8; 64];
    b3[0] = 0x7f;
    b3[1] = b'E';
    b3[2] = b'L';
    b3[3] = b'F';
    b3[4] = 2;
    b3[5] = 1;
    put64(&mut b3, 40, u64::MAX, true); // e_shoff
    put16(&mut b3, 58, 64, true); // e_shentsize
    put16(&mut b3, 60, 1, true); // e_shnum
    assert!(matches!(
        ElfCodec.compute_l(&b3),
        Err(CodecError::Malformed(_))
    ));
}
