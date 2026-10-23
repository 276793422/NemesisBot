//! pe —— 轻量 PE 解析（T3.1）。
//!
//! 双布局：磁盘文件（PointerToRawData 映射）与内存快照（节 blob 独立存储，
//! VirtualAddress 映射）。头部结构两者逐字节同形；映射只在节区段不同。
//! 全程有界读取——损坏/截断输入返回 Err，绝不 panic。
//!
//! 先例：nemesis-injector::ziwei_wow64::pe_export_rvas（M1 已验证的
//! Toolhelp + 导出表走法）；本模块是更完整的纯解析版本。

use serde::{Deserialize, Serialize};

/// PE 解析错误（诚实上抛，不 panic）。
#[derive(Debug, thiserror::Error)]
pub enum PeError {
    #[error("PE 头截断（需 {need} 字节，只有 {have}）")]
    Truncated { need: usize, have: usize },
    #[error("非法 PE 签名（MZ/PE\\0\\0 校验失败）")]
    BadSignature,
    #[error("不支持的 OptionalHeader magic：0x{0:04X}（只支持 PE32 0x10B / PE32+ 0x20B）")]
    BadOptionalMagic(u16),
    #[error("节表项数异常：{0}")]
    BadSectionCount(usize),
    #[error("RVA 0x{0:X} 不在任何节内")]
    RvaUnmapped(u32),
}

#[inline]
fn u16_at(b: &[u8], off: usize) -> Result<u16, PeError> {
    if off + 2 > b.len() {
        return Err(PeError::Truncated {
            need: off + 2,
            have: b.len(),
        });
    }
    Ok(u16::from_le_bytes([b[off], b[off + 1]]))
}

#[inline]
fn u32_at(b: &[u8], off: usize) -> Result<u32, PeError> {
    if off + 4 > b.len() {
        return Err(PeError::Truncated {
            need: off + 4,
            have: b.len(),
        });
    }
    Ok(u32::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
    ]))
}

#[inline]
fn u64_at(b: &[u8], off: usize) -> Result<u64, PeError> {
    if off + 8 > b.len() {
        return Err(PeError::Truncated {
            need: off + 8,
            have: b.len(),
        });
    }
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[off..off + 8]);
    Ok(u64::from_le_bytes(a))
}

/// 节信息（name 截断到 8 字节 ASCII，null 去尾；Deserialize = SnapshotMeta 落盘需要）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionInfo {
    pub name: String,
    /// 虚拟地址（RVA）。
    pub rva: u32,
    /// 虚拟大小（内存布局的真实上限）。
    pub vsize: u32,
    /// 文件内原始大小。
    pub raw_size: u32,
    /// 文件内偏移（磁盘布局映射用；内存快照无意义）。
    pub raw_ptr: u32,
    pub characteristics: u32,
}

impl SectionInfo {
    pub fn is_executable(&self) -> bool {
        self.characteristics & 0x2000_0000 != 0 // IMAGE_SCN_MEM_EXECUTE
    }
    pub fn is_writable(&self) -> bool {
        self.characteristics & 0x8000_0000 != 0 // IMAGE_SCN_MEM_WRITE
    }
}

/// 数据目录索引（我们关心的子集）。
pub const DIRECTORY_EXPORT: usize = 0;
pub const DIRECTORY_IMPORT: usize = 1;
/// 全量数据目录数（PE 规范上限 16）。
pub const NUM_DIRECTORIES: usize = 16;

/// 解析后的 PE 概要（头层信息 + 节表 + 数据目录）。
#[derive(Debug, Clone, Serialize)]
pub struct PeInfo {
    /// IMAGE_FILE_MACHINE（0x8664=x64 / 0x14C=x86）。
    pub machine: u16,
    pub image_base: u64,
    pub entry_rva: u32,
    pub timestamp: u32,
    pub checksum: u32,
    pub sections: Vec<SectionInfo>,
    /// 数据目录 (rva, size)，按规范序号索引；不足 16 补 (0,0)。
    pub directories: Vec<(u32, u32)>,
}

