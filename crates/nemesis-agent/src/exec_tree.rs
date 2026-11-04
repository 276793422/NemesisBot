//! 修6（BUG-C 执行体面，2026-11-04）：同步 exec 族子进程树的生命周期守卫。
//!
//! 同步 exec 族（`exec` / `run_script` / `run_checks` 的 `run_one_stage`
//! 注入内核）spawn 的 shell 子进程会派生真实工作进程（cargo/rustc/编译器）。
//! 此前收杀面只有超时臂的 `tree_kill`（taskkill /T）——estop 与中途 drop
//! 完全不触达，卡死会话里的进程树继续烧 CPU（BUG-A/BUG-C 现场）。
//!
//! 本模块给每次 spawn 挂一个守卫：
//!
//! Windows：进程挂进 **Job Object**（`TerminateJobObject` 一次内核调用收
//! 整棵后代树，无 PID 重用窗口；Win8+ 嵌套 job 使 CI/受管环境下挂载照常
//! 成立）。Unix：`process_group(0)` 已使子进程自成一进程组（pgid == pid），
//! 收杀走 `kill(-pgid, SIGKILL)`。
//!
//! 收杀触发面（全部幂等、薄内核调用）：超时臂显式 terminate；守卫 Drop
//! （RAII——未来被取消即收树，Batch 2 estop select 臂依赖此语义）；
//! `kill_session_trees` 会话级 sweep（estop 联动入口，Batch 2 接线）。
//!
//! 纪律：只收同步 exec 族；B4 `background_start` 有自己的注册表/stop
//! ladder，不进本注册表。正常完成路径 `mark_complete` 后 Drop 变 no-op
//! ——正常路径零行为变化。终止原因单调记录（Estop > Timeout > Drop），
//! 供 run_checks / exec 的「被 estop 终止」诚实标注消费（不记失败/超时）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Weak};

#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicIsize;
#[cfg(not(target_os = "windows"))]
use std::sync::atomic::AtomicU32;

/// 终止原因（单调记录：后写只在优先级更高时生效——estop sweep 之后再来
/// 的 Drop/Timeout 不得降级标注）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminationReason {
    /// 守卫 Drop 兜底（未来被取消）。
    Drop,
    /// 工具超时臂。
    Timeout,
    /// estop 会话级 sweep。
    Estop,
}

const R_NONE: u8 = 0;
const R_DROP: u8 = 1;
const R_TIMEOUT: u8 = 2;
const R_ESTOP: u8 = 3;

pub(crate) struct ExecTreeInner {
    // Windows job handle（isize 载体，0 = 未附着——挂载失败诚实降级，
    // 既有超时 tree_kill 路径照常兜底）。
    #[cfg(target_os = "windows")]
    job: AtomicIsize,
    // Unix 进程组 id（== 子 pid；process_group(0) 保证）；0 = 未附着。
    #[cfg(not(target_os = "windows"))]
    pgid: AtomicU32,
    completed: AtomicBool,
    reason: AtomicU8,
}

impl Drop for ExecTreeInner {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        close_job(self.job.load(Ordering::Relaxed));
    }
}

#[cfg(target_os = "windows")]
fn attach_job(raw: std::os::windows::io::RawHandle) -> isize {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
    // 默认 job：不带 KILL_ON_JOB_CLOSE——句柄关闭（正常完成路径的守卫
    // Drop）绝不杀进程；收杀只走显式 TerminateJobObject。
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return 0;
        }
        if AssignProcessToJobObject(job, raw) == 0 {
            // 挂载失败（子进程瞬时退出等）诚实降级为无守卫，close 干净。
            CloseHandle(job);
            return 0;
        }
        job as isize
    }
}

#[cfg(target_os = "windows")]
fn kill_job(job: isize) -> bool {
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    if job == 0 {
        return false;
    }
    // 一次内核调用收整棵后代树（含已 breakaway 失败的全部后代）；空 job
    // / 已退出进程上调用是 no-op，幂等。
    unsafe { TerminateJobObject(job as *mut core::ffi::c_void, 1) != 0 }
}

#[cfg(target_os = "windows")]
fn close_job(job: isize) {
    use windows_sys::Win32::Foundation::CloseHandle;
    if job != 0 {
        unsafe { CloseHandle(job as *mut core::ffi::c_void) };
    }
}

fn kill_tree_now(inner: &ExecTreeInner) -> bool {
    #[cfg(target_os = "windows")]
    {
        kill_job(inner.job.load(Ordering::Relaxed))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let pgid = inner.pgid.load(Ordering::Relaxed);
        if pgid == 0 {
            return false;
        }
        // SIGKILL 进程组（pgid == pid）；ESRCH = 组已死，幂等。薄系统调用，
        // 同步上下文（Drop）安全。
        unsafe { libc::kill(-(pgid as i32), libc::SIGKILL) };
        true
    }
}

