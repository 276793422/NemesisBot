//! T46 网络分区冻结变体的进程挂起/恢复原语（Windows）。
//!
//! 首选 `NtSuspendProcess` / `NtResumeProcess`（ntdll 导出，GetProcAddress
//! 动态取址——不引 ntdll.lib 链接依赖）：挂起 = 全部线程冻结 —— 网络栈活着
//! 但进程不收不发、不回探针，从对端看 = 节点整体失联（等效双向分区）。
//!
//! ⚠ Windows 11 build 26200 实测收紧：非提权 token（无 SeDebugPrivilege）
//! 调 `NtSuspendProcess` 恒 `0xC0000022`（ACCESS_DENIED），而按线程
//! `SuspendThread`（THREAD_SUSPEND_RESUME）不受限（2026-10-01 探针实证，
//! test-campaign-data/probe-suspend.rs）。故 Nt 拒绝时回退 **按线程枚举
//! 挂起**（toolhelp 快照 + 逐线程 SuspendThread）。回退路径的保真度边界：
//! 冻结窗口内新生的线程不在快照里、保持运行——gateway 的网络面（tokio
//! runtime worker 固定线程集）在启动时全部就位，快照即完整覆盖；按需
//! 新生的 blocking-pool 线程不碰监听 socket，不影响「不回探针」的分区
//! 语义。恢复只解冻**自己挂起的线程**（快照内 TID 列表），已退出线程的
//! ResumeThread 失败无害。
//!
//! worker 是本会话拉起的子进程，无需管理员（防火墙注入才需要——T44 形态
//! A 的 SKIP 原因，本模块不受限）。
//!
//! ⚠ 恢复责任在调用方：冻结的 gateway 不解冻，套件后续所有依赖 B 的
//! 场景级联假红。`FrozenProcess::resume()` 必须在所有路径上调用（含断言
//! 失败提前 return 的路径——用内层 async block + 外层 resume 的结构，
//! 见 T46）。

#[cfg(windows)]
mod imp {
    // kernel32 / ntdll（默认链接 + 动态取址，elevation.rs 同款 extern 形态；
    // 句柄按 isize）。
    unsafe extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn OpenThread(desired_access: u32, inherit_handle: i32, thread_id: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn SuspendThread(handle: isize) -> u32;
        fn ResumeThread(handle: isize) -> u32;
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
        fn Thread32First(snapshot: isize, entry: *mut ThreadEntry32) -> i32;
        fn Thread32Next(snapshot: isize, entry: *mut ThreadEntry32) -> i32;
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn GetProcAddress(
            module: isize,
            name: *const u8,
        ) -> Option<unsafe extern "system" fn() -> isize>;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
        fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut u64) -> i32;
        fn AdjustTokenPrivileges(
            token: isize,
            disable_all: i32,
            new_state: *const TokenPrivileges1,
            buf_len: u32,
            prev: isize,
            ret_len: *mut u32,
        ) -> i32;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LuidAndAttributes {
        luid: u64,
        attributes: u32,
    }

    #[repr(C)]
    struct TokenPrivileges1 {
        privilege_count: u32,
        privs: [LuidAndAttributes; 1],
    }

    #[repr(C)]
    #[derive(Default)]
    struct ThreadEntry32 {
        dw_size: u32,
        cnt_usage: u32,
        thread_id: u32,
        owner_process_id: u32,
        tp_base_pri: i32,
        tp_delta_pri: i32,
        dw_flags: u32,
    }

    const PROCESS_SUSPEND_RESUME: u32 = 0x0008_0000;
    const THREAD_SUSPEND_RESUME: u32 = 0x0002;
    const TH32CS_SNAPTHREAD: u32 = 0x4;
    const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
    const TOKEN_QUERY: u32 = 0x0008;
    const SE_PRIVILEGE_ENABLED: u32 = 0x2;
    const STATUS_SUCCESS: i32 = 0;
    const STATUS_ACCESS_DENIED: i32 = 0xC000_0022u32 as i32;
    const INVALID_HANDLE_VALUE: isize = -1isize;

