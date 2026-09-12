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

// ---------------------------------------------------------------------------
// S3-1：Authenticode byte-range digest（v4 语义：两字段整体跳过 + 证书块排除 +
// 无 L 上界；与 v3 content_hash 的差异只在无 L 截断 + 证书块排除，参照交叉验证钉死）
// ---------------------------------------------------------------------------

use crate::pe::authenticode_digest;
use sha2::{Digest, Sha256};

/// 给「不参与结构」的区域填可辨识字节（不碰任何 PE 头字段）。
fn fill_ranges(b: &mut [u8], ranges: &[(usize, usize)]) {
    for &(s, e) in ranges {
        for (i, x) in b[s..e].iter_mut().enumerate() {
            *x = ((s + i) % 251) as u8;
        }
    }
}

#[test]
fn authenticode_digest_semantics_pinned_by_reference_reimpl() {
    // PE32+，1 section（L=0x300），证书表 [0x400,0x440) 落 overlay 中段，
    // 证书表之后还有 0x20 尾巴——五个参与区段 + 三个特殊区一次钉死。
    let pe = build_pe(&PeSpec {
        plus: true,
        sections: vec![(0x100, 0x200)],
        security: Some((0x400, 0x40)),
        force_len: Some(0x460),
    });
    let mut pe = pe;
    fill_ranges(
        &mut pe,
        &[(0x10, 0x3C), (0x200, 0x300), (0x330, 0x400), (0x400, 0x460)],
    );
    // CheckSum 与 Security 表项保持 0（置零参与语义下，与任何值等价——见对照断言）

    let got = authenticode_digest(&pe).unwrap();
    // 测试内参照重实现（逐区段手拼，Authenticode 实证语义）：
    // [0,cs) ++ [ce,ss) ++ [se,va) ++ [va+size,end)——CheckSum 4B 与 Security 表项
    // 8B 整体跳过（不是置零参与！参照签名内嵌 digest 只与跳过变体逐字节一致，
    // 见 s31_cross_validate_reference_pes），证书块整体排除。
    let (cs, ce) = (P + 88, P + 92); // CheckSum [0x68,0x6C)
    let (ss, se) = (P + 136 + 32, P + 136 + 40); // Security 表项 [0xE8,0xF0)
    let mut h = Sha256::new();
    h.update(&pe[..cs]);
    h.update(&pe[ce..ss]); // CheckSum 跳过
    h.update(&pe[se..0x400]); // Security 表项跳过；overlay 到证书表前全参与
    h.update(&pe[0x440..]); // 证书表之后的尾巴参与
    let expect: [u8; 32] = h.finalize().into();
    assert_eq!(got, expect, "digest 语义 = 测试内参照重实现");

    // 跳过语义对照：CheckSum 填任意值 → digest 不变（字节整体不进哈希）。
    // 注意 Security 表项不能这样测——表项值定义证书区位置，乱值会改变排除区/破坏
    // 解析；表项「被跳过」的钉死靠上面参照重实现里的跳段 +
    // authenticode_digest_security_entry_excluded_from_hash。
    let mut pe2 = pe.clone();
    pe2[cs..ce].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
    assert_eq!(authenticode_digest(&pe2).unwrap(), got);
    // 但字段之外的字节（DOS stub）必须影响 digest
    let mut pe3 = pe;
    pe3[0x10] ^= 0xFF;
    assert_ne!(authenticode_digest(&pe3).unwrap(), got);
}

#[test]
fn authenticode_digest_security_entry_excluded_from_hash() {
    // 无证书数据（Size=0 → 无排除区）时，Security 表项 VA 字节无论存什么值
    // 都被整体跳过（不进哈希）→ digest 不变（若误按原值参与，此处必不等）。
    let mut pe = build_pe(&PeSpec {
        plus: true,
        sections: vec![(0x100, 0x200)],
        security: Some((0, 0)),
        force_len: None,
    });
    let base = authenticode_digest(&pe).unwrap();
    put32(&mut pe, P + 136 + 32, 0xDEADBEEF); // VA 乱值；Size 仍 0
    assert_eq!(
        authenticode_digest(&pe).unwrap(),
        base,
        "Security 表项字节必须被跳过（不参与哈希）"
    );
}

#[test]
fn authenticode_digest_excludes_cert_block_only() {
    // 证书表 [0x400,0x440)：**内部字节翻转不影响 digest**；
    // overlay 前段 [0x330..0x400) 与尾巴 [0x440..0x460) 翻转**必须影响**。
    let mut pe = build_pe(&PeSpec {
        plus: true,
        sections: vec![(0x100, 0x200)],
        security: Some((0x400, 0x40)),
        force_len: Some(0x460),
    });
    fill_ranges(&mut pe, &[(0x330, 0x400), (0x400, 0x460)]);
    let base = authenticode_digest(&pe).unwrap();

    for off in [0x400, 0x420, 0x43F] {
        pe[off] ^= 0xFF;
        assert_eq!(
            authenticode_digest(&pe).unwrap(),
            base,
            "证书表内字节 @{off:#x} 必须被排除"
        );
        pe[off] ^= 0xFF;
    }
    for off in [0x330, 0x3FF, 0x440, 0x45F] {
        pe[off] ^= 0xFF;
        assert_ne!(
            authenticode_digest(&pe).unwrap(),
            base,
            "证书表外字节 @{off:#x} 必须参与 digest"
        );
        pe[off] ^= 0xFF;
    }
}

