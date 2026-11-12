//! 单测公共件：宿主壳 DLL 就绪 + fixture 组件构建 + 管理器装配。
//!
//! 与 `tests/common/mod.rs` 同纪律：壳产物（`plugins/plugin-wasm/target/
//! release/plugin_wasm.dll`）缺席时按需 `cargo build --release`；路径经
//! `NEMESIS_WASM_HOST_DLL` 权威注入（env 候选序第一），随后预热进程级装载
//! 缓存——env 探针类用例（resolve/candidate 路径）之后临时改 env 也不会
//! 污染 `HostDll::open` 的缓存结论。fixture 组件（`plugins/wasm/<slug>`）
//! 产物缺席时按需 `cargo build --target wasm32-wasip2 --release`。

#![allow(dead_code)] // 各测试模块按需引用

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::host_impl::SecretResolver;
use crate::limits::PluginLimits;
use crate::registry::PluginManager;

/// env 探针用例的互斥闸（进程级 env 是全局态，并行用例串行化访问）。
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock")
}

static HOST_READY: OnceLock<()> = OnceLock::new();

fn dll_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "plugin_wasm.dll"
    } else if cfg!(target_os = "macos") {
        "libplugin_wasm.dylib"
    } else {
        "libplugin_wasm.so"
    }
}

/// 确保宿主壳 DLL 在位（构建 + env 注入 + 预热装载缓存）。panic = 环境不
/// 满足（cargo 不可用或壳编译失败），与被测行为回归区分。
pub(crate) fn ensure_host_dll() {
    HOST_READY.get_or_init(|| {
        let path = build_host_dll().expect("宿主壳 DLL 构建失败（环境错误）");
        // SAFETY: OnceLock 串行化的单写点；loader 候选序读取发生在其后。
        unsafe { std::env::set_var("NEMESIS_WASM_HOST_DLL", &path) };
        // 预热 loader 进程缓存：open 结论此后与 env 解耦（env 探针用例
        // 临时改 env 不影响后续 manager 构建）。
        match crate::loader::HostDll::open() {
            Ok(_) => {}
            Err(e) => panic!("宿主壳 DLL 装载失败（环境错误）: {e:?}"),
        }
    });
}

fn build_host_dll() -> Option<PathBuf> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/plugin-wasm");
    let out = crate_dir.join("target/release").join(dll_file_name());
    if !out.exists() && build_shell(&crate_dir, None).is_none() {
        let fallback = crate_dir.join("target/host-dll-fallback");
        return build_shell(&crate_dir, Some(&fallback));
    }
    out.exists().then_some(out)
}

fn build_shell(crate_dir: &Path, target_dir: Option<&Path>) -> Option<PathBuf> {
    let cargo = std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"));
    let mut cmd = std::process::Command::new(cargo);
    cmd.args(["build", "--release"]).current_dir(crate_dir);
    for k in [
        "CARGO_TARGET_DIR",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
    ] {
        cmd.env_remove(k);
    }
    if let Some(td) = target_dir {
        cmd.env("CARGO_TARGET_DIR", td);
    }
    match cmd.status() {
        Ok(s) if s.success() => {}
        s => {
            eprintln!("宿主壳 DLL 构建失败：{s:?}");
            return None;
        }
    }
    let out = target_dir
        .map(|td| td.join("release").join(dll_file_name()))
        .unwrap_or_else(|| crate_dir.join("target/release").join(dll_file_name()));
    out.exists().then_some(out)
}

/// fixture 组件产物路径（缺席时按需构建；构建失败 = None，调用方诚实跳过
/// 对应用例——与集成测试 SKIP 语义一致）。
pub(crate) fn fixture_wasm(slug_dir: &str, artifact: &str) -> Option<PathBuf> {
    static CACHE: OnceLock<std::collections::HashMap<(String, String), Option<PathBuf>>> =
        OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut m = std::collections::HashMap::new();
            let key = (slug_dir.to_string(), artifact.to_string());
            m.insert(key.clone(), build_fixture(slug_dir, artifact));
            m
        })
        .get(&(slug_dir.to_string(), artifact.to_string()))
        .cloned()
        .flatten()
}

