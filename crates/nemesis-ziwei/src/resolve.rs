//! resolve —— 运行时锚扫描（M4/T4.2 支撑）：profile 函数 locate → 运行时地址。
//!
//! - **byte_pattern**：主模块可执行节经 read_mem 分块扫**活体内存**（诚实面：
//!   锚在内存不在场 = 不入表 + note，不猜）；分块带 (pattern_len-1) 重叠窗，
//!   跨块边界不漏配。单节扫描上限 64MB（超大体量目标 v1 诚实边界）。
//! - **rva**：模块基址 + RVA 直算（不稳定布局不推荐——locate 注记即此意）。
//! - str_ref / import_seq v1 不做运行时扫描（诚实边界；profile validate
//!   已拒其作为函数 locate）。
//!
//! 匹配核心（[`find_in`]/[`match_at`]）平台中立——单测可在 Linux 跑；
//! `resolve_locates` 依赖主模块枚举 + read_mem（windows-only）。

use std::collections::HashMap;
// Path 仅 windows-only 的 resolve_locates（主模块枚举 + read_mem）消费。
#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
use nemesis_ziwei_profile::LocateKind;
#[cfg(windows)]
use nemesis_ziwei_profile::parse_byte_pattern;
use nemesis_ziwei_profile::{Conv, LocateSpec};

/// 单节扫描上限（v1 诚实边界；超大 .text 目标转人工分析流程）。
#[cfg(windows)]
const SECTION_SCAN_CAP: u64 = 64 << 20;
/// read_mem 单块大小（与 payload MAX_READ_LEN 同值，双写注记）。
#[cfg(windows)]
const SCAN_CHUNK: u32 = 1 << 20;

/// 约定枚举 → payload conv 码（dispatch CONV_* 同源，平台中立）。
pub fn conv_to_code(conv: Conv) -> u32 {
    match conv {
        Conv::MsX64 => crate::dispatch::CONV_MSX64,
        Conv::Stdcall => crate::dispatch::CONV_STDCALL,
        Conv::Cdecl => crate::dispatch::CONV_CDECL,
    }
}

/// 解析报告（诚实面：未命中/非法/读失败进 notes，不进 addrs）。
#[derive(Debug, Default, Clone)]
pub struct ResolveReport {
    /// 键（函数语义名 / `{语义}::chain{i}`）→ 运行时地址。
    pub addrs: HashMap<String, u64>,
    pub notes: Vec<String>,
}

impl ResolveReport {
    /// 报告 JSON（WSAPI/工具输出面）。
    pub fn to_json(&self) -> serde_json::Value {
        let addrs: serde_json::Map<String, serde_json::Value> = self
            .addrs
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::json!(format!("{v:#x}"))))
            .collect();
        serde_json::json!({ "resolved": addrs, "notes": self.notes })
    }
}

/// mask=1 字节须相等；mask=0 通配。朴素扫描 + 首个非通配字节快跳。
pub fn match_at(data: &[u8], bytes: &[u8], mask: &[u8]) -> bool {
    debug_assert_eq!(bytes.len(), mask.len());
    bytes
        .iter()
        .zip(mask)
        .zip(data)
        .all(|((&b, &m), &d)| m == 0 || d == b)
}

