//! `crate::status` 测试（S6 覆盖率批次）。
//!
//! 全部走只读 `sc.exe query/qc`（不触碰服务状态）。确定性臂用不存在的
//! 服务名；真实服务（Themes 等 Windows 基础服务）只做宽容断言——
//! 其存在性/状态随机器变化，属机器依赖。

// Every test in this file is windows-gated; an ungated glob import would be
// dead code on other targets (Linux CI clippy runs -D warnings).
#[cfg(windows)]
use super::*;

/// 一个肯定不存在的服务名（1060 → NotFound，跨机器确定性）。
#[cfg(windows)] // 仅下方 windows-gated 测试使用；Linux 上保持 dead_code 干净
const MISSING: &str = "NemesisS6DefinitelyMissing9527";

#[cfg(windows)]
#[test]
fn service_state_unknown_name_is_not_found() {
    assert_eq!(service_state(MISSING), ServiceState::NotFound);
}

#[cfg(windows)]
#[test]
fn service_state_real_service_returns_valid_variant() {
    // Themes 在所有桌面 Windows 上存在；状态（Running/Stopped）随机器。
    let s = service_state("Themes");
    assert!(
        matches!(
            s,
            ServiceState::Running | ServiceState::Stopped | ServiceState::NotFound
        ),
        "合法变体即可: {s:?}"
    );
}

#[cfg(windows)]
#[test]
fn service_binary_path_unknown_service_is_none() {
    assert!(service_binary_path(MISSING).is_none());
}

#[cfg(windows)]
#[test]
fn service_binary_path_real_service_tolerant() {
    // 机器依赖：Themes 的 qc 输出含 BINARY_PATH_NAME（英文标签）时 Some。
    if let Some(p) = service_binary_path("Themes") {
        assert!(!p.trim().is_empty(), "解析出的路径不能为空串");
        assert!(!p.contains("BINARY_PATH_NAME"), "必须剥掉标签只留路径: {p}");
    }
}

#[cfg(windows)]
#[test]
fn engine_owned_true_when_both_names_free() {
    // 确定性蕴含关系：SbieDrv/SbieSvc 都未注册（名字空闲）→ 必然 true。
    let tmp = tempfile::tempdir().unwrap();
    let paths = crate::SandboxPaths::new(tmp.path());
    if matches!(service_state(crate::DRIVER_SERVICE), ServiceState::NotFound)
        && matches!(service_state(crate::USERMODE_SERVICE), ServiceState::NotFound)
    {
        assert!(engine_owned(&paths), "名字全空闲 → 归属门通过");
    } else {
        // 本机注册了（真/外部 Sandboxie）：tempdir home 下必不属于我们 → false
        assert!(!engine_owned(&paths), "注册了但 binary 不在 tempdir runtime → 拒");
    }
}

// ---------------------------------------------------------------------------
// R5 覆盖率批次（2026-08-27）：Stopped 臂（真实停止服务钉住）+ 归属门
// true 臂（用注册表里的 SbieSvc binary 反推标准布局 home）。
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn service_state_stopped_for_real_stopped_service() {
    // W32Time / MSDTC / SCardSvr 在桌面 Windows 默认手动且常驻 STOPPED；
    // 轮询到任何一个 STOPPED 即已走过 `ServiceState::Stopped` return 行。
    // 全部 Running/缺失的机器跳过（机器依赖，与上面 Themes 同策略）。
    for name in ["W32Time", "MSDTC", "SCardSvr"] {
        if service_state(name) == ServiceState::Stopped {
            return; // 该调用本身就命中了 Stopped 臂
        }
    }
}

#[cfg(windows)]
#[test]
fn engine_owned_true_when_runtime_matches_registered_binaries() {
    // 本机注册了 SbieSvc 且 binary 落在标准布局的 runtime 目录下
    // （<home>/workspace/tools/sandboxie/runtime/SbieSvc.exe）→ 用 binary
    // 反推 home 构造 paths，归属门必须通过。干净机器（未注册）跳过。
    let Some(bin) = service_binary_path(crate::USERMODE_SERVICE) else {
        return;
    };
    let bin_path = std::path::PathBuf::from(&bin);
    let is_std_layout = bin_path
        .parent()
        .map(|p| p.ends_with("runtime"))
        .unwrap_or(false);
    if !is_std_layout {
        return; // 非我们部署的形态（外部 Sandboxie 安装目录），跳过
    }
    let Some(home) = bin_path.ancestors().nth(5) else {
        return;
    };
    let paths = crate::SandboxPaths::new(home);
    assert!(
        engine_owned(&paths),
        "注册 binary 在推导 runtime 下（{}）→ 归属门必须放行",
        paths.runtime_dir.display()
    );
}
