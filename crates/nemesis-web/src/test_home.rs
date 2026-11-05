//! Home 单例测试竞态锁（env-test-race-lock-pattern 的全 crate 统一版）。
//!
//! `nemesis_path::default_path_manager()` 是进程级单例：重定向 home 的
//! 测试与经单例读写 chat_log / meta 侧车的测试分属不同模块、各持模块内
//! 锁时，跨模块窗口互相踩（2026-09-23 实锤：session_bindings 的 home
//! 重定向让 share / s10b export 的 append-read 对落进不同 home → 偶发
//! count 0，单线程全绿、并行必炸）。全 crate 共享这一把**可重入**锁：
//! 同线程 helper 叠加取锁不死锁，异线程完全串行。
//!
//! 使用纪律：
//! - 重定向 home 的测试：锁内重定向，guard 携带锁（还原先于放锁）；
//! - 只经单例读写 chat_log / meta 的测试：测试体首行
//!   `let _home = crate::test_home::lock_home();`。

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};

pub(crate) static HOME_RACE_LOCK: std::sync::LazyLock<ReentrantMutex<()>> =
    std::sync::LazyLock::new(|| ReentrantMutex::new(()));

pub(crate) fn lock_home() -> ReentrantMutexGuard<'static, ()> {
    HOME_RACE_LOCK.lock()
}
