//! Sandbox command — manage the Sandboxie driver + service install.
//!
//! L2.0 scope: `install` / `uninstall` / `status`. Does NOT touch the executor
//! yet (that's L2.1+: named-pipe transport + Start.exe spawn). See
//! `docs/PLAN/2026-07-09_sandboxie-integration.md`.
//!
//! Install/uninstall need admin (KmdUtil opens SC_MANAGER_CREATE_SERVICE). The
//! non-elevated flow re-launches self elevated via ShellExecuteW("runas") with
//! a hidden `--internal` flag; the elevated child runs KmdUtil synchronously;
//! the parent polls `service_state` to confirm the side effect (relaunch is
//! fire-and-forget — no exit code).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use crate::common;
use nemesis_sandbox::status::ServiceState;

#[derive(Subcommand, Debug)]
pub enum SandboxCommand {
    /// Download + extract the Sandboxie runtime files (no admin / no UAC — just files).
    /// Use `start` to activate the engine (install driver + service).
    Install,
    /// Deactivate the engine: stop + uninstall the driver + service (needs admin → UAC).
    /// --purge also deletes the acquired files (full removal).
    Stop {
        #[arg(long, hide = true)]
        internal: bool,
        #[arg(long)]
        purge: bool,
    },
    /// Show Sandboxie install / service status.
    Status,
    /// List pending workspace files in the box (written by the sandboxed executor,
    /// not yet committed to real disk).
    Pending,
    /// Commit pending files from the box to the real workspace.
    Commit {
        /// Commit ALL pending workspace files.
        #[arg(long)]
        all: bool,
        /// Commit only files whose real path contains one of these (case-insensitive).
        /// Ignored when --all is set.
        files: Vec<String>,
    },
    /// Delete the box's contents (discard pending). Asks before discarding if
    /// there are pending workspace files; --force skips the prompt.
    Clear {
        #[arg(long)]
        force: bool,
    },
    /// Force-terminate ALL processes inside a box and discard its contents —
    /// the engine (driver + SbieSvc) keeps running and other boxes are
    /// untouched. Use when an eval / executor run goes rogue.
    /// No args = all NemesisEvalBox_* boxes (eval leftovers);
    /// --box-name NAME for a specific box (e.g. NemesisBox, the executor box).
    Kill {
        /// Kill this specific box instead of all eval boxes.
        #[arg(long)]
        box_name: Option<String>,
    },
    /// Activate the engine: install driver + service + write ini + start SbieSvc.
    /// Needs admin (kernel driver) → triggers UAC. Requires `install` (files) first.
    Start {
        #[arg(long, hide = true)]
        internal: bool,
    },
    /// Internal-only (hidden): tolerant make-ready used by gateway startup.
    /// Users use `start`/`stop` instead.
    #[command(hide = true)]
    EnsureReady {
        #[arg(long, hide = true)]
        internal: bool,
        #[arg(long, hide = true)]
        home: Option<String>,
    },
    /// Internal-only (hidden): one-shot userland-sandbox self-test child
    /// (G7 D2). Spawned by the gateway's `sandbox.self_test` WSAPI command —
    /// either wrapped by a WrapCommand backend (bwrap/Seatbelt, env
    /// `NEMESISBOT_SELFTEST_BOXED=1`) or applying a SelfApply backend itself
    /// (landlock, env `NEMESISBOT_SELFTEST_ENGAGE=1`). Runs the probes and
    /// prints a single-line JSON verdict to stdout. NEVER call interactively:
    /// the landlock path applies irreversible rules to THIS process.
    #[command(hide = true)]
    SelftestChild,
}

pub async fn run(action: SandboxCommand, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let paths = nemesis_sandbox::SandboxPaths::new(&home);
    match action {
        SandboxCommand::Install => install(&paths).await,
        SandboxCommand::Stop { internal, purge } => stop(&paths, local, internal, purge),
        SandboxCommand::Status => status(&paths),
        SandboxCommand::Pending => pending(&paths, local),
        SandboxCommand::Commit { all, files } => commit(&paths, local, all, files),
        SandboxCommand::Clear { force } => clear(&paths, local, force),
        SandboxCommand::Kill { box_name } => kill(&paths, box_name),
        SandboxCommand::Start { internal } => start(&paths, local, internal),
        SandboxCommand::EnsureReady { internal, home } => {
            let h = home
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| common::resolve_home(local));
            ensure_ready(&h, internal)
        }
        SandboxCommand::SelftestChild => selftest_child(),
    }
}

