//! Shared helpers for CLI commands.
//!
//! Provides path resolution, version info, logger initialization,
//! directory copy, and other common utilities.

use std::path::{Path, PathBuf};
use tracing_subscriber::prelude::*;

/// Ensure the directory containing the current executable is in PATH.
///
/// When users launch nemesisbot from a different working directory,
/// the shell tools invoked by the LLM cannot find `nemesisbot.exe`.
/// This function adds the exe's parent to the process PATH if missing.
///
/// Returns `true` if PATH was modified, `false` if already present.
pub fn ensure_exe_in_path() -> bool {
    let exe_dir = match std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        Some(d) => d,
        None => return false,
    };

    let canonical_exe_dir = match std::fs::canonicalize(&exe_dir) {
        Ok(c) => c,
        Err(_) => exe_dir,
    };

    let path_var = match std::env::var("PATH") {
        Ok(v) => v,
        Err(_) => {
            // No PATH at all — just set it
            // SAFETY: This runs during gateway startup, single-threaded init phase.
            // No other threads are reading or writing the PATH environment variable.
            unsafe { std::env::set_var("PATH", &canonical_exe_dir) };
            return true;
        }
    };

    let separator = if cfg!(windows) { ';' } else { ':' };

    for entry in path_var.split(separator) {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let canonical_entry = std::fs::canonicalize(trimmed);
        if canonical_entry.as_ref().ok() == Some(&canonical_exe_dir) {
            return false;
        }
        // Fallback: direct comparison (canonicalize may fail for missing dirs)
        if Path::new(trimmed) == canonical_exe_dir {
            return false;
        }
    }

    let new_path = if path_var.is_empty() {
        canonical_exe_dir.to_string_lossy().to_string()
    } else {
        format!("{}{}{}", path_var, separator, canonical_exe_dir.display())
    };
    // SAFETY: This runs during gateway startup, single-threaded init phase.
    // No other threads are reading or writing the PATH environment variable.
    unsafe { std::env::set_var("PATH", &new_path) };
    true
}

/// Resolve the NemesisBot home directory.
///
/// Priority:
/// 1. `--local` flag → `{cwd}/.nemesisbot`
/// 2. `NEMESISBOT_HOME` env → `{NEMESISBOT_HOME}/.nemesisbot`
/// 3. Auto-detect cwd → if `{cwd}/.nemesisbot` exists
/// 4. Exe directory → if `{exe_dir}/.nemesisbot` exists
/// 5. Default → `~/.nemesisbot`
pub fn resolve_home(local: bool) -> PathBuf {
    // Priority 1: --local flag
    if local {
        return std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".nemesisbot");
    }
    // Priority 2: NEMESISBOT_HOME env var
    if let Ok(home) = std::env::var("NEMESISBOT_HOME") {
        return PathBuf::from(home).join(".nemesisbot");
    }
    // Priority 3: Exe directory
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
        && exe_dir.join(".nemesisbot").exists()
    {
        return exe_dir.join(".nemesisbot");
    }
    // Priority 4: Auto-detect cwd
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join(".nemesisbot").exists() {
        return cwd.join(".nemesisbot");
    }
    // Priority 5: Default ~/.nemesisbot
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nemesisbot")
}

/// Get the config file path.
pub fn config_path(home: &Path) -> PathBuf {
    home.join("config.json")
}

/// Get the workspace directory path.
pub fn workspace_path(home: &Path) -> PathBuf {
    home.join("workspace")
}

/// Get the MCP config file path.
pub fn mcp_config_path(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点（CLI / web / forge 三方共用）。
    nemesis_path::resolve_mcp_config_path_in_workspace(&workspace_path(home))
}

/// Get the scanner config file path.
pub fn scanner_config_path(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_scanner_config_path_in_workspace(&workspace_path(home))
}

/// Get the security config file path.
pub fn security_config_path(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_security_config_path_in_workspace(&workspace_path(home))
}

/// Get the skills config file path.
pub fn skills_config_path(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_skills_config_path_in_workspace(&workspace_path(home))
}

/// Get the cluster config file path.
pub fn cluster_config_path(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点（CLI / web / cluster 三方共用）。
    nemesis_path::resolve_cluster_config_path_in_workspace(&workspace_path(home))
}

/// Get the enhanced memory config file path.
pub fn enhanced_memory_config_path(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_enhanced_memory_config_path_in_workspace(&workspace_path(home))
}

