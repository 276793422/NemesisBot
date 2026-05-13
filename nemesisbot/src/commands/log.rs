//! Log command - configure LLM and general logging.

use anyhow::Result;
use crate::common;

// ---------------------------------------------------------------------------
// CLI action enums
// ---------------------------------------------------------------------------

#[derive(clap::Subcommand)]
pub enum LogAction {
    /// Manage LLM request/response logging
    Llm {
        #[command(subcommand)]
        action: LlmAction,
    },
    /// Manage general application logging
    General {
        #[command(subcommand)]
        action: GeneralAction,
    },
    /// Enable logging (backward compat alias for llm enable)
    Enable,
    /// Disable logging (backward compat alias for llm disable)
    Disable,
    /// Show all logging status
    Status,
    /// Configure LLM logging detail level and directory
    Config {
        /// Detail level: full, truncated
        #[arg(long)]
        detail_level: Option<String>,
        /// Log directory for LLM request/response files
        #[arg(long)]
        log_dir: Option<String>,
    },
    /// Set log level
    SetLevel {
        /// Log level: DEBUG, INFO, WARN, ERROR
        level: String,
    },
    /// Enable file logging
    EnableFile {
        /// Log file path
        #[arg(long)]
        path: Option<String>,
    },
    /// Disable file logging
    DisableFile,
    /// Enable console logging
    EnableConsole,
    /// Disable console logging
    DisableConsole,
}

#[derive(clap::Subcommand)]
pub enum LlmAction {
    /// Enable LLM logging
    Enable,
    /// Disable LLM logging
    Disable,
    /// Show LLM logging status
    Status,
    /// Configure LLM logging detail level and directory
    Config {
        /// Detail level: full, truncated
        #[arg(long)]
        detail_level: Option<String>,
        /// Log directory for LLM request/response files
        #[arg(long)]
        log_dir: Option<String>,
    },
}

#[derive(clap::Subcommand)]
pub enum GeneralAction {
    /// Enable general logging
    Enable,
    /// Disable general logging
    Disable,
    /// Show general logging status
    Status,
    /// Set general log level
    Level {
        /// Log level: DEBUG, INFO, WARN, ERROR, FATAL
        level: String,
    },
    /// Set log file path
    File {
        /// Path to log file
        path: String,
    },
    /// Toggle console logging
    Console,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Expand tilde (~) in paths to home directory.
fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), &path[1..]);
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    path.to_string()
}

/// Resolve a path: if not absolute, resolve relative to workspace.
fn resolve_path(path: &str, workspace: &std::path::Path) -> String {
    let expanded = expand_tilde(path);
    let p = std::path::Path::new(&expanded);
    if p.is_absolute() {
        expanded
    } else {
        workspace.join(p).to_string_lossy().to_string()
    }
}

/// Read or create the logging section of config.json.
fn read_logging_config(cfg_path: &std::path::Path) -> Result<serde_json::Value> {
    if cfg_path.exists() {
        let data = std::fs::read_to_string(cfg_path)?;
        let cfg: serde_json::Value = serde_json::from_str(&data)?;
        Ok(cfg.get("logging").cloned().unwrap_or_else(|| default_logging_config()))
    } else {
        Ok(default_logging_config())
    }
}

/// Default logging configuration.
fn default_logging_config() -> serde_json::Value {
    serde_json::json!({
        "llm": {
            "enabled": false,
            "detail_level": "full",
            "log_dir": "logs/request_logs"
        },
        "general": {
            "enabled": true,
            "level": "INFO",
            "file": "",
            "console": true,
            "enable_console": true
        }
    })
}