#[test]
fn authenticode_digest_nrva_below_5_hashes_everything_after_checksum() {
    // nrva=4：无 Security 表项（没有「置零参与」区），CheckSum 之后全文件参与
    let mut pe = one_section_pe(true);
    put32(&mut pe, P + 132, 4); // NumberOfRvaAndSizes = 4
    pe.extend(std::iter::repeat_n(0xABu8, 0x40)); // 模拟 overlay
    let base = authenticode_digest(&pe).unwrap();

    pe[P + 120 + 32] ^= 0xFF; // dd[4] 位置是普通字节
    assert_ne!(
        authenticode_digest(&pe).unwrap(),
        base,
        "nrva<5 时无置零区，dd[4] 位置必须参与"
    );
    pe[P + 120 + 32] ^= 0xFF;
    pe[P + 88] ^= 0xFF; // CheckSum 仍被跳过
    assert_eq!(authenticode_digest(&pe).unwrap(), base);
}

#[test]
fn authenticode_digest_rejects_unparseable() {
    assert!(authenticode_digest(b"MZ\x00\x00").is_err());
    let not_mz = vec![0u8; 0x80];
    assert!(authenticode_digest(&not_mz).is_err());
}

// ---------------------------------------------------------------------------
// S3-1 参照交叉验证（goal 判据：本实现 digest 与 signtool/osslsigncode 签的
// 参照 PE 内嵌 messageDigest 逐字节一致——先绿才准进 S3-2）
//
// 参照物在仓库外 spike 目录（默认 C:\AI\NemesisBot\Logs\2026-09-12_authenticode-spike，
// 可用 NEMESIS_AUTHENTICODE_SPIKE 覆盖）；`--ignored` 显式运行。
// 清单：R1-R3 = 存量 osslsigncode 签名（FileAlign=0x200 PE32+）；
// R4 = signtool 直签（spike leaf PFX；SignerInfo.signatureAlgorithm =
// id-ecPublicKey 怪癖参照）；R5 = mingw 构建异构形态（17 sections）；
// R6 = 签名前追加 4KB 随机 overlay（真 overlay 形态）；
// 负控 = test-tampered.exe（签名后翻 0x1000 处 1 字节）。
// ---------------------------------------------------------------------------

use crate::envelope::parse_signed_data;

fn spike_dir() -> String {
    std::env::var("NEMESIS_AUTHENTICODE_SPIKE")
        .unwrap_or_else(|_| r"C:\AI\NemesisBot\Logs\2026-09-12_authenticode-spike".to_string())
}

/// DER 外层 TLV 总长（头 + 内容）；坏头/越界返回 None。
///
/// 真实发现（S3-1 交叉验证）：osslsigncode 的 WIN_CERTIFICATE.bCertificate 在
/// 提取 Security Directory 指向的 WIN_CERTIFICATE 链里全部 bCertificate
/// （CMS ContentInfo DER，按外层 TLV 长度裁掉条目尾部 padding）。
/// 链式布局：dwLength(4) + wRevision(2) + wCertificateType(2)
/// + bCertificate(dwLength-8)，下一项按 8 字节对齐。
///
/// （S3-3 起 [`super::read_certificate_entries`] 为产品化版本；本辅助保留给
/// S3-1 参照交叉验证门的裁剪语义，实现委托 pe.rs 单一真相源。）
fn win_cert_entries(bytes: &[u8]) -> Result<Vec<&[u8]>, CodecError> {
    let layout = super::parse_pe(bytes)?;
    let Some((va, end)) = layout.auth_region else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut pos = va;
    while pos + 8 <= end {
        let dwlen = super::u32le(bytes, pos, "WIN_CERTIFICATE.dwLength")? as usize;
        if dwlen < 8 || pos + dwlen > end {
            break; // 尾部 padding / 畸形项：停止（证书表完整性归签名验证层）
        }
        let blob = &bytes[pos + 8..pos + dwlen];
        let tlv = super::der_tlv_len(blob)
            .ok_or_else(|| CodecError::Malformed("bCertificate 外层 DER 长度头损坏".to_string()))?;
        if tlv > blob.len() {
            return Err(CodecError::Malformed(format!(
                "bCertificate DER 声称 {} 超出条目 {}",
                tlv,
                blob.len()
            )));
        }
        out.push(&blob[..tlv]);
        pos = (pos + dwlen + 7) & !7;
    }
    Ok(out)
}

fn hex32(d: &[u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02X}")).collect()
}

/// 单个参照 PE 的完整交叉验证：本实现 digest == 每个签名条目内嵌 messageDigest。
fn cross_validate_one(path: &std::path::Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} 不可读: {e}", path.display()))?;
    let got = authenticode_digest(&bytes).map_err(|e| e.to_string())?;
    let entries = win_cert_entries(&bytes).map_err(|e| e.to_string())?;
    assert!(!entries.is_empty(), "{} 无证书表条目", path.display());
    for (i, cms) in entries.iter().enumerate() {
        let sig =
            parse_signed_data(cms).map_err(|e| format!("{} 条目 {i}: {e:#}", path.display()))?;
        assert_eq!(
            got,
            sig.content_digest,
            "{} 条目 {i}：本实现 digest {} ≠ 内嵌 {}",
            path.display(),
            hex32(&got),
            hex32(&sig.content_digest)
        );
    }
    println!(
        "OK  {}  entries={}  digest={}",
        path.display(),
        entries.len(),
        hex32(&got)
    );
    Ok(())
}

