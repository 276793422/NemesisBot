//! xref —— 交叉引用索引（T3.2）。
//!
//! 四类索引：字符串引用（.rdata C 串 → 引用指令）、导入调用
//! （call [iat] → dll!func）、立即数常量、数据引用（rip-rel → 数据 RVA）。
//! 输入 = ImageMap + FunctionSet（funcs 的指令表是唯一扫描面——
//! 未识别区域不索引，与函数边界的保守闭包一致）。

use std::collections::HashMap;

use serde::Serialize;

use crate::funcs::FunctionSet;
use crate::pe::ImageMap;

/// 字符串引用：数据侧（串位置+内容+编码）与代码侧（引用指令）各记一笔。
#[derive(Debug, Clone, Serialize)]
pub struct StrRef {
    /// 串所在 RVA。
    pub str_rva: u32,
    /// 串内容（截 256）。
    pub text: String,
    /// UTF-16LE 宽串标记（false = C 串 ASCII）。
    pub wide: bool,
    /// 引用它的指令 RVA 列表（空 = 存在但无已识别函数引用）。
    pub refs: Vec<u32>,
}

/// 导入调用：指令 → iat 槽 → dll!func。
#[derive(Debug, Clone, Serialize)]
pub struct ImportCall {
    /// call/jmp 指令 RVA。
    pub insn_rva: u32,
    pub dll: String,
    pub func: String,
    /// IAT 槽 RVA。
    pub iat_rva: u32,
}

/// 立即数常量站点。
#[derive(Debug, Clone, Serialize)]
pub struct ImmConst {
    pub insn_rva: u32,
    pub value: u64,
}

/// 数据引用站点（rip-rel / imm 当地址）。
#[derive(Debug, Clone, Serialize)]
pub struct DataRef {
    /// 被引用的数据 RVA。
    pub data_rva: u32,
    pub insn_rva: u32,
}

/// 汇总索引。
#[derive(Debug, Default, Serialize)]
pub struct XrefIndex {
    pub str_refs: Vec<StrRef>,
    pub import_calls: Vec<ImportCall>,
    pub imm_consts: Vec<ImmConst>,
    pub data_refs: Vec<DataRef>,
}

/// 反查索引（查询面 xref_to/search/callers 的 O(1) 底座；与 [`XrefIndex`]
/// 同轮一次 O(N) 构建——查询全部 O(1)/O(k)，不重扫指令流）。
#[derive(Debug, Default, Serialize)]
pub struct ReverseIndex {
    /// 字符串内容 → 串所在数据 RVA 列表（同文重复串多址；search string
    /// 第一步，站点经 `data_by_rva` 二跳）。
    pub str_by_content: HashMap<String, Vec<u32>>,
    /// 立即数值 → 引用站点（指令 RVA）。
    pub imm_by_value: HashMap<u64, Vec<u32>>,
    /// 数据 RVA → 引用它的代码站点（含字符串数据——build 里字符串引用
    /// 同时记入 data_refs）。
    pub data_by_rva: HashMap<u32, Vec<u32>>,
    /// 被调函数起点 → 调用方函数起点集合（升序去重；callers action 直供）。
    pub callers_of: HashMap<u32, Vec<u32>>,
    /// 指令站点 → 归属函数起点（查询结果装饰用；未识别区域不在册）。
    pub site_to_func: HashMap<u32, u32>,
}

/// 构建反查索引（输入 = 正向 [`XrefIndex`] + [`FunctionSet`]——不重复解码）。
pub fn build_reverse(idx: &XrefIndex, funcs: &FunctionSet) -> ReverseIndex {
    let mut rev = ReverseIndex::default();
    for s in &idx.str_refs {
        rev.str_by_content
            .entry(s.text.clone())
            .or_default()
            .push(s.str_rva);
    }
    for c in &idx.imm_consts {
        rev.imm_by_value
            .entry(c.value)
            .or_default()
            .push(c.insn_rva);
    }
    for d in &idx.data_refs {
        rev.data_by_rva
            .entry(d.data_rva)
            .or_default()
            .push(d.insn_rva);
    }
    for f in &funcs.functions {
        for insn in &f.insns {
            rev.site_to_func.insert(insn.rva, f.rva);
        }
        for t in &f.calls {
            let callers = rev.callers_of.entry(*t).or_default();
            if !callers.contains(&f.rva) {
                callers.push(f.rva);
            }
        }
    }
    rev
}

