//! 系统文件管理器动作单一归宿：打开目录 / 定位选中文件 / 默认应用打开。
//!
//! 全仓「打开目录」此前 3 份重复实现（projects / skills / sandbox 各一份）
//! 加 fs 侧 reveal / open-file 两份私有——本模块收编为三个函数唯一归宿。
//! 消费方：fs.open / fs.reveal / projects.open_dir / board.project.open_dir
//! / system.open_home_dir / skills.open_dir / sandbox.open_box。
//!
//! **Windows 臂核心（后台进程开窗落后台的根修）**：gateway 是隐藏后台
//! 进程，spawn 出的资源管理器窗口受 Windows 前台激活锁约束，默认落在
//! 后台 z-order 用户看不见（`CREATE_NO_WINDOW` 只压 explorer 自身不留
//! 控制台窗口，不是元凶）。两段式：
//! 1. **窗口创建走 `explorer` spawn**（目录=开该目录窗；文件
//!    reveal=`/select,<路径>` 开父目录并选中）——这是窗口落地的确定性
//!    路径。曾试过 shell 规范 API `SHOpenFolderAndSelectItems`（COM），
//!    生产实测 S_OK 但窗口不落地（无前台权进程发起时 0/4，与线程存活
//!    无关；explorer spawn 同场景 1/1）——弃用。
//! 2. **前台看门线程**（~3s 有界轮询新出现的 `CabinetWClass` 窗口）+
//!    **激活组合拳**拉前台：后台进程无前台权，裸 `SwitchToThisWindow`
//!    被前台激活锁静默拒绝（生产实测：窗开了但不上去）——须
//!    `AttachThreadInput` 借当前前台线程输入队列后再
//!    `SetForegroundWindow`，仍不中再兜底 `SwitchToThisWindow`；预算
//!    半程后回落「标题含目标候选名」的复用窗（explorer 复用旧窗场景；
//!    reveal 场景窗口标题是父目录名，候选含父目录名）。找不到/超时
//!    静默放弃——尽力而为，不报错不阻塞。
//!
//! macOS/Linux 臂保持 `open` / `open -R` / `xdg-open` spawn（平台激活
//! 行为正确，无需看门）。spawn 均不经 shell（`Command::arg` 直传，路径
//! 含空格/特殊字符安全）；spawn 失败诚实返错。

use std::path::Path;

/// 用系统文件管理器打开目录（窗口显示该目录内容）。
pub(crate) fn open_dir(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        explorer_focus(
            path.as_os_str().to_string_lossy().into_owned(),
            vec![leaf_candidate(path)],
        )
    }
    #[cfg(target_os = "macos")]
    {
        spawn_detached(
            || std::process::Command::new("open").arg(path).spawn(),
            "打开目录失败",
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        spawn_detached(
            || std::process::Command::new("xdg-open").arg(path).spawn(),
            "打开目录失败",
        )
    }
}

/// 文件管理器定位文件（不打开、不高亮执行——非白名单扩展名的降级动作；
/// 右键「打开所在目录」语义）：Windows=开父目录并选中该文件；
/// macOS=`open -R`；Linux 无标准 reveal，打开父目录（诚实降级）。
pub(crate) fn reveal_file(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // reveal 场景 explorer 窗口标题是父目录名（文件叶名永不命中），
        // 看门候选 = 文件名 + 父目录名。
        let mut leaves = vec![leaf_candidate(path)];
        if let Some(n) = path.parent().and_then(|p| p.file_name()) {
            leaves.push(n.to_string_lossy().to_string());
        }
        explorer_focus(format!("/select,{}", path.display()), leaves)
    }
    #[cfg(target_os = "macos")]
    {
        spawn_detached(
            || {
                std::process::Command::new("open")
                    .arg("-R")
                    .arg(path)
                    .spawn()
            },
            "定位文件失败",
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let parent = path.parent().unwrap_or(path);
        spawn_detached(
            || std::process::Command::new("xdg-open").arg(parent).spawn(),
            "定位文件失败",
        )
    }
}

/// 默认应用打开文件（Windows=explorer 对文件路径走默认关联程序——
/// 绕开被禁的 `start` 系命令；macOS=`open`；Linux=`xdg-open`）。
/// 此动作无「开窗选中」诉求（关联程序自管窗口），保持裸 spawn 形态、
/// 无前台看门。
pub(crate) fn open_file_default(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        spawn_detached(
            || {
                std::process::Command::new("explorer")
                    .arg(path)
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
            },
            "打开文件失败",
        )
    }
    #[cfg(target_os = "macos")]
    {
        spawn_detached(
            || std::process::Command::new("open").arg(path).spawn(),
            "打开文件失败",
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        spawn_detached(
            || std::process::Command::new("xdg-open").arg(path).spawn(),
            "打开文件失败",
        )
    }
}

