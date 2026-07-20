//! PE（Windows）格式 codec。
//!
//! 职责：
//! 1. 多源综合计算原始内容末尾 L（overlay 起点）= `max(section raw end)`，
//!    并与 Certificate Table（`DataDirectory[4]`）做交叉一致性校验。
//! 2. 定位 Authenticode 证书表区域（`DataDirectory[4]` 指向），供 envelope
//!    扫描时跳过——通过 [`ExecutableCodec::overlay_excludes`] 暴露给外层。
//! 3. [`ExecutableCodec::content_hash`]：SHA-256 over `[0, L)`，排除
//!    `CheckSum`(4B) 与 `DataDirectory[4]`(8B) 两个易变字段。
//!
//! # 偏移依据（PE/COFF 规范，逐字段核对）
//! 设 `P = e_lfanew`（u32 LE @ 文件偏移 `0x3C`）：
//! - PE 签名 `"PE\0\0"` @ `P`
//! - COFF header @ `P+4`：`NumberOfSections`@`P+6`、`SizeOfOptionalHeader`@`P+20`
//! - Optional header @ `P+24`：`Magic`@`P+24`（`0x10b`=PE32 / `0x20b`=PE32+）
//! - `CheckSum` @ `P+88`（PE32 与 PE32+ 偏移相同：BaseOfData+ImageBase 共 8B
//!   vs ImageBase 8B 抵消）
//! - `NumberOfRvaAndSizes` @ `P+116`(PE32) / `P+132`(PE32+)
//! - DataDirectory 起点 @ `P+120`(PE32) / `P+136`(PE32+)（差 16B：stack/heap
//!   reserve+commit 字段 4B vs 8B）
//! - `DataDirectory[4]`（Security）@ 起点+32 = `P+152`(PE32) / `P+168`(PE32+)
//! - section table @ `P+24+SizeOfOptionalHeader`，每项 40B：
//!   `SizeOfRawData`@+16、`PointerToRawData`@+20
//!
//! PE 所有多字节字段一律 little-endian（PE/COFF 规范强制，与运行平台无关）。

use crate::codec::{CodecError, ExecutableCodec, PeCodec};
use sha2::{Digest, Sha256};

/// PE 布局解析结果。
pub(crate) struct PeLayout {
    /// 原始可执行内容末尾（overlay 起点）。
    pub l: usize,
    /// `CheckSum` 字段区间 `[start, end)`（content_hash 排除）。
    pub checksum_range: (usize, usize),
    /// `DataDirectory[4]`（Security 目录项）区间，仅当 `NumberOfRvaAndSizes ≥ 5`。
    pub security_dir_range: Option<(usize, usize)>,
    /// Authenticode 证书表区间 `[VA, VA+Size)`，仅当 `Size > 0`（扫描时跳过）。
    pub auth_region: Option<(usize, usize)>,
}

