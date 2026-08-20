//! Step 4 verification: launch the monitor DLL inside the signed SbieCtrl
//! host, generate in-box activity, and check the JSONL output + exit code.
//! The eval box is created on the fly via SbieIni (env for the DLL points at
//! the eval box; if the section does not exist the box count stays 0 and the
//! DLL exits after the 2s tail — that also verifies the completion logic).

#[cfg(windows)]
fn main() {
    use nemesis_injector::{launch_and_inject_with_env, wait_and_get_exit, close_handles};
    use std::process::Command;

    let sbiectrl = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\SbieCtrl.exe";
    let dll = concat!(env!("CARGO_MANIFEST_DIR"), r"\..\..\plugins\plugin-eval-monitor\target\release\eval_monitor_dll.dll");
    let events = concat!(env!("CARGO_MANIFEST_DIR"), r"\monitor_events.jsonl");
    let sbiedll = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\SbieDll.dll";
    let start_exe = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\Start.exe";
    let box_root = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\box\NemesisEvalBox";

    // Prepare the eval box section (test env; cleaned up by the caller).
    let sbieini = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\SbieIni.exe";
    let _ = Command::new(sbieini).args(["set", "NemesisEvalBox", "Enabled", "y"]).output();
    let _ = Command::new(sbieini).args(["set", "NemesisEvalBox", "FileRootPath", &format!(r"\??\{box_root}")]).output();
    let _ = Command::new(sbieini).args(["set", "NemesisEvalBox", "DropAdminRights", "y"]).output();

    println!("[test] events path = {events}");
    let env_cfg: Vec<(&str, &str)> = vec![
        ("NEMESISBOT_EVAL_BOX", "NemesisEvalBox"),
        ("NEMESISBOT_EVAL_EVENTS_FILE", events),
        ("NEMESISBOT_EVAL_SBIEDLL", sbiedll),
        ("NEMESISBOT_EVAL_TIMEOUT_SECS", "120"),
    ];
    let (hp, ht) = match launch_and_inject_with_env(sbiectrl, dll, 0, &env_cfg) {
        Ok(h) => h,
        Err(e) => { eprintln!("inject failed: {e}"); std::process::exit(1); }
    };
    println!("[test] monitor shell launched");

    // Give the DLL a moment to become leader + start monitor, then spawn an
    // in-box process that does some file activity and exits.
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = Command::new(start_exe)
        .args(["/box:NemesisEvalBox", "/hide_window", "/wait", r"C:\Windows\System32\cmd.exe", "/c", &format!("echo hi > {}\\boxwrite.txt & dir C:\\ > NUL", env!("CARGO_MANIFEST_DIR").replace('/', r"\"))])
        .output();
    println!("[test] in-box activity done");

    // SAFETY: hp/ht 直接来自 launch_and_inject_with_env 的成功返回，未并发
    // 关闭、各只关一次——满足两个 unsafe 函数的契约。
    let code = unsafe { wait_and_get_exit(hp) };
    unsafe { close_handles(hp, ht) };
    println!("[test] monitor shell exited code={code:?}");

    let out = std::fs::read_to_string(events).unwrap_or_default();
    let event_lines = out.lines().filter(|l| l.starts_with('{')).count();
    println!("[test] events file: {} event lines, {} total lines", event_lines, out.lines().count());
    println!("---- head ----");
    for l in out.lines().take(8) { println!("{l}"); }

    let ok = code == Some(0) && event_lines > 0;
    println!("[test] RESULT: {}", if ok { "PASS" } else { "CHECK" });
}

#[cfg(not(windows))]
fn main() {}