// ---------------------------------------------------------------------------
// G7 (D2)：用户态沙盒自检子进程（隐藏子命令，gateway `sandbox.self_test` 专用）
// ---------------------------------------------------------------------------

/// One-shot probe child. Env contract (all set by the parent):
/// - `NEMESISBOT_SELFTEST_WORKSPACE` — workspace path (probes run against it).
/// - `NEMESISBOT_SELFTEST_ENGAGE=1` — apply a SelfApply backend (landlock) to
///   THIS process before probing (irreversible; why this must be a child).
/// - `NEMESISBOT_SELFTEST_BOXED=1` — the parent already wrapped this process
///   with a WrapCommand backend (bwrap/Seatbelt); skip engagement.
/// - `NEMESISBOT_SELFTEST_ALLOW_NETWORK=0|1` — sandbox conf network switch.
///
/// Always exits 0 with a JSON verdict on stdout; engagement failure is a
/// verdict (`ok:false`), not a crash — the parent parses stdout, not exit codes.
fn selftest_child() -> Result<()> {
    let workspace = match std::env::var_os("NEMESISBOT_SELFTEST_WORKSPACE") {
        Some(w) => std::path::PathBuf::from(w),
        None => {
            nemesis_sandbox::selftest::emit(&nemesis_sandbox::selftest::SelftestChildOut {
                ok: false,
                error: Some("NEMESISBOT_SELFTEST_WORKSPACE not set".to_string()),
                checks: vec![],
            });
            return Ok(());
        }
    };
    let engage = std::env::var("NEMESISBOT_SELFTEST_ENGAGE").as_deref() == Ok("1");
    let boxed = std::env::var("NEMESISBOT_SELFTEST_BOXED").as_deref() == Ok("1");

    if engage && !boxed {
        // SelfApply path: landlock rules are irreversible — this is exactly
        // why the probe runs in a one-shot child, never in the gateway.
        use nemesis_sandbox::backend::{SandboxConf, detect_backend};
        let allow_network =
            std::env::var("NEMESISBOT_SELFTEST_ALLOW_NETWORK").as_deref() == Ok("1");
        let conf = SandboxConf {
            writable_roots: vec![workspace.clone()],
            read_exec_roots: vec![std::path::PathBuf::from("/")],
            allow_network,
            label: "selftest".to_string(),
        };
        match detect_backend() {
            Some(backend) => {
                // Full/Partial both fine — the probes tell the truth either way.
                if let Err(e) = backend.apply_to_self(&conf) {
                    nemesis_sandbox::selftest::emit(&nemesis_sandbox::selftest::SelftestChildOut {
                        ok: false,
                        error: Some(format!("apply_to_self({}): {e}", backend.name())),
                        checks: vec![],
                    });
                    return Ok(());
                }
            }
            None => {
                // Backend vanished between parent probe and child spawn — report.
                nemesis_sandbox::selftest::emit(&nemesis_sandbox::selftest::SelftestChildOut {
                    ok: false,
                    error: Some("no userland backend in child (parent saw SelfApply)".to_string()),
                    checks: vec![],
                });
                return Ok(());
            }
        }
    }

    let checks = nemesis_sandbox::selftest::run_probes(&workspace);
    nemesis_sandbox::selftest::emit(&nemesis_sandbox::selftest::SelftestChildOut {
        ok: true,
        error: None,
        checks,
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// pending / commit / clear — manual workspace-commit (L2.3)
// ---------------------------------------------------------------------------

/// %USERPROFILE% — the box's `user/<marker>/` subtree maps here.
fn user_profile() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir)
        .expect("USERPROFILE / home dir")
}

/// The workspace whose subtree is committable (matches what the gateway uses).
fn workspace_dir(local: bool) -> std::path::PathBuf {
    common::resolve_home(local).join("workspace")
}

fn format_size(n: u64) -> String {
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{}K", n / 1024)
    } else {
        format!("{}M", n / (1024 * 1024))
    }
}

