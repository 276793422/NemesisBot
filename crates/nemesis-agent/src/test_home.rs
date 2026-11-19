//! 单元测试进程级 home 重定向（cfg(test) 专用）。
//!
//! 根因（2026-11-19 测试泄漏调查）：`chat_log` 的寻址经
//! `nemesis_path::default_path_manager()` 进程单例——各测试模块用 tempfile
//! 隔离 SessionStore，但 chat_log / meta 侧车写入仍按单例落真实 home，
//! 每跑一轮 `cargo test` 就泄漏几十个夹具会话进
//! `~/.nemesisbot/workspace/logs/session_logs/`（实测存量 1300+ jsonl，
//! 其中真实会话仅 2 个）。
//!
//! 机制：`chat_log::singleton()` 在测试构建下首访时调 [`ensure_redirected`]，
//! 把单例 home 一次性重定向到 `<tmp>/nb-agent-test-home-<pid>`——整个测试
//! 进程内所有经单例的读写都落同一临时目录：跨测试读写自洽（无
//! redirect/read 竞速窗口），无需还原。进程退出后目录残留在系统 temp
//! （libtest 无退出钩子）：同 pid 复用时入口处清掉重开自愈，跨 pid 残留
//! 由 OS temp 清理兜底。
//!
//! 纪律：
//! - ensure 一次性（`TEST_HOME` 置位后不再改写 home）：需要临时自管 home
//!   的测试（`set_home_dir` + RAII 还原）在首个 ensure 之后 capture old
//!   （= 重定向目录）即安全闭环，不要 capture 真实 home——否则还原后
//!   后续写入又落真实盘。
//! - 独立集成测试二进制（`tests/`）是独立进程，沿用 `NEMESISBOT_HOME`
//!   env 先于单例首解析重定向（既有手法），不经本模块。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static TEST_HOME: OnceLock<PathBuf> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// 首个单例访问者把 home 重定向到进程级临时目录（幂等）。
///
/// 锁内 init、`TEST_HOME` **后置**：并发首调线程要么在锁内完成全部
/// 重定向，要么在快路径看到已置位的 `TEST_HOME`（此时 `set_home_dir`
/// 必已完成）——不存在「已见标志但 home 尚未改写」的竞速窗口。
pub(crate) fn ensure_redirected() {
    if TEST_HOME.get().is_some() {
        return;
    }
    let _g = INIT_LOCK.lock().ok();
    if TEST_HOME.get().is_some() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("nb-agent-test-home-{}", std::process::id()));
    // 同 pid 残留（进程 id 复用）清掉重开，保证进程内起点干净。
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        // 极端环境（tmp 不可写）：放弃重定向，保留真实 home——测试自证。
        eprintln!(
            "test_home: create_dir_all({}) 失败（{}）；保留真实 home",
            dir.display(),
            e
        );
        return;
    }
    nemesis_path::default_path_manager().set_home_dir(dir.clone());
    let _ = TEST_HOME.set(dir);
}

// ---------------------------------------------------------------------------
// 进程启动即重定向（CRT 静态初始化器，ctor 宏的 stable 手写形态）。
//
// 为什么不能只靠惰性首访：惰性触发点在 `chat_log::singleton()`，但**测试
// 代码自身**直调 `default_path_manager()` 的点（timing_log_path、replay
// 账本 helper、characterization 路径等）若先于任何 chat_log 写入执行，
// 拿到的是真实 home，之后并行线程的 ensure 又切到 temp——同进程内捕获
// 与写入跨单例分裂（2026-11-19 实锤：s9 夹具因并行 ensure 翻转写进真实
// home 且 cleanup 被失败跳过）。
//
// **可靠性边界（诚实声明）**：dev profile 增量链接**可能丢弃 `.CRT$XCU`
// 静态项**（2026-11-19 实测至少一个 freshly-compiled 测试二进制未触发
// bootstrap）——本机制是尽力而为的提前量，**不是正确性依据**。正确性由
// 两层保证：① `chat_log::singleton()` 惰性 ensure（生产写入路径全覆盖）；
// ② 测试侧直调单例的写/删/捕获点显式调 ensure（timing_log_path、
// characterization 路径 helper、replay unique_key/cleanup_sidecars/
// temp_store、replay s9 目录占位测试、migrate 测试已改 DI 私有目录）。
//
// 平台段说明：Windows MSVC 用 `.CRT$XCU`，Unix 用 `.init_array`——两者
// 都是 C 运行时初始化器表，链接器按序拼接，main 之前执行。ensure 体内
// 只用 OS 级设施（GetEnvironmentVariableW / CreateDirectory），不依赖
// std runtime 已就绪的部分（stdio 等失败分支才用，且尽力而为）。
// ---------------------------------------------------------------------------
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
