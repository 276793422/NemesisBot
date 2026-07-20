//! 可执行文件格式抽象（codec 多态）。
//!
//! 每种二进制格式（PE / ELF / Raw）实现 [`ExecutableCodec`] trait，
//! 负责按格式规则计算"原始可执行内容"的边界（L）与内容哈希。
//! 外层签名/验证逻辑只消费 codec 的输出，不感知具体格式——
//! 未来新增格式（如 Mach-O）只需新增一个 codec 实现。
//!
//! # L 的含义
//! L = "原始可执行内容末尾"（overlay 起点）。`[0, L)` 是被签名保护的内容，
//! `[L, file_len)` 是 overlay（Authenticode 签名、本工具的 envelope、附加数据等）。
//! 多个签名叠加时各自独立，content_hash 永远基于 `[0, L)`。

use sha2::{Digest, Sha256};

/// PE（Windows）格式标记值（写入 envelope footer 的 format_tag 字段）。
pub const FORMAT_TAG_PE: u8 = 1;
/// ELF（Linux / Android）格式标记值。
pub const FORMAT_TAG_ELF: u8 = 2;
/// Raw（裸机 / 固件 / 任意 blob）格式标记值。
pub const FORMAT_TAG_RAW: u8 = 3;

/// codec 解析或哈希过程中的结构性错误。
#[derive(Debug)]
pub enum CodecError {
    /// 不是可识别的可执行文件（无有效格式标记）。
    NotAnExecutable,
    /// 文件过短，无法读取必需字段。
    Truncated,
    /// 某字段偏移越界（附带字段名，便于诊断）。
    FieldOutOfBounds(&'static str),
    /// Optional Header Magic 既非 PE32(0x10b) 也非 PE32+(0x20b)。
    UnknownOptionalHeaderMagic(u16),
    /// ELF e_ident[EI_CLASS] 非 1(32 位) 也非 2(64 位)。
    UnsupportedElfClass(u8),
    /// ELF e_ident[EI_DATA] 非 1(LE) 也非 2(BE)。
    UnsupportedElfData(u8),
    /// 文件结构自相矛盾（如多源交叉校验失败）。
    Malformed(String),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for CodecError {}

/// 单一可执行文件格式的解析与哈希 codec。
///
/// 每种格式一个实现（[`PeCodec`] / [`ElfCodec`] / [`RawCodec`]），
/// 由 [`detect_codec`] 按魔数选取。
pub trait ExecutableCodec: Send + Sync {
    /// 多源综合计算原始可执行内容末尾 L（overlay 起点）。
    ///
    /// 返回 `None` 表示该格式无结构化长度标记（Raw），调用方应改用
    /// envelope 中记录的 `content_len`。
    fn compute_l(&self, bytes: &[u8]) -> Result<Option<usize>, CodecError>;

    /// 计算 `[0, l)` 区间、按本格式规则排除特定字段后的 SHA-256。
    fn content_hash(&self, content: &[u8], l: usize) -> Result<[u8; 32], CodecError>;

    /// overlay `[l, file_len)` 内需要跳过的区域（如 PE 的 Authenticode 证书表），
    /// 供 envelope 扫描时避开，避免误匹配/重复扫描。默认空（ELF / Raw 无需跳过）。
    /// 解析失败时返回空（容错，不阻断扫描——后续 footer_crc/验签仍把关）。
    fn overlay_excludes(&self, _bytes: &[u8]) -> Vec<(usize, usize)> {
        Vec::new()
    }
}

// ---- codec 结构体（PeCodec / ElfCodec 的 impl 分别在 pe / elf 模块）----

/// PE（Windows）codec。impl 在 [`crate::pe`] 模块。
pub struct PeCodec;
/// ELF（Linux / Android）codec。impl 在 [`crate::elf`] 模块。
pub struct ElfCodec;
/// Raw（裸文件）codec：无结构，整段哈希。
pub struct RawCodec;

impl ExecutableCodec for RawCodec {
    fn compute_l(&self, _bytes: &[u8]) -> Result<Option<usize>, CodecError> {
        // Raw 文件无格式标记，L 由 envelope 的 content_len 字段确定。
        Ok(None)
    }

    fn content_hash(&self, content: &[u8], l: usize) -> Result<[u8; 32], CodecError> {
        // 无排除字段，对 [0, l) 整段 SHA-256。
        if l > content.len() {
            return Err(CodecError::Malformed(format!(
                "content_len {} > bytes len {}",
                l,
                content.len()
            )));
        }
        let hash = Sha256::digest(&content[..l]);
        Ok(hash.into())
    }
}

/// 按文件魔数探测格式标记值（用于 envelope footer 的 format_tag 字段）。
pub fn detect_format(bytes: &[u8]) -> u8 {
    if bytes.starts_with(b"MZ") {
        FORMAT_TAG_PE
    } else if bytes.starts_with(b"\x7fELF") {
        FORMAT_TAG_ELF
    } else {
        FORMAT_TAG_RAW
    }
}

/// 按文件魔数选取对应的 codec。
///
/// - `MZ` → [`PeCodec`]
/// - `\x7fELF` → [`ElfCodec`]
/// - 其他 → [`RawCodec`]（兜底，不报错；适用裸机/固件/任意 blob）
pub fn detect_codec(bytes: &[u8]) -> Box<dyn ExecutableCodec> {
    if bytes.starts_with(b"MZ") {
        Box::new(PeCodec)
    } else if bytes.starts_with(b"\x7fELF") {
        Box::new(ElfCodec)
    } else {
        Box::new(RawCodec)
    }
}