#[test]
#[ignore = "需要本机 spike 参照 PE（仓库外）；--ignored 显式运行（S3-1 判据门）"]
fn s31_cross_validate_reference_pes() {
    let dir = spike_dir();
    let refs = [
        "test-full-signed.exe",       // R1: osslsigncode 全链，大 overlay
        "test-compare-signed.exe",    // R2: osslsigncode 对照链
        "test-signed.exe",            // R3: osslsigncode 早期链
        "test-signtool-signed.exe",   // R4: signtool 直签（MS 权威实现）
        "test-ossl-mingw-signed.exe", // R5: mingw 17-section 异构形态
        "test-overlay-signed.exe",    // R6: 签名前追加 4KB 随机 overlay（真 overlay 形态）
    ];
    for r in refs {
        cross_validate_one(std::path::Path::new(&dir).join(r).as_path())
            .unwrap_or_else(|e| panic!("{e}"));
    }
    println!(
        "S3-1 参照交叉验证：{}/{} 参照逐字节一致（含 overlay / 非常规形态）",
        refs.len(),
        refs.len()
    );
}

#[test]
#[ignore = "需要本机 spike 参照 PE（仓库外）；--ignored 显式运行"]
fn s31_negative_control_tampered() {
    // 签名后 0x1000 处翻 1 字节 → 本实现 digest 必须偏离内嵌 digest
    //（证明交叉验证不是「恒等比较」的空转绿）
    let path = std::path::Path::new(&spike_dir()).join("test-tampered.exe");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{} 不可读: {e}", path.display()));
    let got = authenticode_digest(&bytes).unwrap();
    let entries = win_cert_entries(&bytes).unwrap();
    assert!(!entries.is_empty());
    for (i, cms) in entries.iter().enumerate() {
        let sig = parse_signed_data(cms).unwrap_or_else(|e| panic!("条目 {i}: {e:#}"));
        assert_ne!(
            got, sig.content_digest,
            "负控失败：篡改文件的 digest 竟与内嵌一致（交叉验证失去区分力）"
        );
    }
    println!("OK  负控 test-tampered.exe：本实现 digest ≠ 内嵌 digest（区分力成立）");
}

// ---------------------------------------------------------------------------
// S3-2：Certificate Table 写入（WIN_CERTIFICATE + 8B 对齐 + Security 表项更新）
// 布局惯例（dwLength = 8 + roundup8(der) / va 8 对齐 / 文件止于表尾）按 S3-1
// 参照四例（ossl + signtool）实测钉死；本组合成测试钉语义，ignored 门产出真实
// 自签样本供 signtool / Explorer 外部检查。
// ---------------------------------------------------------------------------

use crate::pe::append_certificate_table;

/// 仿 win_cert_entries 的单条目结构断言（读 dwLength/rev/type + TLV 裁剪）。
fn assert_single_entry_shape(signed: &[u8], expect_der: &[u8], what: &str) {
    let layout = super::parse_pe(signed).expect("signed PE 可解析");
    let (va, end) = layout.auth_region.expect("auth_region 在场");
    assert_eq!(va % 8, 0, "{what}: 证书表 VA 必须 8 对齐");
    assert_eq!(end, signed.len(), "{what}: 文件恰止于表尾（参照惯例）");
    let dwlen = super::u32le(signed, va, "dwLength").unwrap() as usize;
    let rev = super::u16le(signed, va + 4, "wRevision").unwrap();
    let typ = super::u16le(signed, va + 6, "wCertificateType").unwrap();
    assert_eq!(
        va + dwlen,
        end,
        "{what}: Security.Size == dwLength == 表全长"
    );
    assert_eq!(rev, 0x0200, "{what}: wRevision = REVISION_2_0");
    assert_eq!(typ, 0x0002, "{what}: wCertificateType = PKCS_SIGNED_DATA");
    // dwLength = 8 + roundup8(der)（ossl 与 signtool 实测同惯例）
    assert_eq!(
        dwlen,
        8 + expect_der.len().div_ceil(8) * 8,
        "{what}: 条目 8 对齐含 pad"
    );
    let blob = &signed[va + 8..va + dwlen];
    assert!(blob.len() >= expect_der.len(), "{what}: 条目体容纳 DER");
    assert_eq!(
        &blob[..expect_der.len()],
        expect_der,
        "{what}: bCertificate 前缀逐字节还原"
    );
    assert!(
        blob[expect_der.len()..].iter().all(|&b| b == 0),
        "{what}: DER 后对齐 pad 全零"
    );
}

#[test]
fn append_certificate_table_roundtrip_and_digest_invariant() {
    // 8 对齐底层文件：pad=0 → 「跳过字段 + 排除证书块」语义下，
    // 签名前后 Authenticode digest 必须相等（写路径与摘要语义互证）。
    let base = build_pe(&PeSpec {
        plus: true,
        sections: vec![(0x100, 0x200)],
        security: Some((0, 0)),
        force_len: None, // 自然长 0x158，8 对齐
    });
    assert_eq!(base.len() % 8, 0, "测试前提：底层文件 8 对齐");
    let base_digest = authenticode_digest(&base).unwrap();

    let fake_der = vec![0xA5u8; 100]; // 结构层测试无需真 CMS
    let signed = append_certificate_table(&base, &fake_der).unwrap();
    assert_single_entry_shape(&signed, &fake_der, "8对齐底层");

    // Security 表项已写 VA/Size；authenticode_digest 不变（entry 跳过 + 证书块排除）
    assert_eq!(authenticode_digest(&signed).unwrap(), base_digest);

    // parse_signed_data 层不适用（fake DER 非真 CMS）——真实 CMS 往返在 ignored 门
}

