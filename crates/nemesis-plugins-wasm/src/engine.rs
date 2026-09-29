//! wasmtime Engine 单例 + epoch ticker。
//!
//! 全进程共享一个 Engine（编译缓存/池化配置按 Engine 生效；多 Engine 会
//! 重复编译缓存且浪费内存）。`consume_fuel` + `epoch_interruption` 双预算
//! 开关在 Engine 级打开，per-call 在 Store 级设定具体额度。

use std::sync::OnceLock;

use crate::error::PluginError;

/// epoch ticker 的步进间隔（墙钟超时粒度；50ms 足够细且唤醒开销可忽略）。
pub const EPOCH_TICK_MS: u64 = 50;

static ENGINE: OnceLock<wasmtime::Engine> = OnceLock::new();

/// 取进程级 Engine 单例（首次调用时按默认配置构造 + 启动 epoch ticker）。
///
/// 编译缓存：`cache_root` 提供时，在 `{cache_root}/wasmtime-cache.toml`
/// 写 wasmtime 缓存配置（`directory` 指向该目录下的 entries）并加载；
/// 写/加载失败静默回退无缓存（缓存是性能优化不是正确性依赖）。
pub fn engine(cache_root: Option<&std::path::Path>) -> Result<wasmtime::Engine, PluginError> {
    if let Some(e) = ENGINE.get() {
        return Ok(e.clone());
    }
    let engine = build_engine(cache_root)?;
    // 真正落单例：此前只 get 不 set，单例契约失效——每次调用新建 Engine
    // 并随之泄漏一个永不退出的 epoch ticker 线程（2026-09-29 交付审查 M1）。
    // 并发首调竞态下输家的 engine 被丢弃（一次性，可接受；生产首调在
    // gateway 装配期单点发生）。缓存配置以首个成功构造者为准。
    let _ = ENGINE.set(engine.clone());
    Ok(engine)
}

fn build_engine(cache_root: Option<&std::path::Path>) -> Result<wasmtime::Engine, PluginError> {
    let mut cfg = wasmtime::Config::new();
    cfg.wasm_component_model(true);
    cfg.consume_fuel(true);
    cfg.epoch_interruption(true);
    if let Some(root) = cache_root {
        match enable_workspace_cache(root) {
            Ok(cache) => {
                cfg.cache(Some(cache));
            }
            Err(e) => tracing::warn!(
                root = %root.display(),
                error = %e,
                "[WasmEngine] 编译缓存启用失败，回退无缓存"
            ),
        }
    }
    let engine = wasmtime::Engine::new(&cfg).map_err(|e| PluginError::Compile(e.to_string()))?;
    start_epoch_ticker(engine.clone())?;
    Ok(engine)
}

/// 把 workspace 缓存目录接进 wasmtime Engine 配置。
fn enable_workspace_cache(root: &std::path::Path) -> Result<wasmtime::Cache, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let dir = root.join("entries");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cfg_path = root.join("wasmtime-cache.toml");
    // wasmtime 48 的 CacheConfig `#[serde(deny_unknown_fields)]`：目录键名
    // 是 `directory` 且没有 `enabled` 开关（空 `[cache]` 表即启用，见
    // wasmtime-internal-cache config.rs 的 create_new_config 模板）——写
    // `dir=`/`enabled=` 任一未知键都会让整个 config 解析失败回退无缓存
    //（2026-09-29 实证：每次进程启动必 warn）。directory 同时剥 `\\?\`
    // verbatim 前缀（config 里的路径走常规形态最稳，verbatim 只认反斜杠
    // 且会被下面的斜杠替换破坏）。
    let dir_str = dir.to_string_lossy();
    let dir_str = dir_str
        .strip_prefix(r"\\?\")
        .unwrap_or(dir_str.as_ref())
        .replace('\\', "/");
    let body = format!("[cache]\ndirectory = '{dir_str}'\n");
    std::fs::write(&cfg_path, body).map_err(|e| e.to_string())?;
    wasmtime::Cache::from_file(Some(&cfg_path)).map_err(|e| format!("load cache config: {e}"))
}

/// 启动进程级 epoch ticker（幂等：多调用只起一个线程）。
///
/// 用独立 OS 线程而非 tokio 任务：headless（`nemesisbot run`）与 exec_worker
/// 形态可能没有常驻 runtime，epoch 墙钟闸不能依赖 tokio 在场。
/// spawn 失败诚实报错而非 panic：没有 ticker，epoch deadline 永不推进，
/// 墙钟超时闸（fuel 炸弹的兜底）整体失效——这种构造必须失败而不是带病运行。
fn start_epoch_ticker(engine: wasmtime::Engine) -> Result<(), PluginError> {
    std::thread::Builder::new()
        .name("wasm-epoch-ticker".into())
        .spawn(move || {
            let tick = std::time::Duration::from_millis(EPOCH_TICK_MS);
            loop {
                std::thread::sleep(tick);
                engine.increment_epoch();
            }
        })
        .map(|_| ())
        .map_err(|e| PluginError::Compile(format!("epoch ticker 线程创建失败: {e}")))
}

/// 把墙钟超时毫秒换算成 epoch deadline 的 tick 数（至少 1 tick）。
#[must_use]
pub fn epoch_deadline_ticks(timeout_ms: u64) -> u64 {
    (timeout_ms / EPOCH_TICK_MS).max(1)
}