/// Get the chat config file path.
pub fn chat_config_path(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_chat_config_path_in_workspace(&workspace_path(home))
}

/// Get the Forge self-learning config file path.
pub fn forge_config_path(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_forge_config_path_in_workspace(&workspace_path(home))
}

/// Get the CORS config file path.
///
/// 2026-08-29 收编：落位 `<workspace>/config/cors.json`（nemesis-path 真相源）。
/// 一次性迁移 legacy `<home>/config/cors.json`（copy-once：新位已存在则不动，
/// legacy 保留——与 hooks.json 迁移同款先例）。
pub fn cors_config_path(home: &Path) -> PathBuf {
    let new_path = nemesis_path::resolve_cors_config_path_in_workspace(&workspace_path(home));
    let legacy = home.join("config").join("cors.json");
    if legacy.is_file() && !new_path.exists() {
        if let Some(parent) = new_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(&legacy, &new_path);
    }
    new_path
}

/// Get the cluster data directory path (`{home}/workspace/cluster/`).
///
/// This is where `peers.toml` and `state.toml` live, matching Go's
/// `workspace/cluster/` layout (NOT `home/cluster/`).
pub fn cluster_dir(home: &Path) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点（集群身份/拓扑/续行快照根目录）。
    nemesis_path::cluster_dir_in_workspace(&workspace_path(home))
}

/// Get the cron store path.
pub fn cron_store_path(home: &Path) -> PathBuf {
    home.join("workspace").join("cron").join("jobs.json")
}

/// Get the sessions directory path (`{home}/workspace/sessions/`).
pub fn sessions_dir(home: &Path) -> PathBuf {
    home.join("workspace").join("sessions")
}

/// Print a check mark or cross.
// Only used by the forge + memory status commands; exclude from builds where
// both are off (e.g. minimal-iot). `, test` keeps it available for its unit
// tests regardless of feature组合 — mirrors `constant_time_eq` below.
#[cfg(any(feature = "forge", feature = "memory", test))]
pub fn status_icon(ok: bool) -> &'static str {
    if ok { "OK" } else { "MISSING" }
}

/// Constant-time comparison to prevent timing attacks.
#[cfg(any(feature = "cluster", test))]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

/// Format a token for display (show first/last 4 chars).
pub fn format_token(token: &str) -> String {
    if token.is_empty() {
        return "(not set)".to_string();
    }
    if token.len() > 8 {
        let end = nemesis_types::utils::floor_char_boundary(token, 4);
        let start = nemesis_types::utils::ceil_char_boundary(token, token.len() - 4);
        format!("{}...{}", &token[..end], &token[start..])
    } else {
        "***".to_string()
    }
}

/// Version info filled in by build script environment variables.
pub struct VersionInfo {
    pub version: &'static str,
    pub git_commit: &'static str,
    pub build_time: &'static str,
    pub rust_version: &'static str,
}

pub static VERSION_INFO: VersionInfo = VersionInfo {
    version: env!("CARGO_PKG_VERSION"),
    git_commit: env!("NEMESISBOT_GIT_COMMIT"),
    build_time: env!("NEMESISBOT_BUILD_TIME"),
    rust_version: env!("NEMESISBOT_RUSTC_VERSION"),
};

/// Format the version string with optional git commit.
pub fn format_version() -> String {
    let mut v = VERSION_INFO.version.to_string();
    if !VERSION_INFO.git_commit.is_empty() {
        v = format!("{} (git: {})", v, VERSION_INFO.git_commit);
    }
    v
}

/// Print version (matching Go's PrintVersion output).
#[allow(dead_code)]
pub fn print_version() {
    println!("nemesisbot {}", format_version());
    if !VERSION_INFO.build_time.is_empty() {
        println!("  Build: {}", VERSION_INFO.build_time);
    }
    if !VERSION_INFO.rust_version.is_empty() {
        println!("  Rust: {}", VERSION_INFO.rust_version);
    }
}

/// Print full version and build info (matching Go's PrintVersionInfo).
pub fn print_version_info() {
    println!("nemesisbot {}", format_version());
    if !VERSION_INFO.build_time.is_empty() {
        println!("  Build: {}", VERSION_INFO.build_time);
    }
    if !VERSION_INFO.rust_version.is_empty() {
        println!("  Rust: {}", VERSION_INFO.rust_version);
    }
}

