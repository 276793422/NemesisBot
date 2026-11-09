use super::MAX_AGE_DAYS;

/// 钳制上界必须落在安全带内：非零（0 无清理语义）、小于 chrono
/// `Duration::days` 的 panic 下界（~1.07e14 天 = i64::MAX ms / 86_400_000，
/// 留 6 个数量级余量），且为整数年倍数（365×100）。
/// 常量卫语句是刻意形态（钉死单一真相源的不变式），显式压 clippy。
#[allow(clippy::assertions_on_constants)]
#[test]
fn max_age_days_is_within_safe_band() {
    assert!(MAX_AGE_DAYS > 0);
    // chrono Duration::days panic bound ≈ 106_751_991_167_300；上界必须
    // 远低于它，使 `Duration::days(MAX_AGE_DAYS)` 恒不 panic。
    assert!(MAX_AGE_DAYS < 1_000_000);
    assert_eq!(MAX_AGE_DAYS % 365, 0);
}

/// 钳制语义：对全部合理取值恒等（≤100 年原样通过），只收敛荒谬量级。
#[test]
fn clamp_is_identity_for_sane_values() {
    let clamp = |v: i64| v.clamp(0, MAX_AGE_DAYS);
    assert_eq!(clamp(0), 0);
    assert_eq!(clamp(1), 1);
    assert_eq!(clamp(7), 7);
    assert_eq!(clamp(90), 90);
    assert_eq!(clamp(MAX_AGE_DAYS), MAX_AGE_DAYS);
    // 荒谬量级（panic 带 / as-cast 回绕带）收敛到上界。
    assert_eq!(clamp(1_000_000), MAX_AGE_DAYS);
    assert_eq!(clamp(i64::MAX), MAX_AGE_DAYS);
    // 负值（u64 as i64 回绕产物 → cutoff 落未来 → 全删）钳到 0。
    assert_eq!(clamp(-1), 0);
    assert_eq!(clamp(i64::MIN), 0);
}