/// Persist the logging section back into config.json.
fn write_logging_config(cfg_path: &std::path::Path, logging: &serde_json::Value) -> Result<()> {
    let mut cfg: serde_json::Value = if cfg_path.exists() {
        let data = std::fs::read_to_string(cfg_path)?;
        serde_json::from_str(&data)?
    } else {
        serde_json::json!({})
    };

    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("logging".to_string(), logging.clone());
    }

    let dir = cfg_path.parent().unwrap();
    let _ = std::fs::create_dir_all(dir);
    std::fs::write(cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// LLM logging sub-commands
// ---------------------------------------------------------------------------

fn cmd_llm_enable(cfg_path: &std::path::Path, workspace: &std::path::Path) -> Result<()> {
    let mut logging = read_logging_config(cfg_path)?;

    // Ensure llm section exists with defaults
    if logging.get("llm").is_none() {
        logging["llm"] = serde_json::json!({
            "enabled": false,
            "detail_level": "full",
            "log_dir": "logs/request_logs"
        });
    }

    if let Some(llm) = logging.get_mut("llm").and_then(|v| v.as_object_mut()) {
        llm.insert("enabled".to_string(), serde_json::Value::Bool(true));
        // Set defaults if empty
        if llm.get("log_dir").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
            llm.insert("log_dir".to_string(), serde_json::Value::String("logs/request_logs".to_string()));
        }
        if llm.get("detail_level").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
            llm.insert("detail_level".to_string(), serde_json::Value::String("full".to_string()));
        }
    }

    write_logging_config(cfg_path, &logging)?;

    let log_dir = logging.get("llm")
        .and_then(|l| l.get("log_dir"))
        .and_then(|v| v.as_str())
        .unwrap_or("logs/request_logs");
    let detail = logging.get("llm")
        .and_then(|l| l.get("detail_level"))
        .and_then(|v| v.as_str())
        .unwrap_or("full");

    let display_dir = resolve_path(log_dir, workspace);

    // Create log directory
    let _ = std::fs::create_dir_all(&display_dir);

    println!("📋 LLM request logging enabled");
    println!("  Log directory: {}", display_dir);
    println!("  Detail level:  {}", detail);
    Ok(())
}

fn cmd_llm_disable(cfg_path: &std::path::Path) -> Result<()> {
    let mut logging = read_logging_config(cfg_path)?;
    if let Some(llm) = logging.get_mut("llm").and_then(|v| v.as_object_mut()) {
        llm.insert("enabled".to_string(), serde_json::Value::Bool(false));
    }
    write_logging_config(cfg_path, &logging)?;
    println!("🔇 LLM logging disabled.");
    Ok(())
}

fn cmd_llm_status(cfg_path: &std::path::Path, workspace: &std::path::Path) -> Result<()> {
    let logging = read_logging_config(cfg_path)?;
    let llm = logging.get("llm");

    println!("📋 LLM Request Logging Status:");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    if let Some(llm) = llm {
        let enabled = llm.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut log_dir = llm.get("log_dir").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut detail = llm.get("detail_level").and_then(|v| v.as_str()).unwrap_or("").to_string();

        // Apply defaults if empty
        if log_dir.is_empty() {
            log_dir = "logs/request_logs".to_string();
        }
        if detail.is_empty() {
            detail = "full".to_string();
        }

        let resolved_dir = resolve_path(&log_dir, workspace);

        println!("  Status:        {}", if enabled { "Enabled" } else { "Disabled" });
        println!("  Log Directory: {}", resolved_dir);
        println!("  Detail Level:  {}", detail);
        println!("  Config File:   {}", cfg_path.display());
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        // Show recent log directories if enabled
        if enabled {
            let dir_path = std::path::Path::new(&resolved_dir);
            if dir_path.exists() {
                let mut entries: Vec<_> = std::fs::read_dir(dir_path)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .collect();
                entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
                let recent: Vec<_> = entries.iter().take(5).collect();

                if !recent.is_empty() {
                    println!();
                    println!("  Recent Logs:");
                    for entry in &recent {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if let Ok(files) = std::fs::read_dir(entry.path()) {
                            let file_list: Vec<_> = files
                                .filter_map(|f| f.ok())
                                .filter(|f| f.file_type().map(|t| t.is_file()).unwrap_or(false))
                                .collect();
                            let count = file_list.len();
                            let total_size: u64 = file_list.iter()
                                .filter_map(|f| f.metadata().ok().map(|m| m.len()))
                                .sum();
                            let size_kb = total_size as f64 / 1024.0;
                            println!("    {} ({} files, {:.1} KB)", name, count, size_kb);
                        }
                    }
                }
            }
        }
    } else {
        println!("  Status:        Disabled");
        println!("  Using defaults: detail_level=full, log_dir=logs/request_logs");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    }
    Ok(())
}