/// Print the main help banner.
///
/// Mirrors Go's `PrintHelp()` with detailed descriptions and sections.
#[allow(dead_code)]
pub fn print_help() {
    println!(
        "nemesisbot - Personal AI Assistant v{}",
        VERSION_INFO.version
    );
    println!();
    println!("Usage: nemesisbot [OPTIONS] <COMMAND>");
    println!();
    println!("Commands:");
    println!("  onboard       Initialize nemesisbot configuration and workspace");
    println!("  agent         Interact with the agent directly");
    println!("  auth          Manage authentication (login, logout, status)");
    println!("  gateway       Start nemesisbot gateway");
    println!("  status        Show nemesisbot status");
    println!("  channel       Manage communication channels (list, enable, disable, status)");
    println!("  cluster       Manage bot cluster (status, config, enable, disable)");
    println!("  cors          Manage CORS configuration (list, add, remove, validate)");
    println!("  model         Manage LLM models (list, add, remove)");
    println!("  cron          Manage scheduled tasks");
    println!("  mcp           Manage MCP servers (list, add, remove, test)");
    println!("  security      Manage security settings (enable, disable, status, config, audit)");
    println!("  log           Manage LLM request logging");
    println!("  migrate       Migrate from OpenClaw to NemesisBot");
    println!("  skills        Manage skills (install, list, remove)");
    println!("  forge         Manage self-learning module (status, reflect, list, evaluate)");
    println!("  workflow      Manage DAG workflows (list, run, status, template)");
    println!("  scanner       Manage virus scanner (enable, check, install)");
    println!("  shutdown      Graceful shutdown");
    println!("  daemon        Run as a background daemon");
    println!("  version       Show version information");
    println!();
    println!("Options:");
    println!("      --local   Use .nemesisbot in current directory");
    println!("  -h, --help    Show help");
    println!("  -V, --version Show version");
    println!();
    println!("Quick Start:");
    println!("  nemesisbot onboard default          # Out-of-box setup (recommended)");
    println!("  nemesisbot onboard default --local  # Out-of-box setup, config in current dir");
    println!("  nemesisbot onboard                  # Step-by-step guided setup");
    println!("  nemesisbot onboard --local          # Step-by-step setup, config in current dir");
    println!();
    println!("  nemesisbot model add --model zhipu/glm-4.7 --key YOUR_KEY --default");
    println!("  nemesisbot gateway                  # Start service");
    println!();
    println!("Scanner Setup:");
    println!("  nemesisbot security scanner enable clamav    # Enable ClamAV engine");
    println!("  nemesisbot security scanner check            # Check installation status");
    println!("  nemesisbot security scanner install          # Download install + virus database");
    println!("  nemesisbot gateway                           # Scanner engines auto-load on start");
    println!();
    println!("Docs: https://github.com/276793422/NemesisBot");
}

// =========================================================================
// Logger initialization
// =========================================================================

/// Initialize a default console logger for simple CLI commands.
///
/// Uses `std::sync::OnceLock` to ensure the subscriber is only installed once.
/// Commands like gateway/agent/daemon call `init_logger_from_config()` instead,
/// which reads the logging section from config.json. This function is for all
/// other commands (status, model, cron, etc.) that just need basic console output.
pub fn ensure_default_logger() {
    use std::sync::OnceLock;

    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let _ = tracing_subscriber::fmt()
            .event_format(nemesis_logger::GoStyleFormatter)
            .with_max_level(tracing::Level::INFO)
            .with_writer(std::io::stderr)
            .try_init();
    });
}

/// Bitmask flags returned by `init_logger_from_config`.
pub const LOG_DEBUG: u32 = 1;
pub const LOG_QUIET: u32 = 2;
pub const LOG_NO_CONSOLE: u32 = 4;

