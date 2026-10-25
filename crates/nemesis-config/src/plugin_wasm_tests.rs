//! plugin_wasm 开关文件单测（临时目录隔离，无全局状态）。

use crate::plugin_wasm::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// set_enabled/set_config/remove 走 config_write_lock（进程级串行锁）；
// 读路径迁移写不罩锁——BUG-20261025-45 修复后写侧走 write_file_atomic
// 唯一临时名（.tmp-{pid}-{seq}-{nanos}），同毫秒撞名已消除；
// 此锁保留为廉价的串行卫生。
static SERIAL: Mutex<()> = Mutex::new(());

fn temp_config_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nb_plugin_wasm_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn legacy_instance(config_dir: &Path, slug: &str, entries: &str) {
    let dir = config_dir.join("plugins");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{slug}.json")),
        format!(r#"{{"enabled":true,"entries":{entries}}}"#),
    )
    .unwrap();
}

#[test]
fn missing_file_and_slug_default_to_disabled() {
    let dir = temp_config_dir("missing");
    let _g = SERIAL.lock().unwrap();
    // 文件/键缺席 = 未启用（与 native 的缺省启用相反）；纯读不得落盘
    let sw = switch(&dir, "my-plugin");
    assert!(!sw.enabled);
    assert!(sw.config.is_empty());
    assert!(!plugin_wasm_config_path(&dir).exists(), "纯读不得落盘");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_enabled_roundtrip_preserves_other_entries_and_config() {
    let dir = temp_config_dir("roundtrip");
    let _g = SERIAL.lock().unwrap();
    set_enabled(&dir, "alpha", true).unwrap();
    set_enabled(&dir, "beta", false).unwrap();
    let mut cfg = BTreeMap::new();
    cfg.insert("include_spaces".to_string(), serde_json::json!("true"));
    set_config(&dir, "alpha", cfg).unwrap();

    assert!(switch(&dir, "alpha").enabled);
    assert_eq!(
        switch(&dir, "alpha").config.get("include_spaces"),
        Some(&serde_json::json!("true"))
    );
    assert!(!switch(&dir, "beta").enabled);
    assert!(!switch(&dir, "gamma").enabled, "未记录 slug 缺省 false");
    // 翻回不改 config
    set_enabled(&dir, "alpha", false).unwrap();
    let sw = switch(&dir, "alpha");
    assert!(!sw.enabled);
    assert_eq!(
        sw.config.get("include_spaces"),
        Some(&serde_json::json!("true")),
        "开关翻转不得抹掉实例配置"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_is_idempotent_and_only_touches_target() {
    let dir = temp_config_dir("remove");
    let _g = SERIAL.lock().unwrap();
    set_enabled(&dir, "alpha", true).unwrap();
    set_enabled(&dir, "beta", true).unwrap();
    assert!(switch(&dir, "alpha").enabled); // 落盘就绪
    remove(&dir, "alpha").unwrap();
    remove(&dir, "alpha").unwrap(); // 幂等
    assert!(!switch(&dir, "alpha").enabled);
    assert!(switch(&dir, "beta").enabled, "只摘目标条目");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_file_is_fail_closed() {
    let dir = temp_config_dir("corrupt");
    std::fs::write(plugin_wasm_config_path(&dir), "{ not valid json !!!").unwrap();
    // 损坏 = 全部未启用（与 native 的 fail-open 相反——没明确开就不加载）
    let all = load_switches(&dir);
    assert!(all.is_empty(), "损坏文件须 fail-closed 空 map");
    assert!(!switch(&dir, "alpha").enabled);
    // 损坏文件上写 = 以空底重建（无法解析无从保留）
    set_enabled(&dir, "alpha", true).unwrap();
    assert!(switch(&dir, "alpha").enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_instance_migrates_with_enabled_forced_false() {
    let dir = temp_config_dir("legacy_migrate");
    let _g = SERIAL.lock().unwrap();
    legacy_instance(&dir, "old-plugin", r#"{"k1":"v1","k2":"v2"}"#);
    // 首次读触发迁移：条目并入（enabled 强制 false）、config 保留、旧文件删除
    let sw = switch(&dir, "old-plugin");
    assert!(
        !sw.enabled,
        "迁移后 enabled 必须强制 false（新纪律全量接管）"
    );
    assert_eq!(sw.config.get("k1"), Some(&serde_json::json!("v1")));
    assert_eq!(sw.config.get("k2"), Some(&serde_json::json!("v2")));
    assert!(
        !dir.join("plugins/old-plugin.json").exists(),
        "旧实例文件应已删除"
    );
    assert!(!dir.join("plugins").exists(), "空旧目录应顺手清掉");
    // 迁移结果落盘——重读稳定
    let again = switch(&dir, "old-plugin");
    assert_eq!(again.enabled, sw.enabled);
    assert_eq!(again.config, sw.config);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_migration_via_load_switches_and_discard_conflict() {
    let dir = temp_config_dir("legacy_load");
    let _g = SERIAL.lock().unwrap();
    legacy_instance(&dir, "gone", r#"{"a":"1"}"#);
    legacy_instance(&dir, "kept", r#"{"b":"2"}"#);
    // 用户已用新纪律给 kept 表过态 → 主文件权威，旧副本只清理不复活 enabled
    set_enabled(&dir, "kept", true).unwrap();
    let all = load_switches(&dir);
    assert!(!all["gone"].enabled, "并入条目强制 false");
    assert_eq!(all["gone"].config.get("a"), Some(&serde_json::json!("1")));
    assert!(all["kept"].enabled, "用户已表态的条目不被旧文件覆盖");
    assert!(!dir.join("plugins/gone.json").exists());
    assert!(!dir.join("plugins/kept.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_dir_noise_handled() {
    let dir = temp_config_dir("legacy_noise");
    let _g = SERIAL.lock().unwrap();
    std::fs::create_dir_all(dir.join("plugins")).unwrap();
    std::fs::write(dir.join("plugins/readme.txt"), "不是实例文件").unwrap();
    std::fs::write(dir.join("plugins/broken.json"), "{{{").unwrap();
    // 只有 *.json 进迁移（readme.txt 忽略）；broken.json 解析失败按缺省
    // （空 entries）并入 enabled=false 条目并删除旧文件
    let all = load_switches(&dir);
    assert_eq!(all.len(), 1, "readme.txt 忽略，broken.json 并入");
    let sw = &all["broken"];
    assert!(!sw.enabled);
    assert!(sw.config.is_empty());
    assert!(dir.join("plugins/readme.txt").exists(), "非 json 文件不动");
    assert!(
        !dir.join("plugins/broken.json").exists(),
        "已迁移的旧文件删除"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ==================== BUG-20261025-45（BUG-HUNT R123）回归锁 ====================
//
// write_switch_file 曾是手搓 tmp+rename：tmp 名只到毫秒精度（读路径迁移写
// 不罩锁，同毫秒并发可撞名），且无 sync_all（崩溃/断电撕裂目标文件——读侧
// fail-closed 全禁 + 下次写以空底重建）。修复 = 走 write_file_atomic（唯一
// 临时名 + fsync 先于 rename + 失败清理）。进程内无法真实注入写中崩溃
// （BUG-43/44 同族诚实边界），本批锁可观察不变量：兄弟条目跨写保留、
// 文件字节形态（尾随换行）不变、写路径与迁移路径盘上零 .tmp 残留。

#[test]
fn r45_writes_preserve_siblings_trailing_newline_and_no_tmp_residue() {
    let dir = temp_config_dir("r45_atomic");
    let _g = SERIAL.lock().unwrap();
    set_enabled(&dir, "alpha", true).unwrap();
    set_enabled(&dir, "beta", false).unwrap();
    let mut cfg = BTreeMap::new();
    cfg.insert("include_spaces".to_string(), serde_json::json!("true"));
    set_config(&dir, "alpha", cfg).unwrap();
    set_enabled(&dir, "beta", true).unwrap();
    // 多轮 RMW 后兄弟条目 + config 全保留
    assert!(switch(&dir, "alpha").enabled);
    assert!(switch(&dir, "beta").enabled);
    assert_eq!(
        switch(&dir, "alpha").config.get("include_spaces"),
        Some(&serde_json::json!("true"))
    );
    // 字节形态：尾随换行保留（与修复前 json + "\n" 一致，文件消费者无感）
    let raw = std::fs::read(plugin_wasm_config_path(&dir)).unwrap();
    assert!(raw.ends_with(b"\n"), "尾随换行必须保留");
    assert_no_tmp_residue(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn r45_migration_write_via_read_path_leaves_no_residue() {
    let dir = temp_config_dir("r45_migrate");
    let _g = SERIAL.lock().unwrap();
    legacy_instance(&dir, "old-plugin", r#"{"k":"v"}"#);
    // 首读触发迁移写（不罩锁的路径）——迁移条目落盘 + 零 .tmp 残留
    let sw = switch(&dir, "old-plugin");
    assert!(!sw.enabled);
    assert_eq!(sw.config.get("k"), Some(&serde_json::json!("v")));
    assert_no_tmp_residue(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 盘上零临时文件残留（新旧两种 tmp 命名形态都含 `.tmp` 子串）。
fn assert_no_tmp_residue(dir: &Path) {
    let residue: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(residue.is_empty(), "写侧不得留下 .tmp 残留：{residue:?}");
}
