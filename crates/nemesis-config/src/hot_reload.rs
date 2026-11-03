//! 通用文件热重载器（2026-08-29 热重载统一收编）。
//!
//! 收编此前各自手写的"文件变了才重读"模式（tier / commands / mcp 各一套，
//! 细节漂移）。新增性热更配置 = 写一个 `fn(&Path) -> T` load 函数 + 一行声明，
//! 不再复制检测逻辑。调用方每消息/每轮主动 `check()`（一次小文件直读，可
//! 忽略），无回调订阅、无事件总线——与既有语义一致。
//!
//! NOT 线程语义：记账用 std Mutex、状态用 std RwLock（本 crate 无
//! parking_lot 依赖）；锁内不做 IO（盘读在锁外，锁内只比对/换值）。
//!
//! 变更检测 = **内容比对不赌钟**（BUG-20261104-359 同族收编）：mtime 等值
//! 判定在 Windows 毫秒级粒度（write_file_atomic 临时名同 tick 同 nanos，
//! 346d3dc2 / c584086a CI 实证同族）下漏检同刻改写，配置静默永久陈旧。
//! 现改为比对盘上原始文本（`fsutil::read_config_string` 同源，含 BOM 剥离）：
//! 内容相等才跳过——内容变必重载，与任何时钟粒度无关。缺失/不可读记账为
//! `None`（消失 → 重载 → load 函数的缺省语义 = 既有"文件消失换默认"不变）。

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

/// Load a value of `T` from `path`. Must tolerate missing files (return the
/// type's default/empty semantics) — HotReloader never surfaces load errors.
pub type HotLoadFn<T> = fn(&Path) -> T;

pub struct HotReloader<T> {
    path: PathBuf,
    /// 上次记账的盘上原始文本（read_config_string 同源；None = 缺失/不可读）。
    raw: Mutex<Option<String>>,
    state: RwLock<T>,
    load: HotLoadFn<T>,
}

impl<T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static> HotReloader<T> {
    /// Create and load once. `load` must tolerate missing files.
    pub fn new(path: PathBuf, load: HotLoadFn<T>) -> Self {
        let raw = nemesis_utils::fsutil::read_config_string(&path).ok();
        let state = load(&path);
        Self {
            path,
            raw: Mutex::new(raw),
            state: RwLock::new(state),
            load,
        }
    }

    /// Re-read if the file's content changed since the last check.
    /// Returns `true` when a reload actually happened.
    pub fn check(&self) -> bool {
        // 读在记账锁外（锁内不做 IO）；同刻写入的内容变化在此必现。
        let raw = nemesis_utils::fsutil::read_config_string(&self.path).ok();
        {
            let mut last = self.raw.lock().unwrap();
            if raw == *last {
                return false; // 内容未变（含 mtime 相同的同刻改写）
            }
            *last = raw.clone();
        }
        let value = (self.load)(&self.path);
        *self.state.write().unwrap() = value;
        true
    }

    /// 绕过比对强制重读（写方落盘后立即自我刷新；保留为写方自刷新契约——
    /// 2026-10-22 CI Windows 审批 pattern 假红回归钉，auditor 写路径消费）。
    /// 成功返回 true；读失败按 None 记账并返回 false（下次 `check()` 兜底）。
    pub fn force_reload(&self) -> bool {
        let raw = nemesis_utils::fsutil::read_config_string(&self.path).ok();
        let value = (self.load)(&self.path);
        *self.raw.lock().unwrap() = raw.clone();
        *self.state.write().unwrap() = value;
        raw.is_some()
    }

    /// Current cached value (clone).
    pub fn get(&self) -> T {
        self.state.read().unwrap().clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
