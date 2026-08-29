//! Tests for `HotReloader`（热重载统一收编，2026-08-29）——经公开 API 验证
//! mtime 变更重载/未变 no-op/缺失文件与垃圾内容兜底。

use nemesis_config::{load_commands_config, CommandsConfig, HotReloader};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

fn load_commands(path: &Path) -> CommandsConfig {
    // 与生产 load_commands_config 同语义（缺失文件 = 空表）。
    load_commands_config(path)
}

#[test]
fn hot_reloader_picks_up_table_edits_and_unchanged_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.commands.json");
    std::fs::write(
        &path,
        r#"{ "commands": [ { "name": "a", "prompt": "P1" } ] }"#,
    )
    .unwrap();

    let hot: HotReloader<CommandsConfig> = HotReloader::new(path.clone(), load_commands);
    assert_eq!(hot.get().commands.len(), 1);

    // 未变化 → check() false（no-op）。
    assert!(!hot.check(), "unchanged mtime must not reload");

    // 变化 → check() true + 内容更新。
    sleep(Duration::from_millis(20));
    std::fs::write(
        &path,
        r#"{ "commands": [ { "name": "a", "prompt": "P1" }, { "name": "b", "prompt": "P2" } ] }"#,
    )
    .unwrap();
    assert!(hot.check(), "changed mtime must reload");
    assert_eq!(hot.get().commands.len(), 2);
    assert_eq!(hot.get().commands[1].name, "b");
}

#[test]
fn hot_reloader_tolerates_missing_file_and_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let hot: HotReloader<CommandsConfig> = HotReloader::new(path.clone(), load_commands);
    assert_eq!(hot.get().commands.len(), 0, "缺失文件 = 空表");

    // 空表 → 垃圾内容：mtime 变化触发重载，load 侧按空表兜底（不 panic）。
    sleep(Duration::from_millis(20));
    std::fs::write(&path, "{ not json").unwrap();
    assert!(hot.check(), "mtime changed → reload happens");
    assert_eq!(hot.get().commands.len(), 0);
}