/// Initialize the logger based on configuration and CLI overrides.
///
/// Reads log configuration from the main config file, then applies
/// CLI argument overrides (`--debug`, `--quiet`, `--no-console`).
///
/// Returns a bitmask of what was overridden:
/// - bit 0 (`LOG_DEBUG`): `--debug` was used
/// - bit 1 (`LOG_QUIET`): `--quiet` was used
/// - bit 2 (`LOG_NO_CONSOLE`): `--no-console` was used
pub fn init_logger_from_config(config_path: &Path, check_args: &[String]) -> u32 {
    let mut level = tracing::Level::INFO;
    let mut enable_console = true;
    let mut file_path: Option<String> = None;

    // Read from config file if it exists
    if config_path.exists()
        && let Ok(data) = std::fs::read_to_string(config_path)
        && let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data)
        && let Some(logging) = cfg.get("logging").and_then(|v| v.get("general"))
    {
        // Console switch
        if let Some(console) = logging.get("enable_console").and_then(|v| v.as_bool()) {
            enable_console = console;
        }
        // Log level
        if let Some(lvl) = logging.get("level").and_then(|v| v.as_str()) {
            level = match lvl.to_uppercase().as_str() {
                "DEBUG" | "TRACE" => tracing::Level::DEBUG,
                "INFO" => tracing::Level::INFO,
                "WARN" | "WARNING" => tracing::Level::WARN,
                "ERROR" => tracing::Level::ERROR,
                _ => tracing::Level::INFO,
            };
        }
        // File path — resolve relative paths against the workspace directory
        // (config_path's parent is the home dir; workspace is home/workspace).
        // This keeps logs in `.nemesisbot/workspace/logs/` regardless of CWD.
        if let Some(fp) = logging.get("file").and_then(|v| v.as_str())
            && !fp.is_empty()
        {
            let p = std::path::Path::new(fp);
            let resolved = if p.is_absolute() {
                p.to_path_buf()
            } else {
                config_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("workspace")
                    .join(p)
            };
            file_path = Some(resolved.to_string_lossy().into_owned());
        }
    }

    // Check CLI argument overrides
    let mut override_flags: u32 = 0;

    for arg in check_args {
        match arg.as_str() {
            "--quiet" | "-q" => {
                // Completely disable logging
                override_flags |= LOG_QUIET;
            }
            "--no-console" => {
                enable_console = false;
                override_flags |= LOG_NO_CONSOLE;
            }
            "--debug" | "-d" => {
                level = tracing::Level::DEBUG;
                override_flags |= LOG_DEBUG;
            }
            _ => {}
        }
    }

    // Apply configuration
    if override_flags & LOG_QUIET != 0 {
        // Quiet mode: use no-op subscriber
        return override_flags;
    }

    // Build the layered subscriber.
    //
    // Architecture:
    //   - GlobalSseLogLayer: always on, forwards every event to the SSE EventHub callback
    //     (installed later by the gateway once the EventHub exists).
    //   - Console layer: stderr + GoStyleFormatter (human-readable text). Enabled when
    //     `enable_console` is true.
    //   - File layer: RollingFileAppender (daily rotation) + JsonLinesFormatter. Produces
    //     `nemesisbot.YYYY-MM-DD.log` files where each line is a JSON-serialized SseLogEvent,
    //     byte-identical to what SSE pushes to the dashboard. This is what enables history
    //     loading + seq-based dedup.
    //
    // Layers are collected into a Vec<Box<dyn Layer<Registry>>> so the if/else combinations
    // above don't produce incompatible types.
    use tracing_subscriber::Registry;
    use tracing_subscriber::layer::Layer;

    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync + 'static>> =
        vec![Box::new(nemesis_logger::GlobalSseLogLayer)];

    if enable_console {
        layers.push(Box::new(
            tracing_subscriber::fmt::layer()
                .event_format(nemesis_logger::GoStyleFormatter)
                .with_writer(std::io::stderr),
        ));
    }

    if let Some(fp) = &file_path {
        let path = std::path::Path::new(fp);
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        // tracing-appender 0.2 only supports (rotation, dir, prefix) — no suffix.
        // Use the file stem as prefix; resulting files are `{stem}.YYYY-MM-DD` (no `.log`
        // extension). Dashboard matches via regex `^{stem}\.\d{4}-\d{2}-\d{2}$` to avoid
        // colliding with any legacy unrotated file.
        let prefix = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("nemesisbot");

        // (BUG #30, quality-hardening goal 冲刺 S11) 根因：tracing-appender
        // 0.2.5 的 RollingFileAppender::new 内部对日志目录创建失败直接
        // .expect panic（rolling.rs:156）——原先 create_dir_all 失败只 warn
        // 想继续，随后却无条件构造 appender，进程照样 panic（warn-and-
        // continue 意图落空，gateway 启动即崩）。修复：目录创建失败时跳过
        // 文件层，降级为 console-only（文件日志只是增强，不应致命）。
        match std::fs::create_dir_all(dir) {
            Err(e) => {
                eprintln!(
                    "[Logger] Warning: failed to create log directory '{}': {} — file logging disabled, console only",
                    dir.display(),
                    e
                );
            }
            Ok(()) => {
                eprintln!(
                    "[Logger] Logging to file (daily rotation): {}/{}<YYYY-MM-DD>",
                    dir.display(),
                    prefix
                );

                let appender = nemesis_logger::RollingFileAppender::new(
                    nemesis_logger::Rotation::DAILY,
                    dir,
                    prefix,
                );

                layers.push(Box::new(
                    tracing_subscriber::fmt::layer()
                        .event_format(nemesis_logger::JsonLinesFormatter)
                        .with_writer(appender),
                ));
            }
        }
    }

    let layered = tracing_subscriber::registry()
        .with(layers)
        .with(max_level_filter(level));
    if tracing::subscriber::set_global_default(layered).is_err() {
        // Global subscriber already set (e.g. by a previous call or default logger).
        // This is non-fatal: the existing subscriber will handle logs at whatever
        // level it was configured with.
        eprintln!("[Logger] Warning: global subscriber already set, config-based init skipped");
    }

    override_flags
}

