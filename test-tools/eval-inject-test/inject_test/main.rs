//! 测试主程序：挂起启动 SbieCtrl.exe + 注入 test_dll.dll，验证签名墙绕过。
//! 结果 DLL 自己写到 inject_result.txt，本程序等进程退出后读出来打印。

#[cfg(windows)]
fn main() {
    use nemesis_injector::{launch_and_inject, wait_and_get_exit, close_handles};

    let target = r"C:\AI\NemesisBot\NemesisBot_Rust\bin\bin_windows\.nemesisbot\workspace\tools\sandboxie\runtime\SbieCtrl.exe";
    let dll = r"C:\AI\NemesisBot\NemesisBot_Rust\test-tools\eval-inject-test\test_dll\target\release\test_dll.dll";

    // 先清旧结果
    let _ = std::fs::remove_file(r"C:\AI\NemesisBot\NemesisBot_Rust\test-tools\eval-inject-test\test_dll\inject_result.txt");

    println!("[test] launching suspended {} + inject {}", target, dll);
    let (hp, ht) = match launch_and_inject(target, dll, 0) {
        Ok(h) => { println!("[test] inject installed, waiting for host to run Run()..."); h }
        Err(e) => { eprintln!("[test] inject failed: {e}"); std::process::exit(1); }
    };

    let code = wait_and_get_exit(hp);
    close_handles(hp, ht);
    println!("[test] host exited code={:?}", code);

    let log = r"C:\AI\NemesisBot\NemesisBot_Rust\test-tools\eval-inject-test\test_dll\inject_result.txt";
    match std::fs::read_to_string(log) {
        Ok(s) => { println!("===== DLL RESULT =====\n{s}\n======================"); }
        Err(e) => println!("[test] no result file (DLL Run() may not have run): {e}"),
    }
}

#[cfg(not(windows))]
fn main() { eprintln!("windows only"); }
