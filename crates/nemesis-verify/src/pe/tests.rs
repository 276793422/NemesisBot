//! PE codec 单测（M5 补测，quality-hardening goal 2026-08-25）。
//!
//! 此前 pe.rs 0 单测（基线 §9.3 可疑点：T1-T5 端到端跑通走的是
//! exe-sign-tool/verify-loader **手工 bin**，cargo 测试从不经过 PE 解析）。
//! 这里用手工构造的最小 PE 字节流直接钉住布局解析的每个决策点：
//! L 的多源综合（section 末尾 / section table 兜底 / 文件长度截断）、
//! content_hash 的两个易变字段排除（CheckSum / Security 目录项）、
//! Authenticode 区域暴露与交叉校验、全部错误路径。

use crate::codec::{CodecError, ExecutableCodec, PeCodec};

const P: usize = 0x40; // e_lfanew

fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

struct PeSpec {
    /// PE32+（0x20b）还是 PE32（0x10b）。
    plus: bool,
    /// (SizeOfRawData, PointerToRawData) 列表。
    sections: Vec<(u32, u32)>,
    /// DataDirectory[4] (VA, Size)；None = 不写。
    security: Option<(u32, u32)>,
    /// 强制文件总长（模拟 overlay 或截断文件）；None = 按结构自然长度。
    force_len: Option<usize>,
}

/// 构造最小合法 PE：MZ + e_lfanew + PE\0\0 + COFF + Optional header
/// （含非零 CheckSum=0xDEADBEEF、nrva=16）+ section table（每项只写
/// raw size/ptr）+ 可选 Security 目录项。所有多字节字段 LE。
fn build_pe(spec: &PeSpec) -> Vec<u8> {
    let size_of_opt: usize = if spec.plus { 240 } else { 224 };
    let sec_tbl = P + 24 + size_of_opt;
    let mut len = sec_tbl + spec.sections.len() * 40;
    for &(size, ptr) in &spec.sections {
        if size > 0 {
            len = len.max((ptr + size) as usize);
        }
    }
    if let Some((va, sz)) = spec.security
        && sz > 0
    {
        len = len.max((va + sz) as usize);
    }
    if let Some(fl) = spec.force_len {
        len = fl;
    }
    assert!(
        len >= 0x40 + 24 + size_of_opt + spec.sections.len() * 40,
        "file must cover headers"
    );
    let mut b = vec![0u8; len];
    b[0] = b'M';
    b[1] = b'Z';
    put32(&mut b, 0x3C, P as u32);
    b[P..P + 4].copy_from_slice(b"PE\0\0");
    put16(&mut b, P + 6, spec.sections.len() as u16);
    put16(&mut b, P + 20, size_of_opt as u16);
    put16(&mut b, P + 24, if spec.plus { 0x20b } else { 0x10b });
    put32(&mut b, P + 88, 0xDEADBEEF); // CheckSum 非零（排除才可观测）
    let (nrva_off, dd_start) = if spec.plus {
        (P + 132, P + 136)
    } else {
        (P + 116, P + 120)
    };
    put32(&mut b, nrva_off, 16);
    if let Some((va, sz)) = spec.security {
        put32(&mut b, dd_start + 32, va);
        put32(&mut b, dd_start + 36, sz);
    }
    for (i, &(size, ptr)) in spec.sections.iter().enumerate() {
        let s = sec_tbl + i * 40;
        put32(&mut b, s + 16, size);
        put32(&mut b, s + 20, ptr);
    }
    b
}

fn one_section_pe(plus: bool) -> Vec<u8> {
    build_pe(&PeSpec {
        plus,
        sections: vec![(0x100, 0x200)], // raw end 0x300
        security: None,
        force_len: None,
    })
}

#[test]
fn compute_l_is_max_section_raw_end() {
    let pe = one_section_pe(false);
    assert_eq!(PeCodec.compute_l(&pe).unwrap(), Some(0x300));

    // 两个 section：第二个更靠后 → L 取 max
    let pe2 = build_pe(&PeSpec {
        plus: false,
        sections: vec![(0x100, 0x200), (0x50, 0x400)], // ends 0x300 / 0x450
        security: None,
        force_len: None,
    });
    assert_eq!(PeCodec.compute_l(&pe2).unwrap(), Some(0x450));

    // PE32+ 变体（magic 0x20b，不同的 nrva/datadir 偏移）
    let pe_plus = one_section_pe(true);
    assert_eq!(PeCodec.compute_l(&pe_plus).unwrap(), Some(0x300));
}