#[test]
fn append_certificate_table_pads_unaligned_base_and_invariant_holds() {
    // 非 8 对齐底层：pad 区参与 digest → 签名后 digest = 手算四段
    let mut base = build_pe(&PeSpec {
        plus: false,
        sections: vec![(0x100, 0x200)],
        security: Some((0, 0)),
        force_len: None,
    });
    base.push(0x77); // 0x159 → pad 7 字节
    let der = vec![0x5Au8; 13];
    let signed = append_certificate_table(&base, &der).unwrap();
    assert_single_entry_shape(&signed, &der, "非对齐底层");

    // 手算：[0,cs) ++ [ce,ss) ++ [se,va)（含 7B 零 pad）；证书块与表项被排除
    let (cs, ce) = (P + 88, P + 92);
    let (ss, se) = (P + 120 + 32, P + 120 + 40);
    let (va, _) = super::parse_pe(&signed).unwrap().auth_region.unwrap();
    let mut h = Sha256::new();
    h.update(&signed[..cs]);
    h.update(&signed[ce..ss]);
    h.update(&signed[se..va]);
    let expect: [u8; 32] = h.finalize().into();
    assert_eq!(authenticode_digest(&signed).unwrap(), expect);
}

#[test]
fn append_certificate_table_rejects_signed_and_broken_shapes() {
    let unsigned = build_pe(&PeSpec {
        plus: true,
        sections: vec![(0x100, 0x200)],
        security: Some((0, 0)),
        force_len: None,
    });
    let der = vec![0x01u8; 16];
    // 已有证书表（Size>0）→ 拒绝（S3-3 策略域）
    let mut already = unsigned.clone();
    let va = already.len().div_ceil(8) * 8;
    let entry_len = 8 + der.len().div_ceil(8) * 8;
    already.resize(va + entry_len, 0);
    let dd32 = P + 136 + 32; // PE32+ Security 表项 VA 域
    already[dd32..dd32 + 4].copy_from_slice(&(va as u32).to_le_bytes());
    already[dd32 + 4..dd32 + 8].copy_from_slice(&(entry_len as u32).to_le_bytes());
    assert!(
        append_certificate_table(&already, &der).is_err(),
        "已签名 PE 必须拒绝"
    );

    // nrva < 5 → 无表项可写，拒绝
    let mut narrow = unsigned;
    let (nrva_off, _) = (P + 132, P + 136);
    put32(&mut narrow, nrva_off, 4);
    assert!(
        append_certificate_table(&narrow, &der).is_err(),
        "nrva<5 必须拒绝"
    );

    // 空 DER → 拒绝
    let fresh = build_pe(&PeSpec {
        plus: true,
        sections: vec![(0x100, 0x200)],
        security: Some((0, 0)),
        force_len: None,
    });
    assert!(
        append_certificate_table(&fresh, &[]).is_err(),
        "空 DER 必须拒绝"
    );

    // 不可解析输入 → 传播解析错误
    assert!(append_certificate_table(b"MZ\x00\x00", &der).is_err());
}

#[test]
#[ignore = "产出真实自签 PE 样本到 spike 目录（仓库外），供 signtool / Explorer 外部检查；--ignored 显式运行（S3-2 判据门）"]
fn s32_write_self_signed_sample() {
    use crate::envelope::build_signed_data;

    let dir = spike_dir();
    let base_path = std::path::Path::new(&dir).join("test-compare.exe");
    let base =
        std::fs::read(&base_path).unwrap_or_else(|e| panic!("{} 不可读: {e}", base_path.display()));
    let h = crate::keygen::generate().expect("keygen hierarchy");
    let digest = authenticode_digest(&base).expect("digest");
    let cms = build_signed_data(
        &digest,
        &h.leaf_sk,
        1_800_000_000, // 2027-01：落在自生成链有效期**内**（keygen 链从 2026-09 起；
        // 2023 旧值会让 Windows 按签名时间验链时报 HashMismatch——S3-2 实证）
        &h.chain(),
        Some("NemesisBot Code Signing"),
        Some("https://nemesisbot.example"),
    )
    .expect("build_signed_data");
    let signed = append_certificate_table(&base, &cms).expect("append");
    let out_path = std::path::Path::new(&dir).join("test-nb-signed.exe");
    std::fs::write(&out_path, &signed)
        .unwrap_or_else(|e| panic!("写 {} 失败: {e}", out_path.display()));

    // 自方结构 + 摘要闭环（信任链判定归 S4-1 管线）：
    let entries = win_cert_entries(&signed).expect("条目提取");
    assert_eq!(entries.len(), 1);
    let sig = parse_signed_data(entries[0]).expect("真 CMS 可解析");
    assert_eq!(sig.content_digest, digest, "内嵌 digest == 写入前 digest");
    assert_eq!(
        authenticode_digest(&signed).unwrap(),
        digest,
        "签名后文件 digest 不变（写入的恰是排除域）"
    );
    // Rust 侧密码学闭环：对落盘字节的 signedAttrs（SET OF，RFC 5652 §5.4）验签
    {
        use ecdsa::signature::Verifier;
        let der_sig = <ecdsa::der::Signature<p256::NistP256>>::try_from(sig.signature.as_slice())
            .expect("DER ECDSA 形态");
        h.leaf_vk()
            .verify(&sig.signature_message, &der_sig)
            .expect("落盘样本签名须对 signedAttrs 验签通过");
    }
    println!(
        "OK  自签样本已写出：{}  ({} 字节, CMS {} 字节)——待 signtool/Explorer 外部检查",
        out_path.display(),
        signed.len(),
        cms.len()
    );
}

