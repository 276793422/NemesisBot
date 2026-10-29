//! ziwei_agent_dll —— 紫微系统 payload（【万物皆工具】；注入到三方进程内运行）。
//!
//! 职责（总控 §5/§7；T1.2）：
//! - 被 `nemesis-injector::ziwei_attach::attach_inject` 注入目标进程：
//!   远线程 LoadLibraryW(本 DLL) → GetProcAddress("ZiWeiInit") → ZiWeiInit(参数块)。
//! - 作为**具名管道 CLIENT** 连回 bot 的管道 SERVER
//!   （`\\.\pipe\ZiWei_<pid>_<rand>`，名字与令牌来自注入参数块）。
//! - 握手（首帧）+ 命令循环：ping / read_mem / unload。
//!
//! 纪律：
//! - **零依赖**（静态 CRT、无分配器扰动）；所有 WinAPI 手工声明。
//! - **DllMain 纪律**：ATTACH 只做 DisableThreadLibraryCalls + 记录 HMODULE；
//!   真正工作在 `ZiWeiInit` 导出（远线程上下文，无 loader lock）里 CreateThread。
//! - read_mem 用 ReadProcessMemory(self)（ntdll 内部 SEH 守护，坏地址诚实报错
//!   而非崩溃）；残余风险（释放竞态 TOCTOU）已在总控 §14 声明。
//! - unload 先过 `unload_prepare` 三关闸（在途 call / 泄漏线程 / 在场 hook /
//!   VEH 全部清场）才 FreeLibraryAndExitThread——任一关不过 = 诚实拒绝继续
//!   服务（宿主进程安全优先于卸载意愿，BUG-200）。
//!
//! 帧协议：`[u32 LE 长度][JSON UTF-8]`（长度=JSON 字节数，≤4MB）。
//! 值域约束：本协议所有字符串值不含 JSON 转义字符（令牌/base64/固定命令名），
//! 手工解析按此约定（与 bot 侧同源实现）。
//!
//! # Safety 边界
//! 本 crate 是注入载荷，`unsafe` 集中在 WinAPI 调用与静态参数快照；
//! 参数块指针有效性由注入方（bot）保证（写入目标内存后才调 ZiWeiInit）。

use std::ffi::c_void;

// ---------------------------------------------------------------------------
// 注入参数块（与 bot 侧 nemesis-injector/src/ziwei_attach.rs **逐字节同源**，
// 两边都是 repr(C) Rust 结构体——改动必须双写同步，字段/顺序/对齐都不能单边动）。
// ---------------------------------------------------------------------------
/// 参数块魔数（'ZWI1' 小端呈现 31 49 57 5A）。
pub const PARAM_MAGIC: u32 = 0x5A57_4931;
/// 管道协议版本。
pub const PROTO_VERSION: u32 = 1;
/// 单次 read_mem 上限（bot 侧同值双写）。
pub const MAX_READ_LEN: u32 = 1 << 20;
/// 单帧上限。
pub const MAX_FRAME: u32 = 4 << 20;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParamBlock {
    pub magic: u32,
    pub proto: u32,
    pub fn_load_library_w: u64,
    pub fn_get_proc_address: u64,
    pub dll_path: [u16; 260],
    pub init_name: [u8; 16],
    pub token: [u8; 64],
    pub pipe_name: [u16; 128],
    pub arch: [u8; 8],
}

static mut G_PARAM: Option<ParamBlock> = None;
static mut G_HMODULE: *mut c_void = std::ptr::null_mut();

// ---------------------------------------------------------------------------
// WinAPI 手工声明（零依赖）
// ---------------------------------------------------------------------------
const DLL_PROCESS_ATTACH: u32 = 1;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;
const ERROR_PIPE_BUSY: u32 = 231;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn DisableThreadLibraryCalls(hLibModule: *mut c_void) -> i32;
    fn CreateThread(
        lpThreadAttributes: *mut c_void,
        dwStackSize: usize,
        lpStartAddress: unsafe extern "system" fn(*mut c_void) -> u32,
        lpParameter: *mut c_void,
        dwCreationFlags: u32,
        lpThreadId: *mut u32,
    ) -> *mut c_void;
    fn Sleep(dwMilliseconds: u32);
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: *mut c_void,
    ) -> *mut c_void;
    fn ReadFile(
        hFile: *mut c_void,
        lpBuffer: *mut c_void,
        nNumberOfBytesToRead: u32,
        lpNumberOfBytesRead: *mut u32,
        lpOverlapped: *mut c_void,
    ) -> i32;
    fn WriteFile(
        hFile: *mut c_void,
        lpBuffer: *const c_void,
        nNumberOfBytesToWrite: u32,
        lpNumberOfBytesWritten: *mut u32,
        lpOverlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(hObject: *mut c_void) -> i32;
    fn WaitForSingleObject(hHandle: *mut c_void, dwMilliseconds: u32) -> u32;
    fn GetLastError() -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
    fn ReadProcessMemory(
        hProcess: *mut c_void,
        lpBaseAddress: *const c_void,
        lpBuffer: *mut c_void,
        nSize: usize,
        lpNumberOfBytesRead: *mut usize,
    ) -> i32;
    fn FreeLibraryAndExitThread(hLibModule: *mut c_void, dwExitCode: u32) -> !;
}

/// DllMain：只做线程库调用禁用 + 记录自身模块句柄（无任何重型工作）。
#[no_mangle]
extern "system" fn DllMain(hinst: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        unsafe {
            DisableThreadLibraryCalls(hinst);
            G_HMODULE = hinst;
        }
    }
    1
}

/// 注入入口（bot 远线程调用）。校验参数块 → 快照到静态 → 起工作线程 → 立即返回。
///
/// # Safety
///
/// `param` 必须指向合法 `ParamBlock`（进程内可读、magic/proto 匹配）；
/// 本函数只快照参数块并起线程，返回后调用方即可释放该内存。
///
/// 返回 0=成功；1=魔数错；2=协议版本错；3=起线程失败。
#[no_mangle]
pub unsafe extern "system" fn ZiWeiInit(param: *const ParamBlock) -> u32 {
    if param.is_null() {
        return 1;
    }
    let block = *param;
    if block.magic != PARAM_MAGIC {
        return 1;
    }
    if block.proto != PROTO_VERSION {
        return 2;
    }
    unsafe {
        G_PARAM = Some(block);
        let h = CreateThread(
            std::ptr::null_mut(),
            0,
            worker_main,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
        );
        if h.is_null() {
            return 3;
        }
        CloseHandle(h);
    }
    0
}

