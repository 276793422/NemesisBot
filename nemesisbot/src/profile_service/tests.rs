//! profile_service 单测（独立文件纪律——生产文件零内联测试）。
//!
//! 覆盖：estop 急停闸（2026-11-25 R2880）——engaged 时 `sweep_tick` 零 IO
//! 直接跳过（画像锚点不推进），disengaged 时 sweep 照常落盘推进锚点。
//! 断言走盘上产物（`store::try_load_profile` 的 `last_sweep_ms`），不是
//! 目录存在性。

use std::path::PathBuf;
use std::sync::Arc;

use super::sweep_tick;
use nemesis_forge::userprofile::store;

/// 隔离 home（config.json 指 home 根；workspace = home/workspace）。
fn temp_home() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    (dir, home)
}

/// 写最小 config：user_profile.enabled=true（sweep_once 的启用前置）。
fn write_enabled_config(home: &std::path::Path) {
    std::fs::write(
        crate::common::config_path(home),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .expect("write config.json");
}

/// 预置 enabled_at 锚点：没有它 sweep_once 对 disengaged 也不落盘
/// （sweep_window_start_ms 返回 None = 「从未开启」早退），闸外因素会
/// 污染两侧断言。
fn seed_anchor(workspace: &std::path::Path) {
    std::fs::create_dir_all(workspace).expect("mkdir workspace");
    store::enable_profile(workspace, 200).expect("enable_profile");
}

#[test]
fn estop_engaged_skips_tick_zero_io() {
    let (_dir, home) = temp_home();
    write_enabled_config(&home);
    let workspace = crate::common::workspace_path(&home);
    seed_anchor(&workspace);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();

    let r = sweep_tick(&home, rt.handle(), None, &estop).expect("engaged tick 必须诚实 Ok");
    assert_eq!(r, 0);
    // 零 IO：锚点未被推进（sweep 没跑——急停不变量「engaged → 自主活动
    // 不发起」在此 tick 的落地面）。
    let profile = store::try_load_profile(&workspace).expect("load profile");
    assert!(
        profile.last_sweep_ms.is_none(),
        "estop engaged 时锚点不得推进（sweep 必须整体跳过）"
    );
}

#[test]
fn estop_disengaged_tick_persists_anchor() {
    let (_dir, home) = temp_home();
    write_enabled_config(&home);
    let workspace = crate::common::workspace_path(&home);
    seed_anchor(&workspace);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());

    sweep_tick(&home, rt.handle(), None, &estop).expect("disengaged tick 照常执行");
    // 同输入对照：sweep 照常跑并持久化水线（证明上一测的跳过是闸的功劳，
    // 不是环境因素让 sweep 恒 no-op）。
    let profile = store::try_load_profile(&workspace).expect("load profile");
    assert!(
        profile.last_sweep_ms.is_some(),
        "disengaged 时 sweep 必须照常落盘推进锚点"
    );
}
