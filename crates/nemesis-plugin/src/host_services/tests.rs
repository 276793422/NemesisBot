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
    let _tc3 = tc; // Clone
    assert!(tc2.user_data.is_null());
    assert!(std::ptr::eq(tc.on_menu_click as *const (), cb as *const ()));
}

// ==================== 补覆盖：get_plugin_config_dir / download_file ====================
// 对应 llvm-cov 未达行：160-166（config_dir 全函数）、220-240（download_file 全函数）。
// 注意：WORKSPACE_DIR_PTR / CONFIG_DIR_PTR 是进程级 OnceLock（先到先得），
// 本段所有断言都以 get_workspace_dir 的【实际返回值】为基准，
// 不假设进程里是哪个测试先把 workspace 设置成了哪个目录。

/// 从 i8 缓冲区读回前 len 字节并按 UTF-8 解码。
fn read_c_str(buf: &[i8], len: usize) -> String {
    let bytes: Vec<u8> = buf[..len].iter().map(|&b| b as u8).collect();
    String::from_utf8(bytes).expect("host path should be utf-8")
}

#[test]
fn get_plugin_config_dir_writes_workspace_config_plugins() {
    let hs = build_host_services(&std::env::temp_dir());

    // 基准：进程内实际生效的 workspace（OnceLock 先到先得）
    let mut ws_buf = vec![0i8; 4096];
    let ws_n = (hs.get_workspace_dir.unwrap())(ws_buf.as_mut_ptr(), ws_buf.len());
    assert!(ws_n > 0);
    let ws = read_c_str(&ws_buf, ws_n as usize);

    let mut buf = vec![0i8; 4096];
    let n = (hs.get_plugin_config_dir.unwrap())(buf.as_mut_ptr(), buf.len());
    assert!(n > 0, "config dir path should fit in 4096 bytes");
    let got = read_c_str(&buf, n as usize);

    let expect = Path::new(&ws).join("config").join("plugins");
    assert_eq!(Path::new(&got), expect, "config dir = <ws>/config/plugins");
}

#[test]
fn get_plugin_config_dir_bad_buf_returns_negative() {
    let hs = build_host_services(&std::env::temp_dir());
    // 缓冲区过小 → 返回 -(所需长度)
    let mut small = vec![0i8; 2];
    let n = (hs.get_plugin_config_dir.unwrap())(small.as_mut_ptr(), small.len());
    assert!(n < 0, "small buffer must return negative required size");
    // null buf / 长度 0 → -1
    let n2 = (hs.get_plugin_config_dir.unwrap())(std::ptr::null_mut(), 0);
    assert_eq!(n2, -1);
}

#[test]
fn get_plugin_data_dir_matches_workspace_join() {
    // 语义验证：data dir = <ws>/plugins/<name>（host 侧顺带保证目录存在）
    let hs = build_host_services(&std::env::temp_dir());
    let mut ws_buf = vec![0i8; 4096];
    let ws_n = (hs.get_workspace_dir.unwrap())(ws_buf.as_mut_ptr(), ws_buf.len());
    assert!(ws_n > 0);
    let ws = read_c_str(&ws_buf, ws_n as usize);

    let plugin = CString::new("cov-plugin").unwrap();
    let mut buf = vec![0i8; 4096];
    let n =
        (hs.get_plugin_data_dir.unwrap())(plugin.as_ptr(), buf.as_mut_ptr(), buf.len());
    assert!(n > 0);
    let got = read_c_str(&buf, n as usize);
    assert_eq!(
        Path::new(&got),
        Path::new(&ws).join("plugins").join("cov-plugin")
    );
}

/// 起一个一次性裸 HTTP 服务器（std 同步线程），返回端口。
/// host_download_file 自建 current_thread runtime 再 block_on，
/// 配套服务器不能依赖外层 tokio runtime，直接用同步 socket 实现。
fn spawn_one_shot_http_server(body: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local server");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf); // 丢弃请求（读到多少不影响直接回包）
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        }
    });
    port
}

/// 挑一个保证无监听的回环端口（bind :0 后立刻 drop → 后续连接必被拒绝）。
fn dead_loopback_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

#[test]
fn download_file_null_pointers_return_minus_1() {
    let hs = build_host_services(&std::env::temp_dir());
    let dl = hs.download_file.unwrap();
    let url = CString::new("http://127.0.0.1:1/x").unwrap();
    let dest = CString::new("/definitely/not/used").unwrap();
    assert_eq!(dl(std::ptr::null(), dest.as_ptr()), -1);
    assert_eq!(dl(url.as_ptr(), std::ptr::null()), -1);
}

#[test]
fn download_file_unreachable_upstream_returns_minus_3() {
    // 连接拒绝（本地回环确定性失败）→ download_file Err → -3
    let hs = build_host_services(&std::env::temp_dir());
    let dl = hs.download_file.unwrap();
    let port = dead_loopback_port();
    let url = CString::new(format!("http://127.0.0.1:{port}/model.bin")).unwrap();
    let dest_path = std::env::temp_dir().join("hs_dl_refused.bin");
    let _ = std::fs::remove_file(&dest_path); // 清掉历史残留，保证 exists 断言干净
    let dest = CString::new(dest_path.to_str().unwrap()).unwrap();
    assert_eq!(
        dl(url.as_ptr(), dest.as_ptr()),
        -3,
        "connection refused must map to -3"
    );
    assert!(!dest_path.exists(), "failed download must not write dest file");
    let _ = std::fs::remove_file(&dest_path);
}

#[test]
fn download_file_local_http_success_writes_dest() {
    // 本地回环一次性 HTTP 服务器 → 下载成功返回 0 且落盘内容一致
    let hs = build_host_services(&std::env::temp_dir());
    let dl = hs.download_file.unwrap();
    let port = spawn_one_shot_http_server("MODEL_BYTES_12345");
    let url = CString::new(format!("http://127.0.0.1:{port}/model.bin")).unwrap();
    let dest_path =
        std::env::temp_dir().join(format!("hs_dl_ok_{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&dest_path);
    let dest = CString::new(dest_path.to_str().unwrap()).unwrap();

    assert_eq!(
        dl(url.as_ptr(), dest.as_ptr()),
        0,
        "local http download should succeed"
    );
    assert_eq!(
        std::fs::read(&dest_path).expect("dest file should be written"),
        b"MODEL_BYTES_12345".to_vec()
    );
    let _ = std::fs::remove_file(&dest_path);
}

#[test]
fn s12b_libc_strlen_null_pointer_returns_zero() {
    // S12b batch（quality-hardening goal 冲刺）：libc_strlen 的 null 防御臂此前
    // 从未单独触达。注意只能直接调 strlen 本身——经 write_cstr_to_buf 传 null
    // 会在后续 copy_nonoverlapping 解引用 null（这正是该臂只守 strlen 的原因）。
    unsafe {
        assert_eq!(libc_strlen(std::ptr::null()), 0usize);
        // 对照：非空 C 字符串返回长度
        let s = b"abc\0";
        assert_eq!(libc_strlen(s.as_ptr() as *const c_char), 3usize);
    }
}
