//! 点分版本比较（计划 §7：同版本=修复 / 高=升级 / 低=拒绝）。
//!
//! 形态：`N` 或 `N.N` 或 `N.N.N` 或 `N.N.N.N`（主构建脚本口径
//! `0.0.0.1` / git tag）；每段取数值前缀比较（`3-beta` 按 3），
//! 段数不足补 0。任一段无数值前缀 = 整体不可比（None，诚实拒绝
//! 不猜——调用方对 None 给 loud 报错）。

/// 解析点分版本为数值段向量；完全无法解析返回 None。
pub fn parse(v: &str) -> Option<Vec<u64>> {
    let v = v.trim().trim_start_matches('v');
    if v.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for seg in v.split('.') {
        let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        out.push(digits.parse::<u64>().ok()?);
    }
    Some(out)
}

/// 三态比较：Ordering 直通；段数不同补 0 对齐。
pub fn compare(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    let (va, vb) = (parse(a)?, parse(b)?);
    let n = va.len().max(vb.len());
    for i in 0..n {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            other => return Some(other),
        }
    }
    Some(Ordering::Equal)
}

#[cfg(test)]
mod tests;
