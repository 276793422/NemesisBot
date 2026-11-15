//! 终态令牌等待点 helper 回归锁（BUG-20261115-023）。
//!
//! 测试纪律：**不 spawn 生产循环本体**——run_sweep_once 解析真实 workspace
//! 路径会误扫真实目录；profile sweeper 有 OnceLock spawn-once 不可重入；
//! NOTIFY_DB 是进程级槽会跨测试串扰。只测 helper 机制本身（语义契约：
//! 前置短路 / select 双臂 / 取消优先 / 全局令牌单实例），调用点统一一行
//! `if !terminal_loop_tick(token, wait).await { break; }` 形态由五调用点
//! 目检 + 本 helper 契约覆盖。
//!
//! **全局令牌在测试里绝不可 cancel**（首轮全量套件实录）：CancellationToken
//! 无复位，libtest 并行调度下 cancel 全局令牌会杀死同进程内任何正在
//! terminal_loop_tick 里 park 的生产循环测试（pa2 board watcher 断言假红）。
//! 全局面只测指针同一性（单实例），cancel 幂等性用局部令牌证明。

use std::future::pending;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::shutdown::{terminal_cancel_token, terminal_loop_tick};

/// 预取消 + 永不 ready 的 wait：前置检查短路，立即 false 不 park。
#[tokio::test]
async fn test_terminal_tick_pre_cancelled_short_circuits() {
    let token = CancellationToken::new();
    token.cancel();
    let started = std::time::Instant::now();
    assert!(!terminal_loop_tick(&token, pending::<()>()).await);
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// 活令牌 + 永不 ready 的 wait，等待期间另一任务 cancel：select cancelled
/// 臂胜出，返回 false（调用方据此 break，不再 re-park timer）。
#[tokio::test]
async fn test_terminal_tick_cancel_during_wait_returns_false() {
    let token = CancellationToken::new();
    let canceler = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        canceler.cancel();
    });
    assert!(!terminal_loop_tick(&token, pending::<()>()).await);
}

/// 活令牌 + 已 ready 的 wait：正常一拍，返回 true（调用方继续本拍）。
#[tokio::test]
async fn test_terminal_tick_ready_wait_returns_true() {
    let token = CancellationToken::new();
    assert!(terminal_loop_tick(&token, std::future::ready(())).await);
    // 令牌未消费——后续拍仍正常（终态前循环活性不受影响）。
    assert!(terminal_loop_tick(&token, std::future::ready(())).await);
}

/// 取消优先于已 ready 的 wait：收口语义优先于最后一拍工作量。
#[tokio::test]
async fn test_terminal_tick_cancel_preempts_ready_wait() {
    let token = CancellationToken::new();
    token.cancel();
    assert!(!terminal_loop_tick(&token, std::future::ready(())).await);
}

/// 全局令牌：单实例契约——`terminal_cancel_token()` 重复取回同一指针
/// （27 个调用点 + 收口点消费的是同一个 OnceLock 实例，cancel 才能一次
/// 触达全部循环）。cancel 幂等性（双重调用无害）用**局部令牌**证明——
/// 全局令牌在测试里不可 cancel（见模块文档测试纪律）。
#[tokio::test]
async fn test_global_terminal_token_single_instance() {
    let a = terminal_cancel_token();
    let b = terminal_cancel_token();
    assert!(
        std::ptr::eq(a, b),
        "terminal_cancel_token 必须返回同一 OnceLock 实例"
    );
    assert!(!a.is_cancelled(), "测试进程内终态令牌应保持为活");
    // cancel 幂等性（局部令牌）：双重 cancel 不 panic、状态一致为已取消。
    let local = CancellationToken::new();
    local.cancel();
    local.cancel();
    assert!(local.is_cancelled());
    assert!(!terminal_loop_tick(&local, pending::<()>()).await);
}
