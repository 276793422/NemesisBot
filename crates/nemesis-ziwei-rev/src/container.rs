//! container —— 容器抽象（P0）。
//!
//! S1 魔数分派：MZ → PE、`\x7fELF` → ELF（声明在册，解析器 P1 批次实装，
//! 现阶段诚实报错）。S2/S4 形态注记：镜像装载（ImageMap）与种子装配在本
//! 批保持 PE 专名直用——容器统一抽象（trait 或双变体 enum）等 P1 有了
//! ElfMap 真实第二个变体再定形态，避免单变体 enum 的纯猜测抽象；
//! `BinHeaders::parse` 就是那道接缝：ELF 到来时同一入口诚实分派。
//!
//! S4 种子供给器：`pe_seed_contribs` 把 analyze 的种子装配（入口/导出/
//! pdata/CFG/TLS/nopsled）收进容器侧，analyze 只做汇总（界内过滤在供给器
//! 内完成）。全部来源可选——缺席来源诚实缺席，字节启发式（nopsled）兜底。

use crate::disasm::Arch;
use crate::funcs;
use crate::pe::{self, ImageMap, PeInfo};

/// 容器头解析产物（魔数分派）。
pub enum BinHeaders {
    Pe(PeInfo),
    /// ELF 魔数已识别（\x7fELF）；字段化解析 P1 批次实装。
    Elf,
}

impl BinHeaders {
    /// 魔数分派：MZ → PE 头解析、\x7fELF → Elf 占位；其余诚实报错。
    pub fn parse(bytes: &[u8]) -> Result<BinHeaders, String> {
        if bytes.len() >= 2 && &bytes[0..2] == b"MZ" {
            PeInfo::parse(bytes)
                .map(BinHeaders::Pe)
                .map_err(|e| e.to_string())
        } else if bytes.len() >= 4 && &bytes[0..4] == b"\x7fELF" {
            Ok(BinHeaders::Elf)
        } else {
            Err("未知容器魔数（非 MZ / ELF）".to_string())
        }
    }

    /// 架构归一（容器各自 machine 命名空间 → 内部 Arch；不认识 = None）。
    pub fn arch(&self) -> Option<Arch> {
        match self {
            BinHeaders::Pe(pe) => Arch::from_machine(pe.machine),
            BinHeaders::Elf => None, // P1：Arch::from_elf_machine(e_machine)
        }
    }

    /// 原始 machine 值（错误信息/summary 机器字展示用）。
    pub fn machine_raw(&self) -> u16 {
        match self {
            BinHeaders::Pe(pe) => pe.machine,
            BinHeaders::Elf => 0,
        }
    }
}

/// 单一来源的种子贡献（界内已过滤；summary 按源计数 = LLM 可解释性素材）。
pub struct SeedContrib {
    /// 来源标签：entry / export / pdata / cfg / tls / nopsled。
    pub source: &'static str,
    pub rvas: Vec<u32>,
}

/// PE 容器种子装配供给器（磁盘文件形态：导出/延迟导入无关的表全部解析自
/// exe 文件）。六来源全部可选：目录缺席 → 空贡献；nopsled 字节启发式恒在
/// 兜底（x86 无 .pdata / 无 CFG 表的裸二进制也有种子可下）。
pub fn pe_seed_contribs(img: &ImageMap, exe_file: &[u8]) -> Vec<SeedContrib> {
    // .text 界（首可执行节）；界外种子丢弃——递归下降活动窗口只在界内。
    let bounds = pe_text_bounds(img);
    let Some(bounds) = bounds else {
        return Vec::new();
    };
    let inb = |rva: u32| rva >= bounds.0 && rva < bounds.1;
    let mut out = Vec::new();

    // 入口
    if inb(img.pe.entry_rva) {
        out.push(SeedContrib {
            source: "entry",
            rvas: vec![img.pe.entry_rva],
        });
    }
    // 导出函数
    if let Ok(exports) = pe::parse_exports_file(exe_file, &img.pe) {
        let rvas: Vec<u32> = exports.iter().map(|e| e.rva).filter(|r| inb(*r)).collect();
        if !rvas.is_empty() {
            out.push(SeedContrib {
                source: "export",
                rvas,
            });
        }
    }
    // .pdata 异常表（x64 权威函数起点；x86 无此目录 → 空贡献）
    if let Some(list) = pe::parse_pdata_file(exe_file, &img.pe) {
        let rvas: Vec<u32> = list.iter().map(|(s, _)| *s).filter(|r| inb(*r)).collect();
        if !rvas.is_empty() {
            out.push(SeedContrib {
                source: "pdata",
                rvas,
            });
        }
    }
    // CFG 合法间接调用目标表（B2：函数发现单一最大增强）
    if let Some(list) = pe::parse_cfg_function_table(exe_file, &img.pe) {
        let rvas: Vec<u32> = list.into_iter().filter(|r| inb(*r)).collect();
        if !rvas.is_empty() {
            out.push(SeedContrib {
                source: "cfg",
                rvas,
            });
        }
    }
    // TLS 回调（B2：隐藏入口种子）
    if let Some(list) = pe::parse_tls_callbacks(exe_file, &img.pe) {
        let rvas: Vec<u32> = list.into_iter().filter(|r| inb(*r)).collect();
        if !rvas.is_empty() {
            out.push(SeedContrib {
                source: "tls",
                rvas,
            });
        }
    }
    // nopsled 预埋标记扫描兜底（夹具契约：锚点函数入口 8×0x90）
    let rvas: Vec<u32> = funcs::nopsled_seed_rvas(img)
        .into_iter()
        .filter(|r| inb(*r))
        .collect();
    if !rvas.is_empty() {
        out.push(SeedContrib {
            source: "nopsled",
            rvas,
        });
    }
    out
}

/// 首可执行节界（analyze 原 .text 定位逻辑收编；无可执行节 = None）。
pub fn pe_text_bounds(img: &ImageMap) -> Option<(u32, u32)> {
    img.pe
        .sections
        .iter()
        .find(|s| s.is_executable())
        .map(|s| (s.rva, s.rva.saturating_add(s.vsize)))
}
