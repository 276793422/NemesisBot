//! profile handler 单测：prune_queue cutoff 饱和算术（BUG-20261125-006）。

use super::*;

#[test]
fn prune_cutoff_saturates_extreme_days_without_overflow() {
    // i64::MAX 天裸乘溢出（debug panic / release 环绕成垃圾 cutoff）——
    // 饱和后 cutoff 落纪元前，清理等于不删（诚实极端值行为）。
    assert!(prune_cutoff_ms(i64::MAX) <= 0);
    assert!(prune_cutoff_ms(i64::MAX / 2) <= 0);
    assert!(prune_cutoff_ms(1_000_000_000) <= 0, "~27 亿天同饱和");
}

#[test]
fn prune_cutoff_normal_days_tracks_now_minus_days() {
    let now = chrono::Utc::now().timestamp_millis();
    let c1 = prune_cutoff_ms(1);
    assert!(
        c1 > now - 2 * 86_400_000 && c1 <= now,
        "1 天 cutoff ≈ now-86400s"
    );
    let c30 = prune_cutoff_ms(30);
    assert!(
        c30 > now - 31 * 86_400_000 && c30 <= now - 29 * 86_400_000,
        "30 天 cutoff ≈ now-30*86400s"
    );
}
