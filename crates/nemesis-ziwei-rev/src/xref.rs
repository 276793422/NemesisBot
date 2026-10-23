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

/// 字符串引用：数据侧（串位置+内容）与代码侧（引用指令）各记一笔。
#[derive(Debug, Clone, Serialize)]
pub struct StrRef {
    /// 串所在 RVA。
    pub str_rva: u32,
    /// 串内容（截 256）。
    pub text: String,
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

/// 从数据节扫 C 串（可打印 ASCII ≥4，截 256）。
fn scan_strings(image: &ImageMap) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for (info, bytes) in &image.sections {
        if info.is_executable() || bytes.is_empty() {
            continue; // 只扫非代码节（.rdata/.data）
        }
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
                    out.push((info.rva + s as u32, text));
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
                out.push((info.rva + s as u32, text));
            }
        }
    }
    out.sort_by_key(|(rva, _)| *rva);
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
        .map(|(i, (rva, _))| (*rva, i))
        .collect();

    idx.str_refs = strings
        .into_iter()
        .map(|(rva, text)| StrRef {
            str_rva: rva,
            text,
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