fn pending(paths: &nemesis_sandbox::SandboxPaths, local: bool) -> Result<()> {
    let ws = workspace_dir(local);
    let up = user_profile();
    let pending = nemesis_sandbox::pending::pending_workspace(&paths.box_root, &ws, &up)?;
    if pending.is_empty() {
        println!(
            "No pending workspace files in box {}.",
            paths.box_root.display()
        );
        return Ok(());
    }
    println!("Pending workspace files ({}):", pending.len());
    for p in &pending {
        let rel = p.real_path.strip_prefix(&ws).unwrap_or(&p.real_path);
        println!("  {:>8}  {}", format_size(p.size), rel.display());
    }
    println!("\nCommit with: nemesisbot sandbox commit --all");
    Ok(())
}

fn commit(
    paths: &nemesis_sandbox::SandboxPaths,
    local: bool,
    all: bool,
    files: Vec<String>,
) -> Result<()> {
    let ws = workspace_dir(local);
    let up = user_profile();
    let pending = nemesis_sandbox::pending::pending_workspace(&paths.box_root, &ws, &up)?;
    if pending.is_empty() {
        println!("No pending workspace files to commit.");
        return Ok(());
    }
    let to_commit: Vec<&nemesis_sandbox::pending::PendingFile> = if all {
        pending.iter().collect()
    } else {
        let needles: Vec<String> = files.iter().map(|s| s.to_lowercase()).collect();
        pending
            .iter()
            .filter(|p| {
                let rp = p.real_path.to_string_lossy().to_lowercase();
                needles.iter().any(|n| rp.contains(n))
            })
            .collect()
    };
    if to_commit.is_empty() {
        println!("No pending files matched. Use --all or check `nemesisbot sandbox pending`.");
        return Ok(());
    }
    let mut total = 0u64;
    let mut ok = 0usize;
    for p in &to_commit {
        match nemesis_sandbox::pending::commit_file(p) {
            Err(e) => println!("  FAILED {}: {e}", p.real_path.display()),
            Ok(n) => {
                total += n;
                ok += 1;
                println!("  committed {} ({} bytes)", p.real_path.display(), n);
            }
        }
    }
    println!(
        "Committed {ok}/{} file(s), {} bytes.",
        to_commit.len(),
        total
    );
    Ok(())
}

