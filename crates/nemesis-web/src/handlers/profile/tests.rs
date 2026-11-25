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

// ---------- scope 解析（批三 scope 参数化，2026-11-25） ----------

use super::scope_from;
use nemesis_forge::userprofile::store::ProfileScope;
use serde_json::json;

/// 缺失 / 空 data / 空串 = 对话（兼容旧前端不传 scope）。
#[test]
fn scope_defaults_to_dialogue() {
    assert_eq!(scope_from(None).unwrap(), ProfileScope::Dialogue);
    assert_eq!(
        scope_from(Some(&json!({}))).unwrap(),
        ProfileScope::Dialogue
    );
    assert_eq!(
        scope_from(Some(&json!({"scope": ""}))).unwrap(),
        ProfileScope::Dialogue
    );
}

/// 显式值两态 + 大小写/空白容错。
#[test]
fn scope_parses_both_values_case_insensitive() {
    assert_eq!(
        scope_from(Some(&json!({"scope": "project"}))).unwrap(),
        ProfileScope::Project
    );
    assert_eq!(
        scope_from(Some(&json!({"scope": " Project "}))).unwrap(),
        ProfileScope::Project
    );
    assert_eq!(
        scope_from(Some(&json!({"scope": "Dialogue"}))).unwrap(),
        ProfileScope::Dialogue
    );
}

/// 显式未知值 loud 拒绝（不静默纠偏——拼错 scope 名要当场红，不是悄悄
/// 读错画像）。
#[test]
fn scope_unknown_value_is_loud_error() {
    assert!(scope_from(Some(&json!({"scope": "proj"}))).is_err());
    assert!(scope_from(Some(&json!({"scope": 7}))).is_err());
}
