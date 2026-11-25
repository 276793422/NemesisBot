//! `profile.prune_queue` cutoff 纯函数测试（复查 2026-11-24 饱和算术纪律，
//! BUG-20261125-006）。
//!
//! Handler 级载荷/路由测试见前端 spec 与 l1 注册表；这里只钉 cutoff 计算
//! 的纯函数面（配置面 i64 算术：极端天数不 panic，饱和退化 = 什么都不清）。

use super::prune_cutoff_ms;

/// 常规天数：cutoff 在过去，且等于 now − days×86400×1000（容差 1s 防
/// 时钟前跳）。
#[test]
fn prune_cutoff_normal_days_is_in_the_past() {
    let before = chrono::Utc::now().timestamp_millis();
    let cutoff = prune_cutoff_ms(30);
    let after = chrono::Utc::now().timestamp_millis();
    let expected_low = before - 30 * 24 * 60 * 60 * 1000;
    let expected_high = after - 30 * 24 * 60 * 60 * 1000;
    assert!(
        (expected_low..=expected_high).contains(&cutoff),
        "cutoff 应为 now−30 天：{cutoff} 不在 [{expected_low}, {expected_high}]"
    );
}

/// i64::MAX 天不 panic（复查根修场景）：乘法/减法双饱和，cutoff 退化为
/// 极早时间 = 什么都不清。
#[test]
fn prune_cutoff_huge_days_saturates_without_panic() {
    assert!(
        prune_cutoff_ms(i64::MAX) < 0,
        "饱和后 cutoff 退化为极早时间"
    );
    assert!(prune_cutoff_ms(i64::MAX / 2) <= 0, "半量程同样饱和");
    assert!(prune_cutoff_ms(1_000_000_000) <= 0, "~27 亿天同饱和");
}

/// 边界：days=1 合法（≥1 闸由 handler 负责，这里只验数值形）。
#[test]
fn prune_cutoff_one_day() {
    let after = chrono::Utc::now().timestamp_millis();
    let cutoff = prune_cutoff_ms(1);
    assert!(
        (after - 24 * 60 * 60 * 1000..=after).contains(&cutoff),
        "cutoff 应在最近 24h 内"
    );
}
