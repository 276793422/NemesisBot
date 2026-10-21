//! 通用文件热重载器（2026-08-29 热重载统一收编）。
//!
//! 收编此前各自手写的"mtime 变了才重读"模式（tier / commands / mcp 各一套，
//! 细节漂移）。新增性热更配置 = 写一个 `fn(&Path) -> T` load 函数 + 一行声明，
//! 不再复制 mtime 逻辑。调用方每消息/每轮主动 `check()`（一次 stat，可忽略），
//! 无回调订阅、无事件总线——与既有语义一致。
//!
//! NOT 线程语义：mtime 用 std Mutex、状态用 std RwLock（本 crate 无
//! parking_lot 依赖）；锁内不做 IO（mtime 先记、读盘在锁外——与
//! `check_config_reload` 的既有模式一致）。

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

/// Load a value of `T` from `path`. Must tolerate missing files (return the
/// type's default/empty semantics) — HotReloader never surfaces load errors.
pub type HotLoadFn<T> = fn(&Path) -> T;

pub struct HotReloader<T> {
    path: PathBuf,
    mtime: Mutex<Option<SystemTime>>,
    state: RwLock<T>,
    load: HotLoadFn<T>,
}

impl<T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static> HotReloader<T> {
    /// Create and load once. `load` must tolerate missing files.
    pub fn new(path: PathBuf, load: HotLoadFn<T>) -> Self {
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let state = load(&path);
        Self {
            path,
            mtime: Mutex::new(mtime),
            state: RwLock::new(state),
            load,
        }
    }

    /// Re-read if the file's mtime changed since the last check.
    /// Returns `true` when a reload actually happened.
    pub fn check(&self) -> bool {
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        {
            let mut last = self.mtime.lock().unwrap();
            if mtime == *last {
                return false; // unchanged since last check
            }
            *last = mtime;
        }
        // IO outside the mtime lock (same as check_config_reload's pattern).
        let value = (self.load)(&self.path);
        *self.state.write().unwrap() = value;
        true
    }

    /// 绕过 mtime 比较强制重读（写方落盘后立即自我刷新：mtime 分辨率
    /// 异常 / stat 瞬时失败都可能让下一次 `check()` 漏检——2026-10-22
    /// CI Windows 实录，审批 pattern 落盘后第二问漏检假红）。成功返回
    /// true；stat 失败按 None 记账并返回 false（下次 `check()` 兜底）。
    pub fn force_reload(&self) -> bool {
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        let value = (self.load)(&self.path);
        *self.mtime.lock().unwrap() = mtime;
        *self.state.write().unwrap() = value;
        mtime.is_some()
    }

    /// Current cached value (clone).
    pub fn get(&self) -> T {
        self.state.read().unwrap().clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