// ---------------------------------------------------------------------------
// 工作线程：连管道 → 握手 → 命令循环
// ---------------------------------------------------------------------------
unsafe extern "system" fn worker_main(_arg: *mut c_void) -> u32 {
    let param = unsafe {
        match G_PARAM {
            Some(p) => p,
            None => return 1,
        }
    };
    let pipe = wide_to_string(&param.pipe_name);
    let token = cstr_to_string(&param.token);
    let arch = cstr_to_string(&param.arch);

    // 连管道：重试 50×200ms（bot 侧 server 可能还在就位）
    let mut handle = std::ptr::null_mut();
    let mut connected = false;
    for _ in 0..50 {
        handle = CreateFileW(
            string_to_wide(&pipe).as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        );
        if handle != INVALID_HANDLE_VALUE {
            connected = true;
            break;
        }
        if GetLastError() != ERROR_PIPE_BUSY {
            Sleep(200);
        } else {
            // busy 也是等：bot 侧 server 每连接需补实例，重试即就位
            Sleep(200);
        }
    }
    if !connected {
        return 2;
    }

    // 握手首帧：proto/token/arch/pid（bot 校验 token + 客户端 PID == 目标 PID）
    let hs = format!(
        "{{\"proto\":{},\"token\":\"{}\",\"arch\":\"{}\",\"pid\":{}}}",
        PROTO_VERSION,
        token,
        arch,
        GetCurrentProcessId()
    );
    if !write_frame(handle, hs.as_bytes()) {
        CloseHandle(handle);
        return 3;
    }

    // 命令循环。unload 精确分派先于 handle_command：prepare 成功才应答 bye
    // 并自卸；失败诚实报错继续服务（先发 bye 再补发错误帧会打破管道的
    // 请求-响应配对——bot 侧 detach 以 "ok":true 应答判「已卸」）。
    while let Some(frame) = read_frame(handle) {
        let req = String::from_utf8_lossy(&frame).to_string();
        if json_get_str(&req, "cmd") == Some("unload") {
            match unsafe { unload_prepare() } {
                Ok(n) => {
                    let resp = format!("{{\"ok\":true,\"bye\":true,\"unhooked\":{n}}}");
                    write_frame(handle, resp.as_bytes());
                    // 应答已写回 → 自卸（不返回）
                    unsafe { FreeLibraryAndExitThread(G_HMODULE, 0) }
                }
                Err(e) => {
                    let resp = format!("{{\"ok\":false,\"error\":\"unload 拒绝：{e}\"}}");
                    if !write_frame(handle, resp.as_bytes()) {
                        break;
                    }
                    continue;
                }
            }
        }
        let resp = handle_command(&req);
        if !write_frame(handle, resp.as_bytes()) {
            break;
        }
    }
    CloseHandle(handle);
    0
}

/// 命令分派（v1：ping / read_mem / call_fn / hook_set / hook_unset /
/// poll_events；unload 在 worker 循环先于本函数分派（三关闸语义）；unknown
/// 诚实报错）。
fn handle_command(req: &str) -> String {
    match json_get_str(req, "cmd").unwrap_or("") {
        "ping" => {
            let arch = unsafe { G_PARAM.map(|p| cstr_to_string(&p.arch)).unwrap_or_default() };
            format!(
                "{{\"ok\":true,\"pong\":true,\"arch\":\"{arch}\",\"pid\":{},\"proto\":{PROTO_VERSION}}}",
                unsafe { GetCurrentProcessId() }
            )
        }
        "read_mem" => {
            let addr = json_get_u64(req, "addr").unwrap_or(0);
            let len = json_get_u64(req, "len").unwrap_or(0);
            if len == 0 || len > MAX_READ_LEN as u64 {
                return format!(
                    "{{\"ok\":false,\"error\":\"len 越界（1..={}）\"}}",
                    MAX_READ_LEN
                );
            }
            let mut buf = vec![0u8; len as usize];
            let mut got: usize = 0;
            let ok = unsafe {
                ReadProcessMemory(
                    GetCurrentProcess(),
                    addr as *const c_void,
                    buf.as_mut_ptr() as *mut c_void,
                    len as usize,
                    &mut got,
                )
            };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                return format!(
                    "{{\"ok\":false,\"error\":\"ReadProcessMemory 失败（addr={addr:#x} err={err}）\"}}"
                );
            }
            buf.truncate(got);
            format!(
                "{{\"ok\":true,\"addr\":{addr},\"read\":{},\"data\":\"{}\"}}",
                got,
                base64_encode(&buf)
            )
        }
        "call_fn" => {
            let addr = json_get_u64(req, "addr").unwrap_or(0);
            if addr == 0 {
                return "{\"ok\":false,\"error\":\"addr 缺失\"}".to_string();
            }
            let conv = match json_get_u64(req, "conv").unwrap_or(0) {
                0 => CONV_MSX64,
                1 => CONV_STDCALL,
                2 => CONV_CDECL,
                _ => {
                    return "{\"ok\":false,\"error\":\"conv 非法（0=msx64/1=stdcall/2=cdecl）\"}"
                        .to_string()
                }
            };
            // 防御性二次拦截：args 在场但解析失败（浮点/负数/垃圾）= 诚实拒绝
            if req.contains("\"args\":[") && json_args(req, "args").is_none() {
                return "{\"ok\":false,\"error\":\"args 解析失败（v1 只接受整数位置参数）\"}"
                    .to_string();
            }
            let (args, _) = json_args(req, "args").unwrap_or(([0; 4], 0));
            let timeout_ms = json_get_u64(req, "timeout_ms")
                .unwrap_or(10_000)
                .min(u32::MAX as u64) as u32;
            unsafe { call_fn(addr, conv, args, timeout_ms) }
        }
        "hook_set" => {
            let id = json_get_u64(req, "id").unwrap_or(0) as u32;
            let addr = json_get_u64(req, "addr").unwrap_or(0);
            if id == 0 || addr == 0 {
                return "{\"ok\":false,\"error\":\"id/addr 缺失\"}".to_string();
            }
            match unsafe { hook_set(id, addr) } {
                Ok(s) => s,
                Err(e) => format!("{{\"ok\":false,\"error\":\"{e}\"}}"),
            }
        }
        "hook_unset" => {
            let addr = json_get_u64(req, "addr").unwrap_or(0);
            if addr == 0 {
                return "{\"ok\":false,\"error\":\"addr 缺失\"}".to_string();
            }
            match unsafe { hook_unset(addr) } {
                Ok(s) => s,
                Err(e) => format!("{{\"ok\":false,\"error\":\"{e}\"}}"),
            }
        }
        "poll_events" => {
            let max = json_get_u64(req, "max").unwrap_or(512).min(4096) as u32;
            poll_events(max)
        }
        other => format!("{{\"ok\":false,\"error\":\"unknown command: {other}\"}}"),
    }
}

