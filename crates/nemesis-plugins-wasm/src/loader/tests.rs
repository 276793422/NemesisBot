use super::*;

// -- 平台壳库名（cfg 分支按当前平台取值）-----------------------------------

#[test]
fn dll_file_name_matches_platform() {
    let name = dll_file_name();
    if cfg!(target_os = "windows") {
        assert_eq!(name, "plugin_wasm.dll");
    } else if cfg!(target_os = "macos") {
        assert_eq!(name, "libplugin_wasm.dylib");
    } else {
        assert_eq!(name, "libplugin_wasm.so");
    }
}

// -- 候选路径序列（env 权威覆盖 + 顺序）------------------------------------

#[test]
fn candidate_paths_env_authority() {
    let _guard = crate::test_support::env_lock();
    crate::test_support::ensure_host_dll();
    let marker = tempfile::tempdir().expect("tmp");
    let fake = marker.path().join("shell.dll");
    std::fs::write(&fake, b"fake").expect("write");
    let saved = std::env::var("NEMESIS_WASM_HOST_DLL").ok();
    // SAFETY: env_lock 串行化的测试窗口内读写；结束后恢复原值。
    unsafe { std::env::set_var("NEMESIS_WASM_HOST_DLL", &fake) };
    let paths = candidate_paths();
    unsafe {
        match saved {
            Some(v) => std::env::set_var("NEMESIS_WASM_HOST_DLL", v),
            None => std::env::remove_var("NEMESIS_WASM_HOST_DLL"),
        }
    }
    assert_eq!(paths, vec![fake], "env 显式指定 = 只认该路径不回落");
}

#[test]
fn candidate_paths_no_env_yields_ordered_candidates() {
    let _guard = crate::test_support::env_lock();
    crate::test_support::ensure_host_dll();
    let saved = std::env::var("NEMESIS_WASM_HOST_DLL").ok();
    // SAFETY: env_lock 串行化窗口；结束后恢复。
    unsafe { std::env::remove_var("NEMESIS_WASM_HOST_DLL") };
    let paths = candidate_paths();
    unsafe {
        match saved {
            Some(v) => std::env::set_var("NEMESIS_WASM_HOST_DLL", v),
            None => std::env::remove_var("NEMESIS_WASM_HOST_DLL"),
        }
    }
    assert!(
        paths.len() >= 4,
        "exe 同目录 + plugins/ + 两条开发路径: {paths:?}"
    );
    assert!(
        paths.iter().all(|p| p.ends_with(dll_file_name())),
        "全部候选钉壳库名: {paths:?}"
    );
}

#[test]
fn resolve_host_dll_path_env_authority_and_absence() {
    let _guard = crate::test_support::env_lock();
    crate::test_support::ensure_host_dll();
    let saved = std::env::var("NEMESIS_WASM_HOST_DLL").ok();
    // 指向真实文件 = 直接命中该文件。
    let real = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/plugin-wasm/target/release")
        .join(dll_file_name());
    assert!(
        real.is_file(),
        "壳产物应在位（ensure_host_dll 已构建）: {}",
        real.display()
    );
    unsafe { std::env::set_var("NEMESIS_WASM_HOST_DLL", &real) };
    assert_eq!(resolve_host_dll_path(), Some(real.clone()));
    // 指向不存在路径 = None（无候选回落）。
    unsafe { std::env::set_var("NEMESIS_WASM_HOST_DLL", "Z:/definitely/absent.dll") };
    assert_eq!(resolve_host_dll_path(), None);
    // 空串 = 视同未设置（回落候选序）。
    unsafe { std::env::set_var("NEMESIS_WASM_HOST_DLL", "  ") };
    let _ = resolve_host_dll_path();
    unsafe {
        match saved {
            Some(v) => std::env::set_var("NEMESIS_WASM_HOST_DLL", v),
            None => std::env::remove_var("NEMESIS_WASM_HOST_DLL"),
        }
    }
}

// -- 定点装载失败面（不触进程缓存）------------------------------------------

#[test]
fn open_at_missing_path_fails() {
    let err = open_at(std::path::Path::new("Z:/definitely/absent/plugin_wasm.dll"))
        .err()
        .expect("absent dll");
    let text = err.to_string();
    assert!(text.contains("not available"), "{text}");
}

#[test]
fn open_at_non_dll_file_fails() {
    let dir = tempfile::tempdir().expect("tmp");
    let fake = dir.path().join("fake.dll");
    std::fs::write(&fake, b"this is not a dynamic library").expect("write");
    let err = open_at(&fake).err().unwrap_or_else(|| panic!("not a dll"));
    assert!(err.to_string().contains("not available"));
}

// -- 定点装载成功 + ABI 握手 ------------------------------------------------

#[test]
fn open_at_real_shell_and_engine_init_roundtrip() {
    crate::test_support::ensure_host_dll();
    let real = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/plugin-wasm/target/release")
        .join(dll_file_name());
    let host = open_at(&real).expect("real shell loads");
    assert_eq!(unsafe { (host.abi_version)() }, abi::ABI_VERSION);
    // engine_init 幂等成功（缓存目录 = 测试临时目录）。
    let cache = tempfile::tempdir().expect("cache dir");
    host.engine_init(Some(cache.path())).expect("engine init");
    // 空串缓存根退化为无缓存路径（CString 替换分支）。
    host.engine_init(Some(std::path::Path::new("\0bad"))).ok();
    host.engine_init(None).expect("engine init without cache");
    // take_last_error 消费（成功路径错误槽为空）。
    let _ = host.take_last_error();
}

// -- 组件句柄面（经真实壳；垃圾字节 = 编译失败路径）---------------------------

#[test]
fn component_new_rejects_garbage_and_drop_tolerates_zero() {
    crate::test_support::ensure_host_dll();
    let real = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/plugin-wasm/target/release")
        .join(dll_file_name());
    let host = open_at(&real).expect("shell");
    host.engine_init(None).expect("init");
    let err = host
        .component_new(b"not-wasm-bytes")
        .err()
        .unwrap_or_else(|| panic!("compile fails"));
    assert!(matches!(err, PluginError::Compile(_)), "{err:?}");
    // 0 = 无效哨兵，drop 容忍。
    host.component_drop(0);
    // last_error 非空（组件失败已写槽）。
    assert!(!host.take_last_error().is_empty());
    // 字符串归还端（take_out_string 逻辑由 last_error 消费覆盖非空臂；
    // 空指针臂在此显式钉死）。
    // SAFETY: 空指针入参走 None 早退，不触碰 free。
    let none = unsafe { take_out_string(&host.free_string, std::ptr::null_mut()) };
    assert!(none.is_none());
}
