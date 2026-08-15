//! test_dll —— 被注入进已签名进程（SbieCtrl）的测试 DLL。
//! 验证三件事：
//! 1. SessionLeader 签名墙绕过（已验证过，回归）
//! 2. EnumProcessEx 按盒枚举 pid（leader 状态下是否可用、实时性）
//! 3. QueryProcess 按 pid 查映像名（能否识别盒内子进程是谁）
//! 中途 spawn 盒内"起子进程"活动，观察枚举是否实时反映。

#![cfg(windows)]

use std::ffi::c_void;
use std::io::Write;

const LOG: &str = r"C:\AI\NemesisBot\NemesisBot_Rust\test-tools\eval-inject-test\test_dll\inject_result.txt";
const DLL: &str = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\SbieDll.dll";
const START: &str = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\Start.exe";
const FEED: &str = r"C:\AI\NemesisBot\NemesisBot_Rust\test-tools\eval-inject-test\test_dll\spawn_feed.bat";
const BOX: &str = "NemesisBox";

type FnSL = unsafe extern "C" fn(u32, *mut c_void) -> i32;
type FnMC = unsafe extern "C" fn(*mut u32, *mut u32) -> i32;
type FnMG = unsafe extern "C" fn(*mut u32, *mut u32, *mut u32, *mut u16) -> i32;
// LONG SbieApi_EnumProcessEx(const WCHAR* box, BOOLEAN all_sessions, ULONG which_session, ULONG* pids, ULONG* count)
type FnEnum = unsafe extern "C" fn(*const u16, i32, u32, *mut u32, *mut u32) -> i32;
// LONG SbieApi_QueryProcess(HANDLE pid, WCHAR box[34], WCHAR image[96], WCHAR sid[96], ULONG* session)
type FnQP = unsafe extern "C" fn(*mut c_void, *mut u16, *mut u16, *mut u16, *mut u32) -> i32;

unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
}

fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}
fn from_wide(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[.. buf.iter().position(|&c| c == 0).unwrap_or(0)])
}

