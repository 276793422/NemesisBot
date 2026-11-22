//! 午夜清扫调度纯函数 `secs_until_next_local_midnight` 的时区韧性测试。
//!
//! 缺陷背景：旧实现 `Local.from_local_datetime(..).unwrap()` 在「本地午夜
//! 不存在」的时区（春令时跳变恰好落在 00:00——古巴/智利等真实存在）拿到
//! `LocalResult::None` → unwrap panic → 清扫 task 死亡，SessionStore /
//! rpc_cache / spill 三条 TTL 循环进程内永久停摆。新实现用 naive 墙钟差
//! （无时区查找）：跳变夜墙钟差比真实时长短 1h，睡完恰好落在跳变后第一个
//! 有效时刻（01:00）；回拨夜会早醒一次（幂等清理，无害）。

use chrono::TimeZone;

use super::secs_until_next_local_midnight;

/// 常规时区（无跳变）：距明日午夜 = 墙钟差，与机器本地时区无关。
#[test]
fn normal_night_returns_wall_clock_delta() {
    let now = chrono::Local
        .with_ymd_and_hms(2026, 6, 15, 14, 0, 0)
        .unwrap();
    assert_eq!(secs_until_next_local_midnight(now), 10 * 3600);
}

/// 午夜被跳变吞掉（古巴哈瓦那： 00:00 不存在，00:00→01:00 春拨）：
/// 旧实现 panic；新实现睡墙钟差 10h——真实醒来落在 01:00（跳变后首个有效
/// 时刻）。前置自证 assert 保证 chrono-tz 时区数据与用例假设同步（数据修订
/// 时 loud 失败并指示换时区/日期，不让断言空转）。
#[test]
fn skipped_midnight_does_not_panic_and_lands_after_transition() {
    use chrono_tz::America::Havana;
    let naive = chrono::NaiveDate::from_ymd_opt(2026, 3, 8)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    assert!(
        matches!(
            TimeZone::from_local_datetime(&Havana, &naive),
            chrono::LocalResult::None
        ),
        "前置失效：Havana 2026-03-08 00:00 在当前 chrono-tz 数据中存在，换用其他午夜跳变时区/日期"
    );
    let now = Havana.with_ymd_and_hms(2026, 3, 7, 14, 0, 0).unwrap();
    assert_eq!(secs_until_next_local_midnight(now), 10 * 3600);
}

/// BUG-20261122-033 回归锁：spawn_daily_cleanup 的 per-label once-guard。
///
/// 缺陷背景：`build_*_agent_loop` 在 agent 重建（persona 激活/恢复/
/// 删除-active 的 restart_agent stop→start 链、集群人格切换）时会再跑——
/// 旧形态无闸，每次重建叠加一个 immortal 午夜清理任务并钉住一份建时快照
/// 的陈旧 SessionStore（兄弟 sweep spawn_rpc_cache_cleanup /
/// spawn_daily_spill_cleanup 各自带进程级 OnceGuard，本 sweep 因有
/// Main/Cluster 两个调用方而漏挂）。修复 = 按 label 分桶：同 label 重复
/// 调用跳过（返回 false），不同 label 各自放行（两条构建线互不吞并）。
#[tokio::test]
async fn daily_cleanup_once_guard_is_per_label() {
    let store = std::sync::Arc::new(nemesis_agent::session::SessionStore::new_in_memory());
    assert!(
        super::shared::spawn_daily_cleanup(store.clone(), "bugfix-r2346-guard-a", None),
        "首次调用（未见于守卫表）必须放行 spawn"
    );
    assert!(
        !super::shared::spawn_daily_cleanup(store, "bugfix-r2346-guard-a", None),
        "同 label 二次调用必须跳过——agent 重建再入工厂不叠加午夜任务"
    );
    assert!(
        super::shared::spawn_daily_cleanup(
            std::sync::Arc::new(nemesis_agent::session::SessionStore::new_in_memory()),
            "bugfix-r2346-guard-b",
            None,
        ),
        "不同 label 必须各自放行——Main/Cluster 分桶互不吞并"
    );
}