// ---------------------------------------------------------------------------
// S3-2 回归钉（SpcLink 编码 bug 防复发 + statementType 属性）
// ---------------------------------------------------------------------------

/// S3-2 根修回归钉：SpcIndirectDataContent 的 file 链接字段必须是
/// `A0 20 A2 1E 80 1C <28B BMP>`——[2] 层 **constructed**（0xA2）。
/// der_derive 对 Choice 型 variant 的 EXPLICIT 会按 IMPLICIT 式标签替换
/// 产出 primitive 0x82：Windows 解码 SpcIndirectData 中断 → HashMismatch
/// （2026-09-12 对照实验实证：与 osslsigncode 产物仅差 0x82/0xA2 一字节）。
#[test]
fn spc_indirect_data_file_link_is_constructed_tag() {
    let der = crate::envelope::build_spc_indirect_data(&[0xAB; 32]).expect("spc build");
    // eContent = SEQUENCE { data { OID 311.2.1.15, value }, messageDigest DigestInfo }
    // data.value 起点固定可寻：外层 SEQUENCE 头(2) + data SEQUENCE 头(2) + OID TLV(12)
    let v = &der[16..];
    const EXPECT_PREFIX: [u8; 10] = [0x30, 0x26, 0x03, 0x02, 0x07, 0x80, 0xA0, 0x20, 0xA2, 0x1E];
    assert_eq!(
        &v[..10],
        &EXPECT_PREFIX,
        "file 链接字段头部须为 A0 20 A2 1E（[2] constructed）"
    );
    assert_eq!(v[10], 0x80, "SpcString::unicode = [0] IMPLICIT BMPString");
    assert_eq!(v[11], 0x1C);
    assert_eq!(v[12], 0x00, "BMP 首字符 '<' 高字节");
    // 尾部 30 31 = DigestInfo SEQUENCE 头（不在此钉）
}

/// S3-2：SignerInfo 必含 SPC_STATEMENT_TYPE 属性（signtool / osslsigncode 默认都带；
/// S3-2 对照实验前的缺省实现曾被 Windows 拒——补齐后与参照行为一致）。
#[test]
fn signed_data_includes_statement_type_attr() {
    use crate::envelope::build_signed_data;

    let h = crate::keygen::generate().expect("keygen");
    let der = build_signed_data(
        &[0xAB; 32],
        &h.leaf_sk,
        1_700_000_000,
        &h.chain(),
        None,
        None,
    )
    .expect("build");
    let p = crate::envelope::parse_signed_data(&der).expect("parse");
    // 验签消息 = attrs 的 SET OF DER——statementType 属性 TLV 须在其中；
    // 值 = 单元素 SEQUENCE 包 OID（osslsigncode 逐字节同形态）
    const STMT_VALUE: [u8; 14] = [
        0x30, 0x0C, 0x06, 0x0A, 0x2B, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x01, 0x0B,
    ];
    assert!(
        p.signature_message
            .windows(STMT_VALUE.len())
            .any(|w| w == STMT_VALUE),
        "statementType 属性值（SEQUENCE {{ OID MS_INDIVIDUAL_CODE_SIGNING }}）须在 signedAttrs 中"
    );
}

// ---------------------------------------------------------------------------
// S3-3：已签名文件策略——三态（未签名 / 自方重签 / 他方+自方共存）
// 策略入口 resign_pe_file（envelope 层）；载体原语：read_certificate_entries
// （条目遍历，主签名=首条目）+ replace_certificate_table（剥表重写）。
// **多条目共存形态已实证否决**（signtool /as 对照：Windows 主签名枚举只认
// 首条目；追加多签名的微软真实形态 = SPC_NESTED_SIGNATURE 嵌套属性）。
// 定位自方签名 = enumerate_signatures 展开 → 逐签名证书集含根锚判定；
// 完整 verify 管线归 S4-1，此处钉定位组合可用。
// ---------------------------------------------------------------------------

use crate::envelope::{enumerate_signatures, resign_pe_file};
use crate::pe::{read_certificate_entries, replace_certificate_table};

/// S3-3 公共夹具：8 对齐未签名底层 PE（与 S3-2 测试同规格）。
fn s33_base() -> Vec<u8> {
    build_pe(&PeSpec {
        plus: true,
        sections: vec![(0x100, 0x200)],
        security: Some((0, 0)),
        force_len: None,
    })
}

/// S3-3 公共夹具：用给定密钥体系构造真 CMS（signingTime 可变 → CMS 字节可区分）。
fn s33_cms(h: &crate::keygen::KeyHierarchy, digest: &[u8; 32], when: u64) -> Vec<u8> {
    crate::envelope::build_signed_data(
        digest,
        &h.leaf_sk,
        when,
        &h.chain(),
        Some("NemesisBot Code Signing"),
        Some("https://nemesisbot.example"),
    )
    .expect("build_signed_data")
}