fn record_reason(inner: &ExecTreeInner, r: TerminationReason) {
    let v = match r {
        TerminationReason::Drop => R_DROP,
        TerminationReason::Timeout => R_TIMEOUT,
        TerminationReason::Estop => R_ESTOP,
    };
    let mut cur = inner.reason.load(Ordering::SeqCst);
    while v > cur {
        match inner
            .reason
            .compare_exchange_weak(cur, v, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

fn terminate_inner(inner: &ExecTreeInner, reason: TerminationReason) -> bool {
    record_reason(inner, reason);
    kill_tree_now(inner)
}

fn attach_inner(child: &tokio::process::Child) -> ExecTreeInner {
    #[cfg(target_os = "windows")]
    {
        let job = match child.raw_handle() {
            Some(raw) => attach_job(raw),
            None => 0,
        };
        ExecTreeInner {
            job: AtomicIsize::new(job),
            completed: AtomicBool::new(false),
            reason: AtomicU8::new(R_NONE),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let pgid = child.id().unwrap_or(0);
        ExecTreeInner {
            pgid: AtomicU32::new(pgid),
            completed: AtomicBool::new(false),
            reason: AtomicU8::new(R_NONE),
        }
    }
}

/// 同步 exec 族 spawn 守卫：spawn + 挂 job/进程组 + 会话注册一次完成。
/// 单一持有者（不 Clone）——Drop 即 RAII 收杀兜底。
pub(crate) struct ExecTreeGuard {
    inner: Arc<ExecTreeInner>,
}

impl ExecTreeGuard {
    /// spawn 子进程并附挂守卫；`session_key` 非空时注册进会话注册表
    /// （estop sweep 可达）。spawn 失败原样回传 io 错误（守卫无残留）。
    pub(crate) fn attach_and_register(
        cmd: &mut tokio::process::Command,
        session_key: Option<&str>,
    ) -> std::io::Result<(ExecTreeGuard, tokio::process::Child)> {
        let child = cmd.spawn()?;
        let inner = Arc::new(attach_inner(&child));
        if let Some(key) = session_key.filter(|s| !s.is_empty()) {
            register_tree(key, &inner);
        }
        Ok((ExecTreeGuard { inner }, child))
    }

    /// 正常完成路径：置 completed，Drop 变 no-op（正常路径零行为变化）。
    pub(crate) fn mark_complete(&self) {
        self.inner.completed.store(true, Ordering::SeqCst);
    }

    /// 显式收杀（超时臂 / estop sweep）。已完成进程上 kill 是无害 no-op。
    pub(crate) fn terminate(&self, reason: TerminationReason) {
        terminate_inner(&self.inner, reason);
    }

    /// 当前生效的终止原因（None = 未被收杀）。「被 estop 终止」标注判定源。
    pub(crate) fn reason(&self) -> Option<TerminationReason> {
        match self.inner.reason.load(Ordering::SeqCst) {
            R_ESTOP => Some(TerminationReason::Estop),
            R_TIMEOUT => Some(TerminationReason::Timeout),
            R_DROP => Some(TerminationReason::Drop),
            _ => None,
        }
    }
}

impl Drop for ExecTreeGuard {
    fn drop(&mut self) {
        if !self.inner.completed.load(Ordering::SeqCst) {
            // RAII 兜底：未来被取消（Batch 2 estop select 臂 / 任何中途
            // drop / 工具 panic 隔离 unwind）时整棵树被收。
            terminate_inner(&self.inner, TerminationReason::Drop);
        }
    }
}

// 会话级注册表：session_key -> 在飞守卫（Weak——守卫中途 drop 自动失效，
// 注册机会惰性清理）。estop sweep 的目标面。
static SESSION_TREES: std::sync::LazyLock<
    parking_lot::Mutex<HashMap<String, Vec<Weak<ExecTreeInner>>>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

fn register_tree(session_key: &str, inner: &Arc<ExecTreeInner>) {
    let weak = Arc::downgrade(inner);
    let mut map = SESSION_TREES.lock();
    let entry = map.entry(session_key.to_string()).or_default();
    entry.push(weak);
    // 惰性剪掉已死 weak（mid-flight drop 的守卫），防长会话累积。
    entry.retain(|w| w.strong_count() > 0);
}

/// estop 联动入口（Batch 2 接线）：终止该会话全部在飞 exec 树。纪律
/// （PLAN 修6 钦定）：锁内只快照、杀在锁外——terminate 虽是薄内核调用，
/// 杀在锁外杜绝任何自阻塞面。返回发出收杀的守卫数。
pub(crate) fn kill_session_trees(session_key: &str) -> usize {
    let snapshot: Vec<Arc<ExecTreeInner>> = {
        let mut map = SESSION_TREES.lock();
        match map.remove(session_key) {
            Some(list) => list.into_iter().filter_map(|w| w.upgrade()).collect(),
            None => return 0,
        }
    };
    snapshot
        .iter()
        .filter(|inner| terminate_inner(inner, TerminationReason::Estop))
        .count()
}

/// estop 联动入口（Batch 2 ③ 执行面 sweep）：终止**全部会话**在飞 exec 树。
/// gateway 侧 estop watcher 挂在全局 estop 状态上（不分 session）——急停
/// 冻结的是整个进程的全部 agent 活动，任何会话的在飞子进程树都应被收，
/// 不能只靠逐会话 kill_session_trees（watcher 拿不到「哪些会话在飞」的
/// 权威清单，注册表本身就是权威）。纪律同上：锁内只取键集，杀在锁外。
/// 返回发出收杀的守卫数。
pub fn kill_all_trees() -> usize {
    let keys: Vec<String> = {
        let map = SESSION_TREES.lock();
        map.keys().cloned().collect()
    };
    keys.iter().map(|k| kill_session_trees(k)).sum()
}

#[cfg(test)]
mod tests;