impl PeInfo {
    /// 从头镜像解析（磁盘文件头部或内存快照头部均同形）。
    /// 输入只需覆盖到节表末尾（通常 4KB 头页足够）。
    pub fn parse(bytes: &[u8]) -> Result<PeInfo, PeError> {
        // DOS 头：MZ + e_lfanew@0x3C
        if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
            return Err(PeError::BadSignature);
        }
        let e_lfanew = u32_at(bytes, 0x3C)? as usize;
        // PE 签名 PE\0\0
        if e_lfanew + 4 > bytes.len() || &bytes[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
            return Err(PeError::BadSignature);
        }
        // COFF：machine@+4, num_sections@+6, timestamp@+8, optional_size@+20
        let coff = e_lfanew + 4;
        let machine = u16_at(bytes, coff)?;
        let num_sections = u16_at(bytes, coff + 2)? as usize;
        let timestamp = u32_at(bytes, coff + 4)?;
        let optional_size = u16_at(bytes, coff + 16)? as usize;
        if num_sections > 96 {
            return Err(PeError::BadSectionCount(num_sections));
        }
        // OptionalHeader
        let opt = coff + 20;
        let magic = u16_at(bytes, opt)?;
        let (image_base, dd_off) = match magic {
            0x10B => (u32_at(bytes, opt + 28)? as u64, opt + 96), // PE32
            0x20B => (u64_at(bytes, opt + 24)?, opt + 112),       // PE32+
            m => return Err(PeError::BadOptionalMagic(m)),
        };
        let entry_rva = u32_at(bytes, opt + 16)?;
        let checksum = u32_at(bytes, opt + 64)?;
        let mut directories = vec![(0u32, 0u32); NUM_DIRECTORIES];
        let dd_end = dd_off + NUM_DIRECTORIES * 8;
        if bytes.len() >= dd_end {
            for (i, d) in directories.iter_mut().enumerate() {
                *d = (
                    u32_at(bytes, dd_off + i * 8)?,
                    u32_at(bytes, dd_off + i * 8 + 4)?,
                );
            }
        }
        // 节表紧跟 OptionalHeader
        let sh = opt + optional_size;
        let mut sections = Vec::with_capacity(num_sections);
        for i in 0..num_sections {
            let base = sh + i * 40;
            if base + 40 > bytes.len() {
                return Err(PeError::Truncated {
                    need: base + 40,
                    have: bytes.len(),
                });
            }
            let name_raw = &bytes[base..base + 8];
            let name_end = name_raw.iter().position(|&c| c == 0).unwrap_or(8);
            sections.push(SectionInfo {
                name: String::from_utf8_lossy(&name_raw[..name_end]).into_owned(),
                rva: u32_at(bytes, base + 12)?,
                vsize: u32_at(bytes, base + 8)?,
                raw_size: u32_at(bytes, base + 16)?,
                raw_ptr: u32_at(bytes, base + 20)?,
                characteristics: u32_at(bytes, base + 36)?,
            });
        }
        Ok(PeInfo {
            machine,
            image_base,
            entry_rva,
            timestamp,
            checksum,
            sections,
            directories,
        })
    }

    /// 磁盘布局映射：RVA → 文件内偏移（走 raw_ptr/raw_size）。
    pub fn rva_to_file_offset(&self, rva: u32) -> Option<usize> {
        for s in &self.sections {
            if rva >= s.rva && rva < s.rva + s.raw_size.max(1) {
                return Some((rva - s.rva + s.raw_ptr) as usize);
            }
        }
        None
    }

    pub fn section_by_rva(&self, rva: u32) -> Option<&SectionInfo> {
        self.sections
            .iter()
            .find(|s| rva >= s.rva && rva < s.rva + s.vsize.max(s.raw_size).max(1))
    }

    /// machine 的 iced-x86 bitness（异常 machine → None，调用方诚实拒绝）。
    pub fn bitness(&self) -> Option<u32> {
        match self.machine {
            0x8664 => Some(64),
            0x14C => Some(32),
            _ => None,
        }
    }
}

/// 内存快照镜像：头 + 独立节 blob（snapshot 引擎的节内存储形态）。
/// RVA 读取按节路由，越界诚实 None。
pub struct ImageMap {
    pub pe: PeInfo,
    /// (节信息, 节字节)。字节长度 ≤ 节 vsize（读内存分块结果）。
    pub sections: Vec<(SectionInfo, Vec<u8>)>,
}