fn clear(paths: &nemesis_sandbox::SandboxPaths, local: bool, force: bool) -> Result<()> {
    use std::io::Write as _;

    let ws = workspace_dir(local);
    let up = user_profile();
    let pending = nemesis_sandbox::pending::pending_workspace(&paths.box_root, &ws, &up)?;
    if !pending.is_empty() && !force {
        println!(
            "{} pending workspace file(s) will be LOST when the box is cleared.",
            pending.len()
        );
        print!(
            "Commit all before clearing? [y=commit+clear / n=clear-without-commit / a=abort] (default a): "
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        match line.trim().to_lowercase().as_str() {
            "y" => {
                let mut n = 0usize;
                for p in &pending {
                    if nemesis_sandbox::pending::commit_file(p).is_ok() {
                        n += 1;
                    }
                }
                println!("Committed {n}/{} before clearing.", pending.len());
            }
            "n" => { /* clear without commit */ }
            _ => {
                println!("Aborted — box not cleared.");
                return Ok(());
            }
        }
    }
    println!("Clearing box contents...");
    nemesis_sandbox::pending::delete_box_contents(
        &paths.start_exe(),
        nemesis_sandbox::DEFAULT_BOX_NAME,
    )?;
    println!("Box cleared.");
    Ok(())
}

/// Force-terminate every process inside the box(es) and discard box contents.
/// The engine (driver + SbieSvc) keeps running; other boxes untouched.
///
/// Call pattern is deliberately the one VERIFIED in eval.rs's clean_box since
/// 2026-08-16 (delete_sandbox_silent + null stdio + box-root-exists guard —
/// a missing root makes Start.exe pop a "Code 3" dialog). terminate_all runs
/// first so processes die even if the content delete later fails.
fn kill(paths: &nemesis_sandbox::SandboxPaths, box_name: Option<String>) -> Result<()> {
    let start_exe = paths.start_exe();
    if !start_exe.exists() {
        bail!(
            "Start.exe not found at {} — run `nemesisbot sandbox install` first",
            start_exe.display()
        );
    }

    // Box-section existence guard: calling Start.exe on a box name that has
    // no ini section HANGS (verified: terminate_all on a nonexistent box
    // blocks indefinitely waiting on SbieSvc). Validate against the ini first.
    let ini_sections: std::collections::HashSet<String> = {
        let ini = std::fs::read_to_string(&paths.ini_path)
            .context("read Sandboxie.ini to enumerate box sections")?;
        ini.lines()
            .filter_map(|l| {
                let t = l.trim();
                t.strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .map(String::from)
            })
            .collect()
    };

    let boxes: Vec<String> = match &box_name {
        Some(b) => {
            if !ini_sections.contains(b) {
                println!(
                    "Box '{b}' has no section in {} — nothing to kill.",
                    paths.ini_path.display()
                );
                return Ok(());
            }
            vec![b.clone()]
        }
        None => {
            // All eval boxes: sections named NemesisEvalBox_* in the real ini.
            ini_sections
                .iter()
                .filter(|s| s.starts_with("NemesisEvalBox_"))
                .cloned()
                .collect()
        }
    };
    if boxes.is_empty() {
        println!(
            "No NemesisEvalBox_* sections in ini — nothing to kill. (Use --box <name> for a specific box.)"
        );
        return Ok(());
    }

    for b in &boxes {
        // Box-root existence check（eval clean_box 同款守卫，2026-08-16 实证）：
        // 对 FileRootPath 已不存在的死段调 Start.exe 会挂死/弹 Code 3 对话框。
        // 死段（历史残留）没有活进程可杀——直接跳过 Start.exe，留给 eval 的
        // ini restore / strip 清理。
        let file_root = box_file_root(&paths.ini_path, b);
        if let Some(root) = &file_root {
            if !root.exists() {
                println!(
                    "[sandbox] box {b}: FileRootPath {} 已不存在（历史残留段，无活进程）——跳过 Start.exe，段由下次 eval 还原清理",
                    root.display()
                );
                continue;
            }
        } else {
            println!("[sandbox] box {b}: ini 段缺 FileRootPath——跳过 Start.exe");
            continue;
        }
        println!(
            "[sandbox] killing box {b}: /terminate + delete_sandbox_silent (engine stays up)..."
        );
        // 命令语法（官方 StartCommandLine 文档，2026-08-19 查证）：
        // - 停盒内进程是【开关形式】"/terminate"——裸写 "terminate_all" 会被
        //   Start.exe 当成要启动的程序名 → "找不到指定程序"弹窗串（实錯：
        //   20 个盒 × 1 弹窗，用户屏幕实报）。
        // - delete_sandbox_silent 是位置命令形式（不带斜杠），二者语法不同。
        // 快失败：3s 无响应即树杀并跳过（防个别盒拖垮整轮 kill）。
        let term_ok = run_startexe_timeout(
            &start_exe,
            b,
            "/terminate",
            std::time::Duration::from_secs(3),
        );
        if term_ok {
            run_startexe_timeout(
                &start_exe,
                b,
                "delete_sandbox_silent",
                std::time::Duration::from_secs(10),
            );
            println!("[sandbox] box {b} killed (processes terminated, contents discarded).");
        } else {
            println!(
                "[sandbox] box {b}: /terminate 无响应。已树杀 Start.exe 并跳过。清理正途：重启 SbieSvc（nemesisbot sandbox stop + start）"
            );
        }
    }
    println!(
        "\nDone. Engine (SbieSvc/driver) still running — other boxes (e.g. NemesisBox) untouched."
    );
    Ok(())
}

/// 从 ini 段里读 `FileRootPath=` 的值（盒虚拟根路径）。段缺失该 key → None。
fn box_file_root(ini_path: &std::path::Path, section: &str) -> Option<std::path::PathBuf> {
    let ini = std::fs::read_to_string(ini_path).ok()?;
    let mut in_section = false;
    for line in ini.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t.trim_start_matches('[').trim_end_matches(']') == section;
            continue;
        }
        if in_section && let Some(v) = t.strip_prefix("FileRootPath=") {
            // 形如 `\??\C:\...`——剥掉 NT 前缀
            let p = v.trim().trim_start_matches(r"\??\");
            return Some(std::path::PathBuf::from(p));
        }
    }
    None
}

