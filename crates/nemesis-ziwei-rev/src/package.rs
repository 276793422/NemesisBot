//! package —— LLM 上下文打包（T3.6）。
//!
//! 原则：**不给裸全量汇编**。打包 = 有界函数表（RVA/大小/调用出度/
//! 导入调用名/字符串引用）+ 按需单函数有界片段（助记符流截断）。
//! LLM 经 [`LlmFn`] hook 注入（OnceLock 桥——gateway ctx.rs 装桥，
//! default_slot provider 裸调用；rev/ziwei 不直依赖 providers）。
//! 应答标注解析出的**候选语义名只是建议**——落库唯一通路 =
//! 用户/Agent 显式 annotate（候选与注释不直接落库）。

use std::sync::{Arc, OnceLock};

use serde::Serialize;

use crate::crtsigs;
use crate::funcs::FunctionSet;
use crate::xref::XrefIndex;

/// LLM 调用钩子类型（prompt → reply text）。
pub type LlmFn = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

static LLM_HOOK: OnceLock<LlmFn> = OnceLock::new();

/// 安装钩子（幂等；重复 set 忽略后到者——与 ziwei refresh 钩子同款纪律）。
pub fn set_llm_hook(hook: LlmFn) -> bool {
    LLM_HOOK.set(hook).is_ok()
}

/// 取钩子（未装配 = None——调用方诚实降级为无 LLM 路径）。
pub fn llm_hook() -> Option<LlmFn> {
    LLM_HOOK.get().cloned()
}

/// 函数表条目（打包素材）。
#[derive(Debug, Clone, Serialize)]
pub struct FuncRow {
    pub rva: u32,
    pub size: u32,
    pub insns: usize,
    pub calls_out: usize,
    pub imports: Vec<String>,
    pub strings: Vec<String>,
    /// CRT 折叠建议命中（折叠后不出现在标注候选里）。
    pub crt_folded: Option<String>,
}

/// 有界打包：函数表（截 `max_funcs` 行，CRT 已折叠的压缩为一行注记）。
pub fn function_table(
    funcs: &FunctionSet,
    xref: &XrefIndex,
    crt: &[crtsigs::CrtSuspect],
    max_funcs: usize,
) -> Vec<FuncRow> {
    let folded: std::collections::HashSet<u32> = crt.iter().map(|c| c.rva).collect();
    let mut rows = Vec::new();
    for f in &funcs.functions {
        let is_folded = folded.contains(&f.rva);
        let imports: Vec<String> = xref
            .import_calls
            .iter()
            .filter(|c| f.insns.iter().any(|i| i.rva == c.insn_rva))
            .take(6)
            .map(|c| format!("{}!{}", c.dll, c.func))
            .collect();
        let strings: Vec<String> = xref
            .str_refs
            .iter()
            .filter(|s| s.refs.iter().any(|r| *r >= f.rva && *r < f.end_rva))
            .take(4)
            .map(|s| s.text.chars().take(48).collect())
            .collect();
        rows.push(FuncRow {
            rva: f.rva,
            size: f.end_rva - f.rva,
            insns: f.insns.len(),
            calls_out: f.calls.len(),
            imports,
            strings,
            crt_folded: if is_folded {
                crt.iter()
                    .find(|c| c.rva == f.rva)
                    .map(|c| c.reason.clone())
            } else {
                None
            },
        });
        if rows.len() >= max_funcs {
            break;
        }
    }
    rows
}

/// 单函数有界片段（助记符流，截 `max_insns` 条；不给完整编码字节）。
pub fn function_snippet(funcs: &FunctionSet, rva: u32, max_insns: usize) -> Option<String> {
    let f = funcs.find_by_rva(rva)?;
    let mut out = String::new();
    for insn in f.insns.iter().take(max_insns) {
        out.push_str(&format!("{:08X}  {:?}\n", insn.rva, insn.mnemonic));
    }
    if f.insns.len() > max_insns {
        out.push_str(&format!("…（共 {} 条，已截断）\n", f.insns.len()));
    }
    Some(out)
}

/// 打包 prompt（分析总览：函数表 + 指令 + 已知锚，要求输出建议标注）。
pub fn build_prompt(
    exe_name: &str,
    arch: &str,
    table: &[FuncRow],
    focus_rvas: &[u32],
    max_chars: usize,
) -> String {
    let mut p = String::new();
    p.push_str(&format!(
        "以下是对目标程序 {exe_name}（{arch}）内存快照做静态分析得到的函数表。\n\
         请为每个函数建议语义名（snake_case）与一句话用途说明；只输出如下形态的行，\n\
         每行一个函数：RVA=<hex> NAME=<名> DESC=<一句话>。不确定就写 DESC=unknown。\n\
         注意：标 crt_folded 的为库函数折叠，无需建议。\n\n"
    ));
    for r in table {
        let line = format!(
            "- RVA={:X} size={} insns={} calls={}{}{}{}\n",
            r.rva,
            r.size,
            r.insns,
            r.calls_out,
            if r.imports.is_empty() {
                String::new()
            } else {
                format!(" imports={}", r.imports.join(","))
            },
            if r.strings.is_empty() {
                String::new()
            } else {
                format!(" strs={:?}", r.strings)
            },
            if let Some(c) = &r.crt_folded {
                format!(" crt_folded={c}")
            } else {
                String::new()
            },
        );
        if p.len() + line.len() > max_chars {
            p.push_str("…（函数表已截断）\n");
            break;
        }
        p.push_str(&line);
    }
    if !focus_rvas.is_empty() {
        p.push_str("\n重点关注 RVA：\n");
        for r in focus_rvas {
            p.push_str(&format!("- {r:X}\n"));
        }
    }
    p
}

/// LLM 应答标注（建议形态；不落库）。
#[derive(Debug, Clone, Serialize)]
pub struct SuggestedAnnotation {
    pub rva: u32,
    pub name: String,
    pub desc: String,
}

/// 解析应答（RVA=<hex> NAME=<名> DESC=<话> 行形态；解析失败的行忽略——
/// LLM 输出不可信，宁缺毋滥）。
pub fn parse_annotations(reply: &str) -> Vec<SuggestedAnnotation> {
    let mut out = Vec::new();
    for line in reply.lines() {
        let line = line.trim();
        if !line.starts_with("RVA=") {
            continue;
        }
        let rva = line[4..]
            .split_whitespace()
            .next()
            .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok());
        let name = line
            .split("NAME=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("")
            .to_string();
        let desc = line
            .split("DESC=")
            .nth(1)
            .unwrap_or("unknown")
            .trim()
            .chars()
            .take(120)
            .collect();
        if let (Some(rva), false) = (rva, name.is_empty())
            && out.len() < 256
        {
            out.push(SuggestedAnnotation {
                rva,
                name: name.to_lowercase(),
                desc,
            });
        }
    }
    out
}