// ---------------------------------------------------------------------------
// 帧读写：[u32 LE 长度][JSON]
// ---------------------------------------------------------------------------
fn write_frame(handle: *mut c_void, payload: &[u8]) -> bool {
    if payload.len() > MAX_FRAME as usize {
        return false;
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    let mut off = 0usize;
    while off < frame.len() {
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                handle,
                frame[off..].as_ptr() as *const c_void,
                (frame.len() - off) as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 {
            return false;
        }
        off += written as usize;
    }
    true
}

fn read_exact(handle: *mut c_void, buf: &mut [u8]) -> bool {
    let mut off = 0usize;
    while off < buf.len() {
        let mut got: u32 = 0;
        let ok = unsafe {
            ReadFile(
                handle,
                buf[off..].as_mut_ptr() as *mut c_void,
                (buf.len() - off) as u32,
                &mut got,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || got == 0 {
            return false; // 管道断开
        }
        off += got as usize;
    }
    true
}

fn read_frame(handle: *mut c_void) -> Option<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    if !read_exact(handle, &mut len_buf) {
        return None;
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME as usize {
        return None;
    }
    let mut payload = vec![0u8; len];
    if len > 0 && !read_exact(handle, &mut payload) {
        return None;
    }
    Some(payload)
}

// ---------------------------------------------------------------------------
// 手工 JSON 取值（协议值域不含转义字符，见模块注释）
// ---------------------------------------------------------------------------
fn json_get_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":\"");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn json_get_u64(s: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\":");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let end = rest
        .find(|c: char| {
            !(c.is_ascii_digit()
                || c == 'x'
                || c == 'X'
                || ('a'..='f').contains(&c)
                || ('A'..='F').contains(&c))
        })
        .unwrap_or(rest.len());
    let raw = &rest[..end];
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        raw.parse::<u64>().ok()
    }
}

