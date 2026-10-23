//! candidates —— 锚候选启发式（SetupManager `candidates` 步的分析内核）。
//!
//! 输出锚候选（AnchorType 载荷形态 + 置信注记）：
//! - nopsled ≥4 → byte_pattern（载荷 = nop 串后的首指令字节序列）；
//! - 独特立即数 → imm_const（值域过滤：排除小整数/导入 thunk 区）；
//! - 字符串引用 → str_ref（文本即锚载荷）；
//! - 函数内导入调用序列 → import_call_seq（dll!func 有序子序列）。
//!
//! 候选是**建议**：采纳走显式 annotate（≥1 锚强制），不直接落库。

use serde::Serialize;

use crate::crtsigs;
use crate::funcs::FunctionSet;
use crate::pe::ImageMap;
use crate::xref::XrefIndex;

/// 锚种类（与 nemesis-ziwei-profile 的 AnchorType 同形——字符串形态
/// 跨 crate 传递，窄门面不做类型耦合）。
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    /// "byte_pattern" / "imm_const" / "str_ref" / "import_call_seq"。
    pub kind: String,
    /// 所属函数 RVA（import_call_seq/byte_pattern 有；str_ref/imm_const
    /// 可能悬空在数据侧）。
    pub function_rva: Option<u32>,
    /// 载荷（byte_pattern=hex；imm_const=0x…；str_ref=文本；
    /// import_call_seq="dll!func,dll!func,…"）。
    pub payload: String,
    /// 置信注记（人读；advisory）。
    pub note: String,
}

/// 生成候选（按函数扫描，CRT 折叠的排除）。
pub fn generate(
    image: &ImageMap,
    funcs: &FunctionSet,
    xref: &XrefIndex,
    crt: &[crtsigs::CrtSuspect],
    max_per_kind: usize,
) -> Vec<Candidate> {
    let folded: std::collections::HashSet<u32> = crt.iter().map(|c| c.rva).collect();
    let mut out = Vec::new();

    // ── 1. nopsled → byte_pattern ──
    let mut pat_count = 0usize;
    for f in &funcs.functions {
        if folded.contains(&f.rva) || pat_count >= max_per_kind {
            continue;
        }
        // 函数前导 nop 串 ≥4 → 锚 = nop 串（前导形态本身就是稳定特征）
        let len = (f.end_rva - f.rva) as usize;
        if len == 0 {
            continue;
        }
        if let Some(body) = image.read_rva(f.rva, len.min(64)) {
            let nops = body.iter().take_while(|&&b| b == 0x90).count();
            if nops >= 4 {
                let payload_len = (nops + 8).min(body.len());
                out.push(Candidate {
                    kind: "byte_pattern".into(),
                    function_rva: Some(f.rva),
                    payload: hex::encode_upper(&body[..payload_len]),
                    note: format!("前导 nop ×{nops}（nopsled 特征）"),
                });
                pat_count += 1;
            }
        }
    }

    // ── 2. 独特立即数 → imm_const ──
    // 排除：小整数（< 0x10000，多半是尺寸/循环量）、导入 thunk 区引用
    // 频繁值（出现 >2 次的 = 非独特）。
    let mut imm_hist: std::collections::HashMap<u64, Vec<u32>> = std::collections::HashMap::new();
    for c in &xref.imm_consts {
        imm_hist.entry(c.value).or_default().push(c.insn_rva);
    }
    let mut imm_sorted: Vec<_> = imm_hist
        .iter()
        .filter(|(v, sites)| **v >= 0x1_0000 && sites.len() <= 2)
        .collect();
    imm_sorted.sort_by_key(|(v, _)| **v);
    for (v, sites) in imm_sorted.into_iter().take(max_per_kind) {
        let fn_rva = funcs
            .functions
            .iter()
            .find(|f| sites.iter().any(|s| *s >= f.rva && *s < f.end_rva))
            .map(|f| f.rva);
        if fn_rva.is_some_and(|r| folded.contains(&r)) {
            continue;
        }
        out.push(Candidate {
            kind: "imm_const".into(),
            function_rva: fn_rva,
            payload: format!("0x{v:X}"),
            note: format!("独特立即数（{} 处引用）", sites.len()),
        });
    }

    // ── 3. 字符串引用 → str_ref ──
    for s in xref.str_refs.iter().take(max_per_kind) {
        if s.refs.is_empty() {
            continue; // 无已识别函数引用的裸串不推（噪声）
        }
        let fn_rva = funcs
            .functions
            .iter()
            .find(|f| s.refs.iter().any(|r| *r >= f.rva && *r < f.end_rva))
            .map(|f| f.rva);
        if fn_rva.is_some_and(|r| folded.contains(&r)) {
            continue;
        }
        out.push(Candidate {
            kind: "str_ref".into(),
            function_rva: fn_rva,
            payload: s.text.clone(),
            note: format!("字符串 @0x{:X}，{} 处引用", s.str_rva, s.refs.len()),
        });
    }

    // ── 4. 导入调用序列 → import_call_seq ──
    // 按函数聚合 ≥2 个不同导入的有序序列（单导入太常见，噪声大）。
    let mut per_fn: std::collections::HashMap<u32, Vec<(u32, String)>> =
        std::collections::HashMap::new();
    for c in &xref.import_calls {
        if let Some(f) = funcs
            .functions
            .iter()
            .find(|f| c.insn_rva >= f.rva && c.insn_rva < f.end_rva)
        {
            let e = per_fn.entry(f.rva).or_default();
            e.push((c.insn_rva, format!("{}!{}", c.dll, c.func)));
        }
    }
    let mut seqs: Vec<(u32, Vec<String>)> = per_fn
        .into_iter()
        .filter(|(rva, calls)| !folded.contains(rva) && calls.len() >= 2)
        .map(|(rva, mut calls)| {
            calls.sort_by_key(|(r, _)| *r);
            (rva, calls.into_iter().map(|(_, n)| n).collect())
        })
        .collect();
    seqs.sort_by_key(|(rva, _)| *rva);
    for (rva, names) in seqs.into_iter().take(max_per_kind) {
        out.push(Candidate {
            kind: "import_call_seq".into(),
            function_rva: Some(rva),
            payload: names.join(","),
            note: format!("函数内导入调用序列 ×{}", names.len()),
        });
    }

    out
}
