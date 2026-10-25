//! plugins_native 开关文件单测（临时目录隔离，无全局状态）。

use crate::plugins_native::*;
use std::path::PathBuf;

fn temp_config_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("nb_plugins_native_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn missing_file_and_key_default_to_enabled() {
    let dir = temp_config_dir("missing");
    for p in NativePluginId::ALL {
        assert!(load_enabled(&dir, *p), "{:?} 缺省必须 = 启用", p);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_then_load_roundtrip_preserves_other_entries() {
    let dir = temp_config_dir("roundtrip");
    set_enabled(&dir, NativePluginId::PluginUi, false).unwrap();
    set_enabled(&dir, NativePluginId::PluginWasm, false).unwrap();
    assert!(!load_enabled(&dir, NativePluginId::PluginUi));
    assert!(!load_enabled(&dir, NativePluginId::PluginWasm));
    // 未动过的键保持缺省启用
    assert!(load_enabled(&dir, NativePluginId::PluginOnnx));
    assert!(load_enabled(&dir, NativePluginId::EvalMonitor));
    // 翻回
    set_enabled(&dir, NativePluginId::PluginUi, true).unwrap();
    assert!(load_enabled(&dir, NativePluginId::PluginUi));
    assert!(!load_enabled(&dir, NativePluginId::PluginWasm));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_file_falls_back_to_enabled() {
    let dir = temp_config_dir("corrupt");
    std::fs::write(plugins_native_config_path(&dir), "{ not valid json !!!").unwrap();
    for p in NativePluginId::ALL {
        assert!(load_enabled(&dir, *p), "{:?} 损坏文件须保守放行", p);
    }
    // 损坏文件上 set_enabled = 重建（旧内容不保留——无法解析无从保留）
    set_enabled(&dir, NativePluginId::PluginUi, false).unwrap();
    assert!(!load_enabled(&dir, NativePluginId::PluginUi));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn id_catalog_roundtrip() {
    for p in NativePluginId::ALL {
        assert_eq!(NativePluginId::from_id(p.id()), Some(*p));
    }
    assert_eq!(NativePluginId::from_id("nope"), None);
    // 目录无重复 id
    let mut ids: Vec<_> = NativePluginId::ALL.iter().map(|p| p.id()).collect();
    ids.sort_unstable();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "NativePluginId::ALL 存在重复 id");
}

#[test]
fn immediate_effect_only_eval() {
    assert!(NativePluginId::EvalMonitor.immediate_effect());
    assert!(!NativePluginId::PluginUi.immediate_effect());
    assert!(!NativePluginId::PluginOnnx.immediate_effect());
    assert!(!NativePluginId::PluginWasm.immediate_effect());
    assert!(!NativePluginId::PluginVm.immediate_effect());
    assert!(!NativePluginId::PluginRoaming.immediate_effect());
}

#[test]
fn legacy_file_migrated_on_first_read() {
    let dir = temp_config_dir("migrate_read");
    // 旧文件名在场（用户真实 home 的既有形态）
    std::fs::write(
        dir.join("plugins_native.json"),
        r#"{"plugins":{"plugin_ui":{"enabled":false}}}"#,
    )
    .unwrap();
    // 首读触发迁移：新名读出原内容，旧名消失
    assert!(!load_enabled(&dir, NativePluginId::PluginUi));
    assert!(plugins_native_config_path(&dir).exists());
    assert!(!dir.join("plugins_native.json").exists(), "旧文件应已删除");
    // 迁移后写入读路径照常
    set_enabled(&dir, NativePluginId::PluginUi, true).unwrap();
    assert!(load_enabled(&dir, NativePluginId::PluginUi));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_migration_on_write_preserves_content() {
    let dir = temp_config_dir("migrate_write");
    std::fs::write(
        dir.join("plugins_native.json"),
        r#"{"plugins":{"plugin_onnx":{"enabled":false}}}"#,
    )
    .unwrap();
    // 未读过、直接写：迁移先行，写目标键后旧条目保留
    set_enabled(&dir, NativePluginId::PluginUi, false).unwrap();
    assert!(!load_enabled(&dir, NativePluginId::PluginUi));
    assert!(
        !load_enabled(&dir, NativePluginId::PluginOnnx),
        "旧内容条目须随迁移保留"
    );
    assert!(!dir.join("plugins_native.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_file_discarded_when_new_name_present() {
    let dir = temp_config_dir("migrate_both");
    std::fs::write(
        plugins_native_config_path(&dir),
        r#"{"plugins":{"plugin_ui":{"enabled":false}}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("plugins_native.json"),
        r#"{"plugins":{"plugin_ui":{"enabled":true}}}"#,
    )
    .unwrap();
    // 新名在场 = 权威；旧文件作废弃副本清掉，内容不被复活
    assert!(!load_enabled(&dir, NativePluginId::PluginUi));
    assert!(!dir.join("plugins_native.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// ==================== BUG-20261025-45（BUG-HUNT R123）回归锁 ====================
//
// set_enabled 曾是裸 std::fs::write 直写目标（崩溃撕裂整文件 → 读侧
// fail-open 短期放行，但下次 set_enabled 的 RMW 从撕裂内容解析失败落到
// 空 SwitchFile，静默抹掉全部兄弟插件条目）；legacy 迁移曾是「读+写+删」
// 非原子（中段崩溃新旧两份全丢）。修复 = 写侧走 write_file_atomic、
// 迁移改同目录 rename。进程内无法真实注入写中崩溃（BUG-43/44 同族诚实
// 边界），本批锁可观察不变量：兄弟条目跨写保留、迁移字节保真 + 旧名
// 消失、写路径盘上零 .tmp 残留。

#[test]
fn r45_repeated_writes_preserve_siblings_and_leave_no_tmp_residue() {
    let dir = temp_config_dir("r45_atomic");
    set_enabled(&dir, NativePluginId::PluginUi, false).unwrap();
    set_enabled(&dir, NativePluginId::PluginOnnx, false).unwrap();
    set_enabled(&dir, NativePluginId::PluginWasm, false).unwrap();
    set_enabled(&dir, NativePluginId::PluginOnnx, true).unwrap();
    // 多轮 RMW 后全部条目仍在（撕裂后果的镜像面：兄弟条目不被静默抹掉）
    assert!(!load_enabled(&dir, NativePluginId::PluginUi));
    assert!(load_enabled(&dir, NativePluginId::PluginOnnx));
    assert!(!load_enabled(&dir, NativePluginId::PluginWasm));
    assert!(
        load_enabled(&dir, NativePluginId::EvalMonitor),
        "未动过的键保持缺省启用"
    );
    assert_no_tmp_residue(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn r45_legacy_migration_is_byte_fidelity_rename() {
    let dir = temp_config_dir("r45_rename");
    let legacy_body =
        r#"{"plugins":{"plugin_ui":{"enabled":false},"plugin_onnx":{"enabled":false}}}"#;
    std::fs::write(dir.join("plugins_native.json"), legacy_body).unwrap();
    // 首读迁移：新名内容与旧名字节一致（rename 语义，非重序列化）+ 旧名消失
    assert!(!load_enabled(&dir, NativePluginId::PluginUi));
    let migrated = std::fs::read_to_string(plugins_native_config_path(&dir)).unwrap();
    assert_eq!(
        migrated, legacy_body,
        "迁移必须字节保真（rename 而非读+写+删）"
    );
    assert!(!dir.join("plugins_native.json").exists());
    assert_no_tmp_residue(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 盘上零临时文件残留（write_file_atomic 的 `.tmp-*` 形态）。
fn assert_no_tmp_residue(dir: &std::path::Path) {
    let residue: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(residue.is_empty(), "写侧不得留下 .tmp 残留：{residue:?}");
}