#[test]
fn compute_l_zero_size_sections_fall_back_to_section_table_end() {
    // SizeOfRawData=0 的 section 被跳过 → L 兜底到 section table 末尾
    let pe = build_pe(&PeSpec {
        plus: false,
        sections: vec![(0, 0x200)], // zero-size → skipped
        security: None,
        force_len: None,
    });
    let sec_tbl_end = P + 24 + 224 + 40; // 0x160
    assert_eq!(PeCodec.compute_l(&pe).unwrap(), Some(sec_tbl_end));
}

#[test]
fn compute_l_capped_at_file_len() {
    // section 声称 raw end 0x300，但文件只有 0x200 → L 截到文件长度
    let pe = build_pe(&PeSpec {
        plus: false,
        sections: vec![(0x100, 0x200)],
        security: None,
        force_len: Some(0x200),
    });
    assert_eq!(PeCodec.compute_l(&pe).unwrap(), Some(0x200));
}

#[test]
fn content_hash_excludes_checksum_and_security_dir() {
    // Security Size=0（auth_region None），但目录项 8B 仍在排除区间。
    // 翻转 CheckSum / Security 目录项内的字节 → 哈希不变；
    // 翻其他 header 字节 → 哈希变。
    let mut pe = build_pe(&PeSpec {
        plus: false,
        sections: vec![(0x100, 0x200)],
        security: Some((0, 0)),
        force_len: None,
    });
    let l = 0x300;
    let base = PeCodec.content_hash(&pe, l).unwrap();

    // CheckSum 区间 [0x98, 0x9C) 的首尾字节
    for off in [P + 88, P + 91] {
        pe[off] ^= 0xFF;
        assert_eq!(
            PeCodec.content_hash(&pe, l).unwrap(),
            base,
            "CheckSum byte @{off} excluded"
        );
        pe[off] ^= 0xFF;
    }
    // Security 目录项 [dd+32, dd+40)：VA 字节翻转（Size 仍 0 → 区域仍 None）
    for off in [P + 120 + 32, P + 120 + 33] {
        pe[off] ^= 0xFF;
        assert_eq!(
            PeCodec.content_hash(&pe, l).unwrap(),
            base,
            "Security dir byte @{off} excluded"
        );
        pe[off] ^= 0xFF;
    }
    // 对照：非排除区字节（DOS header / DataDirectory[3] 末字节）→ 哈希变
    for off in [0x10, P + 120 + 31] {
        pe[off] ^= 0xFF;
        assert_ne!(
            PeCodec.content_hash(&pe, l).unwrap(),
            base,
            "byte @{off} must affect hash"
        );
        pe[off] ^= 0xFF;
    }
    // PE32+ 的 Security 目录项在 P+136+32
    let mut pe_plus = build_pe(&PeSpec {
        plus: true,
        sections: vec![(0x80, 0x200)],
        security: Some((0, 0)),
        force_len: None,
    });
    let base_plus = PeCodec.content_hash(&pe_plus, 0x280).unwrap();
    pe_plus[P + 136 + 32] ^= 0xFF;
    assert_eq!(
        PeCodec.content_hash(&pe_plus, 0x280).unwrap(),
        base_plus,
        "PE32+ security dir excluded"
    );
}

#[test]
fn content_hash_rejects_l_beyond_file() {
    let pe = one_section_pe(false);
    assert!(matches!(
        PeCodec.content_hash(&pe, pe.len() + 1),
        Err(CodecError::Malformed(_))
    ));
}

