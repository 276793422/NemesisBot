use super::*;
use std::ffi::CString;
use std::os::raw::c_char;

#[test]
fn build_sets_all_vtable_fields() {
    let dir = std::env::temp_dir().join("hs_test_build");
    std::fs::create_dir_all(&dir).unwrap();
    let hs = build_host_services(&dir);
    assert_eq!(hs.version, HOST_SERVICES_VERSION);
    assert!(hs.log.is_some());
    assert!(hs.get_workspace_dir.is_some());
    assert!(hs.get_plugin_data_dir.is_some());
    assert!(hs.get_plugin_config_dir.is_some());
    assert!(hs.file_exists.is_some());
    assert!(hs.file_size.is_some());
    assert!(hs.download_file.is_some());
    assert!(hs.free_string.is_some());
    // decode_png is left None — caller must set it if available.
    assert!(hs.decode_png.is_none());
}

#[test]
fn log_null_inputs_and_all_levels_no_panic() {
    let hs = build_host_services(&std::env::temp_dir());
    let log = hs.log.unwrap();
    // null tag/msg → early return (no panic)
    log(2, std::ptr::null(), std::ptr::null());
    let tag = CString::new("tag").unwrap();
    let msg = CString::new("hello").unwrap();
    // every level branch (0..4 + default/error)
    for lvl in 0..6 {
        log(lvl, tag.as_ptr(), msg.as_ptr());
    }
}

#[test]
fn get_workspace_dir_writes_path_to_buf() {
    let hs = build_host_services(&std::env::temp_dir());
    let mut buf = vec![0i8; 1024];
    let n = (hs.get_workspace_dir.unwrap())(buf.as_mut_ptr(), buf.len());
    assert!(n > 0, "should write a non-empty path");
}

#[test]
fn get_workspace_dir_small_buf_returns_negative() {
    let hs = build_host_services(&std::env::temp_dir());
    let mut buf = vec![0i8; 2];
    let n = (hs.get_workspace_dir.unwrap())(buf.as_mut_ptr(), buf.len());
    assert!(n < 0, "buf too small → negative required-size");
}

#[test]
fn get_workspace_dir_null_buf_returns_negative() {
    let hs = build_host_services(&std::env::temp_dir());
    let n = (hs.get_workspace_dir.unwrap())(std::ptr::null_mut(), 0);
    assert!(n < 0);
}

#[test]
fn file_exists_and_size_roundtrip() {
    let hs = build_host_services(&std::env::temp_dir());
    let path = std::env::temp_dir().join("hs_test_file.txt");
    std::fs::write(&path, b"hello").unwrap();
    let cpath = CString::new(path.to_str().unwrap()).unwrap();

    assert_eq!((hs.file_exists.unwrap())(cpath.as_ptr()), 1);
    assert_eq!((hs.file_size.unwrap())(cpath.as_ptr()), 5);

    // null → error
    assert!((hs.file_exists.unwrap())(std::ptr::null()) < 0);
    assert!((hs.file_size.unwrap())(std::ptr::null()) < 0);

    // nonexistent → 0 / -1
    let ghost = CString::new("/nonexistent/hs_ghost_path").unwrap();
    assert_eq!((hs.file_exists.unwrap())(ghost.as_ptr()), 0);
    assert_eq!((hs.file_size.unwrap())(ghost.as_ptr()), -1);
}

#[test]
fn get_plugin_data_dir_null_inputs_return_negative() {
    let hs = build_host_services(&std::env::temp_dir());
    let n = (hs.get_plugin_data_dir.unwrap())(std::ptr::null(), std::ptr::null_mut(), 0);
    assert!(n < 0);
}

#[test]
fn get_plugin_data_dir_valid_writes_path() {
    let hs = build_host_services(&std::env::temp_dir());
    let plugin = CString::new("test-plugin").unwrap();
    let mut buf = vec![0i8; 4096];
    let n = (hs.get_plugin_data_dir.unwrap())(plugin.as_ptr(), buf.as_mut_ptr(), buf.len());
    assert!(n > 0, "should write plugin data dir path");
}

#[test]
fn free_string_null_and_real_ptr_no_panic() {
    let hs = build_host_services(&std::env::temp_dir());
    let free = hs.free_string.unwrap();
    free(std::ptr::null_mut()); // null → noop
    let s = CString::new("to free").unwrap().into_raw();
    free(s); // reclaim allocated string
}

#[test]
fn tray_callbacks_copy_and_clone() {
    extern "C" fn cb(_ud: *mut c_void, _id: *const c_char) {}
    let tc = TrayCallbacks {
        user_data: std::ptr::null_mut(),
        on_menu_click: cb,
    };
    let tc2 = tc; // Copy
    let _tc3 = tc.clone(); // Clone
    assert!(tc2.user_data.is_null());
    assert!(std::ptr::eq(tc.on_menu_click as *const (), cb as *const ()));
}
