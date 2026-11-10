//! Windows 控制台：UTF-8 代码页 + GUI 子系统的父控制台回接。
//!
//! Rust `println!` 输出 UTF-8 字节，而中文 Windows 经典控制台（conhost）按
//! OEM 代码页（GBK/936）解码 → 中文全部打成乱码（2026-11-10 用户实录；
//! 仓库此前零处 SetConsoleOutputCP）。启动时切 65001 一劳永逸；Linux/macOS
//! 终端原生 UTF-8，no-op。
//!
//! nbsetup-win 是 GUI 子系统（windows_subsystem = "windows"）——双击零
//! 参数不配控制台（黑框框诉求，2026-11-10 用户实录）；带参数入口
//! （--silent/--uninstall/--help）从控制台启动时 AttachConsole 回接父
//! 控制台，stdout/stderr 照常可见。重定向形态（管道/`> file`）不需要
//! attach——句柄经 STARTUPINFO 直传，GUI 子系统进程照常可写。

/// 把当前控制台输入/输出代码页切到 UTF-8（65001）。非 Windows = no-op。
pub fn set_utf8_codepage() {
    #[cfg(windows)]
    {
        // SAFETY：SetConsoleOutputCP/SetConsoleCP 是无失败面的简单 setter
        //（进程未附着控制台时静默无效，不影响 GUI 重定向场景）。
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
            windows_sys::Win32::System::Console::SetConsoleCP(65001);
        }
    }
    #[cfg(not(windows))]
    {
        // 非 Windows 终端原生 UTF-8，无需处理。
    }
}

/// 回接父控制台（GUI 子系统的命令行入口用）。ATTACH_PARENT_PROCESS =
/// (DWORD)-1；父无控制台（双击/资源管理器启动）或已附加（控制台子系统
/// 的 debug 构建）= 调用失败，静默忽略。非 Windows = no-op。
pub fn attach_parent_console() {
    #[cfg(windows)]
    {
        // SAFETY：AttachConsole 只影响本进程的控制台附着态；失败
        //（ERROR_ACCESS_DENIED = 已附加 / ERROR_INVALID_HANDLE = 父无
        // 控制台）静默忽略，不产生任何副作用。
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
        }
    }
    #[cfg(not(windows))]
    {
        // 非 Unix 无 GUI/控制台子系统之分，无需处理。
    }
}
