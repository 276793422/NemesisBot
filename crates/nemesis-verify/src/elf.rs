//! ELF（Linux / Android）格式 codec。
//!
//! 职责：
//! 1. 多源综合计算 L = `max(所有 PT_LOAD 段 end, section header table end)`。
//!    `[0, L)` 覆盖整个 ELF 结构（含 section headers），overlay = `[L, file_len)`。
//! 2. [`ExecutableCodec::content_hash`]：SHA-256 over `[0, L)`，无排除字段。
//!
//! # 字节序
//! 哈希本身算原始字节、字节序无关；但**读取字段值**（program/section header）
//! 必须按 `e_ident[EI_DATA]`（1=LE，2=BE）选择 `from_le` / `from_be`。
//!
//! # 偏移依据（ELF 规范）
//! - `e_ident[4]` EI_CLASS：1=ELFCLASS32 / 2=ELFCLASS64
//! - `e_ident[5]` EI_DATA：1=LE / 2=BE
//! - ELF32：`e_phoff`@28、`e_phentsize`@42、`e_phnum`@44、`e_shoff`@32、
//!   `e_shentsize`@46、`e_shnum`@48；program header 32B：`p_type`@0、`p_offset`@4、`p_filesz`@16
//! - ELF64：`e_phoff`@32、`e_phentsize`@54、`e_phnum`@56、`e_shoff`@40、
//!   `e_shentsize`@58、`e_shnum`@60；program header 56B：`p_type`@0、`p_offset`@8、`p_filesz`@32
//! - `PT_LOAD = 1`

use crate::codec::{CodecError, ElfCodec, ExecutableCodec};
use sha2::{Digest, Sha256};

const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;
const ELFCLASS32: u8 = 1;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1; // little-endian
const ELFDATA2MSB: u8 = 2; // big-endian
const PT_LOAD: u32 = 1;

/// 读 u16（按字节序）。
fn u16(b: &[u8], off: usize, le: bool, name: &'static str) -> Result<u16, CodecError> {
    let s = b
        .get(off..off + 2)
        .ok_or(CodecError::FieldOutOfBounds(name))?;
    Ok(if le {
        u16::from_le_bytes([s[0], s[1]])
    } else {
        u16::from_be_bytes([s[0], s[1]])
    })
}

/// 读 u32（按字节序）。
fn u32(b: &[u8], off: usize, le: bool, name: &'static str) -> Result<u32, CodecError> {
    let s = b
        .get(off..off + 4)
        .ok_or(CodecError::FieldOutOfBounds(name))?;
    Ok(if le {
        u32::from_le_bytes([s[0], s[1], s[2], s[3]])
    } else {
        u32::from_be_bytes([s[0], s[1], s[2], s[3]])
    })
}

/// 读 u64（按字节序）。
fn u64(b: &[u8], off: usize, le: bool, name: &'static str) -> Result<u64, CodecError> {
    let s = b
        .get(off..off + 8)
        .ok_or(CodecError::FieldOutOfBounds(name))?;
    let arr: [u8; 8] = s
        .try_into()
        .map_err(|_| CodecError::FieldOutOfBounds(name))?;
    Ok(if le {
        u64::from_le_bytes(arr)
    } else {
        u64::from_be_bytes(arr)
    })
}

/// 多源综合计算 ELF 的 L。
fn compute_elf_l(bytes: &[u8]) -> Result<usize, CodecError> {
    if bytes.len() < 0x40 {
        return Err(CodecError::Truncated);
    }
    if &bytes[0..4] != b"\x7fELF" {
        return Err(CodecError::NotAnExecutable);
    }
    let class = bytes[EI_CLASS];
    let data = bytes[EI_DATA];
    let le = match data {
        ELFDATA2LSB => true,
        ELFDATA2MSB => false,
        _ => return Err(CodecError::UnsupportedElfData(data)),
    };
    let is64 = match class {
        ELFCLASS32 => false,
        ELFCLASS64 => true,
        _ => return Err(CodecError::UnsupportedElfClass(class)),
    };

    let mut l: usize = 0;

    // 源 1：max(p_offset + p_filesz) over PT_LOAD
    let (phoff, phentsize, phnum) = if is64 {
        (
            u64(bytes, 32, le, "e_phoff")? as usize,
            u16(bytes, 54, le, "e_phentsize")? as usize,
            u16(bytes, 56, le, "e_phnum")? as usize,
        )
    } else {
        (
            u32(bytes, 28, le, "e_phoff")? as usize,
            u16(bytes, 42, le, "e_phentsize")? as usize,
            u16(bytes, 44, le, "e_phnum")? as usize,
        )
    };
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        let p_type = u32(bytes, ph, le, "p_type")?;
        if p_type != PT_LOAD {
            continue;
        }
        let (p_offset, p_filesz) = if is64 {
            (
                u64(bytes, ph + 8, le, "p_offset")? as usize,
                u64(bytes, ph + 32, le, "p_filesz")? as usize,
            )
        } else {
            (
                u32(bytes, ph + 4, le, "p_offset")? as usize,
                u32(bytes, ph + 16, le, "p_filesz")? as usize,
            )
        };
        l = l.max(p_offset + p_filesz);
    }

    // 源 2：section header table end = e_shoff + e_shnum*e_shentsize（e_shoff>0 时）
    let (shoff, shentsize, shnum): (u64, u64, u64) = if is64 {
        (
            u64(bytes, 40, le, "e_shoff")?,
            u16(bytes, 58, le, "e_shentsize")? as u64,
            u16(bytes, 60, le, "e_shnum")? as u64,
        )
    } else {
        (
            u32(bytes, 32, le, "e_shoff")? as u64,
            u16(bytes, 46, le, "e_shentsize")? as u64,
            u16(bytes, 48, le, "e_shnum")? as u64,
        )
    };
    if shoff > 0 {
        l = l.max((shoff as usize) + (shentsize as usize) * (shnum as usize));
    }

    // 交叉校验：L 不应超过文件长度
    if l > bytes.len() {
        return Err(CodecError::Malformed(format!(
            "ELF L {} > file len {}",
            l,
            bytes.len()
        )));
    }
    Ok(l)
}

impl ExecutableCodec for ElfCodec {
    fn compute_l(&self, bytes: &[u8]) -> Result<Option<usize>, CodecError> {
        Ok(Some(compute_elf_l(bytes)?))
    }

    fn content_hash(&self, content: &[u8], l: usize) -> Result<[u8; 32], CodecError> {
        if l > content.len() {
            return Err(CodecError::Malformed(format!(
                "content_len {} > bytes len {}",
                l,
                content.len()
            )));
        }
        // 无排除字段，整段 [0, l) SHA-256。
        Ok(Sha256::digest(&content[..l]).into())
    }
}