// Windows 臂：explorer spawn + 前台看门 --------------------------------------

#[cfg(target_os = "windows")]
fn leaf_candidate(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Windows 臂唯一机制（open/reveal 共用）：spawn `explorer` + 前台看门
/// 后台线程。窗口创建是 explorer 的异步工作（spawn 成功即诚实返回，
/// 不等窗口）；看门线程只负责把新窗拉前台，失败静默。
/// `arg`：传给 explorer 的单参数（目录=路径本身；reveal=`/select,<路径>`）。
#[cfg(target_os = "windows")]
fn explorer_focus(arg: String, leaves: Vec<String>) -> Result<(), String> {
    // 开窗前快照必须在 spawn 之前取（晚了会把新窗当旧窗，看门新窗臂失明）。
    let before = win::enumerate_cabinet_windows();
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("explorer")
        .arg(arg)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("打开失败（explorer spawn）: {e}"))?;
    let watcher = std::thread::Builder::new()
        .name("shell-open-focus".to_string())
        .spawn(move || win::bring_window_to_front(&before, &leaves));
    watcher.map_err(|e| format!("前台看门线程创建失败: {e}"))?;
    Ok(())
}

#[cfg(target_os = "windows")]
mod win {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, VK_MENU, keybd_event};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, HWND_TOP, IsIconic, MSG, PM_NOREMOVE, PeekMessageW, SW_RESTORE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos, ShowWindow,
        SwitchToThisWindow,
    };

    pub(super) const FRONT_WATCH_POLL: Duration = Duration::from_millis(100);
    pub(super) const FRONT_WATCH_BUDGET: Duration = Duration::from_millis(3000);

    /// 前台看门：轮询至预算耗尽——新出现 CabinetWClass 窗口优先（本次
    /// 开窗产物）；预算半程后回落「标题含目标候选名」的复用窗（explorer
    /// 复用旧窗场景）。命中即激活组合拳强拉前台；未到手继续轮询重试；
    /// 全程找不到 = 静默放弃（WARN 留痕）。
    pub(super) fn bring_window_to_front(before: &[(isize, String)], leaves: &[String]) {
        let deadline = Instant::now() + FRONT_WATCH_BUDGET;
        let fallback_at = Instant::now() + FRONT_WATCH_BUDGET / 2;
        tracing::info!(
            "shell_open 看门启动：before={} 候选={leaves:?}",
            before.len()
        );
        loop {
            std::thread::sleep(FRONT_WATCH_POLL);
            let now = enumerate_cabinet_windows();
            let allow_fallback = Instant::now() >= fallback_at;
            if let Some(hwnd) = pick_focus_target(before, &now, leaves, allow_fallback) {
                let rung = unsafe { force_foreground(hwnd as HWND) };
                let focused = unsafe { GetForegroundWindow() } == hwnd as HWND;
                tracing::info!("shell_open 看门命中 hwnd={hwnd} 阶梯={rung} focused={focused}");
                if focused {
                    return;
                }
            }
            if Instant::now() >= deadline {
                tracing::info!("shell_open 看门超时放弃：目标未见/未到前台");
                return;
            }
        }
    }

    /// 强制前台（四级阶梯，宿主是无前台权的隐藏后台进程——裸
    /// `SwitchToThisWindow`/`SetForegroundWindow` 被前台激活锁静默拒绝，
    /// 生产实测同一 API 序列时灵时不灵，Windows 对无近期用户输入的会话
    /// 收紧得很）：
    /// 1. `AttachThreadInput` 借当前前台窗口的输入队列获得激活权 →
    ///    `SetForegroundWindow`（最正路；**前置坑**：attach 要求调用线程
    ///    持有输入队列，裸 Rust 线程默认没有——先 `PeekMessage
    ///    (PM_NOREMOVE)` 强制创建，否则 attach 假成功整条哑火）；
    /// 2. ALT 键模拟（系统视为用户输入 → 放宽前台锁）后再激活；
    /// 3. 兜底 `SwitchToThisWindow`（AltTab 语义，偶发能过锁）；
    /// 4. **底线 `SetWindowPos(HWND_TOP)`**：无激活权也能把窗口置顶可见
    ///    （z-order 不受前台锁管）——用户诉求的底线是「看得见」，键盘
    ///    焦点丢给当前前台无碍。
    ///
    /// 全部尽力而为，失败不报错（调用方轮询重试直至预算耗尽）。
    unsafe fn force_foreground(hwnd: HWND) -> &'static str {
        unsafe {
            let mut msg = std::mem::zeroed::<MSG>();
            PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
            let cur_tid = GetCurrentThreadId();
            let fg = GetForegroundWindow();
            let fg_tid = if fg.is_null() {
                0
            } else {
                GetWindowThreadProcessId(fg, std::ptr::null_mut())
            };
            let attached =
                fg_tid != 0 && fg_tid != cur_tid && AttachThreadInput(cur_tid, fg_tid, 1) != 0;
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
            if attached {
                AttachThreadInput(cur_tid, fg_tid, 0);
            }
            // 激活是异步生效的——短候再判，别把在途激活误判失败后叠发兜底。
            std::thread::sleep(std::time::Duration::from_millis(60));
            if GetForegroundWindow() == hwnd {
                return "setfg";
            }
            // 兜底 1：ALT 键模拟 + 再激活。
            keybd_event(VK_MENU as u8, 0, 0, 0);
            keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
            SetForegroundWindow(hwnd);
            std::thread::sleep(std::time::Duration::from_millis(60));
            if GetForegroundWindow() == hwnd {
                return "alt_key";
            }
            // 兜底 2：AltTab 语义强切。
            SwitchToThisWindow(hwnd, 1);
            std::thread::sleep(std::time::Duration::from_millis(60));
            if GetForegroundWindow() == hwnd {
                return "switch_to";
            }
            // 底线：z-order 顶置（看得见，无键盘焦点）。
            SetWindowPos(
                hwnd,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            "zorder_top"
        }
    }

    /// 看门决策（纯函数——单测面）：新窗优先；`allow_fallback` 后允许
    /// 标题含任一目标候选名（叶名/父目录名）的已知窗（复用场景）。
    /// None = 继续等/放弃。
    pub(super) fn pick_focus_target(
        before: &[(isize, String)],
        now: &[(isize, String)],
        leaves: &[String],
        allow_fallback: bool,
    ) -> Option<isize> {
        let known: std::collections::HashSet<isize> = before.iter().map(|(h, _)| *h).collect();
        if let Some(&(h, _)) = now.iter().find(|(h, _)| !known.contains(h)) {
            return Some(h);
        }
        if allow_fallback
            && let Some(&(h, _)) = now
                .iter()
                .find(|(_, t)| leaves.iter().any(|l| !l.is_empty() && t.contains(l)))
        {
            return Some(h);
        }
        None
    }

    /// 当前全部顶层 `CabinetWClass` 窗口（explorer 文件夹窗）→ (hwnd, 标题)。
    /// 枚举失败 = 空表（看门静默放弃，不影响打开动作本身）。
    pub(super) fn enumerate_cabinet_windows() -> Vec<(isize, String)> {
        let mut out: Vec<(isize, String)> = Vec::new();
        unsafe {
            EnumWindows(Some(enum_cabinet_proc), (&mut out as *mut _) as LPARAM);
        }
        out
    }

    /// EnumWindows 回调：类名 CabinetWClass 的窗口收入列表（TRUE 继续）。
    unsafe extern "system" fn enum_cabinet_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            // lparam 即调用方传入的 Vec 指针（isize 形态），重建引用写入。
            let out = &mut *(lparam as *mut Vec<(isize, String)>);
            let mut cls = [0u16; 32];
            let n = GetClassNameW(hwnd, cls.as_mut_ptr(), 32);
            let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
            if cls == "CabinetWClass" {
                out.push((hwnd as isize, window_title(hwnd)));
            }
            1
        }
    }

    /// 窗口标题（UTF-16 读取；失败 = 空串）。
    fn window_title(hwnd: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        }
    }
}

// 共用 spawn（不等待；不经 shell——`Command::arg` 直传）-----------------------

fn spawn_detached(
    make: impl FnOnce() -> std::io::Result<std::process::Child>,
    err_prefix: &str,
) -> Result<(), String> {
    make().map(|_| ()).map_err(|e| format!("{err_prefix}: {e}"))
}

#[cfg(test)]
mod tests;
