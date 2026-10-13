//! Runtime config cache — single source of truth.
//!
//! [`ConfigStore`] holds the live `Config` in memory (`Arc<RwLock<Config>>`).
//! Consumers read from memory via [`ConfigHandle`]; mutations go through
//! [`ConfigStore::update`], which applies the change in-memory AND persists
//! to disk. This is the runtime cache layer that `load_config` / `save_config`
//! (pure file IO) alone do not provide.
//!
//! Why: without this, dashboard handlers either re-read `config.json` from
//! disk on every change (e.g. `check_config_reload`'s mtime poll) or don't
//! re-read at all (→ stale until gateway restart — that was the executor.sandbox
//! bug: stop the Sandboxie engine mid-run and the executor still tried the box
//! path for 30s). With `ConfigStore`, `executor.sandbox` / tier / DLP toggles
//! flip live without a gateway restart.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::{Config, Result, load_config, save_config};

/// Runtime cache of the live configuration. Wrap once at startup, share to
/// any consumer via [`ConfigStore::handle`].
pub struct ConfigStore {
    config: Arc<RwLock<Config>>,
    path: PathBuf,
}

impl ConfigStore {
    /// Load the config from disk and wrap it in an in-memory cache.
    pub fn load(path: &Path) -> Result<Self> {
        let config = load_config(path)?;
        Ok(Self::from_config(config, path.to_path_buf()))
    }

    /// Build a store around an already-loaded config (no disk read). Useful
    /// when the caller has already run `load_config` (e.g. gateway startup).
    pub fn from_config(config: Config, path: PathBuf) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            path,
        }
    }

    /// Path of the backing config file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A cheap, cloneable handle for consumers. All handles share the same
    /// underlying `Arc<RwLock<Config>>`, so an `update` is visible to every
    /// handle immediately — no restart, no disk re-read.
    pub fn handle(&self) -> ConfigHandle {
        ConfigHandle {
            config: self.config.clone(),
        }
    }

    /// Apply an in-memory mutation AND persist to disk atomically. The closure
    /// mutates the live config; after it returns the full config is serialized
    /// and written to `path`. The write guard is released before IO so other
    /// readers aren't blocked by the disk write.
    pub fn update<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut Config),
    {
        {
            let mut guard = self.config.write();
            f(&mut guard);
        }
        // Persist a snapshot. save_config takes &mut for local-mode path
        // auto-adjust — clone so we don't re-take the write guard.
        let mut snapshot = self.config.read().clone();
        save_config(&self.path, &mut snapshot)?;
        Ok(())
    }

    /// Re-read the config from disk, replacing the in-memory copy. Used to
    /// reconcile after external edits (CLI `model set-tier`, hand-edited file).
    pub fn reload(&self) -> Result<()> {
        let cfg = load_config(&self.path)?;
        *self.config.write() = cfg;
        Ok(())
    }
}

/// Read-only view onto the live config held by a [`ConfigStore`]. Cheap to
/// clone (Arc bump). `read()` returns a `RwLockReadGuard` — keep it short.
#[derive(Clone)]
pub struct ConfigHandle {
    config: Arc<RwLock<Config>>,
}

impl ConfigHandle {
    /// Read the live config. The guard blocks writers — drop it as soon as
    /// you've extracted what you need (don't hold across `.await`).
    pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, Config> {
        self.config.read()
    }
}

// ---------------------------------------------------------------------------
// Process-wide singleton.
//
// WSAPI handlers (sandbox/config/channels…) historically read config.json
// straight from disk and can't easily be wired with an `Arc<ConfigStore>`
// through AppState (which has dozens of test constructors). The singleton
// gives them a zero-wiring way to reach the *same* live store the gateway
// installed, so a dashboard write is visible everywhere instantly. Set once
// at gateway startup; `global()` returns None in CLI subcommands / tests.
// ---------------------------------------------------------------------------

static GLOBAL_STORE: std::sync::OnceLock<Arc<ConfigStore>> = std::sync::OnceLock::new();

/// Install the process-wide config store. Called once at gateway startup,
/// right after `ConfigStore::load`. Idempotent — later calls are ignored
/// (the first one wins, matching "single source of truth").
pub fn set_global(store: Arc<ConfigStore>) {
    let _ = GLOBAL_STORE.set(store);
}

/// The process-wide config store, if [`set_global`] was called. Returns
/// `None` where no store was installed (CLI subcommands, unit tests) —
/// callers should fall back to `load_config` from disk in that case.
pub fn global() -> Option<Arc<ConfigStore>> {
    GLOBAL_STORE.get().cloned()
}

// ---------------------------------------------------------------------------
// 进程级配置写串行锁（2026-10-13 同类横扫）。
//
// config.json（及 config.skills.json 等同族配置）的每个写命令臂都是
// load→改→save 的 RMW，而 WSAPI 每请求帧独立 tokio::spawn 并发分发——
// 两个 dashboard 写操作（或写臂与 agent 侧写）交错时，后写者以自己的
// 旧快照整体覆盖先写者的改动（save 原子救不了丢更新）。[`ConfigStore`]
// 的内存 RwLock 只串行「update 闭包内」的变异，罩不住「load 克隆→改→
// save_live 整体替换」与 raw JSON RMW 这类跨调用读改写。
//
// 纪律：**只在命令臂最外层取一次**，覆盖整段 RMW（从 load 到 save 落盘）；
// 辅助函数与 [`ConfigStore`] 内部**不得**重复取（Mutex 非重入，嵌套即死锁）。
// 读路径不入锁（rename 原子性保证读到完整旧值或完整新值）。
// ---------------------------------------------------------------------------

static CONFIG_WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// 取配置写串行锁。用法：写命令臂首行 `let _cfg_serial =
/// nemesis_config::config_write_lock();`，guard 存活到 save 落盘为止。
/// 见上方锁注释的取锁纪律（只在命令臂取、勿嵌套）。
pub fn config_write_lock() -> parking_lot::MutexGuard<'static, ()> {
    CONFIG_WRITE_LOCK.lock()
}

/// Convenience: read the live config from the global store (clone). Returns
/// `None` if no store is installed (CLI mode) — callers fall back to
/// `load_config` from disk.
pub fn load_live() -> Option<Config> {
    global().map(|s| s.handle().read().clone())
}

/// Convenience: replace the global store's config AND persist to disk in one
/// step. Returns `None` if no store is installed — callers fall back to
/// `save_config`. Goes through [`ConfigStore::update`], so every live consumer
/// (executor sandbox probe, tier, …) sees the new value on the next read —
/// no gateway restart.
pub fn save_live(new_cfg: Config) -> Option<Result<()>> {
    Some(global()?.update(|c| *c = new_cfg))
}

#[cfg(test)]
mod tests;