impl ImageMap {
    /// 组装：节信息按 PeInfo 节表过滤出快照里实际存在的节。
    pub fn new(pe: PeInfo, blobs: Vec<(String, Vec<u8>)>) -> Result<ImageMap, PeError> {
        let mut sections = Vec::with_capacity(blobs.len());
        for (name, bytes) in blobs {
            let info = pe
                .sections
                .iter()
                .find(|s| s.name == name)
                .ok_or(PeError::RvaUnmapped(0))?
                .clone();
            sections.push((info, bytes));
        }
        Ok(ImageMap { pe, sections })
    }

    /// 读 RVA 处 len 字节（跨节不解析——快照按节存；只在本节内截取）。
    pub fn read_rva(&self, rva: u32, len: usize) -> Option<&[u8]> {
        for (info, bytes) in &self.sections {
            if rva >= info.rva {
                let off = (rva - info.rva) as usize;
                if off < bytes.len() {
                    return Some(&bytes[off..(off + len).min(bytes.len())]);
                }
            }
        }
        None
    }

    /// 节内偏移（diff/candidates 输出用）。
    pub fn section_mut_pair(&mut self, name: &str) -> Option<&mut (SectionInfo, Vec<u8>)> {
        self.sections.iter_mut().find(|(i, _)| i.name == name)
    }
}

// ── 目录 walker（磁盘文件形态：export/import 名字表解析）──

/// 导出函数（name → rva）。
#[derive(Debug, Clone, Serialize)]
pub struct Export {
    pub name: String,
    pub rva: u32,
}

/// 导入：DLL → 函数名清单（+ IAT 槽 RVA 序列，供 xref 的 call [iat] 归因）。
#[derive(Debug, Clone, Serialize)]
pub struct ImportDll {
    pub dll: String,
    /// (函数名, iat 槽 rva)。
    pub funcs: Vec<(String, u32)>,
}

fn cstr_at(bytes: &[u8], off: usize, max: usize) -> Option<String> {
    let end = off + max.min(bytes.len().saturating_sub(off));
    let mut i = off;
    while i < end {
        if bytes[i] == 0 {
            let s = &bytes[off..i];
            if s.is_empty() || s.len() > 256 {
                return None;
            }
            return Some(String::from_utf8_lossy(s).into_owned());
        }
        i += 1;
    }
    None
}

/// 从磁盘文件字节解析导出表（需 EXPORT 目录 rva/size 有效）。
pub fn parse_exports_file(file: &[u8], pe: &PeInfo) -> Result<Vec<Export>, PeError> {
    let (dir_rva, dir_size) = pe.directories[DIRECTORY_EXPORT];
    if dir_rva == 0 || dir_size == 0 {
        return Ok(Vec::new());
    }
    let dir_off = pe
        .rva_to_file_offset(dir_rva)
        .ok_or(PeError::RvaUnmapped(dir_rva))?;
    let num_funcs = u32_at(file, dir_off + 0x14)? as usize;
    let num_names = u32_at(file, dir_off + 0x18)? as usize;
    let names_rva = u32_at(file, dir_off + 0x20)?;
    let funcs_rva = u32_at(file, dir_off + 0x1C)?;
    let names_cap = num_names.min(0x1_0000);
    let mut out = Vec::with_capacity(names_cap);
    let names_off = pe
        .rva_to_file_offset(names_rva)
        .ok_or(PeError::RvaUnmapped(names_rva))?;
    let funcs_off = pe
        .rva_to_file_offset(funcs_rva)
        .ok_or(PeError::RvaUnmapped(funcs_rva))?;
    // 名字序号表（AddressOfNameOrdinals，WORD/项）→ AddressOfFunctions 下标
    let ords_rva = u32_at(file, dir_off + 0x24)?;
    let ords_off = pe
        .rva_to_file_offset(ords_rva)
        .ok_or(PeError::RvaUnmapped(ords_rva))?;
    for i in 0..names_cap {
        let name_rva = u32_at(file, names_off + i * 4)?;
        let ord = u16_at(file, ords_off + i * 2)? as usize;
        if ord >= num_funcs.min(0x1_0000) {
            continue;
        }
        if let Some(name_off) = pe.rva_to_file_offset(name_rva)
            && let Some(name) = cstr_at(file, name_off, 512)
        {
            let func_rva = u32_at(file, funcs_off + ord * 4).unwrap_or(0);
            out.push(Export {
                name,
                rva: func_rva,
            });
        }
    }
    Ok(out)
}

