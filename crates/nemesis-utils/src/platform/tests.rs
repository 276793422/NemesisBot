use super::*;

#[test]
fn test_plugin_library_filename() {
    let name = plugin_library_filename("plugin_ui");
    if cfg!(target_os = "windows") {
        assert_eq!(name, "plugin_ui.dll");
    } else if cfg!(target_os = "macos") {
        assert_eq!(name, "libplugin_ui.dylib");
    } else {
        assert_eq!(name, "libplugin_ui.so");
    }
}

#[test]
fn test_plugin_library_label() {
    let label = plugin_library_label();
    if cfg!(target_os = "windows") {
        assert_eq!(label, "DLL");
    } else if cfg!(target_os = "macos") {
        assert_eq!(label, "dylib");
    } else {
        assert_eq!(label, "shared library");
    }
}

#[test]
fn test_find_plugin_library_returns_none_for_nonexistent() {
    assert!(find_plugin_library("nonexistent_plugin_xyz").is_none());
}

#[test]
fn test_find_plugin_library_in_returns_none_for_nonexistent() {
    let dir = std::env::current_dir().unwrap();
    assert!(find_plugin_library_in(&dir, "nonexistent_plugin_xyz").is_none());
}

#[test]
fn test_find_plugin_library_in_finds_primary_variant() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let lib = plugins.join(plugin_library_filename("plugin_ui"));
    std::fs::write(&lib, b"").unwrap();

    let found = find_plugin_library_in(dir.path(), "plugin_ui").unwrap();
    assert_eq!(found, lib);
}

#[test]
fn test_find_plugin_library_in_falls_back_to_hyphenated_variant() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    // 主名（下划线变体）不存在，只有连字符变体
    let lib = plugins.join(plugin_library_filename("plugin-ui"));
    std::fs::write(&lib, b"").unwrap();

    let found = find_plugin_library_in(dir.path(), "plugin_ui").unwrap();
    assert_eq!(found, lib);
}

/// base_name 不含下划线时 hyphenated == base_name，跳过 fallback 分支
/// 直接返回 None（覆盖 `hyphenated != base_name` 为 false 的路径）。
#[test]
fn test_find_plugin_library_in_no_underscore_skips_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();

    assert!(find_plugin_library_in(dir.path(), "pluginxy").is_none());
}
