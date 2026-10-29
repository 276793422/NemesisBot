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

/// 数据目录索引（我们关心的子集；B2 六解析器补齐 TLS/LoadConfig/DelayImport/
/// Debug/Reloc 五个索引——口径对齐 winnt.h IMAGE_DIRECTORY_ENTRY）。
pub const DIRECTORY_EXPORT: usize = 0;
pub const DIRECTORY_IMPORT: usize = 1;
pub const DIRECTORY_RELOC: usize = 5;
pub const DIRECTORY_DEBUG: usize = 6;
pub const DIRECTORY_TLS: usize = 9;
pub const DIRECTORY_LOAD_CONFIG: usize = 10;
pub const DIRECTORY_DELAY_IMPORT: usize = 13;
pub const DIRECTORY_EXCEPTION: usize = 3;
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
            // 节界比较走 u64（BUG-20261029-154 同族）：损坏 PE 的节
            // rva/raw_size 可顶到 u32 边界，u32 加法 debug panic / release
            // 回绕——把「诚实映射失败」变成崩溃或错误节命中。
            let start = s.rva as u64;
            let end = start + s.raw_size.max(1) as u64;
            let r = rva as u64;
            if r >= start && r < end {
                // 文件偏移同样走 u64：raw_ptr + 节内跨度可破 u32 域
                return Some(((r - start) + s.raw_ptr as u64) as usize);
            }
        }
        None
    }

    pub fn section_by_rva(&self, rva: u32) -> Option<&SectionInfo> {
        let r = rva as u64;
        self.sections
            .iter()
            .find(|s| r >= s.rva as u64 && r < s.rva as u64 + s.vsize.max(s.raw_size).max(1) as u64)
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
        // 目录基址越界即断（BUG-20261029-154 同族：checked 防 u32 加法溢出）
        let desc_rva = match dir_rva.checked_add((di * 20) as u32) {
            Some(v) => v,
            None => break,
        };
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
            let thunk_i = match thunk_rva.checked_add(ti * 8) {
                Some(v) => v,
                None => break, // 名字表基址越界：后续槽位必然越界
            };
            let t_off = match pe.rva_to_file_offset(thunk_i) {
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
            let slot = match first_thunk.checked_add(ti * 8) {
                Some(v) => v,
                None => break, // IAT 基址越界：槽位记录无效，诚实止步
            };
            funcs.push((fname, slot));
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

// ── B2 目录解析面（六解析器；磁盘文件形态，缺席 = None/空，绝不 panic）──

/// VA → RVA（镜像基址归一；低于基址的野 VA 诚实 None）。
fn va_to_rva(va: u64, image_base: u64) -> Option<u32> {
    va.checked_sub(image_base)
        .and_then(|d| u32::try_from(d).ok())
}

/// LoadConfig.GuardCFFunctionTable → 合法间接调用目标 RVA 表（CFG 函数表）。
/// 函数发现单一最大增强：MSVC /guard:cf 二进制的全函数权威清单（含间接
/// 调用/虚表目标，递归下降够不到的）。目录或字段缺席 → None；条目表
/// 读取中断 → 返回已收集部分（半表好过全无，调用方可见信任度递减）。
pub fn parse_cfg_function_table(file: &[u8], pe: &PeInfo) -> Option<Vec<u32>> {
    // winnt.h IMAGE_LOAD_CONFIG_DIRECTORY64 逐字段对齐偏移（Size/TimeStamp/
    // Version/GlobalFlags×2/CriticalSectionTimeout + 7×ULONGLONG + DWORD/2×WORD
    // + 3×ULONGLONG 之后）：GuardCFFunctionTable=0x80(128)、Count=0x88(136)、
    // Flags=0x90(144)。对质过 Windows SDK 10.0.26100 winnt.h。
    const OFF_GUARD_TABLE: usize = 128; // IMAGE_LOAD_CONFIG_DIRECTORY64.GuardCFFunctionTable
    const OFF_GUARD_COUNT: usize = 136; // .GuardCFFunctionCount
    const OFF_GUARD_FLAGS: usize = 144; // .GuardFlags
    let (dir_rva, dir_size) = *pe.directories.get(DIRECTORY_LOAD_CONFIG)?;
    if dir_rva == 0 || (dir_size as usize) < OFF_GUARD_FLAGS + 4 {
        return None;
    }
    let dir_off = pe.rva_to_file_offset(dir_rva)?;
    let table_va = u64_at(file, dir_off + OFF_GUARD_TABLE).ok()?;
    let count = u64_at(file, dir_off + OFF_GUARD_COUNT).ok()? as usize;
    let flags = u32_at(file, dir_off + OFF_GUARD_FLAGS).ok()?;
    if table_va == 0 || count == 0 {
        return None;
    }
    // 条目格式（GFIDS 表）：每条目 = 4 字节 LE RVA + GuardFlags 高 4 位字节的
    // 补充元数据（stride = 4 + meta）。真 PE 对质（Win11 notepad/cmd/where
    // 探针）：131/142/8 条全读出升序 .text RVA；若按 8 字节 VA 读则全表无法
    // 映射回镜像——RVA 形态是唯一与真实二进制自洽的解读。
    let meta_bytes = ((flags >> 28) & 0xF) as usize;
    let stride = 4 + meta_bytes;
    let count = count.min(0x10_0000); // 防御：正常模块远小于此
    let base_rva = va_to_rva(table_va, pe.image_base)?;
    let base_off = pe.rva_to_file_offset(base_rva)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let Ok(rva) = u32_at(file, base_off + i * stride) else {
            break; // 条目表读取中断 → 半表好过全无
        };
        if rva != 0 {
            out.push(rva);
        }
    }
    out.sort_unstable();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

/// TLS 目录回调数组（dd[9]）→ 回调函数 RVA 清单（隐藏入口种子：动态库
/// 加载/DLL_PROCESS_ATTACH 前最先执行的用户代码，递归下降与 .pdata 都
/// 不覆盖 TLS 回调）。AddressOfCallBacks 是 VA 指针数组（NULL 终止）。
pub fn parse_tls_callbacks(file: &[u8], pe: &PeInfo) -> Option<Vec<u32>> {
    const OFF_CALLBACKS: usize = 24; // IMAGE_TLS_DIRECTORY64.AddressOfCallBacks
    let (dir_rva, dir_size) = *pe.directories.get(DIRECTORY_TLS)?;
    if dir_rva == 0 || (dir_size as usize) < OFF_CALLBACKS + 8 {
        return None;
    }
    let dir_off = pe.rva_to_file_offset(dir_rva)?;
    let cb_va = u64_at(file, dir_off + OFF_CALLBACKS).ok()?;
    let arr_rva = va_to_rva(cb_va, pe.image_base)?;
    if arr_rva == 0 {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..64u32 {
        // 防御：正常回调数组个位数，64 封顶
        let Some(slot_off) = pe.rva_to_file_offset(arr_rva.wrapping_add(i * 8)) else {
            break;
        };
        let Ok(fn_va) = u64_at(file, slot_off) else {
            break;
        };
        if fn_va == 0 {
            break; // NULL 终止
        }
        if let Some(rva) = va_to_rva(fn_va, pe.image_base) {
            out.push(rva);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// 延迟加载导入表（dd[13]）→ ImportDll 清单（与 parse_imports_file 同形——
/// xref 的 call [iat] 归因与 import_dlls 计数自动覆盖延迟导入；缺名补全）。
/// grAttrs bit0（dlattrRva）= 字段是 RVA，否则 VA（旧形态，减基址归一）。
pub fn parse_delay_imports_file(file: &[u8], pe: &PeInfo) -> Result<Vec<ImportDll>, PeError> {
    let (dir_rva, dir_size) = pe.directories[DIRECTORY_DELAY_IMPORT];
    if dir_rva == 0 || dir_size == 0 {
        return Ok(Vec::new());
    }
    // 字段读出来先归一成 RVA 再用（attr bit0 决定要不要减基址）。
    let norm = |v: u32, rva_form: bool| -> u32 {
        if rva_form || v == 0 {
            v
        } else {
            va_to_rva(v as u64, pe.image_base).unwrap_or(0)
        }
    };
    let mut out = Vec::new();
    // IMAGE_DELAYLOAD_DESCRIPTOR 32 字节/项，全零终止；上限 512 防御
    for di in 0..512 {
        let desc_off = match dir_rva.checked_add((di * 32) as u32) {
            Some(v) => pe.rva_to_file_offset(v),
            None => None, // 目录基址越界（BUG-20261029-154 同族）
        };
        let desc_off = match desc_off {
            Some(o) => o,
            None => break,
        };
        let attrs = u32_at(file, desc_off)?;
        let name_field = u32_at(file, desc_off + 4)?;
        let int_field = u32_at(file, desc_off + 16)?;
        let iat_field = u32_at(file, desc_off + 12)?;
        if attrs == 0 && name_field == 0 && int_field == 0 && iat_field == 0 {
            break; // 终止项
        }
        let rva_form = attrs & 1 != 0;
        let name_rva = norm(name_field, rva_form);
        let dll = pe
            .rva_to_file_offset(name_rva)
            .and_then(|o| cstr_at(file, o, 256))
            .unwrap_or_default();
        if dll.is_empty() {
            continue;
        }
        // 名字表（pINT）优先；缺了退 IAT（盘上两者同镜像）
        let thunk_rva = match (norm(int_field, rva_form), norm(iat_field, rva_form)) {
            (0, 0) => continue,
            (int_rva, iat_rva) => {
                if int_rva != 0 {
                    int_rva
                } else {
                    iat_rva
                }
            }
        };
        let iat_rva = norm(iat_field, rva_form);
        let mut funcs = Vec::new();
        for ti in 0..0x1_0000u32 {
            let thunk_i = match thunk_rva.checked_add(ti * 8) {
                Some(v) => v,
                None => break,
            };
            let Some(t_off) = pe.rva_to_file_offset(thunk_i) else {
                break;
            };
            let thunk = u64_at(file, t_off)?;
            if thunk == 0 {
                break;
            }
            let fname = if thunk & 0x8000_0000_0000_0000 != 0 {
                format!("#{}", thunk & 0xFFFF)
            } else {
                let hint_rva = (thunk & 0x7FFF_FFFF) as u32;
                pe.rva_to_file_offset(hint_rva)
                    .and_then(|o| cstr_at(file, o + 2, 512))
                    .unwrap_or_else(|| format!("#unk_{hint_rva:X}"))
            };
            let slot = match iat_rva.checked_add(ti * 8) {
                Some(v) => v,
                None => break,
            };
            funcs.push((fname, slot));
        }
        out.push(ImportDll { dll, funcs });
    }
    Ok(out)
}

/// 单条 debug 目录项（we 只投影 type/size/rva 三字段；payload 就地读取）。
#[derive(Debug, Clone, Serialize)]
pub struct DebugEntry {
    /// IMAGE_DEBUG_TYPE（2 = CODEVIEW）。
    pub ty: u32,
    pub size: u32,
    pub rva: u32,
}

/// Debug 目录（dd[6]）解析产物：条目表 + CodeView PDB 路径（白送的语义：
/// 真名/源码结构线索，逆向黄金线索第一名）。
#[derive(Debug, Clone, Serialize)]
pub struct DebugInfo {
    pub entries: Vec<DebugEntry>,
    /// RSDS/NB10 CodeView 载荷里的 PDB 路径（缺席 None）。
    pub pdb_path: Option<String>,
}

/// Debug 目录 → 条目表 + PDB 路径。PointerToRawData 优先（盘上直读），
/// 缺了退 AddressOfRawData（RVA 映射）。
pub fn parse_debug_file(file: &[u8], pe: &PeInfo) -> Option<DebugInfo> {
    let (dir_rva, dir_size) = *pe.directories.get(DIRECTORY_DEBUG)?;
    if dir_rva == 0 || dir_size < 28 {
        return None;
    }
    let mut entries = Vec::new();
    let mut pdb_path = None;
    let n = (dir_size as usize / 28).min(64); // 防御
    for i in 0..n {
        let Some(off) = dir_rva
            .checked_add((i * 28) as u32)
            .and_then(|rva| pe.rva_to_file_offset(rva))
        else {
            break;
        };
        let Ok(ty) = u32_at(file, off + 12) else {
            break;
        };
        let Ok(size) = u32_at(file, off + 16) else {
            break;
        };
        let Ok(rva) = u32_at(file, off + 20) else {
            break;
        };
        let Ok(raw) = u32_at(file, off + 24) else {
            break;
        };
        if ty == 2 && pdb_path.is_none() {
            // CODEVIEW：载荷定位优先文件偏移，缺席退 RVA 映射
            let payload = if raw != 0 {
                file.get(raw as usize..)
            } else {
                pe.rva_to_file_offset(rva).and_then(|o| file.get(o..))
            };
            if let Some(p) = payload
                && p.len() >= 24
            {
                // RSDS：magic 4 + GUID 16 + age 4 → 路径；NB10：magic 4 + sig 4 + age 4 → 路径
                let (skip, ok_magic) = if &p[0..4] == b"RSDS" {
                    (24usize, true)
                } else if &p[0..4] == b"NB10" {
                    (12usize, true)
                } else {
                    (0, false)
                };
                if ok_magic {
                    pdb_path = cstr_at(p, skip, 512);
                }
            }
        }
        entries.push(DebugEntry { ty, size, rva });
    }
    if entries.is_empty() && pdb_path.is_none() {
        return None;
    }
    Some(DebugInfo { entries, pdb_path })
}

/// Rich 头单条工具链指纹（prod_id 对应 VS 产品号；build = 编译器 build 号）。
#[derive(Debug, Clone, Serialize)]
pub struct RichProd {
    pub prod_id: u16,
    pub build: u16,
    pub count: u16,
}

/// Rich 头解析产物：XOR key + 工具链使用计数清单（DOS stub 编译器指纹）。
#[derive(Debug, Clone, Serialize)]
pub struct RichInfo {
    pub key: u32,
    pub prods: Vec<RichProd>,
}

/// DOS stub Rich 头（MSVC 链接器写入的编译器版本指纹）：stub 区找 "Rich"
/// 标记 → 后随 4 字节 XOR key → 往前找 16 对齐的 "DanS"^key 起始 → 条目
/// 区按 8 字节 {id^key, count^key} 解。无 Rich（MinGW/strip 过）= None。
pub fn parse_rich(headers: &[u8]) -> Option<RichInfo> {
    const DANSS: u32 = 0x536E_6144; // "DanS" LE
    const RICH_TAG: u32 = 0x6863_6952; // "Rich" LE
    // 标记必然在头页 DOS stub 区（e_lfanew 之前；上限 0x400 防御）
    let scan_end = headers.len().min(0x400);
    let mut rich_off = None;
    let mut i = 0x40usize;
    while i + 8 <= scan_end {
        if u32::from_le_bytes(headers[i..i + 4].try_into().ok()?) == RICH_TAG {
            rich_off = Some(i);
            break;
        }
        i += 8; // Rich 区 16 对齐，标记搜索 8 步进即可不漏
    }
    let rich = rich_off?;
    let key = u32::from_le_bytes(headers[rich + 4..rich + 8].try_into().ok()?);
    if key == 0 {
        return None;
    }
    // 从 Rich 往前按 8 步进找 DanS^key（16 对齐惯例，8 步进保不漏）
    let mut dans = None;
    let mut j = (rich / 8) * 8;
    while j >= 0x40 {
        let Ok(v) = <[u8; 4]>::try_from(&headers[j..j + 4]) else {
            break;
        };
        if u32::from_le_bytes(v) == DANSS ^ key {
            dans = Some(j);
            break;
        }
        j -= 8;
    }
    let dans = dans?;
    // 条目区 [dans+16, rich)，8 字节/条：(id^key, count^key)；id 低半 prod_id 高半 build
    let mut prods = Vec::new();
    let mut off = dans + 16;
    while off + 8 <= rich {
        let id = u32::from_le_bytes(headers[off..off + 4].try_into().ok()?) ^ key;
        let cw = u32::from_le_bytes(headers[off + 4..off + 8].try_into().ok()?) ^ key;
        off += 8;
        if id == 0 {
            continue; // padding
        }
        prods.push(RichProd {
            prod_id: (id & 0xFFFF) as u16,
            build: (id >> 16) as u16,
            count: (cw & 0xFFFF) as u16,
        });
    }
    (!prods.is_empty()).then_some(RichInfo { key, prods })
}

/// 单个重定位块（页基址 + 页内 12 位偏移清单；type 0 padding 已滤）。
#[derive(Debug, Clone, Serialize)]
pub struct RelocBlock {
    pub page_rva: u32,
    /// 页内偏移（低 12 位；完整地址 = page_rva + offset）。type 随块统一
    /// DIR64/HIGHLOW，v2 池 vtable 扫描先验只消费地址本身。
    pub offsets: Vec<u16>,
}

/// 重定位表（dd[5]）解析产物。B2 只存表（v2 池 vtable 扫描先验），查询面
/// 不暴露。
#[derive(Debug, Clone, Serialize)]
pub struct RelocTable {
    pub blocks: Vec<RelocBlock>,
    /// 全表非 padding 条目总数。
    pub total_entries: usize,
}

/// Base 重定位表 → 块清单。块 {PageRVA, BlockSize} 头 + (BlockSize-8)/2 个
/// u16（高 4 位 type，0 = ABSOLUTE padding 跳过）。
pub fn parse_relocations_file(file: &[u8], pe: &PeInfo) -> Option<RelocTable> {
    let (dir_rva, dir_size) = *pe.directories.get(DIRECTORY_RELOC)?;
    if dir_rva == 0 || dir_size < 8 {
        return None;
    }
    let mut blocks = Vec::new();
    let mut total = 0usize;
    // 块游标按目录起点 + 已消费 BlockSize 走；64 块 / 4MB 防御
    let mut cur = dir_rva;
    let end_rva = dir_rva.saturating_add(dir_size);
    // 余量判边界而非 cur+8（BUG-20261029-154 同族：cur 可顶到 u32::MAX，
    // cur+8 debug panic）
    while end_rva.saturating_sub(cur) >= 8 && blocks.len() < 64 {
        let Some(off) = pe.rva_to_file_offset(cur) else {
            break;
        };
        let Ok(page) = u32_at(file, off) else {
            break;
        };
        let Ok(bsize) = u32_at(file, off + 4) else {
            break;
        };
        if bsize < 8 {
            break; // 坏块（不足头）诚实止步
        }
        let bsize = bsize.min(end_rva.saturating_sub(cur)); // 越界块钳制
        let n = ((bsize - 8) / 2) as usize;
        let mut offsets = Vec::with_capacity(n);
        for k in 0..n {
            let Ok(w) = u16_at(file, off + 8 + k * 2) else {
                break;
            };
            if w >> 12 == 0 {
                continue; // IMAGE_REL_BASED_ABSOLUTE = padding
            }
            offsets.push(w & 0x0FFF);
        }
        total += offsets.len();
        blocks.push(RelocBlock {
            page_rva: page,
            offsets,
        });
        cur += bsize;
    }
    if blocks.is_empty() {
        return None;
    }
    Some(RelocTable {
        blocks,
        total_entries: total,
    })
}
