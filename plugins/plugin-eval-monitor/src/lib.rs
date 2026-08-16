//! eval-monitor-dll — injected into the signed SbieCtrl.exe host by
//! `nemesis-injector` (EP hijack). Runs inside the signed process context so
//! `SbieApi_SessionLeader` passes the signature check, then collects the
//! Sandboxie driver-level monitor events for the eval box (plan record
//! point ④ + in-box process monitoring).
//!
//! Entry contract with the injector shellcode: exports `Run()` (no args,
//! never returns — ExitProcess at the end).
//!
//! Configuration comes from env (set by the eval command process):
//! - `NEMESISBOT_EVAL_BOX`          box name to watch (default NemesisEvalBox)
//! - `NEMESISBOT_EVAL_EVENTS_FILE`  JSONL output path (real path, host side)
//! - `NEMESISBOT_EVAL_SBIEDLL`      full path to SbieDll.dll
//! - `NEMESISBOT_EVAL_TIMEOUT_SECS` total hard timeout in seconds (fuse)
//!
//! Exit codes: 0 = collected normally; 10 = env/config error;
//! 11 = SbieDll load failure; 12 = session leader failure (environment
//! problem — the command process must report and abort the whole eval);
//! 13 = watchdog fired.
//!
//! Windows only.

#![cfg(windows)]

use std::ffi::c_void;
use std::io::Write;

// ---------------------------------------------------------------------------
// FFI
// ---------------------------------------------------------------------------

type FnSL = unsafe extern "C" fn(u32, *mut c_void) -> i32;
type FnMC = unsafe extern "C" fn(*mut u32, *mut u32) -> i32;
type FnMG = unsafe extern "C" fn(*mut u32, *mut u32, *mut u32, *mut u16) -> i32;
type FnEnum = unsafe extern "C" fn(*const u16, i32, u32, *mut u32, *mut u32) -> i32;
type FnQP = unsafe extern "C" fn(*mut c_void, *mut u16, *mut u16, *mut u16, *mut u32) -> i32;

unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(h: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn SetErrorMode(mode: u32) -> u32;
    // ANSI env read — matches the ANSI environment block the injector passes
    // to CreateProcessA (all eval config paths/values are ASCII).
    fn GetEnvironmentVariableA(name: *const u8, buf: *mut u8, size: u32) -> u32;
}

const SEM_FAILCRITICALERRORS: u32 = 0x0001;
const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;

// MonitorGetEx returns 0x8000001A (NO_MORE_ENTRIES) when the buffer is empty;
// the drain loop just treats any non-zero return as "done for now", so the
// constant is not referenced explicitly.
const MONITOR_TYPE_MASK: u32 = 0xFF;

/// Monitor event types (api_flags.h) — only the ones we label.
fn type_label(t: u32) -> &'static str {
    match t {
        0x01 => "SYSCALL",
        0x02 => "PIPE",
        0x03 => "IPC",
        0x09 => "IMAGE",
        0x0A => "FILE",
        0x0B => "KEY",
        0x0D => "NETFW",
        0x11 => "DNS",
        _ => "OTHER",
    }
}

fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn from_wide(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)])
}

fn env_var(key: &str) -> Option<String> {
    let mut buf = [0u8; 1024];
    let n = unsafe {
        let mut k = key.as_bytes().to_vec();
        k.push(0);
        GetEnvironmentVariableA(k.as_ptr(), buf.as_mut_ptr(), buf.len() as u32)
    };
    if n == 0 || n as usize >= buf.len() {
        None
    } else {
        Some(String::from_utf8_lossy(&buf[..n as usize]).to_string())
    }
}

