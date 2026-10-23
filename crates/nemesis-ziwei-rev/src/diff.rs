//! diff —— 行为页 diff（T3.5；T4.4 hook 计数增强复用）。
//!
//! 输入 = 前后两次同布局读取（等长），按页（默认 0x1000）比较，产出
//! 变化页 + 页内变化区段。数据区（.data）变化页是 DataArea 候选的
//! 主线索（dummy-draw 的 G_AREA 发现路径）。

use serde::Serialize;

/// 单变化页：页基 RVA + 页内变化区段列表（(页内偏移, 长度)）。
#[derive(Debug, Clone, Serialize)]
pub struct DiffPage {
    pub rva: u32,
    pub ranges: Vec<(u32, u32)>,
    /// 变化字节数。
    pub bytes_changed: usize,
}

/// 页 diff：前后等长比较。长度不齐 = 前缀对齐比较 + 长度差诚实入报告
/// （不 panic、不静默）。
pub fn page_diff(before: &[u8], after: &[u8], base_rva: u32, page: u32) -> Vec<DiffPage> {
    let n = before.len().min(after.len());
    let mut pages = Vec::new();
    let mut i = 0usize;
    while i < n {
        let end = (i + page as usize).min(n);
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        let mut changed = 0usize;
        let mut run: Option<usize> = None;
        for j in i..end {
            if before[j] != after[j] {
                changed += 1;
                if run.is_none() {
                    run = Some(j)
                }
            } else if let Some(s) = run.take() {
                ranges.push(((s - i) as u32, (j - s) as u32));
            }
        }
        if let Some(s) = run.take() {
            ranges.push(((s - i) as u32, (end - s) as u32));
        }
        if changed > 0 {
            pages.push(DiffPage {
                rva: base_rva + i as u32,
                ranges,
                bytes_changed: changed,
            });
        }
        i = end;
    }
    // 长度差的诚实注记由调用方对比 len 自查（本函数只保证前缀正确性）
    pages
}