fn cmd_llm_config(cfg_path: &std::path::Path, workspace: &std::path::Path, detail_level: Option<&str>, log_dir: Option<&str>) -> Result<()> {
    let mut logging = read_logging_config(cfg_path)?;
    let mut changed = false;

    // Ensure llm section exists
    if logging.get("llm").is_none() {
        logging["llm"] = serde_json::json!({
            "enabled": false,
            "detail_level": "full",
            "log_dir": "logs/request_logs"
        });
    }

    if let Some(detail_level) = detail_level {
        let valid = ["full", "truncated"];
        if !valid.contains(&detail_level) {
            println!("Error: Invalid detail level '{}'. Valid: {:?}", detail_level, valid);
            std::process::exit(1);
        }
        if let Some(llm) = logging.get_mut("llm").and_then(|v| v.as_object_mut()) {
            llm.insert("detail_level".to_string(), serde_json::Value::String(detail_level.to_string()));
        }
        changed = true;
    }

    if let Some(log_dir) = log_dir {
        let resolved = resolve_path(log_dir, workspace);
        if let Some(llm) = logging.get_mut("llm").and_then(|v| v.as_object_mut()) {
            llm.insert("log_dir".to_string(), serde_json::Value::String(resolved.clone()));
        }
        // Create directory if it doesn't exist
        let _ = std::fs::create_dir_all(&resolved);
        changed = true;
    }

    if changed {
        write_logging_config(cfg_path, &logging)?;

        let current_detail = logging.get("llm")
            .and_then(|l| l.get("detail_level"))
            .and_then(|v| v.as_str())
            .unwrap_or("full");
        let current_dir = logging.get("llm")
            .and_then(|l| l.get("log_dir"))
            .and_then(|v| v.as_str())
            .unwrap_or("logs/request_logs");

        println!("⚙️ Configuration updated");
        println!("  Detail level: {}", current_detail);
        println!("  Log directory: {}", current_dir);
    } else {
        println!("No changes specified. Use --detail-level or --log-dir.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// General logging sub-commands
// ---------------------------------------------------------------------------

fn cmd_general_enable(cfg_path: &std::path::Path) -> Result<()> {
    let mut logging = read_logging_config(cfg_path)?;
    if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
        general.insert("enabled".to_string(), serde_json::Value::Bool(true));
    }
    write_logging_config(cfg_path, &logging)?;
    println!("✅ General logging enabled.");
    Ok(())
}

fn cmd_general_disable(cfg_path: &std::path::Path) -> Result<()> {
    let mut logging = read_logging_config(cfg_path)?;
    if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
        general.insert("enabled".to_string(), serde_json::Value::Bool(false));
    }
    write_logging_config(cfg_path, &logging)?;
    println!("🔇 General logging disabled.");
    Ok(())
}

fn cmd_general_status(cfg_path: &std::path::Path) -> Result<()> {
    let logging = read_logging_config(cfg_path)?;
    let general = logging.get("general");

    println!("📋 General Logging Status:");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    if let Some(g) = general {
        let enabled = g.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let level = g.get("level").and_then(|v| v.as_str()).unwrap_or("INFO");
        let file = g.get("file").and_then(|v| v.as_str()).unwrap_or("");
        let console = g.get("enable_console").and_then(|v| v.as_bool())
            .or_else(|| g.get("console").and_then(|v| v.as_bool()))
            .unwrap_or(true);
        println!("  Status:  {}", if enabled { "Enabled" } else { "Disabled" });
        println!("  Level:   {}", level);
        println!("  Console: {}", if console { "enabled" } else { "disabled" });
        println!("  File:    {}", if file.is_empty() { "(none)" } else { file });
    } else {
        println!("  Status:  Enabled");
        println!("  Level:   INFO");
        println!("  Console: enabled");
        println!("  File:    (none)");
    }
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    Ok(())
}

fn cmd_general_level(cfg_path: &std::path::Path, level: &str) -> Result<()> {
    let valid_levels = ["DEBUG", "INFO", "WARN", "ERROR", "FATAL", "TRACE"];
    let upper = level.to_uppercase();
    if !valid_levels.contains(&upper.as_str()) {
        println!("Invalid level: {}. Valid levels: {:?}", level, valid_levels);
        return Ok(());
    }
    let mut logging = read_logging_config(cfg_path)?;
    if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
        general.insert("level".to_string(), serde_json::Value::String(upper.clone()));
    }
    write_logging_config(cfg_path, &logging)?;
    println!("General log level set to: {}", upper);
    Ok(())
}