// ---------------------------------------------------------------------------
// 编码 / 字符串
// ---------------------------------------------------------------------------
fn base64_encode(data: &[u8]) -> String {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TBL[(n >> 18) as usize & 63] as char);
        out.push(TBL[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TBL[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TBL[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// UTF-16（NUL 结尾）→ String。
fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// ASCII（NUL 结尾）→ String。
fn cstr_to_string(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}

/// String → UTF-16（NUL 结尾，按容量截断）。
fn string_to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16()
        .take(259)
        .chain(std::iter::once(0))
        .collect()
}

// ===========================================================================
// M4 扩展（T4.1/T4.2）：call_fn + hook_capture + 事件环 + VEH 看门狗
//
// 设计要点（与 bot 侧 nemesis-ziwei M4 同源契约）：
// - **窃取窗口 = 入口 5 字节全 nop**（E9 rel32 补丁）：bot 侧负责在补丁前用
//   反汇编证据确认入口是 5×0x90；本侧 hook_set 二次实核，不符诚实拒绝。
//   非 nop 序章 v1 不支持——诚实边界，转人工。
// - **stub 中转**：入口补丁 → RX 竞技场 stub（携带 capture id + 恢复地址）→
//   DLL 内 common 汇编（保存参数寄存器 → 调 Rust 记录环 → 原样恢复 →
//   跳回 entry+5）。stub 不回收（卸钩后迟到的在途线程仍安全）。
// - **事件拉取制**：hook 只写进程内环形缓冲；bot 发 poll_events 主动拉——
//   管道命令循环保持严格请求-响应配对，不引入异步插帧。
// - **call_fn 单飞**：一次一个在途调用；调用在专用线程上执行（命令循环
//   不被卡死）；超时 = 看门狗诚实报 timeout（调用线程不强杀——Win32 无
//   安全手段，泄漏已声明，同函数再调用被 active 位拒绝）；硬件异常 =
//   VEH 改写上下文把线程拽到救援栈 + bail 存根 → ExitThread，返回 seh
//   退出码——宿主进程绝不被带崩（§4.6 纪律）。
// ===========================================================================

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

const MEM_RESERVE: u32 = 0x2000;
const MEM_COMMIT: u32 = 0x1000;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualProtect(
        lpAddress: *mut c_void,
        dwSize: usize,
        flNewProtect: u32,
        lpflOldProtect: *mut u32,
    ) -> i32;
    fn VirtualAlloc(
        lpAddress: *mut c_void,
        dwSize: usize,
        flAllocationType: u32,
        flProtect: u32,
    ) -> *mut c_void;
    fn FlushInstructionCache(
        hProcess: *mut c_void,
        lpBaseAddress: *const c_void,
        dwSize: usize,
    ) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn ExitThread(dwExitCode: u32) -> !;
    fn AddVectoredExceptionHandler(
        first: u32,
        handler: unsafe extern "system" fn(*mut c_void) -> i32,
    ) -> *mut c_void;
    fn RemoveVectoredExceptionHandler(Handler: *mut c_void) -> u32;
}

#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    fn ziwei_hook_common_x64();
}
#[cfg(target_arch = "x86")]
unsafe extern "C" {
    fn ziwei_hook_common_x86();
}

// ---------------------------------------------------------------------------
// hook 事件环（4096 槽；溢出诚实计数；读写单调下标；锁 = CAS 自旋）
// ---------------------------------------------------------------------------
const RING_CAP: u64 = 4096;

#[derive(Clone, Copy)]
struct Ev {
    id: u32,
    seq: u32,
    tid: u32,
    _pad: u32,
    regs: [u64; 4],
}

impl Ev {
    const ZERO: Ev = Ev {
        id: 0,
        seq: 0,
        tid: 0,
        _pad: 0,
        regs: [0; 4],
    };
}

struct RingState {
    lock: AtomicU32,
    read: AtomicU64,
    write: AtomicU64,
    dropped: AtomicU64,
    /// ring_lock 互斥下的可变缓冲（UnsafeCell：静态存储 + 显式锁纪律）
    buf: UnsafeCell<[Ev; RING_CAP as usize]>,
}

unsafe impl Sync for RingState {}

static RING: RingState = RingState {
    lock: AtomicU32::new(0),
    read: AtomicU64::new(0),
    write: AtomicU64::new(0),
    dropped: AtomicU64::new(0),
    buf: UnsafeCell::new([Ev::ZERO; RING_CAP as usize]),
};

fn ring_lock() {
    while RING
        .lock
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
}

/// hook 事件入环（common 汇编 → 本函数；目标线程上下文，临界区极短）。
fn ring_push(id: u32, regs: [u64; 4]) {
    ring_lock();
    let w = RING.write.load(Ordering::Relaxed);
    let r = RING.read.load(Ordering::Relaxed);
    if w - r >= RING_CAP {
        RING.dropped.fetch_add(1, Ordering::Relaxed);
    } else {
        unsafe {
            (*RING.buf.get())[(w % RING_CAP) as usize] = Ev {
                id,
                seq: (w & 0xFFFF_FFFF) as u32,
                tid: GetCurrentThreadId(),
                _pad: 0,
                regs,
            };
        }
        RING.write.store(w + 1, Ordering::Release);
    }
    RING.lock.store(0, Ordering::Release);
}

// ---------------------------------------------------------------------------
// common 汇编蹦床（x64 / x86 各一段；stub 用栈携带 id + 恢复地址 + 全状态）
// ---------------------------------------------------------------------------
#[cfg(target_arch = "x86_64")]
std::arch::global_asm!(
    ".balign 16",
    // .globl：跨 CGU 引用（如测试）需全局符号；不影响 DLL 导出表
    ".globl ziwei_hook_common_x64",
    "ziwei_hook_common_x64:",
    // 入场（stub 契约：6×push 自 rsp0 到 C，rsp0 = C+0x30）：
    //   [C+0x00]=id  [C+0x08]=resume  [C+0x10]=orig r11  [C+0x18]=orig r10
    //   [C+0x20]=orig flags  [C+0x28]=orig rax；rax=common（死值，可当载体）；
    //   rcx/rdx/r8/r9=目标实参原值（未触碰）。C 的 16 对齐未知（锚点栈对齐不定）。
    "mov  rax, rsp",        // C
    "sub  rsp, 0x60",
    "and  rsp, -16",        // call emit 前 16 对齐（x64 ABI 硬约束）
    // 帧布局（X=rsp）：[X+0x00..0x20)=shadow  [X+0x20]=第 5 参  [X+0x50]=C
    "mov  [rsp+0x50], rax",
    "mov  [rsp+0x00], rcx", // 保存原 rcx/rdx/r8/r9（emit 返回后还原）
    "mov  [rsp+0x08], rdx",
    "mov  [rsp+0x10], r8",
    "mov  [rsp+0x18], r9",
    "mov  [rsp+0x20], r9",  // 第 5 参 = orig r9
    "mov  ecx, [rax]",      // arg1 = id
    "mov  rdx, [rsp+0x00]", // arg2 = orig rcx
    "mov  r8,  [rsp+0x08]", // arg3 = orig rdx
    "mov  r9,  [rsp+0x10]", // arg4 = orig r8
    "call {emit}",          // extern "system" fn(id: u32, a: u64, b: u64, c: u64, d: u64)
    "mov  rcx, [rsp+0x00]", // 还原 orig rcx/rdx/r8/r9
    "mov  rdx, [rsp+0x08]",
    "mov  r8,  [rsp+0x10]",
    "mov  r9,  [rsp+0x18]",
    "mov  rsp, [rsp+0x50]", // rsp = C（emit 破坏过 rax，此后一律 rsp 相对寻址）
    "mov  r11, [rsp+0x10]", // 还原 orig r11 / r10
    "mov  r10, [rsp+0x18]",
    "add  rsp, 0x30",       // rsp = rsp0（E9 时刻的栈；0x30 = stub 的 6 槽）
    "mov  rax, [rsp-0x08]", // 先救 orig rax（[C+0x28]=rsp0-8）——push resume 会覆写此槽
    "push qword ptr [rsp-0x28]", // resume 压到 rsp0-8（末尾 ret 消费，不占寄存器）
    "push qword ptr [rsp-0x08]", // flags 临时压到 rsp0-16
    "popfq",                // 还原 flags（rsp 回到 rsp0-8）
    "ret",                  // → entry+5；rsp = rsp0；全部寄存器/flags 原值
    emit = sym ziwei_hook_emit_x64,
);

#[cfg(target_arch = "x86_64")]
unsafe extern "system" fn ziwei_hook_emit_x64(id: u32, a: u64, b: u64, c: u64, d: u64) {
    ring_push(id, [a, b, c, d]);
}

#[cfg(target_arch = "x86")]
std::arch::global_asm!(
    ".balign 4",
    // .globl：跨 CGU 引用（如测试）需全局符号；不影响 DLL 导出表
    ".globl _ziwei_hook_common_x86",
    "_ziwei_hook_common_x86:", // i686 cdecl 符号装饰（前导下划线）
    // 入场：[esp]=恢复地址（stub push 的 entry+5），ecx = id；
    // 目标参数：[esp+8..0x14]=arg0..3（[esp+4]=目标返回地址）。
    "mov  eax, esp",        // eax = esp0
    "pushad",               // 保存全部；pushad 后 [esp+32]=eax(=esp0)
    "pushfd",
    "mov  eax, [esp+32]",   // 还原 esp0 到 eax
    "push eax",             // cdecl 参 2 先压（esp0）
    "push ecx",             // cdecl 参 1 后压（id，落在栈顶）
    "call {emit}",          // extern "C"（cdecl）fn(id: u32, esp0: u32)
    "add  esp, 8",
    "popfd",
    "popad",
    "ret",                  // 弹出 stub 压入的恢复地址 → entry+5
    emit = sym ziwei_hook_emit_x86,
);

/// x86 emit：从 esp0 还原目标参数（纯 Rust 读栈，无内联汇编偏移脆性）。
#[cfg(target_arch = "x86")]
unsafe extern "C" fn ziwei_hook_emit_x86(id: u32, esp0: u32) {
    let p = esp0 as *const u32;
    let a0 = *p.add(2);
    let a1 = *p.add(3);
    let a2 = *p.add(4);
    let a3 = *p.add(5);
    ring_push(id, [a0 as u64, a1 as u64, a2 as u64, a3 as u64]);
}

// ---------------------------------------------------------------------------
// hook 表 + RX 竞技场（stub 常驻不回收——在途线程安全优先）
// ---------------------------------------------------------------------------
struct HookMeta {
    addr: u64,
    id: u32,
}

static HOOK_META: std::sync::Mutex<Vec<HookMeta>> = std::sync::Mutex::new(Vec::new());

/// RX stub 竞技场（64KB 按需多块；永不回收）。地址存 usize（static 需 Sync）。
/// 可持有多块：carve 要求与 near 距离 ≤ 复用半径（x64 E9 rel32 约束），不合则新开一块。
struct Arena {
    base: usize,
    next: usize,
    left: usize,
}

/// 竞技场复用半径：块内任意 carve 点与 near 的距离 ≤ 半径 + 64KB ≪ 2GB（rel32 安全）。
const ARENA_REUSE_RADIUS: u64 = 0x6000_0000; // 1.5GB

static ARENA: std::sync::Mutex<Vec<Arena>> = std::sync::Mutex::new(Vec::new());

/// 就地取一块 64KB RWX：near ±1/4/16/64/128/256/512MB 逐候选试，全败回落系统分配。
unsafe fn alloc_arena_near(near: u64) -> Result<usize, String> {
    let mut picked: usize = 0;
    'outer: for &mb in [1u64, 4, 16, 64, 128, 256, 512].iter() {
        for &sign in [1i64, -1].iter() {
            let want = (near as i64 + sign * (mb as i64) * 1024 * 1024) as usize & !0xFFFF;
            if want == 0 {
                continue;
            }
            let p = VirtualAlloc(
                want as *mut c_void,
                0x10000,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE,
            );
            if !p.is_null() {
                picked = p as usize;
                break 'outer;
            }
        }
    }
    if picked == 0 {
        let p = VirtualAlloc(
            std::ptr::null_mut(),
            0x10000,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_EXECUTE_READWRITE,
        );
        if p.is_null() {
            return Err("RX 竞技场分配失败（±2GB 内无空位且系统分配失败）".to_string());
        }
        picked = p as usize;
    }
    Ok(picked)
}

