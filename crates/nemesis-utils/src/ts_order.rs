//! RFC 3339 时间戳跨偏移安全比较（BUG-20261123-016 单一真相源）。
//!
//! 时间戳字符串字典序比较只在「同一写者、同一时区偏移、同一精度」前提下
//! 成立。写侧惯例是 `Local::now().to_rfc3339()`（本地偏移形态，如
//! `2026-11-23T10:00:00+08:00`），而消费方/外部输入常为 UTC `Z` 形态
//! （浏览器 `toISOString()` 惯例）——两种形态混用时字典序按字符排列错序：
//! `2026-11-23T10:00:00+08:00`（= 02:00Z）排在 `2026-11-23T09:00:00Z`
//! 之后，时间窗过滤 / 最新值选取静默出错（证据存在但查不出/显示错）。
//!
//! 语义先例：nemesis-forge monitor D1/D3 的 `parse_ts`/`ts_before`
//! （双侧可解析 = 时刻序；任一侧解析失败 = 回落字符串序）。本模块将该
//! 语义晋升为全仓共享：forge monitor 委托、nemesis-security AuditFilter、
//! web audit 聚合同源消费。

/// 解析 RFC 3339 时间戳（必须带偏移），统一转本地时区。
///
/// 解析失败返回 `None`，调用方自决回落语义（窗口过滤放行 / 字符串序 /
/// 诚实报错）——本函数不做静默纠偏。
pub fn parse_rfc3339_local(s: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local))
}

/// 时间戳比较：双侧可解析 = 时刻序（跨时区偏移正确）；任一侧解析失败 =
/// 回落字符串序（同偏移同格式写者的既有行为，正确性不劣于纯字典序实现）。
pub fn ts_cmp(stamp: &str, boundary: &str) -> std::cmp::Ordering {
    match (
        chrono::DateTime::parse_from_rfc3339(stamp),
        chrono::DateTime::parse_from_rfc3339(boundary),
    ) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => stamp.cmp(boundary),
    }
}

/// `stamp < boundary`（时刻序；任一侧不可解析回落字符串序）。
pub fn ts_before(stamp: &str, boundary: &str) -> bool {
    ts_cmp(stamp, boundary) == std::cmp::Ordering::Less
}

/// `stamp > boundary`（时刻序；任一侧不可解析回落字符串序）。
pub fn ts_after(stamp: &str, boundary: &str) -> bool {
    ts_cmp(stamp, boundary) == std::cmp::Ordering::Greater
}

#[cfg(test)]
mod tests;
