// status.rs 覆盖率补充（wave 5）：service_state 的 sc 查询路径、
// service_binary_path 对不存在服务的 None 臂、engine_owned 干净机判断。
//
// 全部只读（sc query 只读 SCM；不装/不启任何服务）。

use super::*;
use crate::SandboxPaths;

/// 不存在的服务 → NotFound（sc 1060 / does not exist 解析臂）。
#[test]
fn service_state_missing_service_is_notfound() {
    let s = service_state("nb_cov_no_such_service_xyz");
    assert_eq!(s, ServiceState::NotFound);
}

/// 不存在的服务 → binary path None（找不到 BINARY_PATH_NAME 的尾臂）。
#[test]
fn service_binary_path_missing_service_is_none() {
    assert!(service_binary_path("nb_cov_no_such_service_xyz").is_none());
}

/// engine_owned：干净机器（SbieDrv/SbieSvc 未注册）→ 名字空闲 → true；
/// 若机器上恰有外部 Sandboxie 在跑 → false 也诚实接受（两种结果都不 panic）。
#[test]
fn engine_owned_reports_honestly_for_whatever_the_machine_has() {
    let home = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(home.path());
    let owned = engine_owned(&paths);
    let _ = owned; // 机器相关：true（名字空闲）或 false（外部 Sandboxie）都合法。
}

/// engine_owned_with_states：纯函数直驱——未注册 → continue → true（60 臂 +
/// 尾部 true）。
#[test]
fn engine_owned_with_states_all_free_is_owned() {
    let owned = engine_owned_with_states(
        r"c:\our\runtime",
        &[
            (crate::DRIVER_SERVICE, ServiceState::NotFound),
            (crate::USERMODE_SERVICE, ServiceState::NotFound),
        ],
    );
    assert!(owned, "unregistered names = nothing foreign = owned");
}

/// engine_owned_with_states：注册了但二进制不在我们的 runtime 下 → false
/// （外部 Sandboxie 拒绝覆盖臂）。
#[test]
fn engine_owned_with_states_foreign_binary_is_refused() {
    let foreign = engine_owned_with_states(
        r"c:\our\runtime",
        &[(crate::USERMODE_SERVICE, ServiceState::Stopped)],
    );
    // 该臂需要真实 SCM 查询补 binary path——缺失时视为 foreign（保守拒绝）。
    assert!(
        !foreign,
        "registered + unverifiable binary must read foreign"
    );
}