/// Minimal JSON string escaping for the JSONL writer.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Entry point (called by the injector shellcode)
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn Run() {
    // 0. Never show any error/crash dialog from inside the signed host.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }

    let box_name = env_var("NEMESISBOT_EVAL_BOX").unwrap_or_else(|| "NemesisEvalBox".to_string());
    let events_path = env_var("NEMESISBOT_EVAL_EVENTS_FILE");
    let sbiedll = env_var("NEMESISBOT_EVAL_SBIEDLL").unwrap_or_else(|| {
        String::from(r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\SbieDll.dll")
    });
    let timeout_secs: u64 = env_var("NEMESISBOT_EVAL_TIMEOUT_SECS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800); // 30 min fuse by default


    let Some(events_path) = events_path else {
        // Write diagnostics to the temp dir (always writable) so the failure
        // is diagnosable even without the events file path.
        let diag = std::env::temp_dir().join("eval_monitor_dll_diag.txt");
        let _ = std::fs::write(
            &diag,
            format!("missing NEMESISBOT_EVAL_EVENTS_FILE env\nbox={box_name}\nsbiedll={sbiedll}\n"),
        );
        unsafe { ExitProcess(10) }
    };

    let mut log = match std::fs::File::create(&events_path) {
        Ok(f) => f,
        Err(e) => {
            let diag = std::env::temp_dir().join("eval_monitor_dll_diag.txt");
            let _ = std::fs::write(
                &diag,
                format!("cannot create events file {events_path}: {e}\n"),
            );
            unsafe { ExitProcess(10) }
        }
    };
    let mut w = |s: &str| {
        let _ = writeln!(log, "{s}");
        let _ = log.flush();
    };

    // Watchdog: hard total timeout — guarantees the host process exits even
    // if a SbieApi call blocks forever.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(timeout_secs));
        unsafe { ExitProcess(13) }
    });

    w(&format!(
        "# eval-monitor start pid={} box={}",
        unsafe { GetCurrentProcessId() },
        box_name
    ));

    // 1. Load SbieDll.
    let hd = unsafe { LoadLibraryW(wide(&sbiedll).as_ptr()) };
    if hd.is_null() {
        w(&format!("# ERROR load SbieDll: {}", sbiedll));
        unsafe { ExitProcess(11) }
    }
    unsafe {
        let sl: FnSL = std::mem::transmute(GetProcAddress(hd, b"SbieApi_SessionLeader\0".as_ptr()));
        let mc: FnMC = std::mem::transmute(GetProcAddress(hd, b"SbieApi_MonitorControl\0".as_ptr()));
        let mg: FnMG = std::mem::transmute(GetProcAddress(hd, b"SbieApi_MonitorGetEx\0".as_ptr()));
        let en: FnEnum = std::mem::transmute(GetProcAddress(hd, b"SbieApi_EnumProcessEx\0".as_ptr()));
        let qp: FnQP = std::mem::transmute(GetProcAddress(hd, b"SbieApi_QueryProcess\0".as_ptr()));

        // 2. Idempotent reset, then become the session leader. Failure here
        //    is an environment problem (another leader is registered) — the
        //    command process must abort the whole eval with this reason.
        let mut off: u32 = 0;
        let _ = mc(&mut off, std::ptr::null_mut());
        let r = sl(0, std::ptr::null_mut());
        if r != 0 {
            w(&format!(
                "# ERROR SessionLeader=0x{:08X} (session leader occupied? SbieCtrl GUI running?)",
                r as u32
            ));
            ExitProcess(12);
        }

        let mut on: u32 = 1;
        let r = mc(&mut on, std::ptr::null_mut());
        if r != 0 {
            w(&format!("# ERROR MonitorControl(on)=0x{:08X}", r as u32));
            ExitProcess(12);
        }

        let boxw = wide(&box_name);

        // Event attribution. The monitor buffer is GLOBAL — events from every
        // box (e.g. the gateway's NemesisBox) and system processes all arrive
        // here. Three-way classification keeps eval-box events without losing
        // short-lived processes:
        //   "box:<name>"   pid is in the accumulated eval-box pid set. The set
        //                  only GROWS (a process seen in ANY enumeration round
        //                  stays) — a cmd that lives 1-2s spans 2-4 rounds, so
        //                  short-lived children are still attributed. Growth is
        //                  safe: pids are only added while they are verifiably
        //                  in the eval box, and pid reuse within one eval run
        //                  is not a thing Windows does on this timescale.
        //   "other"        pid verifiably belongs to ANOTHER box (QueryProcess
        //                  succeeded with a different box name) — dropped.
        //                  This kills the resident-process noise.
        //   "unattributed" pid already exited AND was never enumerated (died
        //                  between two 500ms rounds) — kept with this marker.
        //                  Better noisy than blind: a sub-second process that
        //                  probed outside the box is exactly what an eval
        //                  wants to see. The command process can filter these
        //                  further if desired.
        // Events without a pid (pid==0, e.g. some SYSCALL rows) are
        // "unattributed" too.
        let mut known_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let unattributed = "unattributed";
        let box_tag = format!("box:{box_name}");

        // Per-event attribution decision.
        let classify = |pid: u32, known: &std::collections::HashSet<u32>| -> Option<String> {
            if pid != 0 && known.contains(&pid) {
                return Some(box_tag.clone());
            }
            if pid != 0 {
                // Ask the driver which box this pid lives in. Success with a
                // different box name → verifiably not ours → drop.
                let (mut bn, mut inm, mut sid) = ([0u16; 34], [0u16; 96], [0u16; 96]);
                let mut ses = 0u32;
                let r = qp(pid as *mut c_void, bn.as_mut_ptr(), inm.as_mut_ptr(), sid.as_mut_ptr(), &mut ses);
                if r == 0 {
                    let b = from_wide(&bn);
                    if !b.is_empty() && b != box_name {
                        return None; // another box's process — noise
                    }
                    if b == box_name {
                        return Some(box_tag.clone()); // late discovery (missed by a round)
                    }
                    // Empty box name: pid outside every box (unsandboxed host
                    // process) — cannot be our eval agent; drop.
                    return None;
                }
                // Query failed: process gone, never enumerated → keep.
                return Some(unattributed.to_string());
            }
            Some(unattributed.to_string()) // pid==0 rows
        };

        // 3.5 Initial process snapshot (informational; the eval box has no
        //     resident baseline — empty count is the completion signal).
        let (mut pids, mut cnt) = ([0u32; 512], 512u32);
        let _ = en(boxw.as_ptr(), 0, u32::MAX, pids.as_mut_ptr(), &mut cnt);
        for i in 0..(cnt as usize).min(512) {
            known_pids.insert(pids[i]);
        }
        w(&format!("# initial process count={cnt}"));

        // 4. Main loop: collect events until the box is empty, then a short
        //    tail window for stragglers, then off + exit.
        let mut total_events: u64 = 0;
        let mut dropped_events: u64 = 0;
        let mut empty_since: Option<u64> = None; // first tick where the box was empty
        loop {
            // -- drain the monitor buffer --
            loop {
                let (mut t, mut pid, mut tid) = (0u32, 0u32, 0u32);
                let mut name = [0u16; 256];
                let r = mg(&mut t, &mut pid, &mut tid, name.as_mut_ptr());
                if r != 0 {
                    break; // NO_MORE_ENTRIES or error — drain done for now
                }
                let ns = from_wide(&name);
                let base = t & MONITOR_TYPE_MASK;
                let open = t & 0x0001_0000 != 0;
                let deny = t & 0x0002_0000 != 0;
                match classify(pid, &known_pids) {
                    Some(owner) => {
                        let ts = GetTickCount64();
                        w(&format!(
                            "{{\"ts\":{},\"type\":\"{}\",\"pid\":{},\"tid\":{},\"open\":{},\"deny\":{},\"box\":\"{}\",\"name\":\"{}\"}}",
                            ts,
                            type_label(base),
                            pid,
                            tid,
                            open,
                            deny,
                            owner,
                            json_escape(&ns)
                        ));
                        total_events += 1;
                    }
                    None => {
                        dropped_events += 1;
                    }
                }
                if total_events % 500 == 0 {
                    // Snapshot progress into the comment channel.
                    let (mut p, mut c) = ([0u32; 512], 512u32);
                    let _ = en(boxw.as_ptr(), 0, u32::MAX, p.as_mut_ptr(), &mut c);
                    let mut imgs = Vec::new();
                    for i in 0..(c as usize).min(4) {
                        let (mut bn, mut inm, mut sid) = ([0u16; 34], [0u16; 96], [0u16; 96]);
                        let mut ses = 0u32;
                        if qp(p[i] as *mut c_void, bn.as_mut_ptr(), inm.as_mut_ptr(), sid.as_mut_ptr(), &mut ses) == 0 {
                            imgs.push(format!("{}:{}", p[i], from_wide(&inm)));
                        }
                    }
                    w(&format!("# progress events={total_events} dropped={dropped_events} procs={c} [{}]", imgs.join(", ")));
                }
            }

            // -- enumerate the box --
            let (mut p, mut c) = ([0u32; 512], 512u32);
            let _ = en(boxw.as_ptr(), 0, u32::MAX, p.as_mut_ptr(), &mut c);
            let now = GetTickCount64();
            // Refresh the accumulated pid set: membership in this round's
            // enumeration is the driver's own answer to "is this pid in the
            // eval box", so every new pid is verifiably ours.
            for i in 0..(c as usize).min(512) {
                known_pids.insert(p[i]);
            }

            if c == 0 {
                match empty_since {
                    None => empty_since = Some(now), // start the tail window
                    Some(t0) => {
                        if now - t0 > 2000 {
                            // Box empty for >2s: collect done.
                            break;
                        }
                    }
                }
            } else {
                empty_since = None; // something is alive again — keep watching
            }

            Sleep(500);
        }

        let mut off2: u32 = 0;
        let _ = mc(&mut off2, std::ptr::null_mut());
        w(&format!("# done total_events={total_events} dropped={dropped_events} known_pids={}", known_pids.len()));
        ExitProcess(0);
    }
}
