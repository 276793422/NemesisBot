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