/// 读 little-endian u16。
fn u16le(b: &[u8], off: usize, name: &'static str) -> Result<u16, CodecError> {
    let s = b.get(off..off + 2).ok_or(CodecError::FieldOutOfBounds(name))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

/// 读 little-endian u32。
fn u32le(b: &[u8], off: usize, name: &'static str) -> Result<u32, CodecError> {
    let s = b.get(off..off + 4).ok_or(CodecError::FieldOutOfBounds(name))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// 解析 PE 布局（多源综合 + 交叉校验）。
fn parse_pe(bytes: &[u8]) -> Result<PeLayout, CodecError> {
    if bytes.len() < 0x40 {
        return Err(CodecError::Truncated);
    }
    if &bytes[0..2] != b"MZ" {
        return Err(CodecError::NotAnExecutable);
    }
    let p = u32le(bytes, 0x3C, "e_lfanew")? as usize;
    // PE 签名 "PE\0\0"
    if bytes.get(p..p + 4) != Some(b"PE\0\0") {
        return Err(CodecError::NotAnExecutable);
    }
    // COFF header
    let num_sections = u16le(bytes, p + 6, "NumberOfSections")? as usize;
    let size_of_opt = u16le(bytes, p + 20, "SizeOfOptionalHeader")? as usize;
    // Optional header magic → PE32 / PE32+
    let magic = u16le(bytes, p + 24, "Magic")?;
    let is_plus = match magic {
        0x10b => false,
        0x20b => true,
        _ => return Err(CodecError::UnknownOptionalHeaderMagic(magic)),
    };
    // CheckSum @ P+88（PE32 与 PE32+ 相同）
    let checksum_range = (p + 88, p + 92);

    // NumberOfRvaAndSizes 与 DataDirectory 起点（PE32 / PE32+ 不同）
    let (nrva_off, datadir_start) = if is_plus {
        (p + 132, p + 136)
    } else {
        (p + 116, p + 120)
    };
    let nrva = u32le(bytes, nrva_off, "NumberOfRvaAndSizes")? as usize;
    // DataDirectory[4]（Security）仅当数组 ≥ 5 项时存在
    let security_dir_range = if nrva >= 5 {
        let off = datadir_start + 4 * 8;
        Some((off, off + 8))
    } else {
        None
    };

    // Authenticode 证书表区域 = DataDirectory[4] 的 VA（文件偏移）+ Size
    let auth_region = if nrva >= 5 {
        let off = datadir_start + 4 * 8;
        let va = u32le(bytes, off, "Security.VA")? as usize;
        let size = u32le(bytes, off + 4, "Security.Size")? as usize;
        if size > 0 {
            Some((va, va + size))
        } else {
            None
        }
    } else {
        None
    };

    // L = max(section PointerToRawData + SizeOfRawData)，仅算 SizeOfRawData>0 的 section
    let sec_tbl = p + 24 + size_of_opt;
    let mut l: usize = 0;
    for i in 0..num_sections {
        let s = sec_tbl + i * 40;
        let size_raw = u32le(bytes, s + 16, "SizeOfRawData")? as usize;
        let ptr_raw = u32le(bytes, s + 20, "PointerToRawData")? as usize;
        if size_raw > 0 {
            l = l.max(ptr_raw + size_raw);
        }
    }
    // 保险：L 至少覆盖到 section table 末尾（确保所有 headers 在 [0, L) 内）。
    let sec_tbl_end = sec_tbl + num_sections * 40;
    l = l.max(sec_tbl_end);
    // L 不超过文件长度
    l = l.min(bytes.len());

    // 交叉一致性校验：Authenticode 区域应在 overlay 内、不越界
    if let Some((va, end)) = auth_region {
        if va < l {
            return Err(CodecError::Malformed(format!(
                "Authenticode VA {} < overlay start L {}",
                va, l
            )));
        }
        if end > bytes.len() {
            return Err(CodecError::Malformed(format!(
                "Authenticode end {} > file len {}",
                end,
                bytes.len()
            )));
        }
    }

    Ok(PeLayout {
        l,
        checksum_range,
        security_dir_range,
        auth_region,
    })
}

impl ExecutableCodec for PeCodec {
    fn compute_l(&self, bytes: &[u8]) -> Result<Option<usize>, CodecError> {
        let layout = parse_pe(bytes)?;
        Ok(Some(layout.l))
    }

    fn content_hash(&self, content: &[u8], l: usize) -> Result<[u8; 32], CodecError> {
        if l > content.len() {
            return Err(CodecError::Malformed(format!(
                "content_len {} > bytes len {}",
                l,
                content.len()
            )));
        }
        let layout = parse_pe(content)?;
        // 收集排除区间（均在 headers 内、属 [0, l)）
        let mut excludes: Vec<(usize, usize)> = vec![layout.checksum_range];
        if let Some(r) = layout.security_dir_range {
            excludes.push(r);
        }
        // 排序 + 顺序游标，分段喂 SHA-256（跳过排除区间，裁剪到 [0, l)）
        excludes.sort_by_key(|r| r.0);
        let mut hasher = Sha256::new();
        let mut cursor = 0usize;
        for (start, end) in excludes {
            let start = start.min(l);
            let end = end.min(l);
            if start > cursor {
                hasher.update(&content[cursor..start]);
            }
            if end > cursor {
                cursor = end;
            }
        }
        if cursor < l {
            hasher.update(&content[cursor..l]);
        }
        Ok(hasher.finalize().into())
    }

    fn overlay_excludes(&self, bytes: &[u8]) -> Vec<(usize, usize)> {
        // 暴露 Authenticode 证书表区域，供 envelope 扫描时跳过；解析失败容错返回空。
        parse_pe(bytes)
            .ok()
            .and_then(|layout| layout.auth_region)
            .into_iter()
            .collect()
    }
}