/// 取竞技场写指针（不足则新开一块）。落点必须与 near 同处 ±(1.5GB+64KB)——
/// x64 入口补丁是 E9 rel32（±2GB 有符号），竞技场离 hook 目标过远会静默截断
/// 成垃圾跳转目标（首次触发即 EXEC AV）；x86 平坦空间无此约束但同逻辑无害。
unsafe fn arena_alloc(near: u64, size: usize) -> Result<usize, String> {
    let mut g = ARENA.lock().unwrap_or_else(|p| p.into_inner());
    for a in g.iter_mut() {
        if a.left >= size && (a.base as u64).abs_diff(near) <= ARENA_REUSE_RADIUS + 0x10000 {
            let p = a.next;
            a.next += size;
            a.left -= size;
            return Ok(p);
        }
    }
    let picked = alloc_arena_near(near)?;
    g.push(Arena {
        base: picked,
        next: picked + size,
        left: 0x10000 - size,
    });
    Ok(picked)
}

/// x64 stub 字节装配（hook_set 与单测共用——单测须复刻与生产逐字节一致的 stub）。
/// 全状态入栈（common 契约，6 槽）：orig rax → flags → orig r10 → orig
/// r11 → resume → id；rax 之后纯当载体。x64 的一切易失寄存器都可能
/// 在 mid-function 锚点存活，stub 不许裸写任何一个。
#[cfg(target_arch = "x86_64")]
fn build_stub_x64(id: u32, resume: usize, common: usize) -> [u8; 64] {
    let mut code = [0xCCu8; 64];
    code[0] = 0x50; // push rax（orig rax → [C+0x28]）
    code[1] = 0x9C; // pushfq（flags → [C+0x20]）
    code[2] = 0x41;
    code[3] = 0x52; // push r10（→ [C+0x18]）
    code[4] = 0x41;
    code[5] = 0x53; // push r11（→ [C+0x10]）
    code[6] = 0x48;
    code[7] = 0xB8; // movabs rax, resume
    code[8..16].copy_from_slice(&(resume as u64).to_le_bytes());
    code[16] = 0x50; // push rax（resume → [C+0x08]）
    code[17] = 0xB8; // mov eax, id
    code[18..22].copy_from_slice(&id.to_le_bytes());
    code[22] = 0x50; // push rax（id → [C+0x00]）
    code[23] = 0x48;
    code[24] = 0xB8; // movabs rax, common
    code[25..33].copy_from_slice(&(common as u64).to_le_bytes());
    code[33] = 0xFF;
    code[34] = 0xE0; // jmp rax
    code
}

/// hook_set：id + 目标地址 → 二次实核入口 5×0x90 → 建 stub → 入口补丁。
unsafe fn hook_set(id: u32, addr: u64) -> Result<String, String> {
    let mut meta = HOOK_META.lock().unwrap_or_else(|p| p.into_inner());
    if meta.len() >= 8 {
        return Err("hook 数已达上限（8）".to_string());
    }
    if meta.iter().any(|h| h.id == id) {
        return Err(format!("capture id {id} 已占用（先 hook_unset）"));
    }
    if meta.iter().any(|h| h.addr == addr) {
        return Err(format!("地址 {addr:#x} 已有 hook（同函数单点限制）"));
    }
    let base = addr as *mut u8;
    // 二次实核：入口 5 字节必须全 0x90（补丁宽度）
    let mut prologue = [0u8; 5];
    core::ptr::copy_nonoverlapping(base, prologue.as_mut_ptr(), 5);
    if prologue.iter().any(|&b| b != 0x90) {
        let hex: String = prologue.iter().map(|b| format!("{b:02X}")).collect();
        return Err(format!(
            "入口非 nop×5（实读 {hex}）：v1 只支持 nop 序章函数 hook"
        ));
    }
    let stub = arena_alloc(addr, 64)?;
    // x64 E9 rel32 有符号 32 位范围硬约束：stub 与 (入口+5) 距离必须 <2GB。
    // 超限诚实报错，绝不静默截断（截断值 = 随机地址 = 首次触发即 EXEC AV）。
    #[cfg(target_arch = "x86_64")]
    let rel_ok = {
        let diff = (stub as i64).wrapping_sub(addr.wrapping_add(5) as i64);
        (i32::MIN as i64..=i32::MAX as i64).contains(&diff)
    };
    #[cfg(target_arch = "x86")]
    let rel_ok = true; // 32 位平坦空间，rel32 环绕语义恒正确
    if !rel_ok {
        return Err(format!(
            "stub 0x{stub:x} 与入口 0x{addr:x} 距离超 ±2GB，rel32 无法表示"
        ));
    }
    #[cfg(target_arch = "x86_64")]
    let common = ziwei_hook_common_x64 as unsafe extern "C" fn() as usize;
    #[cfg(target_arch = "x86")]
    let common = ziwei_hook_common_x86 as unsafe extern "C" fn() as usize;
    let resume = addr as usize + 5;
    // x64 下整包被 build_stub_x64 覆写，填充值只有 x86 读——此处静默误报
    #[cfg_attr(target_arch = "x86_64", allow(unused_assignments))]
    let mut code = [0xCCu8; 64];
    #[cfg(target_arch = "x86_64")]
    {
        code = build_stub_x64(id, resume, common);
    }
    #[cfg(target_arch = "x86")]
    {
        code[0] = 0x68; // push imm32（恢复地址）
        code[1..5].copy_from_slice(&(resume as u32).to_le_bytes());
        code[5] = 0xB9; // mov ecx, id
        code[6..10].copy_from_slice(&id.to_le_bytes());
        code[10] = 0xE9; // jmp rel32 → common（32 位平坦空间恒可达）
        let rel = (common as isize - (stub + 15) as isize) as i32;
        code[11..15].copy_from_slice(&rel.to_le_bytes());
    }
    core::ptr::copy_nonoverlapping(code.as_ptr(), stub as *mut u8, 64);
    FlushInstructionCache(GetCurrentProcess(), stub as *const c_void, 64);

    // 入口补丁：E9 rel32 → stub（x64 范围已由 rel_ok 校验保证）
    let mut old: u32 = 0;
    if VirtualProtect(base as *mut c_void, 5, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
        return Err(format!("VirtualProtect 失败（err={}）", GetLastError()));
    }
    let rel = (stub as isize - (addr as usize + 5) as isize) as i32;
    let mut patch = [0u8; 5];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&rel.to_le_bytes());
    core::ptr::copy_nonoverlapping(patch.as_ptr(), base, 5);
    VirtualProtect(base as *mut c_void, 5, old, &mut old);
    FlushInstructionCache(GetCurrentProcess(), base as *const c_void, 5);

    meta.push(HookMeta { addr, id });
    // 新 hook 从空环计起（防旧事件混入新口径）：read 追平 write + 清丢弃计数
    ring_lock();
    RING.read
        .store(RING.write.load(Ordering::Relaxed), Ordering::Release);
    RING.dropped.store(0, Ordering::Release);
    RING.lock.store(0, Ordering::Release);
    Ok(format!(
        "{{\"ok\":true,\"id\":{id},\"addr\":{addr:#x},\"stolen\":5,\"note\":\"入口 5×0x90 已补丁\"}}"
    ))
}