#[no_mangle]
pub extern "C" fn Run() {
    unsafe {
        let mut log = match std::fs::File::create(LOG) {
            Ok(f) => f,
            Err(_) => { ExitProcess(2); }
        };
        let mut w = |s: &str| { let _ = writeln!(log, "{s}"); let _ = log.flush(); };

        w(&format!("[dll] Run() pid={}", GetCurrentProcessId()));

        let hd = LoadLibraryW(wide(DLL).as_ptr());
        if hd.is_null() { w("[dll] FAIL load SbieDll"); ExitProcess(3); }
        let sl: FnSL = std::mem::transmute(GetProcAddress(hd, b"SbieApi_SessionLeader\0".as_ptr()));
        let mc: FnMC = std::mem::transmute(GetProcAddress(hd, b"SbieApi_MonitorControl\0".as_ptr()));
        let mg: FnMG = std::mem::transmute(GetProcAddress(hd, b"SbieApi_MonitorGetEx\0".as_ptr()));
        let en: FnEnum = std::mem::transmute(GetProcAddress(hd, b"SbieApi_EnumProcessEx\0".as_ptr()));
        let qp: FnQP = std::mem::transmute(GetProcAddress(hd, b"SbieApi_QueryProcess\0".as_ptr()));
        // SbieApi_Call 变参：SbieApi_Call(API_RELOAD_CONF, 2, -1, 0) 让驱动重读 ini
        // API_RELOAD_CONF = API_FIRST(0x12340000) + 15
        type FnCall = unsafe extern "C" fn(u32, i32, ...) -> i32;
        let call: FnCall = std::mem::transmute(GetProcAddress(hd, b"SbieApi_Call\0".as_ptr()));

        // 关键：reload 驱动配置，让驱动认识新加的 [NemesisEvalBox] 段
        let r = call(0x12340000 + 15, 2, -1, 0);
        w(&format!("[dll] RELOAD_CONF=0x{:08X} (0=驱动已重读ini)", r as u32));

        // 先测枚举（SessionLeader 之前——验证非 leader 状态是否可用）
        // 注意：count 参数是【输入容量/输出数量】两用——必须预置 512（容量），传 0 会立即 break 返回 0
        let mut pids = [0u32; 512];
        let mut cnt: u32 = 512;
        let boxw = wide(BOX);
        let r = en(boxw.as_ptr(), 0, u32::MAX, pids.as_mut_ptr(), &mut cnt);
        w(&format!("[dll] PRE-LEADER EnumProcessEx={} count={}", r, cnt));

        // SessionLeader + Monitor on
        let r = sl(0, std::ptr::null_mut());
        w(&format!("[dll] SessionLeader=0x{:08X}", r as u32));
        let mut on: u32 = 1;
        let r = mc(&mut on, std::ptr::null_mut());
        w(&format!("[dll] MonitorControl(on)=0x{:08X}", r as u32));

        // 主循环：30 轮 × ~500ms。第 10 轮向两个盒各 spawn 进程（双盒分流验证）
        let boxw2 = wide("NemesisEvalBox");
        let mut total_events = 0usize;
        for round in 0..30 {
            // 收 monitor 事件（本轮清空 buffer）
            let mut ev = 0usize;
            let mut ev_pids: Vec<u32> = Vec::new();
            for _ in 0..200 {
                let (mut t, mut pid, mut tid) = (0u32, 0u32, 0u32);
                let mut nm = [0u16; 256];
                if mg(&mut t, &mut pid, &mut tid, nm.as_mut_ptr()) != 0 { break; }
                ev += 1;
                if ev <= 5 { ev_pids.push(pid); } // 记前几条事件的 pid（用于归属对照）
            }
            // 枚举两个盒的进程（count 预置容量 512）
            let (mut pids1, mut pids2) = ([0u32; 512], [0u32; 512]);
            let (mut c1, mut c2) = (512u32, 512u32);
            let r1 = en(boxw.as_ptr(), 0, u32::MAX, pids1.as_mut_ptr(), &mut c1);
            let r2 = en(boxw2.as_ptr(), 0, u32::MAX, pids2.as_mut_ptr(), &mut c2);
            // 重叠检测（分流关键：两盒 pid 集合应无交集）
            let set1: std::collections::HashSet<u32> = pids1[..c1 as usize].iter().copied().collect();
            let overlap: Vec<u32> = pids2[..c2 as usize].iter().copied().filter(|p| set1.contains(p)).collect();
            // 第二盒的映像名（新盒里是谁）
            let mut names2 = Vec::new();
            for i in 0..(c2 as usize).min(4) {
                let mut bname = [0u16; 34]; let mut iname = [0u16; 96];
                let mut sid = [0u16; 96]; let mut ses: u32 = 0;
                if qp(pids2[i] as *mut c_void, bname.as_mut_ptr(), iname.as_mut_ptr(), sid.as_mut_ptr(), &mut ses) == 0 {
                    names2.push(format!("pid={} img={} box={}", pids2[i], from_wide(&iname), from_wide(&bname)));
                }
            }
            w(&format!("[dll] r{:02} box1(r{} cnt={}) box2(r{} cnt={}) overlap={} events={} evpids={:?} | box2: {}",
                round, r1, c1, r2, c2, if overlap.is_empty() {"无✅"} else {"有❌"}, ev, ev_pids, names2.join(", ")));
            total_events += ev;

            // 第 10 轮：老盒 spawn feed（cmd 多级子进程）；新盒 spawn hello2
            if round == 10 {
                let msg = spawn_boxed_feed();
                w(&msg);
                let msg2 = spawn_in_box("NemesisEvalBox", r#"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\hello2.exe"#);
                w(&msg2);
            }
            Sleep(500);
        }
        w(&format!("[dll] total events: {}", total_events));
        let mut off: u32 = 0;
        let r = mc(&mut off, std::ptr::null_mut());
        w(&format!("[dll] MonitorControl(off)=0x{:08X}", r as u32));
        w("[dll] DONE, exiting 0");
        ExitProcess(0);
    }
}

unsafe fn spawn_boxed_feed() -> String {
    unsafe extern "system" {
        fn CreateProcessA(app: *const u8, cmd: *mut u8, pa: *const c_void, ta: *const c_void,
            inh: i32, flags: u32, env: *const c_void, cwd: *const u8,
            si: *mut u8, pi: *mut u8) -> i32;
        fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    }
    let mut cmd = Vec::<u8>::new();
    cmd.extend_from_slice(b"\"");
    cmd.extend_from_slice(START.as_bytes());
    cmd.extend_from_slice(b"\" /box:NemesisBox /hide_window \"");
    cmd.extend_from_slice(FEED.as_bytes());
    cmd.extend_from_slice(b"\"");
    cmd.push(0);
    // STARTUPINFOA 68B + PROCESS_INFORMATION 24B，zeroed
    let mut si = [0u8; 104];
    let cb = 68u32.to_le_bytes();
    si[0..4].copy_from_slice(&cb);
    let mut pi = [0u8; 24];
    let cr = CreateProcessA(std::ptr::null(), cmd.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, 0x08000000, std::ptr::null(), std::ptr::null(), si.as_mut_ptr(), pi.as_mut_ptr());
    format!("[dll] spawn boxed feed CreateProcess={}", cr)
    // 不等它——让它和枚举循环并行，观察枚举实时性
}

/// 通用：向指定盒 spawn 任意 exe（实验二：新盒 NemesisEvalBox）
unsafe fn spawn_in_box(box_name: &str, exe_path: &str) -> String {
    unsafe extern "system" {
        fn CreateProcessA(app: *const u8, cmd: *mut u8, pa: *const c_void, ta: *const c_void,
            inh: i32, flags: u32, env: *const c_void, cwd: *const u8,
            si: *mut u8, pi: *mut u8) -> i32;
    }
    let start = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\Start.exe";
    let mut cmd = Vec::<u8>::new();
    cmd.extend_from_slice(b"\"");
    cmd.extend_from_slice(start.as_bytes());
    cmd.extend_from_slice(b"\" /box:");
    cmd.extend_from_slice(box_name.as_bytes());
    cmd.extend_from_slice(b" /hide_window /wait \"");
    cmd.extend_from_slice(exe_path.as_bytes());
    cmd.extend_from_slice(b"\"");
    cmd.push(0);
    let mut si = [0u8; 104];
    si[0..4].copy_from_slice(&68u32.to_le_bytes());
    let mut pi = [0u8; 24];
    let cr = CreateProcessA(std::ptr::null(), cmd.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, 0x08000000, std::ptr::null(), std::ptr::null(), si.as_mut_ptr(), pi.as_mut_ptr());
    format!("[dll] spawn {} CreateProcess={}", box_name, cr)
}