/// 从数据节扫字符串（C 串 + UTF-16LE 宽串，可打印 ≥4 字符，截 256）。
/// 两形态互斥零误报：ASCII 扫描对宽串天然免疫（每字符第二字节 0x00 截断
/// run，长度恒 <4），宽扫描要求 [printable, 0x00] 严格交替、纯 ASCII 不
/// 满足。v1 边界：宽串只认 ASCII 子集——CJK 等非 ASCII 宽字符使 run 截断
/// （与 ASCII 扫描器对非 ASCII 字节的盲区同源，诚实缺席不误报）。
fn scan_strings(image: &ImageMap) -> Vec<(u32, String, bool)> {
    let mut out = Vec::new();
    for (info, bytes) in &image.sections {
        if info.is_executable() || bytes.is_empty() {
            continue; // 只扫非代码节（.rdata/.data）
        }
        // C 串（ASCII run）
        let mut run_start = None;
        for (i, &b) in bytes.iter().enumerate() {
            let printable = (0x20..0x7F).contains(&b);
            if printable && run_start.is_none() {
                run_start = Some(i);
            } else if !printable && let Some(s) = run_start.take() {
                let len = i - s;
                if len >= 4 {
                    let text = String::from_utf8_lossy(&bytes[s..i])
                        .chars()
                        .take(256)
                        .collect();
                    out.push((info.rva + s as u32, text, false));
                }
            }
        }
        // 节尾截断的 run（节以可打印字符结尾）
        if let Some(s) = run_start {
            let len = bytes.len() - s;
            if len >= 4 {
                let text = String::from_utf8_lossy(&bytes[s..])
                    .chars()
                    .take(256)
                    .collect();
                out.push((info.rva + s as u32, text, false));
            }
        }
        // UTF-16LE 宽串：[printable, 0x00] 交替 run，字符数 ≥4
        let mut i = 0;
        while i + 1 < bytes.len() {
            if (0x20..0x7F).contains(&bytes[i]) && bytes[i + 1] == 0 {
                let mut j = i;
                while j + 1 < bytes.len() && (0x20..0x7F).contains(&bytes[j]) && bytes[j + 1] == 0 {
                    j += 2;
                }
                if (j - i) / 2 >= 4 {
                    let text: String = bytes[i..j]
                        .chunks_exact(2)
                        .map(|c| c[0] as char)
                        .take(256)
                        .collect();
                    out.push((info.rva + i as u32, text, true));
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    out.sort_by_key(|(rva, _, _)| *rva);
    out
}

/// 构建索引。`imports` 来自 pe::parse_imports_file（IAT 槽归因）。
pub fn build(image: &ImageMap, funcs: &FunctionSet, imports: &[crate::pe::ImportDll]) -> XrefIndex {
    let mut idx = XrefIndex::default();

    // IAT 槽 → (dll, func)
    let mut iat_map: HashMap<u32, (&str, &str)> = HashMap::new();
    for d in imports {
        for (fname, slot) in &d.funcs {
            iat_map.insert(*slot, (&d.dll, fname));
        }
    }

    // 字符串集 + rva→index
    let strings = scan_strings(image);
    let str_idx: HashMap<u32, usize> = strings
        .iter()
        .enumerate()
        .map(|(i, (rva, _, _))| (*rva, i))
        .collect();

    idx.str_refs = strings
        .into_iter()
        .map(|(rva, text, wide)| StrRef {
            str_rva: rva,
            text,
            wide,
            refs: Vec::new(),
        })
        .collect();

    for f in &funcs.functions {
        for insn in &f.insns {
            // 导入调用：rip-rel 内存操作数落在 IAT 槽
            if let Some(target) = insn.rip_rel_target {
                if let Some((dll, func)) = iat_map.get(&target) {
                    idx.import_calls.push(ImportCall {
                        insn_rva: insn.rva,
                        dll: (*dll).to_string(),
                        func: (*func).to_string(),
                        iat_rva: target,
                    });
                    continue;
                }
                // 数据引用（非 IAT）
                idx.data_refs.push(DataRef {
                    data_rva: target,
                    insn_rva: insn.rva,
                });
                // 字符串归因
                if let Some(&si) = str_idx.get(&target) {
                    idx.str_refs[si].refs.push(insn.rva);
                }
            }
            if let Some(v) = insn.imm {
                idx.imm_consts.push(ImmConst {
                    insn_rva: insn.rva,
                    value: v,
                });
            }
        }
    }
    idx
}