fn cmd_general_file(cfg_path: &std::path::Path, path: &str) -> Result<()> {
    let mut logging = read_logging_config(cfg_path)?;
    if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
        general.insert("file".to_string(), serde_json::Value::String(path.to_string()));
    }
    write_logging_config(cfg_path, &logging)?;
    println!("General log file set to: {}", path);
    Ok(())
}

fn cmd_general_console(cfg_path: &std::path::Path) -> Result<()> {
    let mut logging = read_logging_config(cfg_path)?;
    let current = logging.get("general")
        .and_then(|g| g.get("enable_console").and_then(|v| v.as_bool()))
        .or_else(|| logging.get("general").and_then(|g| g.get("console").and_then(|v| v.as_bool())))
        .unwrap_or(true);
    let new_val = !current;

    if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
        general.insert("enable_console".to_string(), serde_json::Value::Bool(new_val));
        general.insert("console".to_string(), serde_json::Value::Bool(new_val));
    }
    write_logging_config(cfg_path, &logging)?;
    println!("Console logging {}.", if new_val { "enabled" } else { "disabled" });
    Ok(())
}

/// Show all logging status (top-level `log status` command).
fn cmd_all_status(cfg_path: &std::path::Path, workspace: &std::path::Path) -> Result<()> {
    cmd_llm_status(cfg_path, workspace)?;
    println!();
    cmd_general_status(cfg_path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

pub fn run(action: LogAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cfg_path = common::config_path(&home);
    let workspace = common::workspace_path(&home);

    match action {
        LogAction::Llm { action } => {
            match action {
                LlmAction::Enable => cmd_llm_enable(&cfg_path, &workspace)?,
                LlmAction::Disable => cmd_llm_disable(&cfg_path)?,
                LlmAction::Status => cmd_llm_status(&cfg_path, &workspace)?,
                LlmAction::Config { detail_level, log_dir } => {
                    cmd_llm_config(&cfg_path, &workspace, detail_level.as_deref(), log_dir.as_deref())?
                }
            }
        }
        LogAction::General { action } => {
            match action {
                GeneralAction::Enable => cmd_general_enable(&cfg_path)?,
                GeneralAction::Disable => cmd_general_disable(&cfg_path)?,
                GeneralAction::Status => cmd_general_status(&cfg_path)?,
                GeneralAction::Level { level } => cmd_general_level(&cfg_path, &level)?,
                GeneralAction::File { path } => cmd_general_file(&cfg_path, &path)?,
                GeneralAction::Console => cmd_general_console(&cfg_path)?,
            }
        }
        // Backward compat aliases
        LogAction::Enable => cmd_llm_enable(&cfg_path, &workspace)?,
        LogAction::Disable => cmd_llm_disable(&cfg_path)?,
        // Top-level status shows everything
        LogAction::Status => cmd_all_status(&cfg_path, &workspace)?,
        // Top-level config mutates LLM settings
        LogAction::Config { detail_level, log_dir } => {
            cmd_llm_config(&cfg_path, &workspace, detail_level.as_deref(), log_dir.as_deref())?
        }
        LogAction::SetLevel { level } => cmd_general_level(&cfg_path, &level)?,
        LogAction::EnableFile { path } => {
            let file_path = path.unwrap_or_else(|| "logs/nemesisbot.log".to_string());
            let mut logging = read_logging_config(&cfg_path)?;
            if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
                general.insert("file".to_string(), serde_json::Value::String(file_path.clone()));
            }
            write_logging_config(&cfg_path, &logging)?;
            let _ = std::fs::create_dir_all(
                std::path::Path::new(&file_path).parent().unwrap_or(std::path::Path::new("."))
            );
            println!("📝 File logging enabled: {}", file_path);
        }
        LogAction::DisableFile => {
            let mut logging = read_logging_config(&cfg_path)?;
            if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
                general.insert("file".to_string(), serde_json::Value::String(String::new()));
            }
            write_logging_config(&cfg_path, &logging)?;
            println!("🔇 File logging disabled.");
        }
        LogAction::EnableConsole => {
            let mut logging = read_logging_config(&cfg_path)?;
            if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
                general.insert("enable_console".to_string(), serde_json::Value::Bool(true));
                general.insert("console".to_string(), serde_json::Value::Bool(true));
            }
            write_logging_config(&cfg_path, &logging)?;
            println!("🖥️ Console logging enabled.");
        }
        LogAction::DisableConsole => {
            let mut logging = read_logging_config(&cfg_path)?;
            if let Some(general) = logging.get_mut("general").and_then(|v| v.as_object_mut()) {
                general.insert("enable_console".to_string(), serde_json::Value::Bool(false));
                general.insert("console".to_string(), serde_json::Value::Bool(false));
            }
            write_logging_config(&cfg_path, &logging)?;
            println!("🔇 Console logging disabled.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_config(tmp: &TempDir) -> std::path::PathBuf {
        let cfg = tmp.path().join("config.json");
        let config = serde_json::json!({
            "logging": default_logging_config()
        });
        std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        cfg
    }

    #[test]
    fn test_default_logging_config_structure() {
        let cfg = default_logging_config();
        assert_eq!(cfg["llm"]["enabled"], false);
        assert_eq!(cfg["llm"]["detail_level"], "full");
        assert_eq!(cfg["general"]["enabled"], true);
        assert_eq!(cfg["general"]["level"], "INFO");
        assert_eq!(cfg["general"]["console"], true);
    }

    #[test]
    fn test_expand_tilde_home() {
        let expanded = expand_tilde("~/test/path");
        assert!(!expanded.starts_with('~'));
        assert!(expanded.contains("test") || expanded.contains("path"));
    }

    #[test]
    fn test_expand_tilde_root() {
        let expanded = expand_tilde("~");
        assert!(!expanded.starts_with('~') || !dirs::home_dir().is_some());
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let expanded = expand_tilde("/absolute/path");
        assert_eq!(expanded, "/absolute/path");
    }

    #[test]
    fn test_expand_tilde_backslash() {
        let expanded = expand_tilde("~\\test");
        // Should expand on Windows
        assert!(!expanded.starts_with('~') || !dirs::home_dir().is_some());
    }

    #[test]
    fn test_resolve_path_absolute() {
        let tmp = TempDir::new().unwrap();
        let resolved = resolve_path("/absolute/path", tmp.path());
        // On Windows, /absolute/path becomes C:/absolute/path
        assert!(resolved.contains("absolute"));
        assert!(resolved.contains("path"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let tmp = TempDir::new().unwrap();
        let resolved = resolve_path("relative/path", tmp.path());
        assert!(resolved.starts_with(&tmp.path().to_string_lossy().to_string()));
        assert!(resolved.contains("relative"));
    }

    #[test]
    fn test_resolve_path_tilde() {
        let tmp = TempDir::new().unwrap();
        let resolved = resolve_path("~/logs", tmp.path());
        assert!(!resolved.starts_with('~') || !dirs::home_dir().is_some());
    }

    #[test]
    fn test_read_logging_config_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("nonexistent.json");
        let config = read_logging_config(&cfg_path).unwrap();
        assert_eq!(config["llm"]["enabled"], false);
    }

    #[test]
    fn test_read_logging_config_with_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let config = read_logging_config(&cfg).unwrap();
        assert_eq!(config["general"]["level"], "INFO");
    }

    #[test]
    fn test_read_logging_config_file_without_logging() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(&cfg, r#"{"other": "data"}"#).unwrap();
        let config = read_logging_config(&cfg).unwrap();
        // Should return default
        assert_eq!(config["llm"]["enabled"], false);
    }

    #[test]
    fn test_write_logging_config_creates_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let logging = default_logging_config();

        write_logging_config(&cfg, &logging).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(data.get("logging").is_some());
    }

    #[test]
    fn test_write_logging_config_preserves_other_fields() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(&cfg, r#"{"other": "data", "version": "1.0"}"#).unwrap();

        let logging = default_logging_config();
        write_logging_config(&cfg, &logging).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["other"], "data");
        assert_eq!(data["version"], "1.0");
        assert!(data.get("logging").is_some());
    }

    #[test]
    fn test_cmd_llm_enable() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_enable(&cfg, &workspace).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["llm"]["enabled"], true);
    }

    #[test]
    fn test_cmd_llm_disable() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        cmd_llm_disable(&cfg).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["llm"]["enabled"], false);
    }

    #[test]
    fn test_cmd_general_enable() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        // First disable
        cmd_general_disable(&cfg).unwrap();
        // Then enable
        cmd_general_enable(&cfg).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["enabled"], true);
    }

    #[test]
    fn test_cmd_general_disable() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        cmd_general_disable(&cfg).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["enabled"], false);
    }

    #[test]
    fn test_cmd_general_level_valid() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        cmd_general_level(&cfg, "DEBUG").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["level"], "DEBUG");
    }

    #[test]
    fn test_cmd_general_level_case_insensitive() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        cmd_general_level(&cfg, "warn").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["level"], "WARN");
    }

    #[test]
    fn test_cmd_general_level_invalid() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        cmd_general_level(&cfg, "INVALID").unwrap();

        // Level should remain unchanged
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["level"], "INFO");
    }

    #[test]
    fn test_cmd_general_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        cmd_general_file(&cfg, "/tmp/test.log").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["file"], "/tmp/test.log");
    }

    #[test]
    fn test_cmd_general_console_toggle() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        // Default is true, toggle should set to false
        cmd_general_console(&cfg).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["enable_console"], false);

        // Toggle again should set to true
        cmd_general_console(&cfg).unwrap();
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["enable_console"], true);
    }

    #[test]
    fn test_cmd_llm_status() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_status(&cfg, &workspace).unwrap();
    }

    #[test]
    fn test_cmd_general_status() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        cmd_general_status(&cfg).unwrap();
    }

    #[test]
    fn test_cmd_all_status() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_all_status(&cfg, &workspace).unwrap();
    }

    #[test]
    fn test_cmd_llm_config_detail_level() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_config(&cfg, &workspace, Some("truncated"), None).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["llm"]["detail_level"], "truncated");
    }

    #[test]
    fn test_cmd_llm_config_log_dir() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_config(&cfg, &workspace, None, Some("my-logs")).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let log_dir = data["logging"]["llm"]["log_dir"].as_str().unwrap();
        assert!(log_dir.contains("my-logs"));
    }

    #[test]
    fn test_cmd_llm_config_no_changes() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);
        let workspace = tmp.path().join("workspace");

        cmd_llm_config(&cfg, &workspace, None, None).unwrap();
        // Should succeed with no changes
    }

    #[test]
    fn test_llm_enable_no_existing_section() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(&cfg, r#"{"other": true}"#).unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_enable(&cfg, &workspace).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["llm"]["enabled"], true);
    }
}
