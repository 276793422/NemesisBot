//! 壳 DLL 陈旧复发回归锁（2026-10-24）。
//!
//! 实录：plugin_bridge 21 测试集体红、错误全是 `NotAvailable("wasm host
//! engine init failed: ")`（冒号后**空串**）——不是宿主代码回归，是
//! `plugins/plugin-wasm` 壳 DLL 陈旧（独立 CARGO_TARGET_DIR、非 workspace
//! member，不随 workspace 重编自动更新），宿主侧「engine_init 失败路径落
//! 错误槽」的修复没进 DLL；DLL mtime 是拷贝/重链产物，会谎报新鲜度。
//! loader 对 `nvwa_last_error()` 返回 NULL 时 `unwrap_or_default()` 落空
//! 串，真实失败原因被吞。
//!
//! 本测试直装 DLL 验证「当前源码契约」：`engine_init(NULL, NULL)` 按现行
//! 源码必落错误槽 `callbacks pointer is null`（abi.rs::engine_init_impl
//! 首个守卫）。实测错误槽为空 = 装载的 DLL 与源码不符（陈旧），loud 失败
//! 并给出重编指引。这补上 [`common::ensure_host_dll`] 的盲区——它只在
//! DLL **缺席**时构建，对**在场但陈旧**的 DLL 不设防。

mod common;

use std::ffi::{c_char, c_void};

/// 壳 ABI 代际镜像（权威源 `plugins/plugin-wasm/src/abi.rs::ABI_VERSION`；
/// abi 模块在宿主侧私有，此处按值镜像 + 上方指针防漂移）。
const EXPECTED_ABI_VERSION: u32 = 1;

#[test]
fn host_dll_engine_init_null_callbacks_sets_error_slot() {
    let Some(path) = common::host_dll_optional() else {
        eprintln!("SKIP: 壳 DLL 缺席且按需构建不可用（环境问题由 ensure_host_dll 路径 loud 报告）");
        return;
    };

    unsafe {
        // load-once-never-unload 同宿主 loader 纪律：进程内不卸载（libloading
        // Library drop 即 unload，此处让 lib 活到测试结束，避免析构顺序坑）。
        let lib = libloading::Library::new(&path).expect("壳 DLL 必须可装载");

        let abi_version: libloading::Symbol<'_, unsafe extern "C" fn() -> u32> = lib
            .get(b"nvwa_abi_version\0")
            .expect("nvwa_abi_version 导出必须在场");
        assert_eq!(
            abi_version(),
            EXPECTED_ABI_VERSION,
            "壳 DLL ABI 代际与宿主镜像不符——DLL 陈旧，重编：cd plugins/plugin-wasm && cargo build --release"
        );

        let engine_init: libloading::Symbol<
            '_,
            unsafe extern "C" fn(*const c_char, *const c_void) -> u64,
        > = lib
            .get(b"nvwa_engine_init\0")
            .expect("nvwa_engine_init 导出必须在场");
        // NULL 回调表：按现行源码必失败（守卫在最前，零副作用、不触引擎）。
        let handle = engine_init(std::ptr::null(), std::ptr::null());
        assert_eq!(
            handle, 0,
            "engine_init(NULL, NULL) 必须失败——返回非 0 说明 DLL 行为与源码不符"
        );

        let last_error: libloading::Symbol<'_, unsafe extern "C" fn() -> *mut c_char> = lib
            .get(b"nvwa_last_error\0")
            .expect("nvwa_last_error 导出必须在场");
        let free_string: libloading::Symbol<'_, unsafe extern "C" fn(*mut c_char)> = lib
            .get(b"nvwa_free_string\0")
            .expect("nvwa_free_string 导出必须在场");

        let err_ptr = last_error();
        // 陈旧 DLL 的失败路径不落错误槽 → 此处 NULL → 宿主 unwrap_or_default
        // 吞成空串（实录故障形态）。非空 = 失败原因可见 = 修复已在 DLL 内。
        assert!(
            !err_ptr.is_null(),
            "engine_init 失败后错误槽为空：装载的壳 DLL 与源码不符（陈旧——\
             失败路径落错误槽的修复未进 DLL）。重编：cd plugins/plugin-wasm && cargo build --release"
        );
        let err = std::ffi::CStr::from_ptr(err_ptr as *const c_char)
            .to_string_lossy()
            .into_owned();
        free_string(err_ptr);

        assert!(
            err.contains("callbacks"),
            "engine_init(NULL, NULL) 的错误文案应指向回调表守卫，实得: {err}"
        );
    }
}
