//! 测试公共件：宿主壳 DLL 就绪保障。
//!
//! 壳产物（`plugins/plugin-wasm/target/release/plugin_wasm.dll`）缺席时
//! 按需 `cargo build --release`（壳 crate 自有 workspace → 独立 target-dir，
//! 与 adversarial.rs 的 wasm fixture 构建同纪律）；产物路径经
//! `NEMESIS_WASM_HOST_DLL` 注入 loader（env 候选序第一——测试二进制在
//! `target/debug/deps/`，exe-相邻与仓库开发路径都摸不到壳产物）。
//!
//! 壳是本仓库一等公民（同工具链同源码），构建失败按环境错误 loud 失败，
//! 不做 SKIP——与 wasm32 fixture（需额外 target + 网络）的 SKIP 语义区分。

#![allow(dead_code)] // 各测试二进制按需引用

use std::path::PathBuf;
use std::sync::OnceLock;

static HOST_DLL: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 确保宿主壳 DLL 在场（构建 + env 注入）。panic = 环境不满足（cargo 不可用
/// 或壳编译失败），与「被测行为回归」区分。
pub fn ensure_host_dll() {
    let path = HOST_DLL.get_or_init(build_host_dll);
    let Some(p) = path else {
        panic!("宿主壳 DLL 构建失败（环境错误）：需 cargo 可用 + 壳 crate 可编译");
    };
    // SAFETY: 单写点（OnceLock 串行化）；后续读者 = 同进程 loader 候选序
    // 解析，均发生在 ensure 返回之后。
    unsafe { std::env::set_var("NEMESIS_WASM_HOST_DLL", p) };
}

fn dll_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "plugin_wasm.dll"
    } else if cfg!(target_os = "macos") {
        "libplugin_wasm.dylib"
    } else {
        "libplugin_wasm.so"
    }
}

fn build_host_dll() -> Option<PathBuf> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/plugin-wasm");
    let out = crate_dir.join("target/release").join(dll_file_name());
    if !out.exists() {
        let cargo = std::env::var_os("CARGO")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cargo"));
        let mut cmd = std::process::Command::new(cargo);
        cmd.args(["build", "--release"]).current_dir(&crate_dir);
        // 隔离外层 cargo 注入（嵌套 cargo 目标目录 / 编码 flags 串扰）。
        for k in [
            "CARGO_TARGET_DIR",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_TARGET",
        ] {
            cmd.env_remove(k);
        }
        match cmd.status() {
            Ok(s) if s.success() => {}
            s => {
                eprintln!("宿主壳 DLL 构建失败：{s:?}");
                return None;
            }
        }
    }
    out.exists().then_some(out)
}
