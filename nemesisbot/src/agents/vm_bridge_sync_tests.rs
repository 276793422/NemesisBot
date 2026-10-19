//! B9-3 —— vm_bridge.rs 同步目标解析伴生测试（零内联纪律）。
//!
//! 覆盖 [`super::vm_bridge::resolve_project_entry`]（id 精确 / 名称回退 /
//! 未注册诚实报错）与 [`super::vm_bridge::resolve_sync_target`] 的
//! 「管理器未装配」臂（测试进程无 gateway 装配，`MANAGER_SLOT` 恒 None——
//! gateway/runtime.rs 之外的唯一写入点不存在，断言确定性成立）。管理器
//! 在场 + 非 VM 档两臂依赖真实 AgentManager 运行态，归 IT / cluster-uat。

use std::path::Path;

use super::vm_bridge::{resolve_project_entry, resolve_sync_target};

/// 单条目注册表夹具（`<tmp>/config/projects.json`；字段与
/// projects.rs::ProjectEntry serde 形态一致）。
fn write_registry(workspace: &Path, entries: &str) {
    let cfg = workspace.join("config");
    std::fs::create_dir_all(&cfg).expect("create config dir");
    std::fs::write(
        cfg.join("projects.json"),
        format!(r#"{{"projects":[{entries}]}}"#),
    )
    .expect("write registry");
}

fn entry_json(id: &str, name: &str) -> String {
    format!(
        r#"{{"id":"{id}","name":"{name}","path":"C:\\tmp\\{name}","created_at":"2026-10-19T00:00:00Z"}}"#
    )
}

#[test]
fn resolve_entry_by_id_precise_hit() {
    let ws = tempfile::tempdir().unwrap();
    write_registry(ws.path(), &entry_json("pid-1", "demo"));
    let e = resolve_project_entry(ws.path(), "pid-1").expect("id 命中");
    assert_eq!(e.id, "pid-1");
    assert_eq!(e.name, "demo");
}

#[test]
fn resolve_entry_falls_back_to_name() {
    let ws = tempfile::tempdir().unwrap();
    write_registry(ws.path(), &entry_json("pid-1", "demo"));
    // 名称不是任何条目的 id → 回退按名称匹配。
    let e = resolve_project_entry(ws.path(), "demo").expect("名称命中");
    assert_eq!(e.id, "pid-1", "返回的是注册表条目（id 是同步账本键）");
}

#[test]
fn resolve_entry_miss_is_honest_error() {
    let ws = tempfile::tempdir().unwrap();
    write_registry(ws.path(), &entry_json("pid-1", "demo"));
    let err = resolve_project_entry(ws.path(), "nope").unwrap_err();
    assert!(err.contains("项目不存在"), "{err}");
    assert!(err.contains("nope"), "报错点名查不到的项目: {err}");
}

#[test]
fn resolve_target_without_manager_is_honest_error() {
    let ws = tempfile::tempdir().unwrap();
    write_registry(ws.path(), &entry_json("pid-1", "demo"));
    // 测试进程无 gateway 装配（install_manager 仅 gateway boot 调用）——
    // 项目条目解析成功后必须在管理器臂诚实停车。
    let err = resolve_sync_target(ws.path(), "pid-1").unwrap_err();
    assert!(err.contains("管理器未装配"), "{err}");
}