    type NtProcessFn = unsafe extern "system" fn(process_handle: isize) -> i32;

    /// 取 ntdll 导出的 NtSuspendProcess/NtResumeProcess 函数指针。
    unsafe fn nt_fn(name: &str) -> Result<NtProcessFn, String> {
        unsafe {
            let module_name: Vec<u16> = "ntdll.dll"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let ntdll = GetModuleHandleW(module_name.as_ptr());
            if ntdll == 0 {
                return Err("GetModuleHandleW(ntdll.dll) 失败".into());
            }
            let mut bytes = name.as_bytes().to_vec();
            bytes.push(0);
            GetProcAddress(ntdll, bytes.as_ptr())
                .ok_or_else(|| format!("GetProcAddress({name}) 失败（ntdll 导出缺失）"))
                .map(|f| {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, NtProcessFn>(f)
                })
        }
    }

    /// 尽力启用 SeDebugPrivilege（新 Windows 收紧 NtSuspendProcess 后的
    /// 放宽尝试；token 不持有该特权时静默失败——回退路径会接管）。
    fn try_enable_se_debug() {
        unsafe {
            let mut luid: u64 = 0;
            let name: Vec<u16> = "SeDebugPrivilege\0".encode_utf16().collect();
            if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
                return;
            }
            let mut token: isize = 0;
            if OpenProcessToken(
                // GetCurrentProcess() = -1（伪句柄），直接用。
                -1isize,
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            ) == 0
            {
                return;
            }
            let tp = TokenPrivileges1 {
                privilege_count: 1,
                privs: [LuidAndAttributes {
                    luid,
                    attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let _ = AdjustTokenPrivileges(token, 0, &tp, 0, 0, std::ptr::null_mut());
            CloseHandle(token);
        }
    }

    /// 枚举目标进程的线程 TID 列表（toolhelp 快照）。
    fn enumerate_threads(pid: u32) -> Result<Vec<u32>, String> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE || snap == 0 {
                return Err(format!(
                    "CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD) 失败（pid={pid}）"
                ));
            }
            let mut entry = ThreadEntry32 {
                dw_size: std::mem::size_of::<ThreadEntry32>() as u32,
                ..Default::default()
            };
            let mut tids = Vec::new();
            if Thread32First(snap, &mut entry) != 0 {
                loop {
                    if entry.owner_process_id == pid {
                        tids.push(entry.thread_id);
                    }
                    if Thread32Next(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
            if tids.is_empty() {
                return Err(format!("快照内无线程（pid={pid} 已退出？）"));
            }
            Ok(tids)
        }
    }

    /// 冻结的进程：记录挂起方式与（回退路径的）已挂线程 TID，供精确恢复。
    pub struct FrozenProcess {
        pid: u32,
        by_nt: bool,
        suspended_tids: Vec<u32>,
        resumed: std::sync::atomic::AtomicBool,
    }

    impl FrozenProcess {
        /// 解冻（幂等安全：Nt 路径 ResumeProcess；回退路径只解自己挂的线程，
        /// 已退出线程的 ResumeThread 失败无害）。Drop 兜底保证残留冻结不会
        /// 级联炸掉套件（显式 resume 后 Drop 为 no-op）。
        pub fn resume(self) -> Result<(), String> {
            let mut me = self;
            let r = me.resume_inner();
            me.resumed.store(true, std::sync::atomic::Ordering::SeqCst);
            r
        }

        fn resume_inner(&mut self) -> Result<(), String> {
            unsafe {
                if self.by_nt {
                    let f = nt_fn("NtResumeProcess")?;
                    let handle = OpenProcess(PROCESS_SUSPEND_RESUME, 0, self.pid);
                    if handle == 0 {
                        return Err(format!(
                            "OpenProcess(pid={}, PROCESS_SUSPEND_RESUME) 失败（NtResume）",
                            self.pid
                        ));
                    }
                    let status = f(handle);
                    let _ = CloseHandle(handle);
                    if status != STATUS_SUCCESS {
                        return Err(format!(
                            "NtResumeProcess(pid={}) NTSTATUS={status:#x}",
                            self.pid
                        ));
                    }
                    return Ok(());
                }
                let mut last_err = String::new();
                for tid in self.suspended_tids.drain(..) {
                    let th = OpenThread(THREAD_SUSPEND_RESUME, 0, tid);
                    if th == 0 {
                        // 线程已退出：TID 失效，无需解冻。
                        continue;
                    }
                    if ResumeThread(th) == u32::MAX {
                        last_err = format!("ResumeThread(tid={tid}) 失败（gle 见 stdout）");
                    }
                    let _ = CloseHandle(th);
                }
                if last_err.is_empty() {
                    Ok(())
                } else {
                    Err(last_err)
                }
            }
        }
    }

    /// 挂起目标进程（全部线程）。首选 NtSuspendProcess（原子、无快照竞态）；
    /// ACCESS_DENIED（新 Windows 非提权收紧）回退按线程枚举挂起。
    pub fn suspend_process(pid: u32) -> Result<FrozenProcess, String> {
        // 先放宽（能启用则 Nt 路径直接可用；不能启用不影响回退）。
        try_enable_se_debug();
        unsafe {
            if let Ok(f) = nt_fn("NtSuspendProcess") {
                let handle = OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid);
                if handle != 0 {
                    let status = f(handle);
                    let _ = CloseHandle(handle);
                    if status == STATUS_SUCCESS {
                        return Ok(FrozenProcess {
                            pid,
                            by_nt: true,
                            suspended_tids: Vec::new(),
                            resumed: std::sync::atomic::AtomicBool::new(false),
                        });
                    }
                    if status != STATUS_ACCESS_DENIED {
                        return Err(format!("NtSuspendProcess(pid={pid}) NTSTATUS={status:#x}"));
                    }
                    // ACCESS_DENIED → 回退按线程（fallthrough）。
                }
            }
            // 回退：枚举 + 逐线程挂起。
            let tids = enumerate_threads(pid)?;
            let mut suspended = Vec::with_capacity(tids.len());
            let mut failed = 0usize;
            for tid in tids {
                let th = OpenThread(THREAD_SUSPEND_RESUME, 0, tid);
                if th == 0 {
                    // 快照与挂起间隙线程退出：跳过。
                    continue;
                }
                if SuspendThread(th) == u32::MAX {
                    failed += 1;
                } else {
                    suspended.push(tid);
                }
                let _ = CloseHandle(th);
            }
            if suspended.is_empty() {
                return Err(format!(
                    "按线程挂起全部失败（pid={pid}，failed={failed}）——进程已退出或线程访问被拒"
                ));
            }
            Ok(FrozenProcess {
                pid,
                by_nt: false,
                suspended_tids: suspended,
                resumed: std::sync::atomic::AtomicBool::new(false),
            })
        }
    }

    impl Drop for FrozenProcess {
        fn drop(&mut self) {
            if !self.resumed.load(std::sync::atomic::Ordering::SeqCst) {
                // 泄漏兜底：残留冻结会级联假红后续场景，尽力解冻并注记失败。
                if let Err(e) = self.resume_inner() {
                    eprintln!("[win_freeze] ⚠ Drop 兜底解冻失败（pid={}）: {e}", self.pid);
                }
            }
        }
    }
}

#[cfg(windows)]
pub use imp::suspend_process;

#[cfg(not(windows))]
pub struct FrozenProcess;

#[cfg(not(windows))]
pub fn suspend_process(_pid: u32) -> Result<FrozenProcess, String> {
    Err("进程挂起仅 Windows（本场景形态 A 只在 Windows 跑）".into())
}

#[cfg(not(windows))]
impl FrozenProcess {
    pub fn resume(self) -> Result<(), String> {
        Err("进程挂起仅 Windows".into())
    }
}