#[test]
fn replace_certificate_table_strips_old_and_rewrites() {
    let base = s33_base();
    let base_digest = authenticode_digest(&base).unwrap();
    let h = crate::keygen::generate().unwrap();
    let cms1 = s33_cms(&h, &base_digest, 1_800_000_000);
    let cms2 = s33_cms(&h, &base_digest, 1_800_000_100); // signingTime 不同 → CMS 必不同
    let signed1 = append_certificate_table(&base, &cms1).unwrap();

    let signed2 = replace_certificate_table(&signed1, &cms2).unwrap();
    let entries = read_certificate_entries(&signed2).unwrap();
    assert_eq!(entries.len(), 1, "重签后恰一个新条目");
    assert_eq!(entries[0].certificate, cms2, "bCertificate 逐字节还原");
    assert_eq!(entries[0].offset % 8, 0, "条目落点 8 对齐");
    assert!(
        !signed2.windows(cms1.len()).any(|w| w == cms1),
        "旧签名字节须被整体剥除（不留残迹）"
    );
    // 内容区不动 → 替换前后 digest 同值（Authenticode 摘要只覆盖内容）；新签名自洽
    assert_eq!(authenticode_digest(&signed2).unwrap(), base_digest);
    let sig = parse_signed_data(&entries[0].certificate).unwrap();
    assert_eq!(
        sig.content_digest, base_digest,
        "新签名内嵌摘要 == 内容摘要"
    );
    assert_single_entry_shape(&signed2, &cms2, "重签");
}

/// 态1：未签名 PE 上 resign = 首签（直接 append_certificate_table）。
#[test]
fn resign_pe_first_signs_unsigned() {
    let base = s33_base();
    let base_digest = authenticode_digest(&base).unwrap();
    let h = crate::keygen::generate().unwrap();
    let cms = s33_cms(&h, &base_digest, 1_800_000_000);

    let out = resign_pe_file(&base, &cms, &h.root_anchor_fingerprint()).unwrap();
    assert_eq!(
        out,
        append_certificate_table(&base, &cms).unwrap(),
        "态1（未签名）= 逐字节走 append 首签路径"
    );
    let entries = read_certificate_entries(&out).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].certificate, cms, "首签 bCertificate 逐字节还原");
}

/// 态2：表中只有自方签名 → 整表替换（旧签名消失，digest 不变）。
#[test]
fn resign_pe_replaces_own_signature() {
    let base = s33_base();
    let base_digest = authenticode_digest(&base).unwrap();
    let h = crate::keygen::generate().unwrap();
    let anchor = h.root_anchor_fingerprint();
    let cms1 = s33_cms(&h, &base_digest, 1_800_000_000);
    let cms2 = s33_cms(&h, &base_digest, 1_800_000_100); // signingTime 不同 → CMS 必不同

    let signed1 = resign_pe_file(&base, &cms1, &anchor).unwrap();
    let signed2 = resign_pe_file(&signed1, &cms2, &anchor).unwrap();

    let entries = read_certificate_entries(&signed2).unwrap();
    assert_eq!(entries.len(), 1, "替换形态：单条目");
    assert_eq!(entries[0].certificate, cms2, "新签名在位");
    assert!(
        !signed2.windows(cms1.len()).any(|w| w == cms1),
        "旧签名字节须被整体剥除（不留残迹）"
    );
    assert_eq!(
        authenticode_digest(&signed2).unwrap(),
        base_digest,
        "重签不破坏内容摘要"
    );
}

/// 态3：他方主签名 + 自方嵌套共存——核心不变量四连：
/// 单条目表重写 / 主签名字节零变化（unauthAttrs 不在签名覆盖面）/
/// authenticode digest 不变 / 根锚枚举定位恰命中自方嵌套体。
#[test]
fn resign_pe_nests_into_third_party_and_own_is_locatable() {
    let base = s33_base();
    let base_digest = authenticode_digest(&base).unwrap();
    let other = crate::keygen::generate().unwrap(); // 「他方」密钥体系
    let own = crate::keygen::generate().unwrap();
    let cms_other = s33_cms(&other, &base_digest, 1_800_000_200);
    let cms_own = s33_cms(&own, &base_digest, 1_800_000_300);

    let theirs = append_certificate_table(&base, &cms_other).unwrap();
    let anchor = own.root_anchor_fingerprint();
    let out = resign_pe_file(&theirs, &cms_own, &anchor).unwrap();

    // 表回到单条目（嵌套走表重写；多条目形态已实证否决），
    // 条目 = 变长后的 host CMS；枚举展开 = 主 + 嵌套，嵌套体逐字节 = 我方 CMS
    let entries = read_certificate_entries(&out).unwrap();
    assert_eq!(entries.len(), 1, "嵌套形态 = 单条目表");
    let sigs = enumerate_signatures(&entries[0].certificate).unwrap();
    assert_eq!(sigs.len(), 2, "枚举：主签名 + 嵌套签名");
    assert_eq!(sigs[1], cms_own, "嵌套体 = 我方 CMS 原样");

    // 主签名字节零变化 + 他方验签仍过（SPC_NESTED_SIGNATURE 属 unsignedAttrs，
    // RFC 5652 §5.4/§5.6 不在签名覆盖面——密码学层面的共存不互扰证明）
    let p_before = parse_signed_data(&cms_other).unwrap();
    let p_after = parse_signed_data(&sigs[0]).unwrap();
    assert_eq!(p_before.signature, p_after.signature, "主签名值字节不变");
    assert_eq!(
        p_before.signature_message, p_after.signature_message,
        "signedAttrs（签名覆盖面）字节不变"
    );
    assert_eq!(p_before.content_digest, p_after.content_digest);
    {
        use ecdsa::signature::Verifier;
        let der_sig =
            <ecdsa::der::Signature<p256::NistP256>>::try_from(p_after.signature.as_slice())
                .expect("DER ECDSA 形态");
        other
            .leaf_vk()
            .verify(&p_after.signature_message, &der_sig)
            .expect("他方主签名验签在嵌套后仍通过");
    }

    // digest 不变（表重写吞下的只是表自身字节；内容区未动）
    assert_eq!(authenticode_digest(&out).unwrap(), base_digest);

    // 根锚定位：枚举序（Index 0=主, 1=嵌套）中根锚只命中我方嵌套体。
    // certs 是 SetOfVec（按 DER 字节排序），链序不保证——按证书集任一成员匹配。
    let located: Vec<usize> = sigs
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            parse_signed_data(c)
                .map(|p| p.certs.iter().any(|c| c.sha256_fingerprint() == anchor))
                .unwrap_or(false)
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(located, vec![1], "根锚只命中嵌套（我方）签名");
}

