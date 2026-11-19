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
//!   `let _home = crate::test_home:lock_home();`。

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};

pub(crate) static HOME_RACE_LOCK: std::sync::LazyLock<ReentrantMutex<()>> =
    std::sync::LazyLock::new(|| ReentrantMutex::new(()));

/// 进程级测试 home 目录（一次性重定向目标，整个测试二进制共享）。
static TEST_HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

pub(crate) fn lock_home() -> ReentrantMutexGuard<'static, ()> {
    let guard = HOME_RACE_LOCK.lock();
    ensure_redirected();
    guard
}

/// 一次性把单例 home 重定向到进程级临时目录（幂等；2026-11-19 测试泄漏根修）。
///
/// 此前持锁写入 chat_log / meta 侧车的测试只串行不重定向，写盘仍落真实
/// `~/.nemesisbot`（每轮 cargo test 泄漏几十个夹具会话/孤儿 meta）。重定向
/// 后整进程写入落临时目录。进程退出后目录残留在系统 temp（libtest 无退出
/// 钩子）：同 pid 复用时入口处清掉重开自愈，跨 pid 残留由 OS temp 清理兜底。
///
/// 一次性（置位后快路径直返）：同线程嵌套取锁不会打扰测试自管的
/// set_home_dir 重定向；持锁互斥保证首调初始化天然无竞速。已按纪律
/// 「先取锁再重定向」的测试 capture 到的 old home = 本目录，还原闭环。
fn ensure_redirected() {
    if TEST_HOME.get().is_some() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("nb-web-test-home-{}", std::process::id()));
    // 同 pid 残留（进程 id 复用）清掉重开，保证进程内起点干净。
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    nemesis_path::default_path_manager().set_home_dir(dir.clone());
    let _ = TEST_HOME.set(dir);
}

// 进程启动即重定向（CRT 静态初始化器）：`lock_home()` 的惰性触发有窗口——
// 不取锁却直调单例的测试若先于所有取锁测试运行，看到的是真实 home。启动期
// 重定向把窗口收敛到零（agent 侧 test_home 同款机制，2026-11-19）。
extern "C" fn test_home_bootstrap() {
    ensure_redirected();
}

#[cfg(windows)]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static TEST_HOME_BOOTSTRAP_WIN: extern "C" fn() = test_home_bootstrap;

#[cfg(not(windows))]
#[used]
#[unsafe(link_section = ".init_array")]
static TEST_HOME_BOOTSTRAP_UNIX: extern "C" fn() = test_home_bootstrap;
