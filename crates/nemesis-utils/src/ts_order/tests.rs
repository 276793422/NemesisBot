//! BUG-20261123-016 回归锁：RFC 3339 时间戳跨偏移安全比较。
use super::*;

/// 跨偏移等值：`+08:00` 形态与同刻 `Z` 形态必须判 Equal
/// （字典序会按字符错判 +08:00 > Z 后缀形态）。
#[test]
fn test_ts_cmp_equal_instants_across_offsets() {
    // 2026-11-23T10:00:00+08:00 == 2026-11-23T02:00:00Z（同一时刻）
    assert_eq!(
        ts_cmp("2026-11-23T10:00:00+08:00", "2026-11-23T02:00:00Z"),
        std::cmp::Ordering::Equal
    );
    // 带小数秒精度差异的同刻：可解析即时刻序，精度差异不翻转
    assert_eq!(
        ts_cmp("2026-11-23T10:00:00.000+08:00", "2026-11-23T02:00:00Z"),
        std::cmp::Ordering::Equal
    );
}

/// 跨偏移真序：字典序会错判「02:00Z < 09:00+08:00」（字符 0<0,2<9 按
/// 日期时间主体排——实际 09:00+08:00 = 01:00Z 更早），时刻序必须正确。
#[test]
fn test_ts_cmp_cross_offset_true_instant_order() {
    // 09:00+08:00 = 01:00Z，早于 02:00Z
    assert_eq!(
        ts_cmp("2026-11-23T09:00:00+08:00", "2026-11-23T02:00:00Z"),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        ts_cmp("2026-11-23T02:00:00Z", "2026-11-23T09:00:00+08:00"),
        std::cmp::Ordering::Greater
    );
    // 字典序对照组：裸字符串序给出相反（错误）结果——钉死本模块存在理由
    assert!(
        "2026-11-23T02:00:00Z" < "2026-11-23T09:00:00+08:00",
        "precondition: lexical order disagrees with instant order here"
    );
}

/// 时间窗过滤场景（AuditFilter/monitor 消费形态）：Z 形态边界 vs
/// `+08:00` 形态事件，before/after 判定跨偏移正确。
#[test]
fn test_ts_before_after_window_boundaries() {
    // 窗口 [2026-11-23T00:00:00Z, 2026-11-23T04:00:00Z]
    let start = "2026-11-23T00:00:00Z";
    let end = "2026-11-23T04:00:00Z";
    // 事件 2026-11-23T09:30:00+08:00 = 01:30Z → 窗口内
    let ev = "2026-11-23T09:30:00+08:00";
    assert!(!ts_before(ev, start));
    assert!(!ts_after(ev, end));
    // 事件 2026-11-23T13:30:00+08:00 = 05:30Z → 窗口外（晚于 end）
    let ev2 = "2026-11-23T13:30:00+08:00";
    assert!(!ts_before(ev2, start));
    assert!(ts_after(ev2, end));
    // 事件 2026-11-23T07:00:00+08:00 = 23:00Z(前一日) → 窗口外（早于 start）
    let ev3 = "2026-11-23T07:00:00+08:00";
    assert!(ts_before(ev3, start));
    assert!(!ts_after(ev3, end));
}

/// 解析失败回落字符串序：任一侧不可解析 = 纯字典序（同偏移同格式
/// 写者既有行为，正确性不劣于旧实现）。
#[test]
fn test_ts_cmp_unparseable_falls_back_lexical() {
    assert_eq!(
        ts_cmp("not-a-timestamp-a", "not-a-timestamp-b"),
        std::cmp::Ordering::Less
    );
    // 一侧可解析一侧不可解析：回落字符串序（不 panic、不猜）
    assert_eq!(
        ts_cmp("2026-11-23T10:00:00Z", "garbage"),
        std::cmp::Ordering::Less
    );
    assert_eq!(ts_cmp("", ""), std::cmp::Ordering::Equal);
}

/// parse_rfc3339_local：合法解析（带偏移输入 → 本地时区时刻）与
/// 不可解析 None。
#[test]
fn test_parse_rfc3339_local() {
    let parsed = parse_rfc3339_local("2026-11-23T10:00:00+08:00").expect("parseable");
    // 与 Z 形态同刻（DateTime PartialEq 比时刻，与时区投影无关）
    let z = parse_rfc3339_local("2026-11-23T02:00:00Z").expect("parseable");
    assert_eq!(parsed, z);
    assert!(parse_rfc3339_local("nope").is_none());
    // 无偏移形态（RFC 3339 要求偏移）：诚实拒绝
    assert!(parse_rfc3339_local("2026-11-23T10:00:00").is_none());
}