/// 策略诚实失败：混合多条目重签 / 多层嵌套 / 非 PKCS 主条目 / 空 DER / 不可解析输入。
#[test]
fn resign_pe_honest_failures() {
    let base = s33_base();
    let base_digest = authenticode_digest(&base).unwrap();
    let own = crate::keygen::generate().unwrap();
    let other = crate::keygen::generate().unwrap();
    let anchor = own.root_anchor_fingerprint();
    let cms_own = s33_cms(&own, &base_digest, 1_800_000_000);
    let cms_own2 = s33_cms(&own, &base_digest, 1_800_000_050);
    let cms_other = s33_cms(&other, &base_digest, 1_800_000_200);

    // 手工造「自方+他方」混合多条目表（读层可读，写层已实证否决）→ 混合重签拒绝
    let signed_own = append_certificate_table(&base, &cms_own).unwrap();
    let mut mixed = signed_own.clone();
    let (va_old, end_old) = super::parse_pe(&mixed).unwrap().auth_region.unwrap();
    let der = cms_other.as_slice();
    let entry_len = 8 + der.len().div_ceil(8) * 8;
    let va_new = end_old; // append 后文件止于表尾且 8 对齐
    mixed.resize(end_old + entry_len, 0);
    mixed[va_new..va_new + 4].copy_from_slice(&(entry_len as u32).to_le_bytes());
    mixed[va_new + 4..va_new + 6].copy_from_slice(&0x0200u16.to_le_bytes());
    mixed[va_new + 6..va_new + 8].copy_from_slice(&0x0002u16.to_le_bytes());
    mixed[va_new + 8..va_new + 8 + der.len()].copy_from_slice(der);
    let (soff, _) = super::parse_pe(&mixed).unwrap().security_dir_range.unwrap();
    mixed[soff + 4..soff + 8]
        .copy_from_slice(&((end_old - va_old + entry_len) as u32).to_le_bytes());
    assert_eq!(
        read_certificate_entries(&mixed).unwrap().len(),
        2,
        "夹具自检：混合双条目可读"
    );
    assert!(
        resign_pe_file(&mixed, &cms_own2, &anchor).is_err(),
        "自方+他方混合再重签（拆我方留他方）必须诚实拒绝"
    );

    // 多层嵌套：他方表 → 嵌我方（成功）→ 再嵌 → 主签名已含嵌套属性，拒绝
    let theirs = append_certificate_table(&base, &cms_other).unwrap();
    let once = resign_pe_file(&theirs, &cms_own, &anchor).unwrap();
    assert!(
        resign_pe_file(&once, &cms_own2, &anchor).is_err(),
        "主签名已含 SPC_NESTED_SIGNATURE 时二次嵌套必须拒绝"
    );

    // 非 PKCS_SIGNED_DATA 主条目 → 拒绝挂嵌套
    let mut nonpkcs = base.clone();
    let va = nonpkcs.len();
    let payload = vec![0x22u8; 13];
    let elen = 8 + 16; // 13B pad 到 16
    nonpkcs.resize(va + elen, 0);
    nonpkcs[va..va + 4].copy_from_slice(&(elen as u32).to_le_bytes());
    nonpkcs[va + 4..va + 6].copy_from_slice(&0x0200u16.to_le_bytes());
    nonpkcs[va + 6..va + 8].copy_from_slice(&0x0003u16.to_le_bytes());
    nonpkcs[va + 8..va + 8 + 13].copy_from_slice(&payload);
    let (soff2, _) = super::parse_pe(&nonpkcs)
        .unwrap()
        .security_dir_range
        .unwrap();
    nonpkcs[soff2..soff2 + 4].copy_from_slice(&(va as u32).to_le_bytes());
    nonpkcs[soff2 + 4..soff2 + 8].copy_from_slice(&(elen as u32).to_le_bytes());
    assert!(
        resign_pe_file(&nonpkcs, &cms_own, &anchor).is_err(),
        "非 PKCS_SIGNED_DATA 主条目不可挂嵌套"
    );

    // 空 DER：未签名路径（append）拒绝 / 他方路径（嵌套 Any 解析）拒绝
    assert!(resign_pe_file(&base, &[], &anchor).is_err());
    let theirs2 = append_certificate_table(&base, &cms_other).unwrap();
    assert!(resign_pe_file(&theirs2, &[], &anchor).is_err());

    // 不可解析输入 → 传播解析错误
    assert!(resign_pe_file(b"MZ", &cms_own, &anchor).is_err());
}

