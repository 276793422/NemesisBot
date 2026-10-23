//! funcs —— 递归下降函数边界识别（T3.1）。
//!
//! 种子 = 入口点 + 导出 + 直接调用目标（xref 前置：先逐种子线性解码）。
//! 停块条件（保守闭包——宁可欠识别不误识别）：坏字节 / ret 后跟非代码
//! 填充 / 撞进已识别函数体 / 间接跳转（switch 表不展开，标记后停）。
//! 每函数产出：指令表（引用扫描素材）、直接调用集合、imm/rip-rel 引用集合。

use crate::disasm::{self, Arch, Insn};
use crate::pe::ImageMap;

/// 单函数信息。
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    /// 起始 RVA。
    pub rva: u32,
    /// 结束（最后一条指令之后）。
    pub end_rva: u32,
    pub insns: Vec<Insn>,
    /// 直接调用目标集合（含出函数）。
    pub calls: Vec<u32>,
    /// 立即数集合（含地址性与数据性，归因交给 xref）。
    pub imms: Vec<u64>,
    /// rip 相对数据引用目标集合。
    pub rip_refs: Vec<u32>,
    /// 含间接分支（保守标记：调用图到此为止）。
    pub has_indirect_branch: bool,
    /// 截断（max_insns 撞顶——超长函数/数据误判为代码）。
    pub truncated: bool,
}

/// 全部已识别函数（按 rva 升序）。
#[derive(Debug, Default, Clone)]
pub struct FunctionSet {
    pub functions: Vec<FunctionInfo>,
}

impl FunctionSet {
    pub fn find_by_rva(&self, rva: u32) -> Option<&FunctionInfo> {
        self.functions.iter().find(|f| f.rva == rva)
    }
    /// RVA 是否落在某函数体内（含起点）。
    pub fn contains(&self, rva: u32) -> bool {
        self.functions
            .iter()
            .any(|f| rva >= f.rva && rva < f.end_rva)
    }
}

/// 单函数解码上限（防御：数据被误当代码时 bounded 停，不烧穿 .text）。
const MAX_INSNS_PER_FUNC: usize = 8192;

/// 递归下降恢复。
/// - `image`：内存快照（.text 必须在节 blob 内）；
/// - `seeds`：入口 RVA（入口点 + 导出 + 调用目标随过程增长）；
/// - `text_range`：(text_rva, text_end)——出界分支不追。
pub fn recover(arch: Arch, image: &ImageMap, seeds: &[u32], text_range: (u32, u32)) -> FunctionSet {
    let text_bytes = match image.read_rva(text_range.0, (text_range.1 - text_range.0) as usize) {
        Some(b) => b,
        None => return FunctionSet::default(),
    };
    let mut work: Vec<u32> = seeds.to_vec();
    work.sort_unstable();
    work.dedup();
    let mut set = FunctionSet::default();

    while let Some(start) = work.pop() {
        if set.contains(start) || start < text_range.0 || start >= text_range.1 {
            continue;
        }
        let off0 = (start - text_range.0) as usize;
        if off0 >= text_bytes.len() {
            continue;
        }
        let body = &text_bytes[off0..];
        let insns = disasm::decode_block(arch, body, start, MAX_INSNS_PER_FUNC);
        if insns.is_empty() {
            continue; // 坏种子（数据/填充）
        }
        let mut info = FunctionInfo {
            rva: start,
            end_rva: start,
            insns: Vec::with_capacity(insns.len()),
            calls: Vec::new(),
            imms: Vec::new(),
            rip_refs: Vec::new(),
            has_indirect_branch: false,
            truncated: insns.len() >= MAX_INSNS_PER_FUNC,
        };
        for insn in &insns {
            let next = insn.rva + insn.len;
            // 撞已识别函数体：本函数在此截断（重叠让位，先到先得）
            if set.contains(next) {
                break;
            }
            if let Some(t) = insn.branch_target {
                if insn.is_call() {
                    if !info.calls.contains(&t) {
                        info.calls.push(t);
                    }
                    // 直接调用目标 = 新种子（在 .text 内才追）
                    if t >= text_range.0 && t < text_range.1 && !set.contains(t) {
                        work.push(t);
                    }
                } else if !(t >= text_range.0 && t < text_range.1) {
                    // jmp 出 .text：结束（表尾/尾调用到数据边界），保守停
                    info.insns.push(insn.clone());
                    info.end_rva = next;
                    break;
                }
            }
            if insn.is_indirect_branch() {
                info.has_indirect_branch = true;
                // call [reg]/[mem] 后仍继续线性（调用点不是控制流终点）；
                // jmp [reg]/[mem] 停（目标未知，表跳转不猜）
                if insn.is_jmp() {
                    info.insns.push(insn.clone());
                    info.end_rva = next;
                    break;
                }
            }
            if let Some(r) = insn.rip_rel_target
                && !info.rip_refs.contains(&r)
            {
                info.rip_refs.push(r);
            }
            if let Some(v) = insn.imm
                && !info.imms.contains(&v)
            {
                info.imms.push(v);
            }
            if insn.is_ret() {
                info.insns.push(insn.clone());
                info.end_rva = next;
                break;
            }
            info.insns.push(insn.clone());
            info.end_rva = next;
        }
        // ret 未出现且非截断 = 撞数据/填充：仍收（部分函数体无害——
        // 边界偏保守比越界安全）
        set.functions.push(info);
    }

    set.functions.sort_by_key(|f| f.rva);
    set
}

/// nopsled 函数候选判定：函数起始 ≥ `min_nop` 字节的纯 nop 前导。
pub fn leading_nops(arch: Arch, image: &ImageMap, f: &FunctionInfo, min: usize) -> usize {
    let len = (f.end_rva - f.rva) as usize;
    if len == 0 {
        return 0;
    }
    match image.read_rva(f.rva, len) {
        Some(b) => disasm::leading_nop_bytes(arch, b, f.rva, min),
        None => 0,
    }
}

/// nopsled 预埋标记扫描（种子补充）：可执行节内 ≥4 连续 0x90 的游程起点。
/// dummy-draw 类夹具以「入口 8×0x90」预埋锚点（T0.8 契约）——x86 无
/// .pdata、递归下降又够不到经 std `lang_start` 间接进入的用户函数，
/// 由此入册。纯启发式：误报只多一个种子（recover 重叠让位兜底），
/// 不破坏正确性；函数内真实的成串 nop 指令在实践里罕见。
pub fn nopsled_seed_rvas(image: &ImageMap) -> Vec<u32> {
    let mut out = Vec::new();
    for sec in image.pe.sections.iter().filter(|s| s.is_executable()) {
        let Some(body) = image.read_rva(sec.rva, sec.raw_size as usize) else {
            continue;
        };
        let mut run = 0usize;
        for (i, &b) in body.iter().enumerate() {
            if b == 0x90 {
                run += 1;
                continue;
            }
            if run >= 4 {
                out.push(sec.rva + (i - run) as u32);
            }
            run = 0;
        }
        if run >= 4 {
            out.push(sec.rva + (body.len() - run) as u32);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}