fn build_fixture(slug_dir: &str, artifact: &str) -> Option<PathBuf> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/wasm")
        .join(slug_dir);
    let out = crate_dir
        .join("target/wasm32-wasip2/release")
        .join(format!("{artifact}.wasm"));
    if !out.exists() {
        let cargo = std::env::var_os("CARGO")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cargo"));
        let mut cmd = std::process::Command::new(cargo);
        cmd.args(["build", "--target", "wasm32-wasip2", "--release"])
            .current_dir(&crate_dir);
        for k in [
            "CARGO_TARGET_DIR",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_TARGET",
        ] {
            cmd.env_remove(k);
        }
        match cmd.status() {
            Ok(s) if s.success() => {}
            status => {
                eprintln!(
                    "SKIP：fixture 构建失败（{slug_dir}: {status:?}）。\
环境要求：rustup target add wasm32-wasip2 + 网络可取依赖"
                );
                return None;
            }
        }
    }
    out.exists().then_some(out)
}

struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

/// 独立临时 workspace + 显式注入目录的管理器（并行用例零串扰）。
/// TempDir 泄漏到进程结束（用例生命周期 = 进程生命周期，免清理竞态）。
pub(crate) fn fresh_manager(limits: PluginLimits) -> (PathBuf, Arc<PluginManager>) {
    ensure_host_dll();
    let ws = tempfile::tempdir().expect("tempdir");
    let root = ws.path().to_path_buf();
    std::mem::forget(ws);
    let mgr = Arc::new(
        PluginManager::new_with_dirs(
            &root,
            &root.join("plugins_wasm"),
            limits,
            Arc::new(NoSecrets),
        )
        .expect("manager"),
    );
    (root, mgr)
}

/// 默认限额的管理器（多实例并发上限收紧到 4，防满员误伤并行用例）。
pub(crate) fn default_manager() -> (PathBuf, Arc<PluginManager>) {
    let mut limits = PluginLimits::default();
    limits.max_instances = 4;
    fresh_manager(limits)
}

/// 把插件源目录写好（plugin.toml + 可选 wasm 载荷，sha 自动记账），返回
/// staging 目录。wasm=None = 纯声明式（无载荷，仅 kind=panel 合法）。
pub(crate) fn stage_plugin(root: &Path, toml_body: &str, wasm: Option<&[u8]>) -> PathBuf {
    let dir = root.join("src");
    std::fs::create_dir_all(&dir).expect("mkdir staging");
    let manifest = match wasm {
        Some(bytes) => {
            let sha = {
                use sha2::{Digest, Sha256};
                hex::encode(Sha256::digest(bytes))
            };
            let path = dir.join("plugin.wasm");
            std::fs::write(&path, bytes).expect("write wasm");
            format!("wasm-sha256 = \"{sha}\"\n{toml_body}\n")
        }
        None => format!("{toml_body}\n"),
    };
    std::fs::write(dir.join("plugin.toml"), manifest).expect("write manifest");
    dir
}

/// 关掉运行期总闸（`<workspace>/config/config.plugin.native.json` 内
/// plugin_wasm.enabled=false——per-workspace 文件，不影响其他用例）。
pub(crate) fn disable_runtime_gate(workspace_root: &Path) {
    let cfg = workspace_root.join("config");
    std::fs::create_dir_all(&cfg).expect("mkdir config");
    std::fs::write(
        cfg.join("config.plugin.native.json"),
        br#"{"plugins":{"plugin_wasm":{"enabled":false}}}"#,
    )
    .expect("write gate config");
}

/// 写旧 per-slug 实例文件（惰性迁移触发器）。
pub(crate) fn write_legacy_instance(workspace_root: &Path, slug: &str, entries: &str) {
    let legacy = workspace_root.join("config").join("plugins");
    std::fs::create_dir_all(&legacy).expect("mkdir legacy");
    std::fs::write(legacy.join(format!("{slug}.json")), entries).expect("write legacy file");
}

/// 旧载荷迁移触发器：workspace/plugins/lockfile.json + 一条 slug 子目录。
pub(crate) fn stage_legacy_payload(workspace_root: &Path, slug: &str, manifest_body: &str) {
    let legacy = workspace_root.join("plugins");
    let dir = legacy.join(slug);
    std::fs::create_dir_all(&dir).expect("mkdir legacy payload");
    std::fs::write(dir.join("plugin.toml"), manifest_body).expect("write legacy manifest");
    std::fs::write(
        legacy.join("lockfile.json"),
        format!(r#"{{"version":1,"plugins":{{"{slug}":{{"version":"0.1.0","wasm_sha256":"","trust":"trusted","installed_at_ms":0,"signed_by":"","connectors":[]}}}}}}"#),
    )
    .expect("write legacy lockfile");
}
