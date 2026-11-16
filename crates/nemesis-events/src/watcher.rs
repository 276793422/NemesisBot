//! notify RecommendedWatcher 包装：多根递归监听 + 规则热重载时的差分 rearm。
//!
//! notify 回调跑在其自有线程：事件经 bounded tokio channel（容量 1024）进泵。
//! 极端风暴下 channel 满即丢（try_send 失败静默）——事件源丢弃比背压拖垮泵
//! 任务可接受，三闸本就是丢弃语义。只关心 Create/Modify/Remove/Other；
//! Access（编辑器读取等）不算「出事」。
//!
//! `rearm` 差分纪律：只 unwatch 消失的根、只 watch 新增的根——重载时其余根
//! 的 in-flight 事件不断流。

use notify::{RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// 一条原始 fs 事件（未匹配规则；匹配在 manager 侧做）。
#[derive(Debug, Clone)]
pub struct RawFsEvent {
    pub path: String,
    pub at_ms: u64,
}

/// channel 满丢弃计数（观测面；跨 watcher 重挂存活）。
#[derive(Debug, Default)]
pub struct ChannelStats {
    pub dropped: AtomicU64,
}

impl ChannelStats {
    pub fn dropped_total(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

pub struct FsWatcher {
    watcher: Option<notify::RecommendedWatcher>,
    /// 当前已挂监听的根集合（差分基准）。
    armed: BTreeSet<PathBuf>,
    pub stats: Arc<ChannelStats>,
}

impl FsWatcher {
    pub fn new(tx: tokio::sync::mpsc::Sender<RawFsEvent>) -> Result<Self, String> {
        let stats = Arc::new(ChannelStats::default());
        let stats_for_cb = Arc::clone(&stats);
        let watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                let Ok(ev) = res else {
                    return; // 平台级错误（句柄失效等）：丢弃，无根级语义
                };
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return; // 读取类事件不是「出事」
                }
                let at_ms = now_ms();
                for p in ev.paths {
                    let item = RawFsEvent {
                        path: p.to_string_lossy().into_owned(),
                        at_ms,
                    };
                    if tx.try_send(item).is_err() {
                        stats_for_cb.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
            .map_err(|e| format!("notify watcher init failed: {e}"))?;
        Ok(Self {
            watcher: Some(watcher),
            armed: BTreeSet::new(),
            stats,
        })
    }

    /// 差分挂监听：新增根 watch（递归），消失根 unwatch。单根 watch 失败
    /// （目录尚不存在等）= warn 留痕继续（规则仍有效；目录就绪后需再触发
    /// 重载才会挂上——已知边界，P1 接受）。
    pub fn rearm(&mut self, roots: &BTreeSet<PathBuf>) {
        let Some(w) = self.watcher.as_mut() else {
            return;
        };
        for gone in self.armed.difference(roots) {
            if let Err(e) = w.unwatch(gone) {
                tracing::debug!("[Events] unwatch {:?} note: {e}", gone);
            }
        }
        for fresh in roots.difference(&self.armed) {
            match w.watch(fresh, RecursiveMode::Recursive) {
                Ok(()) => tracing::info!("[Events] watching {} (recursive)", fresh.display()),
                Err(e) => tracing::warn!(
                    "[Events] watch {} failed: {e}（目录就绪后重触发规则重载才会挂上）",
                    fresh.display()
                ),
            }
        }
        self.armed = roots.clone();
    }

    /// 停机：drop watcher（后台线程随之收）。
    pub fn shutdown(&mut self) {
        if let Some(mut w) = self.watcher.take() {
            let _ = &mut w; // drop 即停
        }
        self.armed.clear();
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