/// 从磁盘文件字节解析导入表（名字 + IAT 槽 RVA）。
pub fn parse_imports_file(file: &[u8], pe: &PeInfo) -> Result<Vec<ImportDll>, PeError> {
    let (dir_rva, dir_size) = pe.directories[DIRECTORY_IMPORT];
    if dir_rva == 0 || dir_size == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    // 遍历 IMAGE_IMPORT_DESCRIPTOR（20 字节/项，全零终止；上限 512 DLL 防御）
    for di in 0..512 {
        let desc_rva = dir_rva + (di * 20) as u32;
        let desc_off = match pe.rva_to_file_offset(desc_rva) {
            Some(o) => o,
            None => break,
        };
        let orig_first_thunk = u32_at(file, desc_off)?;
        if orig_first_thunk == 0 {
            break; // 终止项
        }
        let name_rva = u32_at(file, desc_off + 12)?;
        let first_thunk = u32_at(file, desc_off + 16)?;
        let dll = pe
            .rva_to_file_offset(name_rva)
            .and_then(|o| cstr_at(file, o, 256))
            .unwrap_or_default();
        if dll.is_empty() {
            continue;
        }
        // 走 OriginalFirstThunk（名字表）；缺了退 FirstThunk（同形）
        let thunk_rva = if orig_first_thunk != 0 {
            orig_first_thunk
        } else {
            first_thunk
        };
        let mut funcs = Vec::new();
        for ti in 0..0x1_0000u32 {
            let t_off = match pe.rva_to_file_offset(thunk_rva + ti * 8) {
                Some(o) => o,
                None => break,
            };
            let thunk = u64_at(file, t_off)?;
            if thunk == 0 {
                break;
            }
            // 按序号导入（高位 1）记作 #ord；名字导入取 IMAGE_IMPORT_BY_NAME
            let fname = if thunk & 0x8000_0000_0000_0000 != 0 {
                format!("#{}", thunk & 0xFFFF)
            } else {
                let hint_rva = (thunk & 0x7FFF_FFFF) as u32;
                pe.rva_to_file_offset(hint_rva)
                    .and_then(|o| cstr_at(file, o + 2, 512))
                    .unwrap_or_else(|| format!("#unk_{hint_rva:X}"))
            };
            funcs.push((fname, first_thunk + ti * 8));
        }
        out.push(ImportDll { dll, funcs });
    }
    Ok(out)
}

/// .pdata 异常目录（DIRECTORY_EXCEPTION=3，x64 RUNTIME_FUNCTION 表）→
/// 函数 (start, end) RVA 列表。读磁盘文件形态（表内容是纯 RVA、不受
/// 重定位影响，内存与盘一致）。MSVC x64 全函数在册——递归下降够不到
/// 经 std `lang_start` 间接进入的用户 main，这里补权威种子。x86 无此
/// 目录 → None（调用方回落 nopsled 预埋扫描）。
pub fn parse_pdata_file(file: &[u8], pe: &PeInfo) -> Option<Vec<(u32, u32)>> {
    const DIRECTORY_EXCEPTION: usize = 3;
    let (dir_rva, dir_size) = *pe.directories.get(DIRECTORY_EXCEPTION)?;
    if dir_rva == 0 || dir_size < 12 {
        return None;
    }
    let base = pe.rva_to_file_offset(dir_rva)?;
    let end_off = (base + dir_size as usize).min(file.len());
    let raw = file.get(base..end_off)?;
    let mut out = Vec::with_capacity(raw.len() / 12);
    for t in raw.chunks_exact(12) {
        let start = u32::from_le_bytes(t[0..4].try_into().expect("4B 切片"));
        let end = u32::from_le_bytes(t[4..8].try_into().expect("4B 切片"));
        if end > start {
            out.push((start, end));
        }
    }
    (!out.is_empty()).then_some(out)
}