/// hook_unset：实核补丁在场 → 还原 5×0x90。stub 不回收（安全优先）。
/// 入口补丁还原为 5×0x90（实核 0xE9 在场 → VirtualProtect → 写 → 还原保护 →
/// 刷 icache）。hook_unset 与 unload_prepare 共用。
unsafe fn restore_entry_nops(addr: u64) -> Result<(), String> {
    let base = addr as *mut u8;
    let mut cur = [0u8; 5];
    core::ptr::copy_nonoverlapping(base, cur.as_mut_ptr(), 5);
    if cur[0] != 0xE9 {
        let hex: String = cur.iter().map(|b| format!("{b:02X}")).collect();
        return Err(format!("入口补丁不在场（实读 {hex}）——可能已被外部改写"));
    }
    let mut old: u32 = 0;
    if VirtualProtect(base as *mut c_void, 5, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
        return Err(format!("VirtualProtect 失败（err={}）", GetLastError()));
    }
    let nop = [0x90u8; 5];
    core::ptr::copy_nonoverlapping(nop.as_ptr(), base, 5);
    VirtualProtect(base as *mut c_void, 5, old, &mut old);
    FlushInstructionCache(GetCurrentProcess(), base as *const c_void, 5);
    Ok(())
}

unsafe fn hook_unset(addr: u64) -> Result<String, String> {
    let mut meta = HOOK_META.lock().unwrap_or_else(|p| p.into_inner());
    let Some(pos) = meta.iter().position(|h| h.addr == addr) else {
        return Err(format!("地址 {addr:#x} 无在场 hook"));
    };
    restore_entry_nops(addr)?;
    let id = meta.remove(pos).id;
    Ok(format!(
        "{{\"ok\":true,\"unset\":true,\"id\":{id},\"addr\":{addr:#x}}}"
    ))
}

/// unload 三关闸（BUG-200）：任何一关不过 = Err（worker 拒绝卸载，管道继续
/// 服务）——宿主进程安全优先于卸载意愿。
/// ① 在途 call_fn：等不得（单飞占位未清）也杀不得（Win32 无安全强杀）→ 拒；
/// ② 泄漏调用线程仍在执行 DLL 代码：等 3s，不退 = 拒（卸载后线程踩已卸载
///    代码 = AV）；
/// ③ 在场 hook 逐个还原入口（LIFO；stub jmp 的 common 在 DLL 镜像内，镜像
///    卸载后 hook 命中即 AV）：已还原的从表内移除，失败的中止并留表（重试
///    unload 可续）。
/// 收尾：摘 VEH handler（代码在镜像内——悬垂 handler 是卸载后宿主任何
/// first-chance 异常的死路）。
/// 返回解除的 hook 数（bot 侧 detach 据此与应答审计对账）。
unsafe fn unload_prepare() -> Result<u32, String> {
    if CALL.active.load(Ordering::Acquire) != 0 {
        return Err("在途 call_fn 未完成".to_string());
    }
    let th = CALL_THREAD.swap(0, Ordering::AcqRel);
    if th != 0 {
        if WaitForSingleObject(th as *mut c_void, 3000) != WAIT_OBJECT_0 {
            CALL_THREAD.store(th, Ordering::Release); // 还回去：下次 unload 再等
            return Err("调用线程 3s 未退出（超时泄漏中）".to_string());
        }
        CloseHandle(th as *mut c_void);
    }
    let mut meta = HOOK_META.lock().unwrap_or_else(|p| p.into_inner());
    let mut unhooked = 0u32;
    while let Some(h) = meta.last() {
        let addr = h.addr;
        if let Err(e) = unsafe { restore_entry_nops(addr) } {
            return Err(format!("hook @{addr:#x} 入口还原失败：{e}"));
        }
        meta.pop();
        unhooked += 1;
    }
    drop(meta);
    let veh = VEH_HANDLE.swap(0, Ordering::AcqRel);
    if veh != 0 {
        RemoveVectoredExceptionHandler(veh as *mut c_void);
    }
    Ok(unhooked)
}

// ---------------------------------------------------------------------------
// call_fn：单飞 + 专用线程 + 看门狗 + VEH 救援
// ---------------------------------------------------------------------------
struct CallState {
    active: AtomicU32,
    thread_id: AtomicU32,
    done: AtomicU32,
    seh: AtomicU32,
    ret: AtomicU64,
    addr: AtomicU64,
    conv: AtomicU32,
    args: [AtomicU64; 4],
}

static CALL: CallState = CallState {
    active: AtomicU32::new(0),
    thread_id: AtomicU32::new(0),
    done: AtomicU32::new(0),
    seh: AtomicU32::new(0),
    ret: AtomicU64::new(0),
    addr: AtomicU64::new(0),
    conv: AtomicU32::new(0),
    args: [
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ],
};

const CONV_MSX64: u32 = 0;
const CONV_STDCALL: u32 = 1;
const CONV_CDECL: u32 = 2;

