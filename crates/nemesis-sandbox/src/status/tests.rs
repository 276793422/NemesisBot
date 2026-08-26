//! `crate::status` 测试（S6 覆盖率批次）。
//!
//! 全部走只读 `sc.exe query/qc`（不触碰服务状态）。确定性臂用不存在的
//! 服务名；真实服务（Themes 等 Windows 基础服务）只做宽容断言——
//! 其存在性/状态随机器变化，属机器依赖。

use super::*;

/// 一个肯定不存在的服务名（1060 → NotFound，跨机器确定性）。
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