/// spawn Start.exe 子命令（silent stdio 防弹窗）+ 轮询超时。返回是否在超时
/// 内完成（true=正常结束，false=超时已树杀）。超时树杀（taskkill /T 连孙
/// 进程）+ WARN——一个坏盒不能卡死整轮 kill。
///
/// 命令语法注意（官方 StartCommandLine 文档，2026-08-19 查证）：
/// - 开关类（/terminate /reload /listpids）带斜杠；裸写会被当【要启动的
///   程序名】→ "找不到指定程序"弹窗串（实测翻车根源）
/// - 位置命令类（delete_sandbox_silent 等）不带斜杠
/// - **/silent 消除 Start.exe 自身的弹窗错误框**——所有调用都带上
///
/// 孤儿防护：Job Object kill-on-close——本进程被外杀时内核自动终止 job
/// 内全部 Start.exe，杜绝孤儿继续弹窗。
fn run_startexe_timeout(
    start_exe: &std::path::Path,
    box_name: &str,
    sub: &str,
    timeout: std::time::Duration,
) -> bool {
    let mut child = match std::process::Command::new(start_exe)
        .arg(format!("/box:{box_name}"))
        .arg("/silent")
        .arg(sub)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[sandbox] WARN: spawn Start.exe {sub} for {box_name} failed: {e}");
            return false;
        }
    };
    // Job Object：把子进程挂进"父死子亡"的 job（一次创建，进程退出时内核
    // 自动杀掉 job 里所有进程——孤儿从根上不可能存在）。Windows 专属（Job
    // Object API）；非 Windows 上 Start.exe 本就不存在，spawn 早退，无需 job。
    #[cfg(target_os = "windows")]
    let _job = ChildJob::assign(&child);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true, // finished (any exit code — best effort)
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // 树杀：Start.exe 可能有孙进程，kill() 只杀直接子。
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &child.id().to_string(), "/T", "/F"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                    let _ = child.kill();
                    let _ = child.wait();
                    eprintln!(
                        "[sandbox] WARN: Start.exe {sub} for box {box_name} 无响应（坏盒）——树杀后跳过",
                    );
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(_) => return false,
        }
    }
}