/// Map a `tracing::Level` to its `LevelFilter` equivalent for layered subscriber setup.
fn max_level_filter(level: tracing::Level) -> tracing_subscriber::filter::LevelFilter {
    use tracing_subscriber::filter::LevelFilter;
    match level {
        tracing::Level::TRACE => LevelFilter::TRACE,
        tracing::Level::DEBUG => LevelFilter::DEBUG,
        tracing::Level::INFO => LevelFilter::INFO,
        tracing::Level::WARN => LevelFilter::WARN,
        tracing::Level::ERROR => LevelFilter::ERROR,
    }
}

// =========================================================================
// Interactive mode
// =========================================================================
// [2026-08-27 R9 死码处置·已删除] run_interactive_mode（原 554-593 行）：
// 全仓库零生产调用方（#[allow(dead_code)] 工具函数，grep 实证仅自身定义），
// 且 stdin 循环在进程内测试无法注入。按用户 2026-08-27 裁决删除——
// 恢复方式：git 历史本提交前的版本。

// =========================================================================
// Directory copy
// =========================================================================

/// Copy a directory recursively from `src` to `dst`.
#[allow(dead_code)]
pub fn copy_directory(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Source directory not found: {}", src.display()),
        ));
    }

    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_directory(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

/// Check if BOOTSTRAP.md exists in the workspace (skip heartbeat).
#[allow(dead_code)]
pub fn should_skip_heartbeat_for_bootstrap(workspace: &Path) -> bool {
    workspace.join("BOOTSTRAP.md").exists()
}

// =========================================================================
// Cron tool setup
// =========================================================================

/// Set up the cron tool, creating a CronService and CronTool, wiring them
/// together with the onJob handler.
///
/// Returns (CronService wrapped in Arc<Mutex>, CronTool) ready to be
/// registered with the agent loop. The caller is responsible for:
/// 1. Registering the CronTool with the AgentLoop
/// 2. Injecting the CronService into BotService
///
/// Mirrors Go's `SetupCronTool()`.
#[allow(dead_code)]
pub fn setup_cron_tool(
    _workspace: &Path,
) -> (
    std::sync::Arc<tokio::sync::Mutex<nemesis_tools::cron::CronService>>,
    nemesis_tools::cron::CronTool,
) {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let cron_service = Arc::new(Mutex::new(nemesis_tools::cron::CronService::new()));
    let cron_tool = nemesis_tools::cron::CronTool::new(Arc::clone(&cron_service));

    (cron_service, cron_tool)
}

#[cfg(test)]
mod tests;

// =========================================================================
// 双击直启（2026-09-17）：无窗重生
// =========================================================================

/// env 标记：本进程是无窗重生出来的子进程（Windows 双击检测后
/// respawn_detached_and_exit 设置）。子进程据此跳过二次检测。
pub const BARE_CHILD_ENV: &str = "NEMESISBOT_BARE_CHILD";

/// env 标记：双击直启（无参启动）语义——gateway 启动完成后自动打开
/// Dashboard（plugin-ui webview 窗口，缺 dll 回落浏览器）。显式
/// `nemesisbot gateway` 不带此标记（server 语义，不弹窗口）。
pub const BARE_LAUNCH_ENV: &str = "NEMESISBOT_BARE_LAUNCH";

/// Windows：本进程是否「独占一个新控制台」——双击 exe 的典型形态。
///
/// GetConsoleProcessList == 1：控制台上只挂着本进程（无父 shell）。
/// 从既有终端（cmd/PowerShell/WT）启动时父 shell 也在列表里（≥2）→
/// 不算双击。无控制台（重定向/已 DETACHED/服务）返回 0 → 不算。
#[cfg(target_os = "windows")]
pub fn console_is_solo_fresh() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;
    let mut buf = [0u32; 16];
    // SAFETY: 传有效缓冲区指针与容量（win32 API 契约）。
    let count = unsafe { GetConsoleProcessList(buf.as_mut_ptr(), 16) };
    count == 1
}

