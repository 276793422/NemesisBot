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

/// DLL 路径（缺席/按需构建失败 = `None`），不 panic、不注入 env。
///
/// [`ensure_host_dll`] 的非致命版，供新鲜度探针类测试诚实 SKIP——缺席时
/// 由其他测试的 ensure 路径负责 loud 报告环境问题，这里不重复报警。
pub fn host_dll_optional() -> Option<PathBuf> {
    HOST_DLL.get_or_init(build_host_dll).clone()
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
    // BUG-HUNT R16：缓存此前只查存在不查新鲜度——壳源码更新（如同日宿主面
    // 接线回调表加槽）后旧 DLL 被静默信任，engine_init ABI 失配 → 全部引擎
    // 依赖套件报 "wasm host engine init failed: "（旧壳未落错误槽=空详情），
    // 本地全量红 27 例而 CI 全新 checkout 无缓存不受影响。改为：产物缺席
    // 或任一源文件比产物新（checkout/提交都会刷新 mtime，方向恒安全）→ 重建。
    if !out.exists() || dll_is_stale(&out, &crate_dir) {
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

// BUG-HUNT R16：过期判定回归锁。common 被每个 wasm 测试二进制各 include
// 一次，本用例会在多个二进制里重复执行——重复无害，断言纯函数级。

/// 源文件比产物新 → 过期（本次事故形态：壳源码更新、缓存 DLL 停留旧版）。
#[test]
fn dll_is_stale_detects_source_newer_than_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("plugin_wasm.dll");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::write(&out, b"dll").expect("write dll");
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(src.join("lib.rs"), b"new source").expect("write src");
    assert!(dll_is_stale(&out, dir.path()), "源新于产物必须判过期");
}

/// 产物最新 → 不过期；产物缺席 → 从严判过期。
#[test]
fn dll_is_stale_false_when_artifact_newest_true_when_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("plugin_wasm.dll");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    std::fs::write(src.join("lib.rs"), b"old source").expect("write src");
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&out, b"dll").expect("write dll");
    assert!(!dll_is_stale(&out, dir.path()), "产物最新不得判过期");
    assert!(
        dll_is_stale(&dir.path().join("absent.dll"), dir.path()),
        "产物缺席必须从严判过期"
    );
}

/// 产物过期判定：壳 crate 的 `src/**`（含目录自身 mtime，捕捉文件增删）
/// 与 `Cargo.toml` 任一比产物新 → 过期。缺席的检查项跳过（缺席不是过期
/// 的证据——如测试沙箱无 Cargo.toml）；产物自身不可读从严重建。
fn dll_is_stale(out: &std::path::Path, crate_dir: &std::path::Path) -> bool {
    let Ok(out_mtime) = std::fs::metadata(out).and_then(|m| m.modified()) else {
        return true;
    };
    let newer_than_out = |p: &std::path::Path| -> Option<bool> {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .map(|t| t > out_mtime)
    };
    let mut stack: Vec<PathBuf> = vec![crate_dir.join("src"), crate_dir.join("Cargo.toml")];
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            if newer_than_out(&p) == Some(true) {
                return true;
            }
            match std::fs::read_dir(&p) {
                Ok(entries) => stack.extend(entries.flatten().map(|e| e.path())),
                Err(_) => continue, // 缺席/不可读目录：跳过，不是过期证据
            }
        } else if newer_than_out(&p) == Some(true) {
            return true;
        }
    }
    false
}