#[test]
fn read_certificate_entries_unsigned_and_non_pkcs_types() {
    // 未签名 → 空 Vec
    assert!(read_certificate_entries(&s33_base()).unwrap().is_empty());

    // 非 PKCS_SIGNED_DATA 条目（如 0x0003）：blob 原样保留，不做 DER TLV 裁剪
    let mut signed = s33_base();
    let va = signed.len();
    let payload = vec![0x11u8; 13];
    let entry_len = 8 + 16; // 13B pad 到 16
    signed.resize(va + entry_len, 0);
    signed[va..va + 4].copy_from_slice(&(entry_len as u32).to_le_bytes());
    signed[va + 4..va + 6].copy_from_slice(&0x0200u16.to_le_bytes());
    signed[va + 6..va + 8].copy_from_slice(&0x0003u16.to_le_bytes());
    signed[va + 8..va + 8 + 13].copy_from_slice(&payload);
    let (soff, _) = super::parse_pe(&signed)
        .unwrap()
        .security_dir_range
        .unwrap();
    signed[soff..soff + 4].copy_from_slice(&(va as u32).to_le_bytes());
    signed[soff + 4..soff + 8].copy_from_slice(&(entry_len as u32).to_le_bytes());

    let entries = read_certificate_entries(&signed).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].cert_type, 0x0003);
    assert_eq!(
        entries[0].certificate.len(),
        16,
        "非 PKCS 条目 blob 原样（含 pad，不做 TLV 裁剪）"
    );
}

#[test]
#[ignore = "需要本机 spike 参照 PE；产出他方(主)+自方(嵌套)共存样本供 signtool 外部检查；--ignored 显式运行（S3-3 判据门）"]
fn s33_coexist_with_third_party_reference_and_write_sample() {
    let dir = spike_dir();
    // 真「他方签名」参照：osslsigncode 早期链签的 test-signed.exe（与本仓库
    // keygen 链无任何关系）——对它嵌套自方签名 = 真实共存形态。
    let ref_path = std::path::Path::new(&dir).join("test-signed.exe");
    let theirs =
        std::fs::read(&ref_path).unwrap_or_else(|e| panic!("{} 不可读: {e}", ref_path.display()));
    let before = read_certificate_entries(&theirs).expect("参照条目");
    assert_eq!(before.len(), 1, "参照 = 单条目他方签名");
    let host_before = parse_signed_data(&before[0].certificate).expect("他方主签名解析");

    let h = crate::keygen::generate().expect("keygen");
    let digest = authenticode_digest(&theirs).expect("他方文件 digest");
    let cms = s33_cms(&h, &digest, 1_800_000_000); // 2027-01：落在自生成链有效期内（S3-2 实证约束）
    let both = resign_pe_file(&theirs, &cms, &h.root_anchor_fingerprint()).expect("嵌套共存");

    // 结构断言：单条目表重写；枚举 = 主 + 嵌套；主签名字节零变化；
    // 他方摘要语义不破坏；根锚定位恰命中自方嵌套体
    let entries = read_certificate_entries(&both).expect("条目提取");
    assert_eq!(entries.len(), 1, "嵌套形态走表重写：单条目");
    let sigs = enumerate_signatures(&entries[0].certificate).expect("签名枚举");
    assert_eq!(sigs.len(), 2, "主（ossl 链）+ 嵌套（我方）");
    assert_eq!(sigs[1], cms, "嵌套体 = 我方 CMS 原样");
    let host_after = parse_signed_data(&sigs[0]).expect("嵌套后主签名解析");
    assert_eq!(
        host_before.signature, host_after.signature,
        "他方主签名值字节不变（unauthAttrs 不在签名覆盖面）"
    );
    assert_eq!(
        host_before.signature_message, host_after.signature_message,
        "他方 signedAttrs 字节不变"
    );
    assert_eq!(
        authenticode_digest(&both).unwrap(),
        digest,
        "嵌套后他方摘要语义不破坏（共存不互扰）"
    );
    let anchor = h.root_anchor_fingerprint();
    let located: Vec<usize> = sigs
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            parse_signed_data(c)
                .map(|p| p.certs.iter().any(|c| c.sha256_fingerprint() == anchor))
                .unwrap_or(false)
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(located, vec![1], "根锚只命中嵌套（我方）签名");

    let out_path = std::path::Path::new(&dir).join("test-nb-coexist.exe");
    std::fs::write(&out_path, &both)
        .unwrap_or_else(|e| panic!("写 {} 失败: {e}", out_path.display()));
    println!(
        "OK  他方(主)+自方(嵌套)共存样本已写出：{}（{} 字节，单条目 + SPC_NESTED_SIGNATURE）",
        out_path.display(),
        both.len()
    );
    println!(
        "外部判据：signtool verify /v /all 期望 2 条 Signature Index（0=他方主签名，1=我方嵌套），\
         每条报 CERT_E_UNTRUSTEDROOT 0x800B0109（D7 默认不信任，Number of errors: 2）"
    );
}