#[test]
fn overlay_excludes_reports_auth_region_and_tolerates_garbage() {
    // auth 区域 = (VA, VA+Size)，要求 VA >= L 且 end <= file len
    let mut spec = PeSpec {
        plus: false,
        sections: vec![(0x100, 0x200)], // L = 0x300
        security: Some((0x310, 0x20)),  // overlay 内合法区域
        force_len: None,
    };
    let pe = build_pe(&spec); // file len 覆盖 0x330
    assert_eq!(PeCodec.overlay_excludes(&pe), vec![(0x310, 0x330)]);

    // Size=0 → 无 auth 区域
    spec.security = Some((0, 0));
    let pe0 = build_pe(&spec);
    assert!(PeCodec.overlay_excludes(&pe0).is_empty());

    // 解析失败（截断）→ 容错空（不 panic、不 Err）
    let garbage = b"MZ\x00\x00".to_vec();
    assert!(PeCodec.overlay_excludes(&garbage).is_empty());

    // 解析失败但报错路径对外可见：compute_l 必须 Err
    assert!(PeCodec.compute_l(&garbage).is_err());
}

/// `NumberOfRvaAndSizes < 5`（无 Security 目录项）是合法 PE 形态：
/// security_dir_range / auth_region 双双走 None 臂（S6 覆盖率批次）。
#[test]
fn nrva_below_5_means_no_security_directory() {
    let mut pe = one_section_pe(false);
    put32(&mut pe, P + 116, 4); // NumberOfRvaAndSizes = 4（< 5）

    // L 不受影响（section 决定）。
    assert_eq!(PeCodec.compute_l(&pe).unwrap(), Some(0x300));

    // 无 Security 目录项 → 排除区只剩 CheckSum；dd+32 处字节翻转必须影响哈希
    // （与 nrva=16 时"被排除、哈希不变"形成对照，钉死 None 臂语义）。
    let l = 0x300;
    let base = PeCodec.content_hash(&pe, l).unwrap();
    pe[P + 120 + 32] ^= 0xFF;
    assert_ne!(
        PeCodec.content_hash(&pe, l).unwrap(),
        base,
        "nrva<5 时 dd[4] 位置是普通字节，必须计入哈希"
    );
    pe[P + 120 + 32] ^= 0xFF;
    // CheckSum 仍被排除。
    pe[P + 88] ^= 0xFF;
    assert_eq!(PeCodec.content_hash(&pe, l).unwrap(), base);
    pe[P + 88] ^= 0xFF;

    // 无 auth 区域 → overlay_excludes 空。
    assert!(PeCodec.overlay_excludes(&pe).is_empty());
}

#[test]
fn parse_error_paths() {
    let codec = PeCodec;
    // < 0x40 字节 → Truncated
    assert!(matches!(
        codec.compute_l(b"MZ".as_slice()),
        Err(CodecError::Truncated)
    ));
    // 非 MZ 开头 → NotAnExecutable
    let not_mz = vec![0u8; 0x80];
    assert!(matches!(
        codec.compute_l(&not_mz),
        Err(CodecError::NotAnExecutable)
    ));
    // MZ 但 e_lfanew 指向处无 PE 签名 → NotAnExecutable
    let mut no_sig = one_section_pe(false);
    no_sig[P + 1] = b'X'; // "PE\0\0" → "PX\0\0"
    assert!(matches!(
        codec.compute_l(&no_sig),
        Err(CodecError::NotAnExecutable)
    ));
    // 未知 Optional Header Magic
    let mut bad_magic = one_section_pe(false);
    bad_magic[P + 24] = 0x99;
    bad_magic[P + 25] = 0x09;
    assert!(matches!(
        codec.compute_l(&bad_magic),
        Err(CodecError::UnknownOptionalHeaderMagic(0x0999))
    ));
    // 交叉校验：auth VA < L → Malformed
    let va_below = build_pe(&PeSpec {
        plus: false,
        sections: vec![(0x100, 0x200)], // L=0x300
        security: Some((0x100, 0x20)),  // VA 0x100 < L
        force_len: None,
    });
    assert!(matches!(
        codec.compute_l(&va_below),
        Err(CodecError::Malformed(_))
    ));
    // 交叉校验：auth end > file len → Malformed
    let end_over = build_pe(&PeSpec {
        plus: false,
        sections: vec![(0x100, 0x200)],
        security: Some((0x310, 0x1000)),
        force_len: Some(0x340),
    });
    assert!(matches!(
        codec.compute_l(&end_over),
        Err(CodecError::Malformed(_))
    ));
    // 字段越界（num_sections 巨大 → section table 读越界）
    let mut huge_sections = one_section_pe(false);
    put16(&mut huge_sections, P + 6, 0xFFFF);
    assert!(codec.compute_l(&huge_sections).is_err());
}
