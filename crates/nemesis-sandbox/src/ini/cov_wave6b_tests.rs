//! ini.rs 覆盖率收尾（Wave6B）：`write_sandboxie_ini` 的 ini_path 无父
//! 目录臂（跳过 create_dir_all）。
//!
//! ini_path 用空路径：parent() == None 走 skip 臂，末尾 fs::write("") 在
//! 任何权限下都失败（非法路径），零落盘副作用。

use super::*;

/// ini_path 无父目录 → 跳过 create_dir_all（30 收口臂）→ fs::write("")
/// 失败 → Err 带 "write" context。
#[test]
fn write_sandboxie_ini_without_parent_skips_mkdir_and_reports_write_error() {
    let box_root = tempfile::tempdir().unwrap();
    let err = write_sandboxie_ini(
        std::path::Path::new(""),
        "CovW6BBox",
        box_root.path(),
        false,
    )
    .expect_err("空 ini 路径必须失败");
    let msg = format!("{err:#}");
    assert!(msg.contains("write"), "必须带 write context: {msg}");
}