/// VEH 救援栈。必须含内部可变性（UnsafeCell）：纯常量初始化的 static 会被
/// rustc 以 LLVM constant 发射进只读 .rdata，导致救援线程首次 push 即
/// 写 AV → 二次异常静默终止进程（exit=原始异常码、绕过全部用户态分发）。
#[repr(align(16))]
struct RescueStack(#[allow(dead_code)] std::cell::UnsafeCell<[u8; 0x8000]>);
unsafe impl Sync for RescueStack {}
static RESCUE: RescueStack = RescueStack(std::cell::UnsafeCell::new([0; 0x8000]));
static BAIL_STUB: AtomicU64 = AtomicU64::new(0);

/// VEH 句柄（文件级；BUG-200①）：handler 代码在 DLL 镜像内，卸载前必须摘除
/// ——悬垂 handler 会让宿主进程任何 first-chance 异常（ntdll 内部 SEH 是常态）
/// 调进已卸载地址 → 进程死。曾经 call_fn 函数局部 static，unload 够不着。
static VEH_HANDLE: AtomicUsize = AtomicUsize::new(0);
/// 最近一通 call_fn 的专用线程句柄（BUG-200②）：泄漏线程执行 DLL 代码期间
/// 卸载 = AV——unload_prepare 需等它退出（超时拒绝卸载）。句柄不主动关：
/// 下一通 call_fn 收尸，或 unload_prepare 收尾，最终一条随进程终止回收
/// （与线程本体同款「泄漏已声明，损失有界」口径）。
static CALL_THREAD: AtomicUsize = AtomicUsize::new(0);

/// 带约定的任意地址调用（调用线程上下文；shadow space / 清栈自管）。
#[cfg(target_arch = "x86_64")]
unsafe fn call_with_conv(addr: usize, conv: u32, a: [u64; 4]) -> u64 {
    let _ = conv; // x64 仅 msx64 一种约定
    let ret: u64;
    core::arch::asm!(
        "sub rsp, 0x20",
        "call rax",
        "add rsp, 0x20",
        in("rax") addr,
        in("rcx") a[0],
        in("rdx") a[1],
        in("r8") a[2],
        in("r9") a[3],
        lateout("rax") ret,
        clobber_abi("system"),
    );
    ret
}

#[cfg(target_arch = "x86")]
unsafe fn call_with_conv(addr: usize, conv: u32, a: [u64; 4]) -> u64 {
    let ret: u32;
    let (a0, a1, a2, a3) = (a[0] as u32, a[1] as u32, a[2] as u32, a[3] as u32);
    core::arch::asm!(
        "push {a3}",
        "push {a2}",
        "push {a1}",
        "push {a0}",
        "call {target}",
        "test {is_cdecl}, {is_cdecl}",
        "jz 2f",
        "add esp, 16",
        "2:",
        a0 = in(reg) a0,
        a1 = in(reg) a1,
        a2 = in(reg) a2,
        a3 = in(reg) a3,
        target = in(reg) addr,
        is_cdecl = in(reg) (conv == CONV_CDECL) as u32,
        lateout("eax") ret,
        clobber_abi("system"),
    );
    ret as u64
}

/// 调用线程入口：从 CALL 静态槽取参 → 调 → 写回 → done。
unsafe extern "system" fn call_thread_main(_arg: *mut c_void) -> u32 {
    CALL.thread_id
        .store(GetCurrentThreadId(), Ordering::Release);
    let addr = CALL.addr.load(Ordering::Acquire) as usize;
    let conv = CALL.conv.load(Ordering::Acquire);
    let args = [
        CALL.args[0].load(Ordering::Acquire),
        CALL.args[1].load(Ordering::Acquire),
        CALL.args[2].load(Ordering::Acquire),
        CALL.args[3].load(Ordering::Acquire),
    ];
    let ret = call_with_conv(addr, conv, args);
    CALL.ret.store(ret, Ordering::Release);
    CALL.done.store(1, Ordering::Release);
    CALL.active.store(0, Ordering::Release);
    0
}

/// VEH：仅在途调用线程的致命硬件异常 → 改写上下文落救援栈 bail 存根。
/// 其余异常一律放行（CONTINUE_SEARCH，不干扰目标自身 SEH/防保全链）。
unsafe extern "system" fn call_veh(info: *mut c_void) -> i32 {
    if CALL.active.load(Ordering::Acquire) == 0 || BAIL_STUB.load(Ordering::Acquire) == 0 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if GetCurrentThreadId() != CALL.thread_id.load(Ordering::Relaxed) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // info = *EXCEPTION_POINTERS { ExceptionRecord, ContextRecord }
    let rec = *(info as *const *const u32);
    let ctx = *((info as *const *const c_void).add(1));
    let code = *rec;
    let rescued = matches!(
        code,
        0xC000_0005 // ACCESS_VIOLATION
            | 0xC000_001D // ILLEGAL_INSTRUCTION
            | 0xC000_0094 // INT_DIVIDE_BY_ZERO
            | 0xC000_0095 // INT_OVERFLOW
            | 0xC000_0096 // PRIVILEGED_INSTRUCTION
            | 0xC000_00FD // STACK_OVERFLOW（换栈正解）
    );
    if !rescued {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    CALL.seh.store(code, Ordering::Release);
    CALL.done.store(1, Ordering::Release);
    CALL.active.store(0, Ordering::Release);
    let bail = BAIL_STUB.load(Ordering::Acquire);
    // RESCUE 栈顶（落场而非 call —— 入口 RSP ≡ 8 mod 16）。
    // ⚠ RESCUE 必须可写（UnsafeCell 保 .data）：常量初始化纯数组 static 会被
    // rustc 落只读 .rdata，bail 存根/ExitThread 首次压栈即二次 AV → 内核
    // 静默终止进程（exit=原始异常码，绕过全部用户态异常分发）。
    let stack_top = std::ptr::addr_of!(RESCUE) as usize + 0x8000 - 8;
    #[cfg(target_arch = "x86_64")]
    {
        let rip = (ctx as *mut u8).add(0xF8) as *mut u64;
        let rsp = (ctx as *mut u8).add(0x98) as *mut u64;
        *rip = bail;
        *rsp = stack_top as u64;
    }
    #[cfg(target_arch = "x86")]
    {
        let eip = (ctx as *mut u8).add(0xB8) as *mut u32;
        let esp = (ctx as *mut u8).add(0xC4) as *mut u32;
        *eip = bail as u32;
        *esp = stack_top as u32;
    }
    EXCEPTION_CONTINUE_EXECUTION
}

/// bail 存根装配（首用；VEH 绝对跳落点）：
///  x64: mov ecx,1 | jmp qword [rip+0] + 8B &ExitThread（共 19B）
///  x86: push 1 | jmp dword [abs] &ExitThread（共 8B）
unsafe fn ensure_bail_stub() -> Result<(), String> {
    if BAIL_STUB.load(Ordering::Acquire) != 0 {
        return Ok(());
    }
    let exit = ExitThread as unsafe extern "system" fn(u32) -> ! as usize;
    let p = arena_alloc(0x1_0000_0000, 20)?;
    #[cfg(target_arch = "x86_64")]
    {
        // 救援退场（原始设计）：mov ecx,1 + jmp [rip+0] → ExitThread(1)。
        // 落场（非 call）——入口 RSP ≡ 8 mod 16（VEH 已把 RSP 置为 RESCUE+0x8000-8）。
        // RESCUE 经 UnsafeCell 保可写（只读 .rdata 是二次异常静默终止的根因）。
        let mut code = [0u8; 19];
        code[0] = 0xB9;
        code[1] = 0x01;
        code[2] = 0x00;
        code[3] = 0x00;
        code[4] = 0x00; // mov ecx, 1
        code[5] = 0xFF;
        code[6] = 0x25; // jmp [rip+0]
        code[7..11].copy_from_slice(&0u32.to_le_bytes());
        code[11..19].copy_from_slice(&(exit as u64).to_le_bytes());
        core::ptr::copy_nonoverlapping(code.as_ptr(), p as *mut u8, 19);
        FlushInstructionCache(GetCurrentProcess(), p as *const c_void, 19);
    }
    #[cfg(target_arch = "x86")]
    {
        let mut code = [0u8; 8];
        code[0] = 0x6A;
        code[1] = 0x01; // push 1
        code[2] = 0xFF;
        code[3] = 0x25; // jmp dword [abs]
        code[4..8].copy_from_slice(&(exit as u32).to_le_bytes());
        core::ptr::copy_nonoverlapping(code.as_ptr(), p as *mut u8, 8);
        FlushInstructionCache(GetCurrentProcess(), p as *const c_void, 8);
    }
    BAIL_STUB.store(p as u64, Ordering::Release);
    Ok(())
}

/// call_fn 命令体：单飞占位 → VEH 注册 → 线程 → 看门狗轮询 → 应答。
unsafe fn call_fn(addr: u64, conv: u32, args: [u64; 4], timeout_ms: u32) -> String {
    if CALL
        .active
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return "{\"ok\":false,\"error\":\"已有在途 call_fn（单飞限制）\"}".to_string();
    }
    if let Err(e) = ensure_bail_stub() {
        CALL.active.store(0, Ordering::Release);
        return format!("{{\"ok\":false,\"error\":\"{e}\"}}");
    }
    CALL.done.store(0, Ordering::Release);
    CALL.seh.store(0, Ordering::Release);
    CALL.ret.store(0, Ordering::Release);
    CALL.thread_id.store(0, Ordering::Release);
    CALL.addr.store(addr, Ordering::Release);
    CALL.conv.store(conv, Ordering::Release);
    for (i, v) in args.iter().enumerate() {
        CALL.args[i].store(*v, Ordering::Release);
    }
    if VEH_HANDLE.load(Ordering::Acquire) == 0 {
        let h = AddVectoredExceptionHandler(1, call_veh);
        if h.is_null() {
            CALL.active.store(0, Ordering::Release);
            return "{\"ok\":false,\"error\":\"VEH 注册失败\"}".to_string();
        }
        VEH_HANDLE.store(h as usize, Ordering::Release);
    }
    let h = CreateThread(
        std::ptr::null_mut(),
        0x40000, // 256KB 栈：常规调用深度够用，泄漏时损失有界
        call_thread_main,
        std::ptr::null_mut(),
        0,
        std::ptr::null_mut(),
    );
    if h.is_null() {
        CALL.active.store(0, Ordering::Release);
        return format!(
            "{{\"ok\":false,\"error\":\"CreateThread 失败（err={})\"}}",
            GetLastError()
        );
    }
    // 句柄留档 CALL_THREAD（unload_prepare 等退/收口）；上一通线程收尸：
    // 等 100ms，已退出则关句柄，未退出留置（与线程同款泄漏口径）
    let prev = CALL_THREAD.swap(h as usize, Ordering::AcqRel);
    if prev != 0 && WaitForSingleObject(prev as *mut c_void, 100) == WAIT_OBJECT_0 {
        CloseHandle(prev as *mut c_void);
    }
    let t0 = std::time::Instant::now();
    loop {
        if CALL.done.load(Ordering::Acquire) != 0 {
            break;
        }
        if t0.elapsed().as_millis() as u32 >= timeout_ms {
            // 看门狗：诚实报 timeout；调用线程不强杀（Win32 无安全手段，
            // 泄漏已声明）；active 位保持——同函数再调用被拒，直到完成。
            return format!(
                "{{\"ok\":false,\"timeout\":true,\"addr\":{addr:#x},\"waited_ms\":{timeout_ms},\"note\":\"调用超时；线程未强杀，完成前新调用将被单飞限制拒绝\"}}"
            );
        }
        Sleep(2);
    }
    let seh = CALL.seh.load(Ordering::Acquire);
    let ret = CALL.ret.load(Ordering::Acquire);
    let elapsed = t0.elapsed().as_millis() as u64;
    if seh != 0 {
        format!(
            "{{\"ok\":false,\"seh\":\"0x{seh:08X}\",\"addr\":{addr:#x},\"elapsed_ms\":{elapsed},\"note\":\"目标函数硬件异常；已 VEH 救援，宿主未受影响\"}}"
        )
    } else {
        format!("{{\"ok\":true,\"ret\":{ret},\"addr\":{addr:#x},\"elapsed_ms\":{elapsed}}}")
    }
}

/// poll_events：批量拉取 hook 事件（拉取制——管道命令循环保持严格配对）。
fn poll_events(max: u32) -> String {
    ring_lock();
    let mut out = String::with_capacity(128);
    out.push_str("{\"ok\":true,\"events\":[");
    let mut n = 0u32;
    let mut r = RING.read.load(Ordering::Relaxed);
    let w = RING.write.load(Ordering::Relaxed);
    while n < max && r < w {
        let ev = unsafe { (*RING.buf.get())[(r % RING_CAP) as usize] };
        r += 1;
        if n > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\":{},\"seq\":{},\"tid\":{},\"regs\":[{},{},{},{}]}}",
            ev.id, ev.seq, ev.tid, ev.regs[0], ev.regs[1], ev.regs[2], ev.regs[3]
        ));
        n += 1;
    }
    RING.read.store(r, Ordering::Release);
    let dropped = RING.dropped.swap(0, Ordering::AcqRel);
    RING.lock.store(0, Ordering::Release);
    out.push_str(&format!("],\"count\":{n},\"dropped\":{dropped}}}"));
    out
}

/// JSON 数组取前 4 个数值（"[v,v,v]" → ([v..], 个数)；非整数形态 → None）。
fn json_args(s: &str, key: &str) -> Option<([u64; 4], usize)> {
    let needle = format!("\"{key}\":[");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let end = rest.find(']')?;
    let mut out = [0u64; 4];
    let mut n = 0;
    for part in rest[..end].split(',') {
        if n >= 4 {
            break;
        }
        let t = part.trim();
        if t.is_empty() {
            break;
        }
        let v = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            u64::from_str_radix(hex, 16).ok()?
        } else {
            t.parse::<u64>().ok()?
        };
        out[n] = v;
        n += 1;
    }
    Some((out, n))
}

#[cfg(test)]
mod tests;