/// Windows Job Object（kill-on-close）：assign 后进程退出时内核自动终止
/// job 内所有进程——防 Start.exe 孤儿（父被外杀后继续弹窗）。
/// Windows 专属（windows-sys 是 target 门控依赖 + Job Object API）。
#[cfg(target_os = "windows")]
struct ChildJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl ChildJob {
    fn assign(child: &std::process::Child) -> Option<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return None;
            }
            let h = match child.as_raw_handle() {
                h if h.is_null() => {
                    let _ = windows_sys::Win32::Foundation::CloseHandle(job);
                    return None;
                }
                h => h as windows_sys::Win32::Foundation::HANDLE,
            };
            if AssignProcessToJobObject(job, h) == 0 {
                // 子进程已退出或句柄无效——job 由 Drop 关闭（空 job 无害）
                let _ = windows_sys::Win32::Foundation::CloseHandle(job);
                return None;
            }
            Some(Self(job))
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for ChildJob {
    fn drop(&mut self) {
        unsafe {
            // 关闭 job 句柄：因 KILL_ON_JOB_CLOSE，本进程退出/句柄关闭时
            // job 内残留子进程被内核强制终止。
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
use std::os::windows::io::AsRawHandle as _;

/// Activate the engine: install driver + service + ini + start SbieSvc. Needs
/// admin (kernel driver) → UAC self-relaunch. Requires files acquired (`install`).
fn start(paths: &nemesis_sandbox::SandboxPaths, local: bool, internal: bool) -> Result<()> {
    if internal {
        println!("[sandbox] activating engine (elevated child)...");
        nemesis_sandbox::install::start(paths)?;
        println!("[sandbox] engine activated — SbieSvc RUNNING.");
        return Ok(());
    }
    if !nemesis_sandbox::elevation::is_elevated() {
        println!("[sandbox] not elevated — requesting UAC...");
        let exe = std::env::current_exe()?;
        nemesis_sandbox::elevation::relaunch_elevated(&exe, &relaunch_args("start", local))?;
        println!("[sandbox] elevated activator launched; waiting for SbieSvc (up to 120s)...");
        let state = nemesis_sandbox::install::wait_for_state(
            nemesis_sandbox::USERMODE_SERVICE,
            ServiceState::Running,
            Duration::from_secs(120),
        );
        if !matches!(state, ServiceState::Running) {
            anyhow::bail!(
                "activate did not complete (SbieSvc state={state:?}); check the UAC prompt"
            );
        }
        println!("[sandbox] engine activated — SbieSvc RUNNING.");
        return Ok(());
    }
    println!("[sandbox] activating engine (already elevated)...");
    nemesis_sandbox::install::start(paths)?;
    println!("[sandbox] engine activated — SbieSvc RUNNING.");
    Ok(())
}

/// Build the argv for the elevated self-relaunch (`sandbox <subcmd> --internal [--local]`).
fn relaunch_args(subcmd: &str, local: bool) -> Vec<String> {
    let mut v = vec![
        "sandbox".to_string(),
        subcmd.to_string(),
        "--internal".to_string(),
    ];
    if local {
        v.push("--local".to_string());
    }
    v
}

async fn install(paths: &nemesis_sandbox::SandboxPaths) -> Result<()> {
    // Acquire files only (download + extract). No driver/service/ini, no UAC.
    println!("[sandbox] acquiring Sandboxie files (download + extract, no UAC)...");
    nemesis_sandbox::install::install(paths).await?;
    println!(
        "[sandbox] files acquired at {}.\nRun `nemesisbot sandbox start` (or the dashboard 启动 button) to activate the engine (installs driver, triggers UAC).",
        paths.runtime_dir.display()
    );
    Ok(())
}

/// Deactivate the engine: stop + uninstall driver + service. --purge also removes
/// the acquired files. Needs admin → UAC self-relaunch.
fn stop(
    paths: &nemesis_sandbox::SandboxPaths,
    local: bool,
    internal: bool,
    purge: bool,
) -> Result<()> {
    if internal {
        println!(
            "[sandbox] deactivating engine (elevated child){}...",
            if purge { " + purging files" } else { "" }
        );
        nemesis_sandbox::install::stop(paths, purge)?;
        println!("[sandbox] engine deactivated.");
        return Ok(());
    }
    if !nemesis_sandbox::elevation::is_elevated() {
        println!("[sandbox] not elevated — requesting UAC...");
        let exe = std::env::current_exe()?;
        let mut args = relaunch_args("stop", local);
        if purge {
            args.push("--purge".to_string());
        }
        nemesis_sandbox::elevation::relaunch_elevated(&exe, &args)?;
        println!(
            "[sandbox] elevated deactivator launched; waiting for SbieSvc to disappear (up to 60s)..."
        );
        let state = nemesis_sandbox::install::wait_for_state(
            nemesis_sandbox::USERMODE_SERVICE,
            ServiceState::NotFound,
            Duration::from_secs(60),
        );
        println!("[sandbox] SbieSvc state after stop: {state:?}");
        return Ok(());
    }
    println!("[sandbox] deactivating engine (already elevated)...");
    nemesis_sandbox::install::stop(paths, purge)?;
    println!("[sandbox] engine deactivated.");
    Ok(())
}

/// Internal worker for `ensure_sandbox_ready` (gateway startup): tolerant full
/// install (registers whatever's missing + starts the service). Hidden CLI
/// subcommand; users use `sandbox start`/`stop` instead.
fn ensure_ready(home: &std::path::Path, internal: bool) -> Result<()> {
    let paths = nemesis_sandbox::SandboxPaths::new(home);
    if internal {
        nemesis_sandbox::install::ensure_installed(&paths)?;
        println!("[sandbox] engine ensured installed + SbieSvc running");
        return Ok(());
    }
    if !nemesis_sandbox::elevation::is_elevated() {
        println!("[sandbox] not elevated — requesting UAC for ensure-ready...");
        let exe = std::env::current_exe()?;
        let home_str = home.to_string_lossy().to_string();
        let args = vec![
            "sandbox".to_string(),
            "ensure-ready".to_string(),
            "--internal".to_string(),
            "--home".to_string(),
            home_str,
        ];
        nemesis_sandbox::elevation::relaunch_elevated(&exe, &args)?;
        println!("[sandbox] elevated ensure-ready launched; waiting for SbieSvc (up to 120s)...");
        let state = nemesis_sandbox::install::wait_for_state(
            nemesis_sandbox::USERMODE_SERVICE,
            ServiceState::Running,
            Duration::from_secs(120),
        );
        println!("[sandbox] SbieSvc state after ensure-ready: {state:?}");
        return Ok(());
    }
    nemesis_sandbox::install::ensure_installed(&paths)?;
    println!("[sandbox] engine ensured installed + SbieSvc running");
    Ok(())
}

/// Gateway 启动时按"沙盒启动决策表"确保引擎就绪（8 行全覆盖）。驱动/服务是两个
/// 独立组件，分别检测；正常关停只停服务、驱动常驻（路线 A，避 UAC+BSOD）。
///
/// | 开关 | 工具 | 驱动 | 服务 | 动作 |
/// |---|---|---|---|---|
/// | 关 | * | * | * | 不管 |
/// | 开 | 未安装 | * | * | 不管 |
/// | 开 | 已安装 | 未安装/已安装(任一NotFound) | (任一NotFound) | 提权 ensure-installed（UAC，fire-and-forget）|
/// | 开 | 已安装 | 已安装 | 停止 | 起服务（不弹 UAC）|
/// | 开 | 已安装 | 已安装 | 执行 | 复用 |
///
/// Rows 3–6（驱动或服务没注册）统一走提权 `ensure-ready` 子进程（tolerant 全量装，
/// 自动适配任一半装态），fire-and-forget 不卡 gateway——box 就绪后下次启动复用。
/// Row 7（注册了但服务停了）直接起服务，无 UAC。Row 8 复用。
pub fn ensure_sandbox_ready(home: &std::path::Path, sandbox_enabled: bool) {
    // Row 1: 开关关 → 不管
    if !sandbox_enabled {
        return;
    }
    let paths = nemesis_sandbox::SandboxPaths::new(home);

    // Row 2: 工具(文件)未安装 → 不管（没文件，后面都没意义）
    if paths.verify_runtime().is_err() {
        tracing::info!(
            "[sandbox] executor.sandbox is on but runtime files are missing — \
             run `nemesisbot sandbox install` first; skipping engine ensure"
        );
        return;
    }

    // 归属门：若已注册的 SbieDrv/SbieSvc 不是我们的（别人的系统级 Sandboxie），
    // 不碰、也不装（名字冲突装不了自己的）→ 降级（无 box）。
    if !nemesis_sandbox::status::engine_owned(&paths) {
        tracing::warn!(
            "[sandbox] a foreign Sandboxie is registered (SbieDrv/SbieSvc binary not under {}) — \
             not touching it and can't install ours (name conflict); degrading to no-box",
            paths.runtime_dir.display()
        );
        return;
    }

    let drv = nemesis_sandbox::status::service_state(nemesis_sandbox::DRIVER_SERVICE);
    let svc = nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE);
    let drv_installed = !matches!(drv, ServiceState::NotFound);
    let svc_registered = !matches!(svc, ServiceState::NotFound);

    // Rows 3–6: 驱动或服务没注册 → ensure-installed（tolerant 全量装）。
    // 已提权 → 直接跑（不弹 UAC）；否则 relaunch 提权子进程（fire-and-forget，不卡 gateway）。
    if !drv_installed || !svc_registered {
        if nemesis_sandbox::elevation::is_elevated() {
            tracing::info!(
                "[sandbox] engine not fully installed (drv={drv:?}, svc={svc:?}); \
                 running ensure-installed directly (already elevated)"
            );
            if let Err(e) = nemesis_sandbox::install::ensure_installed(&paths) {
                tracing::warn!("[sandbox] ensure-installed failed: {e}");
            }
            return;
        }
        tracing::info!(
            "[sandbox] engine not fully installed (drv={drv:?}, svc={svc:?}); \
             triggering elevated ensure-ready (UAC prompt, fire-and-forget)"
        );
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("[sandbox] current_exe failed: {e}");
                return;
            }
        };
        let home_str = home.to_string_lossy().to_string();
        let args = vec![
            "sandbox".to_string(),
            "ensure-ready".to_string(),
            "--internal".to_string(),
            "--home".to_string(),
            home_str,
        ];
        if let Err(e) = nemesis_sandbox::elevation::relaunch_elevated(&exe, &args) {
            tracing::warn!(
                "[sandbox] ensure-ready elevation failed: {e} \
                 (run `nemesisbot sandbox start` manually)"
            );
        }
        return;
    }

    // 到这里：驱动 + 服务都注册了
    // Row 7: 服务停止 → 起服务（不弹 UAC）; Row 8: 服务执行 → 复用
    if !matches!(svc, ServiceState::Running) {
        tracing::info!(
            "[sandbox] SbieSvc registered but stopped (state={svc:?}); \
             starting service (no UAC)"
        );
        if let Err(e) = nemesis_sandbox::install::start_service(&paths) {
            tracing::warn!(
                "[sandbox] start_service failed: {e} \
                 (the service may require elevation on this system; \
                 if the box is not applied, run `nemesisbot sandbox start`)"
            );
        }
    } else {
        tracing::info!("[sandbox] engine ready (SbieDrv + SbieSvc running) — reusing, no UAC");
    }
}