#[cfg(not(target_os = "windows"))]
pub fn console_is_solo_fresh() -> bool {
    false
}

/// Windows：双击直启的无窗重生——spawn 同参数 DETACHED_PROCESS 子进程
/// （stdio 全 NUL，防 println! 断句柄 panic；env [`BARE_CHILD_ENV`]=1），
/// 返回 true 时调用方立即 exit(0)，父控制台（双击弹出的黑窗）随本进程
/// 退出即灭（~100ms 闪烁是 console 子系统固有，无法避免）。
///
/// 重生失败（极端：exe 被移走/句柄耗尽）返回 false：保留当前控制台继续
/// 跑，比「双击无反应」好——用户至少看得到 gateway 日志。
///
/// 只在「无参/gateway 语义 + 独占新控制台 + 非 BARE_CHILD」时调用
/// （调用方判定，见 main.rs）；其他子命令保持控制台输出（短命 CLI
/// 操作，双击场景下用户需要看到输出）。
#[cfg(target_os = "windows")]
pub fn try_respawn_detached() -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const DETACHED_PROCESS: u32 = 0x0000_0008;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    // 用原始 argv（含 --local 等），子进程按同一套规则重新解析。
    let args: Vec<String> = std::env::args().skip(1).collect();

    match Command::new(&exe)
        .args(&args)
        .env(BARE_CHILD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS)
        .spawn()
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!(
                "[nemesisbot] detached respawn failed ({}): running in this console instead",
                e
            );
            false
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn try_respawn_detached() -> bool {
    false
}

/// P0 vault（B3，2026-09-22 计划）：秘密字段引用解析统一 helper。
///
/// vault:/env:/yaml: 前缀经 nemesis-config 全局解析器现查（vault 解析器
/// 由 vault_runtime 在启动路径注入）；字面量原样返回。解析失败 error!
/// （带字段名与根因）并返回空串——消费方按"凭据缺失"响亮失败，绝不把
/// 引用字符串本身当值用。
pub fn resolve_secret_or_empty(raw: &str, field: &str) -> String {
    match nemesis_config::resolve_secret_field(raw, field) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[vault] {field}: 引用解析失败: {e}");
            String::new()
        }
    }
}

/// 鉴权/验证类凭据解析（web.auth_token、websocket.auth_token、line
/// channel_secret 等**用于校验入站请求**的字段）。
///
/// 与 [`resolve_secret_or_empty`] 的差别在失败分支：这类字段空串 =
/// 校验直通（`verify_token` 对空期望值放行任何人）——解析失败若也回
/// 空串，一条坏掉的 `vault:` 引用就把鉴权静默关掉了（fail-open）。这里
/// 失败时回**随机一次性 token**：校验保持开启但无人能匹配（fail-closed），
/// error! 日志带补救指引，用户修复别名重启即恢复。
///
/// 出站类凭据（line.channel_access_token、搜索 api_key）继续用
/// [`resolve_secret_or_empty`]——空值只会让出站失败，天然 fail-closed。
pub fn resolve_auth_token_or_random(raw: &str, field: &str) -> String {
    match nemesis_config::resolve_secret_field(raw, field) {
        Ok(v) => v,
        Err(e) => {
            let throwaway = uuid::Uuid::new_v4().to_string();
            tracing::error!(
                "[vault] {field}: 引用解析失败——本次启动用一次性随机 token 保持鉴权开启\
                 （fail-closed，所有人都连不上）：{e}"
            );
            throwaway
        }
    }
}

