//! A4 装配纪律 helper（工程模式专项）：**等待「子进程就绪」的循环
//! 必须耦合子进程存活断言**。
//!
//! 病灶形态：spawn 子进程后轮询某个就绪信号（HTTP health / 管道连接 /
//! 状态文件），轮询循环只看就绪信号、从不看子进程死活——子进程半路崩了
//! （启动 panic / AV 拦截 / 参数错 / 依赖缺失），调用方照样傻等满 deadline
//! 只拿到一句「未就绪超时」，真正的死因（退出码）被吞掉。
//!
//! 本模块把「就绪探针 + 存活断言」的耦合做成单一入口 [`wait_for_ready`]：
//! 每 tick 先跑就绪探针（单次至多 `poll_interval`，超窗取消下轮重试），再
//! 断言子进程存活——早死立即 [`ReadyError::ChildDied`] 带退出码现场，绝不
//! 傻等到 deadline。子进程类型不感知（std / tokio Child 都行）：调用方用
//! aliveness 闭包自查自家句柄，返回 `Some(现场)` = 已死。
//!
//! 接线点（2026-10-19）：`nemesisbot dashboard` start_and_wait（gateway 启
//! 动等待）、remote_executor_tool 管道传输的子进程连接等待。delegate 握手
//! 等待已有 per-tick 存活耦合（rpc_round），仅补退出码注记，不套本 helper。
//!
//! 探针纪律：poll 闭包必须**幂等可重入**（单次尝试可能被 poll_interval
//! 取消后重试）；aliveness 闭包必须廉价（每 tick 一次 try_wait 级别）。

use std::future::Future;
use std::time::Duration;

/// 等待失败三态：调用方负责把 `detail` / `what` 语境翻译成领域错误文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadyError {
    /// 子进程在就绪前退出——`detail` 携带退出码/信号现场（调用方闭包产出）。
    ChildDied { detail: String },
    /// 子进程全程存活，但 deadline 内未就绪。
    Timeout,
}

impl std::fmt::Display for ReadyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadyError::ChildDied { detail } => {
                write!(f, "child exited before ready ({detail})")
            }
            ReadyError::Timeout => write!(f, "not ready within deadline"),
        }
    }
}

/// 等待就绪，每 tick 耦合子进程存活断言。
///
/// - `poll`：就绪探针，`Some(ready)` 即成功返回；单次尝试至多等
///   `poll_interval`，超窗被取消、下一 tick 重试（因此必须幂等可重入）。
/// - `aliveness`：存活断言，`Some(why)` = 子进程已死，立即
///   [`ReadyError::ChildDied`]（`why` 是退出码/信号现场字符串）。
///
/// 时序基线用 `tokio::time::Instant`（测试 `start_paused` 虚拟时钟下可确定性跑）。
pub async fn wait_for_ready<T, P, PF, A, AF>(
    poll_interval: Duration,
    deadline: Duration,
    poll: &mut P,
    aliveness: &mut A,
) -> Result<T, ReadyError>
where
    P: FnMut() -> PF,
    PF: Future<Output = Option<T>>,
    A: FnMut() -> AF,
    AF: Future<Output = Option<String>>,
{
    let start = tokio::time::Instant::now();
    loop {
        // ① 就绪探针：限窗单次尝试（超窗取消——探针必须可安全重入）。
        match tokio::time::timeout(poll_interval, poll()).await {
            Ok(Some(ready)) => return Ok(ready),
            Ok(None) => {}
            Err(_elapsed) => {}
        }
        // ② A4 纪律核心：就绪未达成 → 断言子进程存活，早死立即报死因。
        if let Some(why) = aliveness().await {
            return Err(ReadyError::ChildDied { detail: why });
        }
        // ③ 到点未就绪（子进程全程存活）= 诚实 Timeout。
        if start.elapsed() >= deadline {
            return Err(ReadyError::Timeout);
        }
        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests;