/// 停掉**我们自己的** SbieSvc 服务（per-run）。驱动**不动**（常驻，避 UAC + BSOD
/// 风险），只有 `sandbox stop` 显式卸载。只有「binary 路径在我们 runtime 目录下」
/// 的服务才停——别人的系统级 Sandboxie 不碰。
///
/// ⚠️ **当前未在 gateway 退出路径调用**（`gateway.rs` 里的调用已注释禁用）。
/// 原因：网关非提权，停特权 SbieSvc 经 `KmdUtil.exe stop` 会被拒 + 弹 GUI 权限窗，
/// 阻塞 bot 退出。保留此函数与「is_ours」判定逻辑，供将来走**提权路径**停服务时
/// 复用。如需恢复退出时停服务，必须改成提权执行（见 `elevation.rs`），否则会
/// 重新弹窗。
#[allow(dead_code)]
pub fn stop_service_if_ours(home: &std::path::Path) {
    let svc = nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE);
    if !matches!(svc, ServiceState::Running) {
        return; // 没在跑，无需停（驱动照常常驻）
    }
    let paths = nemesis_sandbox::SandboxPaths::new(home);
    let is_ours = nemesis_sandbox::status::service_binary_path(nemesis_sandbox::USERMODE_SERVICE)
        .map(|bin| {
            let runtime = paths.runtime_dir.to_string_lossy().to_lowercase();
            bin.to_lowercase().contains(&runtime)
        })
        .unwrap_or(false);
    if !is_ours {
        tracing::info!(
            "[sandbox] SbieSvc running but not ours (binary not under {}) — leaving it",
            paths.runtime_dir.display()
        );
        return;
    }
    tracing::info!("[sandbox] stopping our SbieSvc (driver stays resident, no UAC)");
    if let Err(e) = nemesis_sandbox::install::stop_service(&paths) {
        tracing::warn!("[sandbox] stop_service failed: {e}");
    }
}

fn status(paths: &nemesis_sandbox::SandboxPaths) -> Result<()> {
    let sbiesvc = nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE);
    let sbiedrv = nemesis_sandbox::status::service_state(nemesis_sandbox::DRIVER_SERVICE);
    let start_exe = paths.start_exe();
    let ready = matches!(sbiesvc, ServiceState::Running) && start_exe.exists();

    println!("Sandboxie status");
    println!("  SbieSvc (service): {sbiesvc:?}");
    println!("  SbieDrv (driver):  {sbiedrv:?}");
    println!(
        "  Start.exe:         {} [{}]",
        start_exe.display(),
        if start_exe.exists() {
            "present"
        } else {
            "MISSING"
        }
    );
    println!(
        "  Sandboxie.ini:     {} [{}]",
        paths.ini_path.display(),
        if paths.ini_path.exists() {
            "present"
        } else {
            "absent"
        }
    );
    println!(
        "  runtime dir:       {} [{}]",
        paths.runtime_dir.display(),
        if paths.runtime_dir.exists() {
            "present"
        } else {
            "absent"
        }
    );
    println!("  sandbox ready:     {ready}");
    Ok(())
}

#[cfg(test)]
mod tests;