/// 窗口内首次全匹配位置（无 = None）。
pub fn find_in(window: &[u8], bytes: &[u8], mask: &[u8]) -> Option<usize> {
    if bytes.is_empty() || window.len() < bytes.len() {
        return None;
    }
    let first_idx = mask.iter().position(|&m| m == 1)?;
    let first = bytes[first_idx];
    let last_start = window.len() - bytes.len();
    let mut i = 0usize;
    while i <= last_start {
        let rel = window[i + first_idx..=last_start + first_idx]
            .iter()
            .position(|&b| b == first)?;
        i += rel;
        if match_at(&window[i..i + bytes.len()], bytes, mask) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 待扫描 pattern（已编译 bytes/mask）。
#[cfg(windows)]
struct Pending {
    key: String,
    bytes: Vec<u8>,
    mask: Vec<u8>,
}

/// 把 profile 侧 locate 列表编译成（键, LocateSpec）待解析清单的公共入口——
/// 函数 locate 键 = 语义名；链步骤键 = `{语义}::chain{i}`（T4.6 链式捕获）。
pub fn collect_profile_locates(
    profile: &nemesis_ziwei_profile::Profile,
) -> Vec<(String, LocateSpec)> {
    let mut out = Vec::new();
    for f in &profile.functions {
        out.push((f.semantic.clone(), f.locate.clone()));
        if let Some(cap) = &f.capture {
            for (i, step) in cap.chain.iter().enumerate() {
                out.push((format!("{}::chain{}", f.semantic, i), step.locate.clone()));
            }
        }
    }
    out
}

/// 运行时解析入口（windows）：主模块枚举 → PE 节表（盘文件）→ 可执行节
/// read_mem 分块扫描。返回报告（addrs 供会话 fn_addrs 整表替换）。
#[cfg(windows)]
pub async fn resolve_locates(
    link: &mut crate::pipe::PipeLink,
    pid: u32,
    locates: Vec<(String, LocateSpec)>,
) -> Result<ResolveReport, String> {
    let (base, _size, path, _name) = crate::setup::main_module(pid)?;
    let file = std::fs::read(Path::new(&path))
        .map_err(|e| format!("主模块文件读取失败（{path}）：{e}"))?;
    let pe = nemesis_ziwei_rev::pe::PeInfo::parse(&file)
        .map_err(|e| format!("主模块 PE 解析失败（{path}）：{e}"))?;

    let mut report = ResolveReport::default();
    let mut pending: Vec<Pending> = Vec::new();
    for (key, loc) in locates {
        match loc.kind {
            LocateKind::Rva => match loc.rva {
                Some(rva) => {
                    report.addrs.insert(key, base + rva);
                }
                None => report.notes.push(format!("{key}: rva 定位缺 rva 值")),
            },
            LocateKind::BytePattern => {
                let Some(pat) = loc.pattern.clone() else {
                    report
                        .notes
                        .push(format!("{key}: byte_pattern 定位缺 pattern"));
                    continue;
                };
                match parse_byte_pattern(&pat) {
                    Ok((bytes, mask)) => {
                        if mask.iter().all(|&m| m == 0) {
                            report.notes.push(format!("{key}: pattern 全通配（拒绝）"));
                        } else {
                            pending.push(Pending { key, bytes, mask });
                        }
                    }
                    Err(e) => report.notes.push(format!("{key}: pattern 非法（{e}）")),
                }
            }
        }
    }

    // 可执行节分块扫内存（按 rva 升序——先命中先得）
    let mut sections: Vec<_> = pe.sections.iter().filter(|s| s.is_executable()).collect();
    sections.sort_by_key(|s| s.rva);
    let max_pat = pending
        .iter()
        .map(|p| p.bytes.len())
        .max()
        .unwrap_or(1)
        .max(1);
    for sec in sections {
        if pending.is_empty() {
            break;
        }
        let sec_base = base + u64::from(sec.rva);
        let total = u64::from(sec.vsize).min(SECTION_SCAN_CAP);
        let mut carry: Vec<u8> = Vec::new();
        let mut off: u64 = 0;
        while off < total && !pending.is_empty() {
            let want = SCAN_CHUNK.min((total - off) as u32);
            let buf = match crate::pipe::read_mem(link, sec_base + off, want).await {
                Ok(b) if b.is_empty() => break,
                Ok(b) => b,
                Err(e) => {
                    report
                        .notes
                        .push(format!("节 {} 读取失败 @+{off:#x}: {e}", sec.name));
                    break;
                }
            };
            // 窗口 = 上一块尾重叠 + 本块；窗口基址回退 carry 长度
            let win_base = sec_base + off - carry.len() as u64;
            let mut window = std::mem::take(&mut carry);
            window.extend_from_slice(&buf);
            pending.retain_mut(|p| {
                if let Some(pos) = find_in(&window, &p.bytes, &p.mask) {
                    report.addrs.insert(p.key.clone(), win_base + pos as u64);
                    false // 已命中出队
                } else {
                    true
                }
            });
            let keep = max_pat.saturating_sub(1).min(window.len());
            carry = window.split_off(window.len() - keep);
            if (buf.len() as u32) < want {
                break; // 节尾（读不满 = 到边）
            }
            off += buf.len() as u64;
        }
    }
    for p in pending {
        report
            .notes
            .push(format!("{}: 锚未命中（内存不在场——目标可能已更新）", p.key));
    }
    Ok(report)
}

/// 非 Windows：诚实 stub。
#[cfg(not(windows))]
pub async fn resolve_locates(
    _link: (),
    _pid: u32,
    _locates: Vec<(String, LocateSpec)>,
) -> Result<ResolveReport, String> {
    Err("紫微系统运行时锚解析仅支持 Windows（活体注入/管道能力）".to_string())
}

#[cfg(test)]
mod tests;
