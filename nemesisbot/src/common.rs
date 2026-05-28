//! Shared helpers for CLI commands.
//!
//! Provides path resolution, version info, logger initialization,
//! interactive mode, directory copy, and other common utilities.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

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
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(".nemesisbot");
    }
    // Priority 2: NEMESISBOT_HOME env var
    if let Ok(home) = std::env::var("NEMESISBOT_HOME") {
        return PathBuf::from(home).join(".nemesisbot");
    }
    // Priority 3: Exe directory
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if exe_dir.join(".nemesisbot").exists() {
                return exe_dir.join(".nemesisbot");
            }
        }
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
    home.join("workspace").join("config").join("config.mcp.json")
}

/// Get the scanner config file path.
pub fn scanner_config_path(home: &Path) -> PathBuf {
    home.join("workspace").join("config").join("config.scanner.json")
}

/// Get the security config file path.
pub fn security_config_path(home: &Path) -> PathBuf {
    home.join("workspace").join("config").join("config.security.json")
}

/// Get the skills config file path.
pub fn skills_config_path(home: &Path) -> PathBuf {
    home.join("workspace").join("config").join("config.skills.json")
}

/// Get the cluster config file path.
pub fn cluster_config_path(home: &Path) -> PathBuf {
    home.join("workspace").join("config").join("config.cluster.json")
}

/// Get the enhanced memory config file path.
pub fn enhanced_memory_config_path(home: &Path) -> PathBuf {
    home.join("workspace").join("config").join("config.enhanced_memory.json")
}

/// Get the chat config file path.
pub fn chat_config_path(home: &Path) -> PathBuf {
    home.join("workspace").join("config").join("config.chat.json")
}

/// Get the CORS config file path.
pub fn cors_config_path(home: &Path) -> PathBuf {
    home.join("config").join("cors.json")
}

/// Get the cluster data directory path (`{home}/workspace/cluster/`).
///
/// This is where `peers.toml` and `state.toml` live, matching Go's
/// `workspace/cluster/` layout (NOT `home/cluster/`).
pub fn cluster_dir(home: &Path) -> PathBuf {
    workspace_path(home).join("cluster")
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
pub fn status_icon(ok: bool) -> &'static str {
    if ok { "OK" } else { "MISSING" }
}

/// Constant-time comparison to prevent timing attacks.
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
        format!("{}...{}", &token[..4], &token[token.len() - 4..])
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
    println!("nemesisbot - Personal AI Assistant v{}", VERSION_INFO.version);
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
pub fn init_logger_from_config(
    config_path: &Path,
    check_args: &[String],
) -> u32 {
    let mut level = tracing::Level::INFO;
    let mut enable_console = true;
    let mut file_path: Option<String> = None;

    // Read from config file if it exists
    if config_path.exists() {
        if let Ok(data) = std::fs::read_to_string(config_path) {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(logging) = cfg.get("logging").and_then(|v| v.get("general")) {
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
                    // File path
                    if let Some(fp) = logging.get("file").and_then(|v| v.as_str()) {
                        if !fp.is_empty() {
                            file_path = Some(fp.to_string());
                        }
                    }
                }
            }
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

    // Build the appropriate writer
    let writer = if !enable_console {
        // No console: if file path is set, write to file only (not stderr)
        match &file_path {
            Some(fp) => {
                match nemesis_logger::DualMakeWriter::file_only(fp) {
                    Ok(mw) => {
                        eprintln!("[Logger] Logging to file only: {}", fp);
                        mw
                    }
                    Err(e) => {
                        eprintln!("[Logger] Warning: failed to open log file '{}': {}", fp, e);
                        // Fallback: discard all output (no console, no file)
                        nemesis_logger::DualMakeWriter::console_only()
                    }
                }
            }
            None => {
                // No console and no file — discard all output
                nemesis_logger::DualMakeWriter::console_only()
            }
        }
    } else if let Some(fp) = &file_path {
        // Console + file
        match nemesis_logger::DualMakeWriter::with_file(fp) {
            Ok(mw) => {
                eprintln!("[Logger] Logging to console + file: {}", fp);
                mw
            }
            Err(e) => {
                eprintln!("[Logger] Warning: failed to open log file '{}': {}", fp, e);
                nemesis_logger::DualMakeWriter::console_only()
            }
        }
    } else {
        // Console only
        nemesis_logger::DualMakeWriter::console_only()
    };

    if enable_console {
        if tracing_subscriber::fmt()
            .event_format(nemesis_logger::GoStyleFormatter)
            .with_max_level(level)
            .with_writer(writer)
            .try_init()
            .is_err()
        {
            // Global subscriber already set (e.g. by a previous call or default logger).
            // This is non-fatal: the existing subscriber will handle logs at whatever
            // level it was configured with.
            eprintln!("[Logger] Warning: global subscriber already set, config-based init skipped");
        }
    } else {
        // No console mode: init with the dual writer (file only or discard)
        let _ = tracing_subscriber::fmt()
            .event_format(nemesis_logger::GoStyleFormatter)
            .with_max_level(level)
            .with_writer(writer)
            .try_init();
    }

    override_flags
}

// =========================================================================
// Interactive mode
// =========================================================================

/// Run a simple interactive CLI chat loop.
///
/// Reads lines from stdin, calls the provided handler for each input,
/// and prints the response. Type "exit" or "quit" to stop.
#[allow(dead_code)]
pub fn run_interactive_mode<F>(prompt: &str, handler: F) -> anyhow::Result<()>
where
    F: Fn(&str) -> anyhow::Result<String>,
{
    println!("Interactive mode. Type 'exit' or 'quit' to stop.");
    println!();

    let stdin = io::stdin();
    loop {
        print!("{}", prompt);
        io::stdout().flush()?;

        let mut line = String::new();
        let bytes_read = stdin.read_line(&mut line)?;
        if bytes_read == 0 {
            // EOF
            println!("Goodbye!");
            return Ok(());
        }

        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" {
            println!("Goodbye!");
            return Ok(());
        }

        match handler(input) {
            Ok(response) => println!("\n{}\n", response),
            Err(e) => eprintln!("Error: {}\n", e),
        }
    }
}

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