// ---------------------------------------------------------------------------
// SEC-001：本机 → 远程可达模式的凭据引导状态机（隐式，无新增持久字段）
// ---------------------------------------------------------------------------

/// onboard 新装写入的固定初始 web 令牌（`commands/onboard.rs` 同源引用）。
/// 引导值集合 = { `""`（模板缺省，verify_token 对空期望放行所有人）, 本常量 }。
pub const BOOTSTRAP_WEB_TOKEN: &str = "276793422";

/// SEC-001 引导态凭据判定：web 控制面凭据仍是「引导值」时返回 true。
///
/// 隐式状态机的唯一判定点——判断基于 config 原始值：
/// - 空 / [`BOOTSTRAP_WEB_TOKEN`] → bootstrap（true）
/// - `vault:` / `env:` / `yaml:` 引用 → 已初始化的正式凭据（false）
/// - 其余任何非空值 = 用户已显式设置（false）
///
/// 不变量：**引导态凭据只允许守护回环控制面**（消费方见
/// [`ensure_control_plane_credential`]）。恢复旧配置 / 重复 onboard 后若
/// 原始值真是引导值，它就该被视为 bootstrap——语义正确而非缺陷；
/// 「设置新令牌并标记 initialized」由换掉引导值天然达成，无状态与值失步。
pub fn is_bootstrap_web_credential(raw: &str) -> bool {
    let v = raw.trim();
    v.is_empty() || v == BOOTSTRAP_WEB_TOKEN
}

/// SEC-001：绑定 host 是否只落回环。返回 `None` = 无法判定（主机名解析
/// 失败或零结果）——调用方放行：绑不上的地址不构成暴露面，让既有 bind
/// 流程自然报错。`0.0.0.0` / `::`（unspecified，绑所有网卡）按非回环处理。
pub fn bind_host_is_loopback(host: &str) -> Option<bool> {
    use std::net::ToSocketAddrs;
    let h = host.trim();
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return Some(ip.is_loopback());
    }
    let addrs: Vec<_> = match (h, 0u16).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => return None,
    };
    if addrs.is_empty() {
        return None;
    }
    Some(addrs.iter().all(|a| a.ip().is_loopback()))
}

/// SEC-001 网关启动闸（纯函数便于单测）：引导态凭据只允许守护回环控制面。
///
/// - 绑定 host 全回环 → Ok（bootstrap + 回环 = onboard 的合法初始形态）
/// - 绑定 host 非回环且凭据是引导值 → Err（附三条补救指引）
/// - 绑定 host 非回环且凭据已初始化 → Ok（集群/远程场景的正道）
/// - host 解析失败（[`bind_host_is_loopback`] 回 None）→ Ok
///
/// 消费点：gateway 正常启动路径对 `web_bind_and_display_hosts` 返回的
/// **绑定 host** 闸（统一覆盖 0.0.0.0/空/显式 LAN IP 三分支）。web 通道
/// （/ws）与 web server（/api）共用同一监听与同一原始 token 值，此处一闸
/// 双护。websocket 通道是独立监听面，单独过同一闸。`--relay` 纯中继豁免：
/// 不装配 /ws 与 /api/*，控制面闸无对象。
///
/// `field`：报错里如实指认来源配置键（如 `channels.web.auth_token` /
/// `channels.websocket.auth_token`）——两个闸共用本函数，文案不得硬编码
/// 通道名误导排障（真机验证 2026-09-23：websocket 闸触发时旧文案错指 web）。
pub fn ensure_control_plane_credential(
    bind_host: &str,
    raw_auth_token: &str,
    field: &str,
) -> Result<(), String> {
    if !is_bootstrap_web_credential(raw_auth_token) {
        return Ok(());
    }
    match bind_host_is_loopback(bind_host) {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "SEC-001 拒绝启动：{field} 所在控制面将绑定到非回环地址（{bind_host}），但\
             该 auth_token 仍是引导值（空或默认令牌）——同网段任何设备都能无凭据\
             操作本机 agent。三选一后重启：\n  \
             1) nemesisbot channel web auth-set <你的令牌>（web 通道）\n  \
             2) 手改 config.json 的 {field}\n  \
             3) 填入 vault:/env: 引用（如 vault:web_token）"
        )),
        None => Ok(()), // 解析失败不闸（见 bind_host_is_loopback）
    }
}
