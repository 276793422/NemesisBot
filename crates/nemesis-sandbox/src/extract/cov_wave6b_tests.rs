//! extract.rs 覆盖率收尾（Wave6B）：`seven_zip_status` 的 system/none 两
//! 态与 `find_system_7z` 的 where 命中 / 候选目录遍历 / None 收口。
//!
//! 确定性方案：PATH 注入用例与 tests.rs 的 probe 共用
//! [`super::cov_tests::PATH_LOCK`] 串行——
//! - 前插 fakebin → where 必命中 → (true, "system")（139-140 + 155）；
//! - PATH 换成空目录 + 本机无 Program Files 7z → 候选遍历到 None →
//!   (false, "none")（141-142 + 166/167）；
//! - PATH 换成空目录 + 本机装了 7-Zip → where 失手后候选目录遍历命中 →
//!   Some（156 收口 + 157-164 候选链）。
//!
//! 关键：第三态是候选链的**确定性**覆盖——不能依赖进程继承的 PATH 恰好
//! 没有 7z（2026-09-25 实测：7-Zip 进了 shell PATH 后 `where 7z.exe` 直
//! 接命中，候选链随机失覆盖）。三态按机器形态互补 SKIP，合起来在任何
//! 机器上都不留 PATH 依赖的漂移。
//!
//! 空 PATH 只影响 PATH 解析（where 7z.exe 失手正是目的）；sc/cmd 等系统
//! 工具走 System32 搜索序不受影响，并行测试安全。

#![cfg(windows)]

use super::*;

/// 没装系统 7-Zip 时才跑 none 态断言（装了则候选必命中，语义交给
/// system 态用例）。
fn system_7z_candidates_absent() -> bool {
    !std::path::Path::new(r"C:\Program Files\7-Zip\7z.exe").exists()
        && !std::path::Path::new(r"C:\Program Files (x86)\7-Zip\7z.exe").exists()
}

/// 前插 fakebin → where 必命中 → status 报 system（139-140 + where 命中
/// 链收口 155）。断言只钉 source，不钉具体路径（与 probe 用例避免路径
/// 级互斥）。
#[test]
fn seven_zip_status_reports_system_via_path_front_insert() {
    let _path_guard = super::cov_tests::PATH_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let fake_dir = tmp.path().join("fakebin_covw6b");
    std::fs::create_dir_all(&fake_dir).unwrap();
    std::fs::write(fake_dir.join("7z.exe"), b"not-a-pe").unwrap();

    let old = std::env::var("PATH").unwrap_or_default();
    // SAFETY: 进程全局副作用，已持 PATH_LOCK 串行；退出前恢复。
    unsafe {
        std::env::set_var("PATH", format!("{};{}", fake_dir.display(), old));
    }

    let rt = tmp.path().join("no_cache");
    let (available, source) = seven_zip_status(&rt);
    assert_eq!(
        (available, source),
        (true, "system"),
        "前插 fakebin 必报 system"
    );

    // SAFETY: 恢复原 PATH。
    unsafe {
        std::env::set_var("PATH", old);
    }
}

/// PATH 换成空目录（where 必失手）+ 本机无 Program Files 7z → 候选遍历
/// 到 None → status 报 none（141-142 + 166/167 全链）。
#[test]
fn seven_zip_status_reports_none_when_nowhere_to_find() {
    let _path_guard = super::cov_tests::PATH_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if !system_7z_candidates_absent() {
        eprintln!("SKIP: 本机装了系统 7-Zip，none 态不可达（由 system 用例接管）");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let empty = tmp.path().join("empty_path_covw6b");
    std::fs::create_dir_all(&empty).unwrap();

    let old = std::env::var("PATH").unwrap_or_default();
    // SAFETY: 进程全局副作用，已持 PATH_LOCK 串行；退出前恢复。
    unsafe {
        std::env::set_var("PATH", &empty);
    }

    let rt = tmp.path().join("no_cache");
    let (available, source) = seven_zip_status(&rt);
    assert_eq!((available, source), (false, "none"), "无 7z 可寻必报 none");
    assert!(find_system_7z().is_none(), "find 必须走到 None 收口");

    // SAFETY: 恢复原 PATH。
    unsafe {
        std::env::set_var("PATH", old);
    }
}

/// PATH 换成空目录（where 必失手）+ 本机装了系统 7-Zip → 候选目录遍历
/// 命中 → Some + status 报 system（156 收口 + 157-164 候选链，PATH 状态
/// 无关的确定性覆盖）。未装 7-Zip 的机器上该链不可达（由 none 态用例
/// 接管）。
#[test]
fn find_system_7z_falls_back_to_install_dir_candidates() {
    let _path_guard = super::cov_tests::PATH_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if system_7z_candidates_absent() {
        eprintln!("SKIP: 本机无 Program Files 7-Zip，候选链不可达（由 none 用例接管）");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let empty = tmp.path().join("empty_path_covw6b_cand");
    std::fs::create_dir_all(&empty).unwrap();

    let old = std::env::var("PATH").unwrap_or_default();
    // SAFETY: 进程全局副作用，已持 PATH_LOCK 串行；退出前恢复。
    unsafe {
        std::env::set_var("PATH", &empty);
    }

    let found = find_system_7z();
    assert!(found.is_some(), "候选目录有 7z 必命中 Some");
    assert!(found.unwrap().exists(), "命中的必须是真实存在的 7z.exe");
    let rt = tmp.path().join("no_cache");
    assert_eq!(
        seven_zip_status(&rt),
        (true, "system"),
        "候选目录命中也报 system"
    );

    // SAFETY: 恢复原 PATH。
    unsafe {
        std::env::set_var("PATH", old);
    }
}
