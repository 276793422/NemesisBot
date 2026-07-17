//! 全局急停开关（E-stop / Kill Switch）状态。
//!
//! 单一实例挂在 `SharedResources` 上（`gateway::run` 里只建一次），每个
//! `AgentLoop` 经 `set_estop` 绑到它。触发后，agent 循环在每轮顶部 break、
//! 并在工具分发前拒绝调用。
//!
//! 线程安全：`AtomicBool`（廉价检查）+ `tokio::sync::watch`（异步订阅者，
//! 供未来 Phase 2「中途打断 LLM 调用」的 select arm 使用）。
//!
//! 设计要点：状态本体**不**挂在 `AgentLoop` 上（AgentLoop 每次 start/stop
//! 都重建），而是挂在 `SharedResources`（跨重启存活）。`AgentLoop` 只持有一
//! 个 `Option<Arc<EstopState>>` 引用——工厂每次重建 loop 时都重新绑定到同
//! 一个 Arc，所以「急停中」的状态在 agent 重启后**自动保持**（这才是真急停）。

use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

/// 全局急停状态。`Arc` 共享廉价，`AtomicBool` 检查廉价。
#[derive(Debug)]
pub struct EstopState {
    engaged: AtomicBool,
    tx: watch::Sender<bool>,
}

impl EstopState {
    /// 新建一个未触发（已释放）的急停开关。
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(false);
        Self {
            engaged: AtomicBool::new(false),
            tx,
        }
    }

    /// 触发急停：冻结 agent 循环、拒绝后续工具调用。
    pub fn trigger(&self) {
        self.engaged.store(true, Ordering::Release);
        // watch::send 出错只可能是无订阅者，忽略。
        let _ = self.tx.send(true);
    }

    /// 释放急停：下一条消息起 agent 恢复正常处理。
    pub fn release(&self) {
        self.engaged.store(false, Ordering::Release);
        let _ = self.tx.send(false);
    }

    /// 当前是否处于急停状态。
    pub fn is_engaged(&self) -> bool {
        self.engaged.load(Ordering::Acquire)
    }

    /// 订阅触发/释放的状态变化（供未来「中途打断 LLM 调用」钩子用）。
    /// receiver 的当前值即最新状态。
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }
}

impl Default for EstopState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
