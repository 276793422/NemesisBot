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
