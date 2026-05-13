//! Shared helpers for CLI commands.
//!
//! Provides path resolution, version info, logger initialization,
//! interactive mode, directory copy, and other common utilities.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Resolve the NemesisBot home directory.
///
/// Priority: `--local` flag > `NEMESISBOT_HOME` env > auto-detect > `~/.nemesisbot`
///
/// Auto-detect: if the current working directory contains a `.nemesisbot`
/// subdirectory, use it automatically (matching Go's DetectLocal behaviour).
pub fn resolve_home(local: bool) -> PathBuf {
    if local {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(".nemesisbot");
    }
    if let Ok(home) = std::env::var("NEMESISBOT_HOME") {
        return PathBuf::from(home).join(".nemesisbot");
    }
    // Auto-detect: if current directory has .nemesisbot, use it
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join(".nemesisbot").exists() {
        return cwd.join(".nemesisbot");
    }
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
    let mut _enable_file = false;
    let mut _file_path: Option<String> = None;

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
                            _enable_file = true;
                            _file_path = Some(fp.to_string());
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

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level);

    if enable_console {
        let _ = subscriber.try_init();
    } else {
        // No console: init with a writer that discards output
        let _ = subscriber.with_writer(io::sink).try_init();
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
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_resolve_home_local() {
        let home = resolve_home(true);
        assert!(home.to_string_lossy().contains(".nemesisbot"));
    }

    #[test]
    fn test_config_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(config_path(&home), PathBuf::from("/tmp/test/config.json"));
    }

    #[test]
    fn test_workspace_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(workspace_path(&home), PathBuf::from("/tmp/test/workspace"));
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn test_format_token() {
        assert_eq!(format_token(""), "(not set)");
        assert_eq!(format_token("abcd1234efgh"), "abcd...efgh");
        assert_eq!(format_token("short"), "***");
    }

    #[test]
    fn test_init_logger_no_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_init_logger_debug_flag() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let args = vec!["--debug".to_string()];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, LOG_DEBUG);
    }

    #[test]
    fn test_init_logger_quiet_flag() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let args = vec!["--quiet".to_string()];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, LOG_QUIET);
    }

    #[test]
    fn test_init_logger_with_config_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config_data = serde_json::json!({
            "logging": {
                "general": {
                    "enabled": true,
                    "enable_console": false,
                    "level": "WARN",
                    "file": ""
                }
            }
        });
        fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_copy_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "hello").unwrap();
        fs::write(src.join("sub").join("b.txt"), "world").unwrap();

        copy_directory(&src, &dst).unwrap();

        assert!(dst.join("a.txt").exists());
        assert!(dst.join("sub").join("b.txt").exists());
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "hello");
    }

    #[test]
    fn test_copy_directory_nonexistent() {
        let nonexistent = format!("C:/__nonexistent_copy_src_{}", std::process::id());
        let dst = format!("C:/__nonexistent_copy_dst_{}", std::process::id());
        let result = copy_directory(Path::new(&nonexistent), Path::new(&dst));
        assert!(result.is_err());
    }

    #[test]
    fn test_should_skip_heartbeat() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(!should_skip_heartbeat_for_bootstrap(tmp.path()));

        fs::write(tmp.path().join("BOOTSTRAP.md"), "bootstrap").unwrap();
        assert!(should_skip_heartbeat_for_bootstrap(tmp.path()));
    }

    #[test]
    fn test_resolve_home_env_var() {
        let tmp = tempfile::TempDir::new().unwrap();
        let custom_path = tmp.path().to_string_lossy().to_string();
        unsafe { std::env::set_var("NEMESISBOT_HOME", &custom_path); }
        let home = resolve_home(false);
        unsafe { std::env::remove_var("NEMESISBOT_HOME"); }
        assert!(home.to_string_lossy().contains(".nemesisbot"));
        // Check the parent directory matches
        assert_eq!(home.parent().unwrap(), tmp.path());
    }

    #[test]
    fn test_resolve_home_auto_detect() {
        // Test auto-detect when cwd has .nemesisbot
        let home = resolve_home(false);
        // Should resolve to some .nemesisbot path (either auto-detect or home dir)
        assert!(home.to_string_lossy().contains(".nemesisbot"));
    }

    #[test]
    fn test_mcp_config_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(
            mcp_config_path(&home),
            PathBuf::from("/tmp/test/workspace/config/config.mcp.json")
        );
    }

    #[test]
    fn test_scanner_config_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(
            scanner_config_path(&home),
            PathBuf::from("/tmp/test/workspace/config/config.scanner.json")
        );
    }

    #[test]
    fn test_security_config_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(
            security_config_path(&home),
            PathBuf::from("/tmp/test/workspace/config/config.security.json")
        );
    }

    #[test]
    fn test_skills_config_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(
            skills_config_path(&home),
            PathBuf::from("/tmp/test/workspace/config/config.skills.json")
        );
    }

    #[test]
    fn test_cluster_config_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(
            cluster_config_path(&home),
            PathBuf::from("/tmp/test/workspace/config/config.cluster.json")
        );
    }

    #[test]
    fn test_cors_config_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(
            cors_config_path(&home),
            PathBuf::from("/tmp/test/config/cors.json")
        );
    }

    #[test]
    fn test_cluster_dir_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(
            cluster_dir(&home),
            PathBuf::from("/tmp/test/workspace/cluster")
        );
    }

    #[test]
    fn test_cron_store_path() {
        let home = PathBuf::from("/tmp/test");
        assert_eq!(
            cron_store_path(&home),
            PathBuf::from("/tmp/test/workspace/cron/jobs.json")
        );
    }

    #[test]
    fn test_status_icon_ok() {
        assert_eq!(status_icon(true), "OK");
        assert_eq!(status_icon(false), "MISSING");
    }

    #[test]
    fn test_format_token_empty() {
        assert_eq!(format_token(""), "(not set)");
    }

    #[test]
    fn test_format_token_short() {
        assert_eq!(format_token("abc"), "***");
        assert_eq!(format_token("1234567"), "***");
    }

    #[test]
    fn test_format_token_exact_8() {
        // Exactly 8 chars: len == 8, NOT > 8, shows "***"
        assert_eq!(format_token("abcd1234"), "***");
        // 9 chars: len > 8, shows first/last 4
        assert_eq!(format_token("abcd12345"), "abcd...2345");
    }

    #[test]
    fn test_format_token_long() {
        assert_eq!(format_token("abcdefghijklmnop"), "abcd...mnop");
    }

    #[test]
    fn test_format_version() {
        let v = format_version();
        // Should contain version number (non-empty)
        assert!(!v.is_empty());
    }

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"x", b"x"));
    }

    #[test]
    fn test_constant_time_eq_not_equal() {
        assert!(!constant_time_eq(b"hello", b"hella"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"longer", b"short"));
    }

    #[test]
    fn test_init_logger_with_debug_level() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config_data = serde_json::json!({
            "logging": {
                "general": {
                    "enable_console": true,
                    "level": "DEBUG",
                    "file": ""
                }
            }
        });
        fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_init_logger_no_console_flag() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let args = vec!["--no-console".to_string()];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, LOG_NO_CONSOLE);
    }

    #[test]
    fn test_init_logger_multiple_flags() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let args = vec!["--debug".to_string(), "--no-console".to_string()];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, LOG_DEBUG | LOG_NO_CONSOLE);
    }

    #[test]
    fn test_init_logger_short_flags() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let args = vec!["-d".to_string(), "-q".to_string()];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, LOG_DEBUG | LOG_QUIET);
    }

    #[test]
    fn test_init_logger_error_level() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config_data = serde_json::json!({
            "logging": {
                "general": {
                    "enable_console": true,
                    "level": "ERROR",
                    "file": "/tmp/test.log"
                }
            }
        });
        fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_copy_directory_with_nested_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        fs::create_dir_all(src.join("a").join("b")).unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();
        fs::write(src.join("a").join("level1.txt"), "l1").unwrap();
        fs::write(src.join("a").join("b").join("level2.txt"), "l2").unwrap();

        copy_directory(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("root.txt")).unwrap(), "root");
        assert_eq!(fs::read_to_string(dst.join("a").join("level1.txt")).unwrap(), "l1");
        assert_eq!(fs::read_to_string(dst.join("a").join("b").join("level2.txt")).unwrap(), "l2");
    }

    // ============================================================
    // Additional tests for coverage improvement
    // ============================================================

    #[test]
    fn test_resolve_home_local_returns_cwd_based() {
        let home = resolve_home(true);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        assert_eq!(home, cwd.join(".nemesisbot"));
    }

    #[test]
    fn test_resolve_home_env_var_custom_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let custom_path = tmp.path().to_string_lossy().to_string();
        unsafe { std::env::set_var("NEMESISBOT_HOME", &custom_path); }
        let home = resolve_home(false);
        unsafe { std::env::remove_var("NEMESISBOT_HOME"); }
        assert_eq!(home, tmp.path().join(".nemesisbot"));
    }

    #[test]
    fn test_all_path_builders_consistency() {
        let home = PathBuf::from("/data/bot");
        // config_path
        assert_eq!(config_path(&home), PathBuf::from("/data/bot/config.json"));
        // workspace_path
        assert_eq!(workspace_path(&home), PathBuf::from("/data/bot/workspace"));
        // mcp_config_path
        assert!(mcp_config_path(&home).ends_with("config.mcp.json"));
        // scanner_config_path
        assert!(scanner_config_path(&home).ends_with("config.scanner.json"));
        // security_config_path
        assert!(security_config_path(&home).ends_with("config.security.json"));
        // skills_config_path
        assert!(skills_config_path(&home).ends_with("config.skills.json"));
        // cluster_config_path
        assert!(cluster_config_path(&home).ends_with("config.cluster.json"));
        // cors_config_path
        assert!(cors_config_path(&home).ends_with("cors.json"));
        // cluster_dir
        assert!(cluster_dir(&home).ends_with("cluster"));
        // cron_store_path
        assert!(cron_store_path(&home).ends_with("jobs.json"));
    }

    #[test]
    fn test_mcp_config_path_under_workspace() {
        let home = PathBuf::from("/tmp/bot");
        let mcp = mcp_config_path(&home);
        assert!(mcp.starts_with("/tmp/bot/workspace/config"));
    }

    #[test]
    fn test_cluster_dir_is_under_workspace() {
        let home = PathBuf::from("/tmp/bot");
        let cdir = cluster_dir(&home);
        let ws = workspace_path(&home);
        assert!(cdir.starts_with(&ws));
    }

    #[test]
    fn test_cron_store_path_under_workspace() {
        let home = PathBuf::from("/tmp/bot");
        let cron = cron_store_path(&home);
        assert!(cron.starts_with(workspace_path(&home)));
    }

    #[test]
    fn test_format_token_boundary_cases() {
        // Exactly 8 chars -> too short, shows "***"
        assert_eq!(format_token("12345678"), "***");
        // 9 chars -> shows first 4 and last 4
        assert_eq!(format_token("123456789"), "1234...6789");
        // 5 chars (short)
        assert_eq!(format_token("12345"), "***");
        // Single char
        assert_eq!(format_token("a"), "***");
        // Unicode token - len() counts bytes, not chars
        let token = "abcd\u{4e2d}\u{56fd}efgh"; // 4 + 6 + 4 = 14 bytes
        let formatted = format_token(token);
        assert!(formatted.contains("..."));
    }

    #[test]
    fn test_constant_time_eq_symmetry() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"abc", b"ABC"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        // Symmetry: different lengths always false
        assert!(!constant_time_eq(b"longstring", b"short"));
    }

    #[test]
    fn test_status_icon_values() {
        assert_eq!(status_icon(true), "OK");
        assert_eq!(status_icon(false), "MISSING");
    }

    #[test]
    fn test_init_logger_with_warn_level() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config_data = serde_json::json!({
            "logging": {
                "general": {
                    "enable_console": true,
                    "level": "WARN",
                    "file": ""
                }
            }
        });
        fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_init_logger_with_trace_level() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config_data = serde_json::json!({
            "logging": {
                "general": {
                    "enable_console": true,
                    "level": "TRACE",
                    "file": ""
                }
            }
        });
        fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_init_logger_with_invalid_level_defaults_to_info() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config_data = serde_json::json!({
            "logging": {
                "general": {
                    "enable_console": true,
                    "level": "INVALID_LEVEL",
                    "file": ""
                }
            }
        });
        fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_init_logger_with_file_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config_data = serde_json::json!({
            "logging": {
                "general": {
                    "enable_console": true,
                    "level": "INFO",
                    "file": "/tmp/nemesisbot-test.log"
                }
            }
        });
        fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_init_logger_invalid_json_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        fs::write(&cfg, "not valid json{{{").unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_init_logger_empty_json_object() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        fs::write(&cfg, "{}").unwrap();
        let args: Vec<String> = vec![];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_init_logger_quiet_overrides_debug() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let args = vec!["--debug".to_string(), "--quiet".to_string()];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, LOG_DEBUG | LOG_QUIET);
    }

    #[test]
    fn test_init_logger_unrelated_args_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let args = vec!["gateway".to_string(), "--local".to_string()];
        let flags = init_logger_from_config(&cfg, &args);
        assert_eq!(flags, 0);
    }

    #[test]
    fn test_copy_directory_overwrites_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("file.txt"), "new content").unwrap();

        // Create existing file with different content
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("file.txt"), "old content").unwrap();

        copy_directory(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("file.txt")).unwrap(), "new content");
    }

    #[test]
    fn test_should_skip_heartbeat_false_by_default() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(!should_skip_heartbeat_for_bootstrap(tmp.path()));
    }

    #[test]
    fn test_should_skip_heartbeat_true_with_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("BOOTSTRAP.md"), "content").unwrap();
        assert!(should_skip_heartbeat_for_bootstrap(tmp.path()));
    }

    #[test]
    fn test_format_version_not_empty() {
        let v = format_version();
        assert!(!v.is_empty());
    }

    #[test]
    fn test_version_info_fields_not_empty() {
        // version and rust_version should always be set
        assert!(!VERSION_INFO.version.is_empty());
        assert!(!VERSION_INFO.rust_version.is_empty());
    }

    #[test]
    fn test_log_flag_constants() {
        assert_eq!(LOG_DEBUG, 1);
        assert_eq!(LOG_QUIET, 2);
        assert_eq!(LOG_NO_CONSOLE, 4);
    }
}
