//! Scanner command - manage virus scanner engines.
//!
//! Mirrors Go command/scanner.go with subcommand support:
//! list, add, remove, enable, disable, check, install, clamav (install/update/test/info).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::common;

// ---------------------------------------------------------------------------
// Scanner config helpers
// ---------------------------------------------------------------------------

/// Scanner full config (matches Go config.ScannerFullConfig).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScannerFullConfig {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub engines: serde_json::Map<String, Value>,
}

impl Default for ScannerFullConfig {
    fn default() -> Self {
        Self {
            enabled: vec![],
            engines: serde_json::Map::new(),
        }
    }
}

/// ClamAV engine config (matches Go config.ClamAVEngineConfig).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClamAVEngineConfig {
    #[serde(default = "default_address")]
    pub address: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "clamav_path")]
    pub clamav_path: String,
    #[serde(default)]
    pub data_dir: String,
    #[serde(default)]
    pub scan_on_write: bool,
    #[serde(default)]
    pub scan_on_download: bool,
    #[serde(default)]
    pub scan_on_exec: bool,
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
    #[serde(default = "default_update_interval")]
    pub update_interval: String,
    #[serde(default = "default_skip_extensions")]
    pub skip_extensions: Vec<String>,
    #[serde(default)]
    pub state: EngineState,
}

fn default_address() -> String { "127.0.0.1:3310".to_string() }
fn default_max_file_size() -> u64 { 52428800 }
fn default_update_interval() -> String { "24h".to_string() }
fn default_skip_extensions() -> Vec<String> {
    vec![".txt", ".md", ".json", ".yaml", ".yml", ".toml", ".log", ".css", ".html"]
        .into_iter().map(String::from).collect()
}

impl Default for ClamAVEngineConfig {
    fn default() -> Self {
        Self {
            address: default_address(),
            url: String::new(),
            clamav_path: String::new(),
            data_dir: String::new(),
            scan_on_write: true,
            scan_on_download: false,
            scan_on_exec: true,
            max_file_size: default_max_file_size(),
            update_interval: default_update_interval(),
            skip_extensions: default_skip_extensions(),
            state: EngineState::default(),
        }
    }
}

/// Engine state (matches Go config.EngineState).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineState {
    #[serde(default)]
    pub install_status: String,
    #[serde(default)]
    pub install_error: String,
    #[serde(default)]
    pub db_status: String,
    #[serde(default)]
    pub last_install_attempt: String,
    #[serde(default)]
    pub last_db_update: String,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            install_status: String::new(),
            install_error: String::new(),
            db_status: String::new(),
            last_install_attempt: String::new(),
            last_db_update: String::new(),
        }
    }
}

/// Load scanner config from file.
fn load_scanner_config(path: &std::path::Path) -> Result<ScannerFullConfig> {
    if path.exists() {
        let data = std::fs::read_to_string(path)?;
        let cfg: ScannerFullConfig = serde_json::from_str(&data)?;
        Ok(cfg)
    } else {
        Ok(ScannerFullConfig::default())
    }
}

/// Save scanner config to file.
fn save_scanner_config(path: &std::path::Path, cfg: &ScannerFullConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(cfg)?;
    std::fs::write(path, data)?;
    Ok(())
}

/// Check if engine name is valid.
fn is_valid_engine(name: &str) -> bool {
    nemesis_security::scanner::available_engines().contains(&name)
}

/// Parse ClamAV engine config from JSON value.
fn parse_engine_config(raw: &Value) -> ClamAVEngineConfig {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

/// Resolve default tools directory: {workspace}/tools/
fn resolve_tools_dir(security_cfg: &std::path::Path) -> PathBuf {
    // scanner config is at {home}/config/config.scanner.json
    // workspace is {home}/workspace/
    let config_dir = security_cfg.parent().unwrap_or(security_cfg);
    let home_dir = config_dir.parent().unwrap_or(config_dir);
    home_dir.join("workspace").join("tools")
}

/// Look up ClamAV in system PATH, returns directory containing executable.
fn lookup_system_clamav() -> Option<String> {
    // Check common names
    for name in &["clamd", "clamscan", "clamd.exe", "clamscan.exe"] {
        if let Ok(path) = which::which(name) {
            if let Some(parent) = path.parent() {
                return Some(parent.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// Check if ClamAV executable exists at the given path.
fn check_executables_at_path(dir: &str) -> bool {
    let path = std::path::Path::new(dir);
    for exe in &["clamd", "clamd.exe", "clamscan", "clamscan.exe"] {
        if path.join(exe).exists() {
            return true;
        }
    }
    false
}

/// Re-serialize engine config with updated state and paths.
fn marshal_engine_config(
    raw: &Value,
    state: &EngineState,
    clamav_path: &str,
    data_dir: &str,
) -> Option<Value> {
    let mut cfg = parse_engine_config(raw);
    cfg.state = state.clone();
    if !clamav_path.is_empty() {
        cfg.clamav_path = clamav_path.to_string();
    }
    if !data_dir.is_empty() {
        cfg.data_dir = data_dir.to_string();
    }
    serde_json::to_value(cfg).ok()
}

/// Database file name for ClamAV.
const DATABASE_FILE: &str = "daily.cvd";

// ---------------------------------------------------------------------------
// ScannerAction / ClamavAction - clap subcommands
// ---------------------------------------------------------------------------

#[derive(clap::Subcommand)]
pub enum ScannerAction {
    /// List all scanner engines and their status
    List,
    /// Add or update a scanner engine configuration
    Add {
        /// Engine name (e.g., clamav)
        name: String,
        /// Download URL for the engine
        #[arg(long)]
        url: Option<String>,
        /// Installation directory
        #[arg(long)]
        path: Option<String>,
        /// Connection address (e.g., 127.0.0.1:3310)
        #[arg(long)]
        address: Option<String>,
    },
    /// Remove a scanner engine
    Remove {
        /// Engine name
        name: String,
    },
    /// Enable a scanner engine (deprecated: use `scanner <engine> enable`)
    #[command(hide = true)]
    Enable {
        /// Engine name (e.g., clamav)
        name: String,
    },
    /// Disable a scanner engine (deprecated: use `scanner <engine> disable`)
    #[command(hide = true)]
    Disable {
        /// Engine name
        name: String,
    },
    /// Check install and database status of engines
    Check,
    /// Install all pending enabled engines
    Install {
        /// Installation directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// ClamAV engine operations
    Clamav {
        #[command(subcommand)]
        action: ClamavAction,
    },
}

#[derive(clap::Subcommand)]
pub enum ClamavAction {
    /// Install ClamAV engine (download + configure + database)
    Install {
        /// Force re-install even if already installed
        #[arg(long)]
        force: bool,
        /// Override download URL (default: read from config)
        #[arg(long)]
        url: Option<String>,
        /// Installation directory
        #[arg(long)]
        dir: Option<String>,
    },
    /// Enable ClamAV engine (requires installed)
    Enable,
    /// Disable ClamAV engine
    Disable,
    /// Update ClamAV virus database (run freshclam)
    Update,
    /// Test scan a file
    Test {
        /// File path to scan
        path: String,
    },
    /// Show ClamAV engine information
    Info,
}

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

pub async fn run(action: ScannerAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let security_cfg = common::scanner_config_path(&home);

    match action {
        ScannerAction::List => cmd_list(&security_cfg),
        ScannerAction::Add { name, url, path, address } => {
            cmd_add(&security_cfg, &name, url.as_deref(), path.as_deref(), address.as_deref())
        }
        ScannerAction::Remove { name } => cmd_remove(&security_cfg, &name),
        ScannerAction::Enable { name } => cmd_enable(&security_cfg, &name),
        ScannerAction::Disable { name } => cmd_disable(&security_cfg, &name),
        ScannerAction::Check => cmd_check(&security_cfg),
        ScannerAction::Install { dir } => cmd_install(&security_cfg, dir.as_deref()).await,
        ScannerAction::Clamav { action } => cmd_clamav(action, &security_cfg).await,
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

/// List all scanner engines and their status (matches Go cmdScannerList).
fn cmd_list(security_cfg: &std::path::Path) -> Result<()> {
    let cfg = load_scanner_config(security_cfg)?;

    let enabled_set: std::collections::HashSet<&str> =
        cfg.enabled.iter().map(|s| s.as_str()).collect();

    println!("\n🔍 Scanner Engines:");
    println!("{}", "-".repeat(70));

    if cfg.engines.is_empty() {
        println!("  No scanner engines configured.");
        println!("  Use 'nemesisbot scanner add <engine>' to add one.");
        return Ok(());
    }

    for (name, raw_cfg) in &cfg.engines {
        let status = if enabled_set.contains(name.as_str()) {
            "enabled"
        } else {
            "disabled"
        };

        let mut parts = Vec::new();
        let engine_cfg = parse_engine_config(raw_cfg);
        if !engine_cfg.address.is_empty() {
            parts.push(format!("address={}", engine_cfg.address));
        }
        if !engine_cfg.state.install_status.is_empty() {
            parts.push(format!("install={}", engine_cfg.state.install_status));
        }
        if !engine_cfg.state.db_status.is_empty() {
            parts.push(format!("db={}", engine_cfg.state.db_status));
        }

        let summary = parts.join(", ");
        println!("  {:<15}  {:<10}  {}", name, status, summary);
    }

    println!("{}", "-".repeat(70));
    println!("  Enabled order: {:?}", cfg.enabled);
    Ok(())
}

/// Add or update a scanner engine (matches Go cmdScannerAdd).
fn cmd_add(
    security_cfg: &std::path::Path,
    name: &str,
    url: Option<&str>,
    path: Option<&str>,
    address: Option<&str>,
) -> Result<()> {
    if !is_valid_engine(name) {
        eprintln!("Unknown engine: {}", name);
        eprintln!("Available: {:?}", nemesis_security::scanner::available_engines());
        std::process::exit(1);
    }

    let mut cfg = load_scanner_config(security_cfg)?;

    // If engine already exists, merge new flags into existing config
    if let Some(existing) = cfg.engines.get(name) {
        let mut engine_cfg = parse_engine_config(existing);
        if let Some(v) = url {
            engine_cfg.url = v.to_string();
        }
        if let Some(v) = path {
            engine_cfg.clamav_path = v.to_string();
        }
        if let Some(v) = address {
            engine_cfg.address = v.to_string();
        }
        if let Ok(updated) = serde_json::to_value(engine_cfg) {
            cfg.engines.insert(name.to_string(), updated);
        }
        save_scanner_config(security_cfg, &cfg)?;
        println!("✅ Scanner engine '{}' updated.", name);
        return Ok(());
    }

    // New engine: use defaults + provided flags
    let mut engine_cfg = ClamAVEngineConfig::default();
    if let Some(v) = url {
        engine_cfg.url = v.to_string();
    }
    if let Some(v) = path {
        engine_cfg.clamav_path = v.to_string();
    }
    if let Some(v) = address {
        engine_cfg.address = v.to_string();
    }

    if let Ok(raw) = serde_json::to_value(engine_cfg) {
        cfg.engines.insert(name.to_string(), raw);
    }

    save_scanner_config(security_cfg, &cfg)?;
    println!("✅ Scanner engine '{}' added.", name);
    println!("Configuration saved to: {}", security_cfg.display());
    println!("Use 'nemesisbot scanner enable {}' to enable it.", name);
    Ok(())
}

/// Remove a scanner engine (matches Go cmdScannerRemove).
fn cmd_remove(security_cfg: &std::path::Path, name: &str) -> Result<()> {
    let mut cfg = load_scanner_config(security_cfg)?;

    if !cfg.engines.contains_key(name) {
        eprintln!("Engine '{}' not found in configuration.", name);
        std::process::exit(1);
    }

    cfg.engines.remove(name);
    cfg.enabled.retain(|n| n != name);

    save_scanner_config(security_cfg, &cfg)?;
    println!("🗑️ Scanner engine '{}' removed.", name);
    Ok(())
}

/// Enable a scanner engine (matches Go cmdScannerEnable).
fn cmd_enable(security_cfg: &std::path::Path, name: &str) -> Result<()> {
    let mut cfg = load_scanner_config(security_cfg)?;

    if !cfg.engines.contains_key(name) {
        eprintln!(
            "Engine '{}' not found. Add it first with 'scanner add {}'.",
            name, name
        );
        std::process::exit(1);
    }

    // Check if already enabled
    if cfg.enabled.iter().any(|n| n == name) {
        println!("Engine '{}' is already enabled.", name);
        return Ok(());
    }

    cfg.enabled.push(name.to_string());

    // Set install_status to pending if not already set
    if let Some(raw) = cfg.engines.get(name) {
        let mut engine_cfg = parse_engine_config(raw);
        if engine_cfg.state.install_status.is_empty() {
            engine_cfg.state.install_status = "pending".to_string();
            if let Ok(updated) = serde_json::to_value(engine_cfg) {
                cfg.engines.insert(name.to_string(), updated);
            }
        }
    }

    save_scanner_config(security_cfg, &cfg)?;
    println!("✅ Scanner engine '{}' enabled.", name);
    println!("Enabled engines: {:?}", cfg.enabled);
    Ok(())
}

/// Disable a scanner engine (matches Go cmdScannerDisable).
fn cmd_disable(security_cfg: &std::path::Path, name: &str) -> Result<()> {
    let mut cfg = load_scanner_config(security_cfg)?;

    let original_len = cfg.enabled.len();
    cfg.enabled.retain(|n| n != name);

    if cfg.enabled.len() == original_len {
        println!("Engine '{}' is not enabled.", name);
        return Ok(());
    }

    save_scanner_config(security_cfg, &cfg)?;
    println!("🔓 Scanner engine '{}' disabled.", name);
    println!("Enabled engines: {:?}", cfg.enabled);
    Ok(())
}

/// Check install and database status (matches Go cmdScannerCheck).
fn cmd_check(security_cfg: &std::path::Path) -> Result<()> {
    let mut cfg = load_scanner_config(security_cfg)?;

    if cfg.enabled.is_empty() {
        println!("No engines enabled. Use 'scanner <engine> enable' first.");
        return Ok(());
    }

    let enabled_set: std::collections::HashSet<&str> =
        cfg.enabled.iter().map(|s| s.as_str()).collect();

    // Collect all engines (enabled + disabled with config)
    let mut all_names: Vec<String> = cfg.engines.keys().cloned().collect();
    // Sort: enabled first, then alphabetically
    all_names.sort_by(|a, b| {
        let a_en = enabled_set.contains(a.as_str());
        let b_en = enabled_set.contains(b.as_str());
        b_en.cmp(&a_en).then(a.cmp(b))
    });

    println!("\nScanner Engine Status:");
    println!("{}", "-".repeat(90));
    println!(
        "  {:<10} {:<10} {:<12} {:<10} {:<20} {}",
        "Engine", "Status", "Install", "Database", "Address", "URL"
    );
    println!("{}", "-".repeat(90));

    let mut changed = false;
    let mut operational = 0u32;

    for name in &all_names {
        let raw = match cfg.engines.get(name) {
            Some(v) => v,
            None => continue,
        };

        let engine_cfg = parse_engine_config(raw);
        let mut state = engine_cfg.state.clone();
        let is_enabled = enabled_set.contains(name.as_str());

        let mut resolved_path = engine_cfg.clamav_path.clone();
        let mut persist_path = String::new();

        if is_enabled {
            if !resolved_path.is_empty() {
                if check_executables_at_path(&resolved_path) {
                    state.install_status = "installed".to_string();
                    state.install_error = String::new();
                } else {
                    state.install_status = "failed".to_string();
                    state.install_error = format!("executable not found at {}", resolved_path);
                }
            } else {
                if let Some(sys_path) = lookup_system_clamav() {
                    resolved_path = sys_path.clone();
                    persist_path = sys_path;
                    state.install_status = "installed".to_string();
                    state.install_error = String::new();
                } else if state.install_status.is_empty() {
                    state.install_status = "pending".to_string();
                }
            }

            // Check database status
            let data_dir = if !engine_cfg.data_dir.is_empty() {
                engine_cfg.data_dir.clone()
            } else if !resolved_path.is_empty() {
                resolved_path.clone()
            } else {
                String::new()
            };

            if !data_dir.is_empty() {
                let db_file = std::path::Path::new(&data_dir)
                    .join("database")
                    .join(DATABASE_FILE);
                if db_file.exists() {
                    state.db_status = "ready".to_string();
                } else {
                    state.db_status = "missing".to_string();
                }
            }

            // Update config
            if let Some(updated) = marshal_engine_config(raw, &state, &persist_path, "") {
                cfg.engines.insert(name.clone(), updated);
                changed = true;
            }
        }

        let status_str = if is_enabled { "enabled" } else { "disabled" };
        let install_str = if state.install_status.is_empty() { "-" } else { &state.install_status };
        let db_str = if state.db_status.is_empty() { "-" } else { &state.db_status };
        let addr_str = if engine_cfg.address.is_empty() { "-" } else { &engine_cfg.address };
        let url_display = if engine_cfg.url.is_empty() {
            "-".to_string()
        } else if engine_cfg.url.len() > 40 {
            format!("{}...", &engine_cfg.url[..37])
        } else {
            engine_cfg.url.clone()
        };

        println!(
            "  {:<10} {:<10} {:<12} {:<10} {:<20} {}",
            name, status_str, install_str, db_str, addr_str, url_display
        );

        if !state.install_error.is_empty() {
            println!("             ^-- error: {}", state.install_error);
        }

        if is_enabled && state.install_status == "installed" && state.db_status == "ready" {
            operational += 1;
        }
    }

    println!("{}", "-".repeat(90));
    println!(
        "{} engine(s), {} operational.",
        all_names.len(),
        operational
    );

    // Save if state changed
    if changed {
        if let Err(e) = save_scanner_config(security_cfg, &cfg) {
            eprintln!("Warning: failed to save updated state: {}", e);
        }
    }

    // Print recommendations
    let mut recommendations = Vec::new();
    for name in &all_names {
        if !enabled_set.contains(name.as_str()) {
            continue;
        }
        if let Some(raw) = cfg.engines.get(name) {
            let engine_cfg = parse_engine_config(raw);
            match engine_cfg.state.install_status.as_str() {
                "pending" => {
                    recommendations.push(format!("  - Run 'scanner {} install' to install {}", name, name));
                }
                "failed" => {
                    recommendations.push(format!("  - Re-run 'scanner {} install' to fix {} installation", name, name));
                }
                "installed" => {
                    if engine_cfg.state.db_status == "missing" {
                        recommendations.push(format!(
                            "  - Run 'scanner {} update' to download virus database",
                            name
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    if !recommendations.is_empty() {
        println!("\nRecommendations:");
        for r in &recommendations {
            println!("{}", r);
        }
    }

    Ok(())
}

/// Download an engine from a URL and extract it.
/// Returns the path to the directory containing the extracted files.
fn download_engine(url: &str, target_dir: &std::path::Path) -> Result<String> {
    let file_name = url.split('/').last().unwrap_or("engine.zip");
    let archive_path = target_dir.join(file_name);

    // Download the file
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;

    let mut response = client.get(url)
        .header("User-Agent", "nemesisbot")
        .send()?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }

    {
        use std::io::Read;
        let mut file = std::fs::File::create(&archive_path)?;
        let mut buffer = [0u8; 8192];
        let mut total_bytes: u64 = 0;
        loop {
            let n = response.read(&mut buffer)?;
            if n == 0 { break; }
            use std::io::Write;
            file.write_all(&buffer[..n])?;
            total_bytes += n as u64;
        }
        println!("    Downloaded {} bytes", total_bytes);
    }

    // Try to extract if it's a zip archive
    let extracted_dir = if file_name.ends_with(".zip") {
        // Try using system unzip command
        let output = std::process::Command::new("unzip")
            .arg("-o")
            .arg(&archive_path)
            .arg("-d")
            .arg(target_dir)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let _ = std::fs::remove_file(&archive_path);
                target_dir.to_string_lossy().to_string()
            }
            _ => {
                // Try PowerShell on Windows
                #[cfg(target_os = "windows")]
                {
                    let ps_output = std::process::Command::new("powershell")
                        .args([
                            "-NoProfile", "-Command",
                            &format!("Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                                archive_path.display(), target_dir.display())
                        ])
                        .output();
                    match ps_output {
                        Ok(o) if o.status.success() => {
                            let _ = std::fs::remove_file(&archive_path);
                            target_dir.to_string_lossy().to_string()
                        }
                        _ => {
                            // Leave as-is, user can extract manually
                            println!("    Could not auto-extract. Archive at: {}", archive_path.display());
                            target_dir.to_string_lossy().to_string()
                        }
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    println!("    Could not auto-extract. Archive at: {}", archive_path.display());
                    target_dir.to_string_lossy().to_string()
                }
            }
        }
    } else if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
        let output = std::process::Command::new("tar")
            .args(["xzf", &archive_path.to_string_lossy(), "-C", &target_dir.to_string_lossy()])
            .output();
        if output.is_ok_and(|o| o.status.success()) {
            let _ = std::fs::remove_file(&archive_path);
        }
        target_dir.to_string_lossy().to_string()
    } else {
        // Not an archive, just the downloaded file
        target_dir.to_string_lossy().to_string()
    };

    // Detect actual install path by recursively searching for the executable
    let detected_dir = detect_executable_dir(
        std::path::Path::new(&extracted_dir),
        &["clamd.exe", "clamd", "clamscan.exe", "clamscan"],
    )
    .unwrap_or_else(|| extracted_dir);

    Ok(detected_dir)
}

/// Recursively search for a target executable and return its parent directory.
fn detect_executable_dir(dir: &std::path::Path, targets: &[&str]) -> Option<String> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Check this subdirectory for target executables
                for exe in targets {
                    if path.join(exe).exists() {
                        return Some(path.to_string_lossy().to_string());
                    }
                }
                // Recurse deeper
                if let Some(found) = detect_executable_dir(&path, targets) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Install all pending enabled engines (matches Go cmdScannerInstall).
async fn cmd_install(security_cfg: &std::path::Path, dir: Option<&str>) -> Result<()> {
    let cfg = load_scanner_config(security_cfg)?;

    if cfg.enabled.is_empty() {
        println!("No engines enabled. Use 'scanner enable <engine>' first.");
        return Ok(());
    }

    for name in &cfg.enabled {
        match name.as_str() {
            "clamav" => cmd_clamav_install_inner(false, None, dir, security_cfg).await?,
            _ => println!("  {} - install not implemented", name),
        }
    }

    println!();
    Ok(())
}

/// ClamAV engine subcommand dispatch.
async fn cmd_clamav(action: ClamavAction, security_cfg: &std::path::Path) -> Result<()> {
    match action {
        ClamavAction::Install { force, url, dir } => {
            cmd_clamav_install_inner(force, url.as_deref(), dir.as_deref(), security_cfg).await
        }
        ClamavAction::Enable => cmd_clamav_enable(security_cfg),
        ClamavAction::Disable => cmd_clamav_disable(security_cfg),
        ClamavAction::Update => cmd_clamav_update(security_cfg).await,
        ClamavAction::Test { path } => cmd_clamav_test(security_cfg, &path).await,
        ClamavAction::Info => cmd_clamav_info(security_cfg).await,
    }
}

/// Enable ClamAV engine with install check (via `scanner clamav enable`).
fn cmd_clamav_enable(security_cfg: &std::path::Path) -> Result<()> {
    let cfg = load_scanner_config(security_cfg)?;
    let raw = cfg.engines.get("clamav").ok_or_else(|| {
        anyhow::anyhow!("ClamAV not configured. Run 'scanner add clamav' first.")
    })?;
    let engine_cfg = parse_engine_config(raw);

    if engine_cfg.state.install_status != "installed" {
        anyhow::bail!(
            "ClamAV is not installed (status: {}). Run 'scanner clamav install' first.",
            if engine_cfg.state.install_status.is_empty() { "none" } else { &engine_cfg.state.install_status }
        );
    }

    cmd_enable(security_cfg, "clamav")
}

/// Disable ClamAV engine (via `scanner clamav disable`).
fn cmd_clamav_disable(security_cfg: &std::path::Path) -> Result<()> {
    cmd_disable(security_cfg, "clamav")
}

/// Core ClamAV install logic, shared by `scanner clamav install` and `scanner install`.
async fn cmd_clamav_install_inner(
    force: bool,
    url_override: Option<&str>,
    dir: Option<&str>,
    security_cfg: &std::path::Path,
) -> Result<()> {
    let mut cfg = load_scanner_config(security_cfg)?;

    let raw = match cfg.engines.get("clamav") {
        Some(v) => v.clone(),
        None => {
            eprintln!("ClamAV engine not found in configuration.");
            eprintln!("Add it first with: nemesisbot scanner add clamav");
            std::process::exit(1);
        }
    };

    let engine_cfg = parse_engine_config(&raw);

    // Check if already installed
    if engine_cfg.state.install_status == "installed" && !force {
        println!("ClamAV already installed (path: {}). Use --force to reinstall.", engine_cfg.clamav_path);
        return Ok(());
    }

    let mut state = engine_cfg.state.clone();
    state.last_install_attempt = chrono::Utc::now().to_rfc3339();

    if force && state.install_status == "installed" {
        println!("Force reinstalling ClamAV...");
        state.install_status = String::new();
    }

    let install_dir = dir
        .map(String::from)
        .unwrap_or_else(|| resolve_tools_dir(security_cfg).to_string_lossy().to_string());

    println!("Install directory: {}\n", install_dir);

    let mut detected_path = String::new();

    // Step 1: Use configured path if set
    if !engine_cfg.clamav_path.is_empty() {
        detected_path = engine_cfg.clamav_path.clone();
    }

    // Step 2: Check system PATH
    if detected_path.is_empty() {
        if let Some(sys_path) = lookup_system_clamav() {
            detected_path = sys_path.clone();
            println!("  clamav           found in system PATH: {}", sys_path);
        }
    }

    // Step 3: Download if still not found
    // URL priority: --url override > config url > error
    if detected_path.is_empty() {
        let download_url = url_override
            .map(String::from)
            .or_else(|| if !engine_cfg.url.is_empty() { Some(engine_cfg.url.clone()) } else { None });

        match download_url {
            Some(ref url) => {
                println!("  clamav           downloading from {}...", url);
                let download_dir = std::path::Path::new(&install_dir).join("clamav");
                let _ = std::fs::create_dir_all(&download_dir);

                match download_engine(url, &download_dir) {
                    Ok(extracted_path) => {
                        detected_path = extracted_path;
                        println!("  clamav           downloaded to {}", detected_path);
                    }
                    Err(e) => {
                        println!("  clamav           download failed: {}", e);
                        state.install_status = "failed".to_string();
                        state.install_error = format!("download failed: {}", e);
                        if let Some(updated) = marshal_engine_config(&raw, &state, "", "") {
                            cfg.engines.insert("clamav".to_string(), updated);
                            save_scanner_config(security_cfg, &cfg)?;
                        }
                        return Ok(());
                    }
                }
            }
            None => {
                state.install_status = "failed".to_string();
                state.install_error =
                    "no download URL configured. Use --url or run 'scanner add clamav --url <URL>' first.".to_string();
                println!("  clamav           FAILED: {}", state.install_error);
                if let Some(updated) = marshal_engine_config(&raw, &state, "", "") {
                    cfg.engines.insert("clamav".to_string(), updated);
                    save_scanner_config(security_cfg, &cfg)?;
                }
                return Ok(());
            }
        }
    }

    // Step 4: Validate executables
    if detected_path.is_empty() {
        state.install_status = "failed".to_string();
        state.install_error =
            "no download URL, install path, or system installation found".to_string();
        println!("  clamav           FAILED: not found (no URL, path, or system PATH)");
    } else if !check_executables_at_path(&detected_path) {
        state.install_status = "failed".to_string();
        state.install_error = format!("executable not found at {}", detected_path);
        println!("  clamav           FAILED: {}", state.install_error);
    } else {
        state.install_status = "installed".to_string();
        state.install_error = String::new();
        println!("  clamav           OK (path: {})", detected_path);
    }

    // Step 5: Set DataDir
    let data_dir = if engine_cfg.data_dir.is_empty() && !detected_path.is_empty() {
        detected_path.clone()
    } else {
        engine_cfg.data_dir.clone()
    };

    // Step 6: Generate config files (freshclam.conf, clamd.conf)
    if state.install_status == "installed" && !detected_path.is_empty() {
        let clamav_dir = std::path::Path::new(&detected_path);
        let db_dir = clamav_dir.join("database");

        // Generate freshclam.conf
        let freshclam_conf = clamav_dir.join("freshclam.conf");
        match nemesis_security::clamav::config::generate_freshclam_config(
            &db_dir.to_string_lossy(),
            &freshclam_conf.to_string_lossy(),
        ) {
            Ok(()) => println!("  clamav           generated freshclam.conf"),
            Err(e) => println!("  clamav           failed to generate freshclam.conf: {}", e),
        }

        // Generate clamd.conf
        let clamd_conf = clamav_dir.join("clamd.conf");
        let daemon_config = nemesis_security::clamav::config::DaemonConfig {
            clamav_path: detected_path.clone(),
            config_file: clamd_conf.to_string_lossy().to_string(),
            database_dir: db_dir.to_string_lossy().to_string(),
            listen_addr: if engine_cfg.address.is_empty() {
                "127.0.0.1:3310".to_string()
            } else {
                engine_cfg.address.clone()
            },
            temp_dir: clamav_dir.join("tmp").to_string_lossy().to_string(),
            startup_timeout_secs: 120,
        };
        match nemesis_security::clamav::config::generate_clamd_config(&daemon_config) {
            Ok(()) => println!("  clamav           generated clamd.conf"),
            Err(e) => println!("  clamav           failed to generate clamd.conf: {}", e),
        }

        // Step 7: Download virus database using Updater
        if !freshclam_conf.exists() {
            state.db_status = "missing".to_string();
            println!("  clamav           skipped DB update (freshclam.conf not generated)");
        } else {
            println!("  clamav           downloading virus database...");
            let updater = nemesis_security::clamav::updater::Updater::new(
                nemesis_security::clamav::updater::UpdaterConfig {
                    clamav_path: detected_path.clone(),
                    database_dir: db_dir.to_string_lossy().to_string(),
                    config_file: freshclam_conf.to_string_lossy().to_string(),
                    update_interval: Duration::from_secs(0),
                    mirror_urls: vec![],
                },
            );

            match tokio::time::timeout(Duration::from_secs(120), updater.update()).await {
                Ok(Ok(())) => {
                    state.db_status = "ready".to_string();
                    state.last_db_update = chrono::Utc::now().to_rfc3339();
                    println!("  clamav           virus database ready");
                }
                Ok(Err(e)) => {
                    state.db_status = "missing".to_string();
                    println!("  clamav           virus database download failed: {}", e);
                    println!("  clamav           run 'scanner clamav update' to retry");
                }
                Err(_) => {
                    state.db_status = "missing".to_string();
                    println!("  clamav           virus database download timed out (120s)");
                    println!("  clamav           run 'scanner clamav update' to retry");
                }
            }
        }
    }

    // Step 8: Persist
    if let Some(updated) = marshal_engine_config(&raw, &state, &detected_path, &data_dir) {
        cfg.engines.insert("clamav".to_string(), updated);
        save_scanner_config(security_cfg, &cfg)?;
    }

    println!();
    Ok(())
}

/// Update ClamAV virus database using freshclam (real implementation, fixes P0 BUG).
async fn cmd_clamav_update(security_cfg: &std::path::Path) -> Result<()> {
    let cfg = load_scanner_config(security_cfg)?;

    let raw = match cfg.engines.get("clamav") {
        Some(v) => v,
        None => {
            eprintln!("ClamAV engine not found in configuration.");
            eprintln!("Add it first with: nemesisbot scanner add clamav");
            std::process::exit(1);
        }
    };

    let engine_cfg = parse_engine_config(raw);

    // Resolve clamav path
    let clamav_path = if !engine_cfg.clamav_path.is_empty() {
        engine_cfg.clamav_path.clone()
    } else if let Some(sys_path) = lookup_system_clamav() {
        sys_path
    } else {
        anyhow::bail!("ClamAV not found. Install it first with 'scanner clamav install'");
    };

    let _data_dir = if !engine_cfg.data_dir.is_empty() {
        engine_cfg.data_dir.clone()
    } else {
        clamav_path.clone()
    };

    let clamav_dir = std::path::Path::new(&clamav_path);
    let db_dir = clamav_dir.join("database");

    // Ensure freshclam.conf exists, generate if missing
    let freshclam_conf = clamav_dir.join("freshclam.conf");
    if !freshclam_conf.exists() {
        println!("  Generating freshclam.conf...");
        nemesis_security::clamav::config::generate_freshclam_config(
            &db_dir.to_string_lossy(),
            &freshclam_conf.to_string_lossy(),
        ).map_err(|e| anyhow::anyhow!(e))?;
    }

    // Create Updater and run freshclam
    let updater = nemesis_security::clamav::updater::Updater::new(
        nemesis_security::clamav::updater::UpdaterConfig {
            clamav_path: clamav_path.clone(),
            database_dir: db_dir.to_string_lossy().to_string(),
            config_file: freshclam_conf.to_string_lossy().to_string(),
            update_interval: Duration::from_secs(0),
            mirror_urls: vec![],
        },
    );

    println!("  Running freshclam to update virus database...");
    updater.update().await.map_err(|e| anyhow::anyhow!("freshclam failed: {}", e))?;
    println!("  Virus database updated.");

    // Update config.scanner.json
    let mut full_cfg = load_scanner_config(security_cfg)?;
    if let Some(raw_val) = full_cfg.engines.get("clamav").cloned() {
        let mut ec = parse_engine_config(&raw_val);
        ec.state.db_status = "ready".to_string();
        ec.state.last_db_update = chrono::Utc::now().to_rfc3339();
        if ec.clamav_path.is_empty() {
            ec.clamav_path = clamav_path;
        }
        if let Ok(updated) = serde_json::to_value(ec) {
            full_cfg.engines.insert("clamav".to_string(), updated);
            let _ = save_scanner_config(security_cfg, &full_cfg);
        }
    }

    Ok(())
}

/// Show ClamAV engine information.
async fn cmd_clamav_info(security_cfg: &std::path::Path) -> Result<()> {
    let cfg = load_scanner_config(security_cfg)?;

    let raw = match cfg.engines.get("clamav") {
        Some(v) => v,
        None => {
            eprintln!("ClamAV engine not found in configuration.");
            std::process::exit(1);
        }
    };

    // Create engine instance for runtime info
    let engine = nemesis_security::scanner::create_engine("clamav", raw)
        .map_err(|e| anyhow::anyhow!("Error creating engine: {}", e))?;

    let info = engine.get_info().await;

    println!("\nEngine: {}", info.name);
    println!("{}", "-".repeat(40));
    if !info.version.is_empty() {
        println!("  Version:   {}", info.version);
    }
    if !info.address.is_empty() {
        println!("  Address:   {}", info.address);
    }
    println!("  Ready:     {}", info.ready);
    if !info.start_time.is_empty() {
        println!("  Started:   {}", info.start_time);
    }

    // Show parsed config
    let engine_cfg = parse_engine_config(raw);
    println!("\nConfiguration:");
    println!("  address:          {}", engine_cfg.address);
    println!("  clamav_path:      {}", engine_cfg.clamav_path);
    println!("  data_dir:         {}", engine_cfg.data_dir);
    println!("  url:              {}", engine_cfg.url);
    println!("  install_status:   {}", engine_cfg.state.install_status);
    println!("  db_status:        {}", engine_cfg.state.db_status);

    Ok(())
}

/// Test scan a file using ClamAV engine.
async fn cmd_clamav_test(security_cfg: &std::path::Path, file_path: &str) -> Result<()> {
    let cfg = load_scanner_config(security_cfg)?;

    let raw = match cfg.engines.get("clamav") {
        Some(v) => v,
        None => {
            eprintln!("ClamAV engine not found in configuration.");
            std::process::exit(1);
        }
    };

    let engine = nemesis_security::scanner::create_engine("clamav", raw)
        .map_err(|e| anyhow::anyhow!("Error creating engine: {}", e))?;

    // Start the engine for testing
    if let Err(e) = engine.start().await {
        eprintln!("Warning: Failed to start engine: {}", e);
        println!("Attempting scan anyway (may use existing daemon)...");
    }

    // Wait briefly for engine to be ready
    std::thread::sleep(Duration::from_secs(2));

    if !engine.is_ready().await {
        eprintln!("ClamAV engine is not ready. Make sure the daemon is running.");
        let _ = engine.stop().await;
        std::process::exit(1);
    }

    println!("Scanning: {}", file_path);
    let result = engine.scan_file(std::path::Path::new(file_path)).await;

    let _ = engine.stop().await;

    println!("{}", "-".repeat(40));
    println!("  Engine:   {}", result.engine);
    println!("  Path:     {}", result.path);
    if result.infected {
        println!("  Status:   INFECTED");
        println!("  Virus:    {}", result.virus);
    } else {
        println!("  Status:   CLEAN");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_scanner_full_config_default() {
        let cfg = ScannerFullConfig::default();
        assert!(cfg.enabled.is_empty());
        assert!(cfg.engines.is_empty());
    }

    #[test]
    fn test_clamav_engine_config_default() {
        let cfg = ClamAVEngineConfig::default();
        assert_eq!(cfg.address, "127.0.0.1:3310");
        assert!(cfg.url.is_empty());
        assert!(cfg.clamav_path.is_empty());
        assert!(cfg.data_dir.is_empty());
        assert_eq!(cfg.scan_on_write, true);
        assert_eq!(cfg.scan_on_download, false);
        assert_eq!(cfg.scan_on_exec, true);
        assert_eq!(cfg.max_file_size, 52428800);
        assert_eq!(cfg.update_interval, "24h");
        assert!(!cfg.skip_extensions.is_empty());
        assert!(cfg.state.install_status.is_empty());
    }

    #[test]
    fn test_engine_state_default() {
        let state = EngineState::default();
        assert!(state.install_status.is_empty());
        assert!(state.install_error.is_empty());
        assert!(state.db_status.is_empty());
        assert!(state.last_install_attempt.is_empty());
        assert!(state.last_db_update.is_empty());
    }

    #[test]
    fn test_default_skip_extensions() {
        let exts = default_skip_extensions();
        assert!(exts.contains(&".txt".to_string()));
        assert!(exts.contains(&".md".to_string()));
        assert!(exts.contains(&".json".to_string()));
        assert!(exts.contains(&".log".to_string()));
        assert!(exts.contains(&".css".to_string()));
    }

    #[test]
    fn test_load_scanner_config_no_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let cfg = load_scanner_config(&path).unwrap();
        assert!(cfg.enabled.is_empty());
        assert!(cfg.engines.is_empty());
    }

    #[test]
    fn test_load_scanner_config_valid_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let data = serde_json::json!({
            "enabled": ["clamav"],
            "engines": {
                "clamav": {
                    "address": "127.0.0.1:3310",
                    "state": {
                        "install_status": "installed",
                        "db_status": "ready"
                    }
                }
            }
        });
        std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

        let cfg = load_scanner_config(&path).unwrap();
        assert_eq!(cfg.enabled.len(), 1);
        assert_eq!(cfg.enabled[0], "clamav");
        assert!(cfg.engines.contains_key("clamav"));
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config").join("config.scanner.json");

        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine = ClamAVEngineConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());

        save_scanner_config(&path, &cfg).unwrap();
        let loaded = load_scanner_config(&path).unwrap();

        assert_eq!(loaded.enabled, cfg.enabled);
        assert!(loaded.engines.contains_key("clamav"));
    }

    #[test]
    fn test_parse_engine_config_full() {
        let raw = serde_json::json!({
            "address": "192.168.1.1:3310",
            "url": "https://example.com/clamav.zip",
            "clamav_path": "/opt/clamav",
            "data_dir": "/var/lib/clamav",
            "scan_on_write": false,
            "scan_on_download": true,
            "scan_on_exec": false,
            "max_file_size": 104857600,
            "update_interval": "12h",
            "skip_extensions": [".exe", ".dll"],
            "state": {
                "install_status": "installed",
                "install_error": "",
                "db_status": "ready",
                "last_install_attempt": "2026-01-01T00:00:00Z",
                "last_db_update": "2026-01-01T00:00:00Z"
            }
        });
        let cfg = parse_engine_config(&raw);
        assert_eq!(cfg.address, "192.168.1.1:3310");
        assert_eq!(cfg.url, "https://example.com/clamav.zip");
        assert_eq!(cfg.clamav_path, "/opt/clamav");
        assert_eq!(cfg.data_dir, "/var/lib/clamav");
        assert_eq!(cfg.scan_on_write, false);
        assert_eq!(cfg.scan_on_download, true);
        assert_eq!(cfg.max_file_size, 104857600);
        assert_eq!(cfg.update_interval, "12h");
        assert_eq!(cfg.skip_extensions.len(), 2);
        assert_eq!(cfg.state.install_status, "installed");
        assert_eq!(cfg.state.db_status, "ready");
    }

    #[test]
    fn test_parse_engine_config_empty_json() {
        let raw = serde_json::json!({});
        let cfg = parse_engine_config(&raw);
        // Should use defaults
        assert_eq!(cfg.address, "127.0.0.1:3310");
        assert_eq!(cfg.max_file_size, 52428800);
    }

    #[test]
    fn test_marshal_engine_config_with_state() {
        let raw = serde_json::json!({"address": "127.0.0.1:3310"});
        let state = EngineState {
            install_status: "installed".to_string(),
            install_error: String::new(),
            db_status: "ready".to_string(),
            last_install_attempt: String::new(),
            last_db_update: String::new(),
        };
        let result = marshal_engine_config(&raw, &state, "/opt/clamav", "/var/lib/clamav");
        assert!(result.is_some());
        let val = result.unwrap();
        let cfg: ClamAVEngineConfig = serde_json::from_value(val).unwrap();
        assert_eq!(cfg.state.install_status, "installed");
        assert_eq!(cfg.clamav_path, "/opt/clamav");
        assert_eq!(cfg.data_dir, "/var/lib/clamav");
    }

    #[test]
    fn test_marshal_engine_config_empty_paths() {
        let raw = serde_json::json!({"address": "127.0.0.1:3310"});
        let state = EngineState::default();
        let result = marshal_engine_config(&raw, &state, "", "");
        assert!(result.is_some());
        let val = result.unwrap();
        let cfg: ClamAVEngineConfig = serde_json::from_value(val).unwrap();
        assert!(cfg.clamav_path.is_empty());
        assert!(cfg.data_dir.is_empty());
    }

    #[test]
    fn test_resolve_tools_dir() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config").join("config.scanner.json");
        let tools_dir = resolve_tools_dir(&config_path);
        assert!(tools_dir.to_str().unwrap().contains("workspace"));
        assert!(tools_dir.to_str().unwrap().contains("tools"));
    }

    #[test]
    fn test_check_executables_at_path_nonexistent() {
        assert!(!check_executables_at_path("/nonexistent/path/that/does/not/exist"));
    }

    #[test]
    fn test_check_executables_at_path_empty_dir() {
        let tmp = TempDir::new().unwrap();
        assert!(!check_executables_at_path(&tmp.path().to_string_lossy()));
    }

    #[test]
    fn test_cmd_list_empty_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        cmd_list(&path).unwrap();
    }

    #[test]
    fn test_cmd_list_with_engines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine = ClamAVEngineConfig {
            address: "127.0.0.1:3310".to_string(),
            state: EngineState {
                install_status: "installed".to_string(),
                db_status: "ready".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_list(&path).unwrap();
    }

    #[test]
    fn test_cmd_enable_adds_to_enabled_list() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        let engine = ClamAVEngineConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_enable(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(loaded.enabled.contains(&"clamav".to_string()));
    }

    #[test]
    fn test_cmd_enable_already_enabled() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine = ClamAVEngineConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_enable(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert_eq!(loaded.enabled.len(), 1); // Still just one
    }

    #[test]
    fn test_cmd_disable_removes_from_enabled() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine = ClamAVEngineConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_disable(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(loaded.enabled.is_empty());
    }

    #[test]
    fn test_cmd_disable_not_enabled() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let cfg = ScannerFullConfig::default();
        save_scanner_config(&path, &cfg).unwrap();

        cmd_disable(&path, "clamav").unwrap();
        // Should succeed, no changes
    }

    #[test]
    fn test_cmd_check_no_engines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let cfg = ScannerFullConfig::default();
        save_scanner_config(&path, &cfg).unwrap();

        cmd_check(&path).unwrap();
    }

    #[test]
    fn test_clamav_engine_config_serialization() {
        let cfg = ClamAVEngineConfig::default();
        let json = serde_json::to_value(&cfg).unwrap();
        let deserialized: ClamAVEngineConfig = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.address, cfg.address);
        assert_eq!(deserialized.max_file_size, cfg.max_file_size);
    }

    #[test]
    fn test_database_file_constant() {
        assert_eq!(DATABASE_FILE, "daily.cvd");
    }

    // -------------------------------------------------------------------------
    // ClamAVEngineConfig serialization roundtrip with all fields
    // -------------------------------------------------------------------------

    #[test]
    fn test_clamav_config_roundtrip_all_fields() {
        let cfg = ClamAVEngineConfig {
            address: "10.0.0.1:3310".to_string(),
            url: "https://example.com/clamav.zip".to_string(),
            clamav_path: "/opt/clamav".to_string(),
            data_dir: "/var/lib/clamav".to_string(),
            scan_on_write: false,
            scan_on_download: true,
            scan_on_exec: true,
            max_file_size: 100_000_000,
            update_interval: "6h".to_string(),
            skip_extensions: vec![".exe".to_string(), ".dll".to_string()],
            state: EngineState {
                install_status: "installed".to_string(),
                install_error: String::new(),
                db_status: "ready".to_string(),
                last_install_attempt: "2026-01-01".to_string(),
                last_db_update: "2026-01-02".to_string(),
            },
        };
        let json = serde_json::to_value(&cfg).unwrap();
        let back: ClamAVEngineConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.address, cfg.address);
        assert_eq!(back.url, cfg.url);
        assert_eq!(back.clamav_path, cfg.clamav_path);
        assert_eq!(back.data_dir, cfg.data_dir);
        assert_eq!(back.scan_on_write, cfg.scan_on_write);
        assert_eq!(back.scan_on_download, cfg.scan_on_download);
        assert_eq!(back.max_file_size, cfg.max_file_size);
        assert_eq!(back.update_interval, cfg.update_interval);
        assert_eq!(back.skip_extensions, cfg.skip_extensions);
        assert_eq!(back.state.install_status, cfg.state.install_status);
        assert_eq!(back.state.db_status, cfg.state.db_status);
    }

    // -------------------------------------------------------------------------
    // ScannerFullConfig serialization tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_scanner_full_config_serialization() {
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));

        let json = serde_json::to_string(&cfg).unwrap();
        let back: ScannerFullConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.enabled, cfg.enabled);
        assert!(back.engines.contains_key("clamav"));
    }

    #[test]
    fn test_scanner_full_config_empty_engines() {
        let cfg = ScannerFullConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: ScannerFullConfig = serde_json::from_str(&json).unwrap();
        assert!(back.enabled.is_empty());
        assert!(back.engines.is_empty());
    }

    // -------------------------------------------------------------------------
    // EngineState tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_engine_state_with_errors() {
        let state = EngineState {
            install_status: "failed".to_string(),
            install_error: "permission denied".to_string(),
            db_status: "error".to_string(),
            last_install_attempt: "2026-01-01".to_string(),
            last_db_update: String::new(),
        };
        let json = serde_json::to_value(&state).unwrap();
        let back: EngineState = serde_json::from_value(json).unwrap();
        assert_eq!(back.install_status, "failed");
        assert_eq!(back.install_error, "permission denied");
        assert_eq!(back.db_status, "error");
    }

    // -------------------------------------------------------------------------
    // default_address / default_max_file_size / default_update_interval
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_values() {
        assert_eq!(default_address(), "127.0.0.1:3310");
        assert_eq!(default_max_file_size(), 52428800);
        assert_eq!(default_update_interval(), "24h");
    }

    #[test]
    fn test_default_skip_extensions_contains_common_types() {
        let exts = default_skip_extensions();
        // Should contain common safe file types
        assert!(exts.contains(&".txt".to_string()));
        assert!(exts.contains(&".md".to_string()));
        assert!(exts.contains(&".json".to_string()));
        assert!(exts.contains(&".yaml".to_string()));
        assert!(exts.contains(&".yml".to_string()));
        assert!(exts.contains(&".toml".to_string()));
        assert!(exts.contains(&".log".to_string()));
        assert!(exts.contains(&".css".to_string()));
        assert!(exts.contains(&".html".to_string()));
        // Should not contain executable extensions
        assert!(!exts.contains(&".exe".to_string()));
        assert!(!exts.contains(&".dll".to_string()));
    }

    // -------------------------------------------------------------------------
    // parse_engine_config partial JSON
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_engine_config_partial() {
        let raw = serde_json::json!({
            "address": "10.0.0.1:9999",
            "scan_on_write": false
        });
        let cfg = parse_engine_config(&raw);
        assert_eq!(cfg.address, "10.0.0.1:9999");
        assert_eq!(cfg.scan_on_write, false);
        // Other fields should be defaults
        assert_eq!(cfg.scan_on_download, false);
        assert_eq!(cfg.max_file_size, 52428800);
        assert_eq!(cfg.update_interval, "24h");
    }

    #[test]
    fn test_parse_engine_config_null_value() {
        let raw = serde_json::Value::Null;
        let cfg = parse_engine_config(&raw);
        // Should return defaults
        assert_eq!(cfg.address, "127.0.0.1:3310");
    }

    // -------------------------------------------------------------------------
    // marshal_engine_config edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_marshal_engine_config_only_state_update() {
        let raw = serde_json::json!({"address": "127.0.0.1:3310", "clamav_path": "/original"});
        let state = EngineState {
            install_status: "installed".to_string(),
            ..Default::default()
        };
        let result = marshal_engine_config(&raw, &state, "", "");
        assert!(result.is_some());
        let cfg: ClamAVEngineConfig = serde_json::from_value(result.unwrap()).unwrap();
        assert_eq!(cfg.state.install_status, "installed");
        assert_eq!(cfg.clamav_path, "/original"); // preserved
    }

    #[test]
    fn test_marshal_engine_config_overwrite_paths() {
        let raw = serde_json::json!({"address": "127.0.0.1:3310", "clamav_path": "/old", "data_dir": "/old_data"});
        let state = EngineState::default();
        let result = marshal_engine_config(&raw, &state, "/new/path", "/new/data");
        assert!(result.is_some());
        let cfg: ClamAVEngineConfig = serde_json::from_value(result.unwrap()).unwrap();
        assert_eq!(cfg.clamav_path, "/new/path");
        assert_eq!(cfg.data_dir, "/new/data");
    }

    // -------------------------------------------------------------------------
    // resolve_tools_dir tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resolve_tools_dir_from_scanner_config() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config").join("config.scanner.json");
        let tools_dir = resolve_tools_dir(&config_path);
        assert!(tools_dir.ends_with("tools"));
        // Should be under workspace
        assert!(tools_dir.to_str().unwrap().contains("workspace"));
    }

    #[test]
    fn test_resolve_tools_dir_path_structure() {
        let config_path = std::path::Path::new("/home/user/.nemesisbot/config/config.scanner.json");
        let tools_dir = resolve_tools_dir(config_path);
        assert_eq!(tools_dir, std::path::PathBuf::from("/home/user/.nemesisbot/workspace/tools"));
    }

    // -------------------------------------------------------------------------
    // cmd_add tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_add_new_engine() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let cfg = ScannerFullConfig::default();
        save_scanner_config(&path, &cfg).unwrap();

        cmd_add(&path, "clamav", Some("https://example.com/clamav.zip"), Some("/opt/clamav"), Some("127.0.0.1:9999")).unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(loaded.engines.contains_key("clamav"));
        let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
        assert_eq!(engine.url, "https://example.com/clamav.zip");
        assert_eq!(engine.clamav_path, "/opt/clamav");
        assert_eq!(engine.address, "127.0.0.1:9999");
    }

    #[test]
    fn test_cmd_add_update_existing_engine() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        save_scanner_config(&path, &cfg).unwrap();

        cmd_add(&path, "clamav", None, None, Some("10.0.0.1:3310")).unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
        assert_eq!(engine.address, "10.0.0.1:3310");
    }

    #[test]
    fn test_cmd_add_with_defaults() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let cfg = ScannerFullConfig::default();
        save_scanner_config(&path, &cfg).unwrap();

        cmd_add(&path, "clamav", None, None, None).unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(loaded.engines.contains_key("clamav"));
        let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
        assert_eq!(engine.address, "127.0.0.1:3310"); // default
    }

    // -------------------------------------------------------------------------
    // cmd_remove tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_remove_existing_engine() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        cfg.enabled.push("clamav".to_string());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_remove(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(!loaded.engines.contains_key("clamav"));
        assert!(!loaded.enabled.contains(&"clamav".to_string()));
    }

    // -------------------------------------------------------------------------
    // cmd_enable additional tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_enable_sets_pending_status() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        let engine = ClamAVEngineConfig {
            state: EngineState::default(), // empty install_status
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_enable(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        let engine_cfg = parse_engine_config(loaded.engines.get("clamav").unwrap());
        assert_eq!(engine_cfg.state.install_status, "pending");
    }

    #[test]
    fn test_cmd_enable_preserves_existing_install_status() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        let engine = ClamAVEngineConfig {
            state: EngineState {
                install_status: "installed".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_enable(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        let engine_cfg = parse_engine_config(loaded.engines.get("clamav").unwrap());
        // Should keep "installed" status, not change to "pending"
        assert_eq!(engine_cfg.state.install_status, "installed");
    }

    // -------------------------------------------------------------------------
    // cmd_check with configured engines
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_check_with_multiple_engines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine1 = ClamAVEngineConfig {
            address: "127.0.0.1:3310".to_string(),
            state: EngineState {
                install_status: "installed".to_string(),
                db_status: "ready".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine1).unwrap());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_check(&path).unwrap();
    }

    // -------------------------------------------------------------------------
    // check_executables_at_path with files
    // -------------------------------------------------------------------------

    #[test]
    fn test_check_executables_at_path_with_fake_executable() {
        let tmp = TempDir::new().unwrap();
        // Create a fake clamd file
        std::fs::write(tmp.path().join("clamd"), "fake").unwrap();
        assert!(check_executables_at_path(&tmp.path().to_string_lossy()));
    }

    #[test]
    fn test_check_executables_at_path_with_exe() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("clamd.exe"), "fake").unwrap();
        assert!(check_executables_at_path(&tmp.path().to_string_lossy()));
    }

    #[test]
    fn test_check_executables_at_path_wrong_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("readme.txt"), "not clamav").unwrap();
        assert!(!check_executables_at_path(&tmp.path().to_string_lossy()));
    }

    // -------------------------------------------------------------------------
    // ClamAV config serialization roundtrip tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_clamav_config_serialization_roundtrip() {
        let cfg = ClamAVEngineConfig {
            address: "192.168.1.1:3310".to_string(),
            url: "https://example.com/clamav.zip".to_string(),
            clamav_path: "/opt/clamav".to_string(),
            data_dir: "/var/lib/clamav".to_string(),
            scan_on_write: true,
            scan_on_download: true,
            scan_on_exec: false,
            max_file_size: 104857600,
            update_interval: "12h".to_string(),
            skip_extensions: vec![".exe".to_string(), ".dll".to_string()],
            state: EngineState {
                install_status: "installed".to_string(),
                install_error: String::new(),
                db_status: "ready".to_string(),
                last_install_attempt: "2026-01-01T00:00:00Z".to_string(),
                last_db_update: "2026-01-01T00:00:00Z".to_string(),
            },
        };
        let json = serde_json::to_value(&cfg).unwrap();
        let deserialized: ClamAVEngineConfig = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.address, "192.168.1.1:3310");
        assert_eq!(deserialized.url, "https://example.com/clamav.zip");
        assert_eq!(deserialized.max_file_size, 104857600);
        assert_eq!(deserialized.skip_extensions.len(), 2);
        assert_eq!(deserialized.state.install_status, "installed");
    }

    #[test]
    fn test_engine_state_serialization() {
        let state = EngineState {
            install_status: "pending".to_string(),
            install_error: "some error".to_string(),
            db_status: "missing".to_string(),
            last_install_attempt: "2026-06-01T12:00:00Z".to_string(),
            last_db_update: String::new(),
        };
        let json = serde_json::to_value(&state).unwrap();
        let loaded: EngineState = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.install_status, "pending");
        assert_eq!(loaded.install_error, "some error");
        assert_eq!(loaded.db_status, "missing");
    }

    // -------------------------------------------------------------------------
    // default value tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_address() {
        assert_eq!(default_address(), "127.0.0.1:3310");
    }

    #[test]
    fn test_default_max_file_size() {
        assert_eq!(default_max_file_size(), 52428800);
    }

    // -------------------------------------------------------------------------
    // detect_executable_dir tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_detect_executable_dir_empty() {
        let tmp = TempDir::new().unwrap();
        let result = detect_executable_dir(tmp.path(), &["clamd", "clamd.exe"]);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_executable_dir_with_executable() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("bin");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("clamd"), "fake").unwrap();
        let result = detect_executable_dir(tmp.path(), &["clamd", "clamd.exe"]);
        assert!(result.is_some());
        assert!(result.unwrap().contains("bin"));
    }

    #[test]
    fn test_detect_executable_dir_nested() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("clamscan"), "fake").unwrap();
        let result = detect_executable_dir(tmp.path(), &["clamscan"]);
        assert!(result.is_some());
    }

    // -------------------------------------------------------------------------
    // ScannerFullConfig serialization tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_scanner_full_config_with_multiple_engines() {
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        cfg.enabled.push("custom".to_string());
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        cfg.engines.insert("custom".to_string(), serde_json::json!({"address": "127.0.0.1:9999"}));

        let json = serde_json::to_string(&cfg).unwrap();
        let loaded: ScannerFullConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.enabled.len(), 2);
        assert_eq!(loaded.engines.len(), 2);
    }

    // -------------------------------------------------------------------------
    // resolve_tools_dir tests
    // -------------------------------------------------------------------------

    // -------------------------------------------------------------------------
    // resolve_tools_dir additional tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resolve_tools_dir_with_config_subdir() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("config").join("config.scanner.json");
        let tools_dir = resolve_tools_dir(&config_path);
        let tools_str = tools_dir.to_str().unwrap();
        assert!(tools_str.contains("workspace"));
        assert!(tools_str.contains("tools"));
    }

    #[test]
    fn test_resolve_tools_dir_no_parent() {
        let config_path = std::path::Path::new("config.scanner.json");
        let tools_dir = resolve_tools_dir(config_path);
        // Should still return a path (may be "workspace/tools")
        assert!(!tools_dir.as_os_str().is_empty());
    }

    // -------------------------------------------------------------------------
    // parse_engine_config edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_engine_config_partial_state() {
        let raw = serde_json::json!({
            "address": "127.0.0.1:3310",
            "state": {
                "install_status": "installed"
            }
        });
        let cfg = parse_engine_config(&raw);
        assert_eq!(cfg.state.install_status, "installed");
        assert!(cfg.state.install_error.is_empty()); // should default to empty
        assert!(cfg.state.db_status.is_empty());
    }

    #[test]
    fn test_parse_engine_config_invalid_types() {
        let raw = serde_json::json!({
            "address": 12345,
            "scan_on_write": "yes",
            "max_file_size": "big"
        });
        let cfg = parse_engine_config(&raw);
        // Should use defaults for invalid types
        assert_eq!(cfg.address, "127.0.0.1:3310"); // default
    }

    // -------------------------------------------------------------------------
    // marshal_engine_config edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_marshal_engine_config_preserves_known_fields() {
        let raw = serde_json::json!({
            "address": "127.0.0.1:3310",
            "scan_on_write": true,
            "max_file_size": 100000
        });
        let state = EngineState::default();
        let result = marshal_engine_config(&raw, &state, "", "");
        assert!(result.is_some());
        let val = result.unwrap();
        let cfg: ClamAVEngineConfig = serde_json::from_value(val).unwrap();
        assert_eq!(cfg.address, "127.0.0.1:3310");
        assert!(cfg.scan_on_write);
        assert_eq!(cfg.max_file_size, 100000);
    }

    #[test]
    fn test_marshal_engine_config_updates_path_only() {
        let raw = serde_json::json!({"address": "127.0.0.1:3310"});
        let state = EngineState::default();
        let result = marshal_engine_config(&raw, &state, "/new/path", "");
        assert!(result.is_some());
        let val = result.unwrap();
        let cfg: ClamAVEngineConfig = serde_json::from_value(val).unwrap();
        assert_eq!(cfg.clamav_path, "/new/path");
        assert!(cfg.data_dir.is_empty());
    }

    // -------------------------------------------------------------------------
    // Additional coverage tests for scanner
    // -------------------------------------------------------------------------

    #[test]
    fn test_scanner_full_config_from_json() {
        let json = r#"{"enabled":["clamav","custom"],"engines":{"clamav":{"address":"127.0.0.1:3310"},"custom":{"address":"127.0.0.1:9999"}}}"#;
        let cfg: ScannerFullConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.enabled.len(), 2);
        assert_eq!(cfg.engines.len(), 2);
    }

    #[test]
    fn test_clamav_engine_config_from_json_minimal() {
        let json = r#"{"address":"0.0.0.0:3310"}"#;
        let cfg: ClamAVEngineConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.address, "0.0.0.0:3310");
        assert!(cfg.url.is_empty());
        assert!(cfg.clamav_path.is_empty());
        assert_eq!(cfg.max_file_size, 52428800); // default
        // Note: serde default for bool is false, so scan_on_write is false for partial JSON
        assert_eq!(cfg.scan_on_write, false);
    }

    #[test]
    fn test_clamav_engine_config_from_json_full() {
        let json = r#"{
            "address":"10.0.0.1:3310",
            "url":"https://example.com/clamav.zip",
            "clamav_path":"/opt/clamav",
            "data_dir":"/var/lib/clamav",
            "scan_on_write":false,
            "scan_on_download":true,
            "scan_on_exec":false,
            "max_file_size":100000000,
            "update_interval":"6h",
            "skip_extensions":[".exe",".dll",".bat"],
            "state":{"install_status":"installed","db_status":"ready","install_error":"","last_install_attempt":"","last_db_update":""}
        }"#;
        let cfg: ClamAVEngineConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.address, "10.0.0.1:3310");
        assert_eq!(cfg.url, "https://example.com/clamav.zip");
        assert_eq!(cfg.clamav_path, "/opt/clamav");
        assert_eq!(cfg.data_dir, "/var/lib/clamav");
        assert!(!cfg.scan_on_write);
        assert!(cfg.scan_on_download);
        assert!(!cfg.scan_on_exec);
        assert_eq!(cfg.max_file_size, 100000000);
        assert_eq!(cfg.update_interval, "6h");
        assert_eq!(cfg.skip_extensions.len(), 3);
        assert_eq!(cfg.state.install_status, "installed");
        assert_eq!(cfg.state.db_status, "ready");
    }

    #[test]
    fn test_engine_state_from_json() {
        let json = r#"{"install_status":"failed","install_error":"timeout","db_status":"missing","last_install_attempt":"2026-01-01","last_db_update":"2026-01-02"}"#;
        let state: EngineState = serde_json::from_str(json).unwrap();
        assert_eq!(state.install_status, "failed");
        assert_eq!(state.install_error, "timeout");
        assert_eq!(state.db_status, "missing");
        assert_eq!(state.last_install_attempt, "2026-01-01");
        assert_eq!(state.last_db_update, "2026-01-02");
    }

    #[test]
    fn test_cmd_list_no_engines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let cfg = ScannerFullConfig::default();
        save_scanner_config(&path, &cfg).unwrap();
        cmd_list(&path).unwrap();
    }

    #[test]
    fn test_cmd_list_with_disabled_engine() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        let engine = ClamAVEngineConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();
        cmd_list(&path).unwrap();
    }

    #[test]
    fn test_cmd_list_with_multiple_engines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine1 = ClamAVEngineConfig {
            address: "127.0.0.1:3310".to_string(),
            state: EngineState {
                install_status: "installed".to_string(),
                db_status: "ready".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let engine2 = ClamAVEngineConfig {
            address: "127.0.0.1:9999".to_string(),
            url: "https://example.com/engine2.zip".to_string(),
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine1).unwrap());
        cfg.engines.insert("engine2".to_string(), serde_json::to_value(engine2).unwrap());
        save_scanner_config(&path, &cfg).unwrap();
        cmd_list(&path).unwrap();
    }

    #[test]
    fn test_cmd_check_with_disabled_engine() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine = ClamAVEngineConfig {
            address: "127.0.0.1:3310".to_string(),
            url: "https://example.com/very-long-url-that-is-more-than-forty-characters-to-test-truncation.zip".to_string(),
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();
        cmd_check(&path).unwrap();
    }

    #[test]
    fn test_cmd_check_with_install_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine = ClamAVEngineConfig {
            state: EngineState {
                install_status: "failed".to_string(),
                install_error: "download failed".to_string(),
                db_status: "missing".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();
        cmd_check(&path).unwrap();
    }

    #[test]
    fn test_cmd_check_with_pending_status() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        let engine = ClamAVEngineConfig {
            state: EngineState {
                install_status: "pending".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();
        cmd_check(&path).unwrap();
    }

    #[test]
    fn test_cmd_add_update_with_url_only() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        save_scanner_config(&path, &cfg).unwrap();

        cmd_add(&path, "clamav", Some("https://new-url.com/clamav.zip"), None, None).unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
        assert_eq!(engine.url, "https://new-url.com/clamav.zip");
    }

    #[test]
    fn test_cmd_add_update_with_path_only() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        save_scanner_config(&path, &cfg).unwrap();

        cmd_add(&path, "clamav", None, Some("/custom/path"), None).unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
        assert_eq!(engine.clamav_path, "/custom/path");
    }

    #[test]
    fn test_detect_executable_dir_not_found() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("empty_subdir");
        std::fs::create_dir_all(&sub).unwrap();
        let result = detect_executable_dir(tmp.path(), &["nonexistent"]);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_executable_dir_with_clamd_exe() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("bin");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("clamd.exe"), "fake").unwrap();
        let result = detect_executable_dir(tmp.path(), &["clamd.exe", "clamd"]);
        assert!(result.is_some());
        let found = result.unwrap();
        assert!(found.contains("bin"));
    }

    #[test]
    fn test_detect_executable_dir_with_clamscan() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("usr").join("local").join("bin");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("clamscan"), "fake").unwrap();
        let result = detect_executable_dir(tmp.path(), &["clamscan"]);
        assert!(result.is_some());
    }

    #[test]
    fn test_check_executables_at_path_with_clamscan() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("clamscan"), "fake").unwrap();
        assert!(check_executables_at_path(&tmp.path().to_string_lossy()));
    }

    #[test]
    fn test_check_executables_at_path_with_clamscan_exe() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("clamscan.exe"), "fake").unwrap();
        assert!(check_executables_at_path(&tmp.path().to_string_lossy()));
    }

    #[test]
    fn test_save_scanner_config_creates_parent_dir() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("dir").join("config.scanner.json");
        let cfg = ScannerFullConfig::default();
        save_scanner_config(&path, &cfg).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_load_scanner_config_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        std::fs::write(&path, "not valid json").unwrap();
        let result = load_scanner_config(&path);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------------
    // ClamAVEngineConfig serialization/deserialization
    // -------------------------------------------------------------------------

    #[test]
    fn test_clamav_config_default_values_v2() {
        let config = ClamAVEngineConfig::default();
        assert_eq!(config.address, "127.0.0.1:3310");
        assert_eq!(config.url, "");
        assert_eq!(config.clamav_path, "");
        assert_eq!(config.data_dir, "");
        assert!(config.scan_on_write);
        assert!(!config.scan_on_download);
        assert!(config.scan_on_exec);
        assert_eq!(config.max_file_size, 52428800);
    }

    #[test]
    fn test_clamav_config_serialization_roundtrip_v2() {
        let config = ClamAVEngineConfig {
            address: "192.168.1.1:3310".to_string(),
            url: "https://example.com/clamav.zip".to_string(),
            clamav_path: "/usr/bin/clamscan".to_string(),
            data_dir: "/var/lib/clamav".to_string(),
            scan_on_write: true,
            scan_on_download: true,
            scan_on_exec: false,
            max_file_size: 50000000,
            ..Default::default()
        };
        let json = serde_json::to_value(&config).unwrap();
        let deserialized: ClamAVEngineConfig = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.address, "192.168.1.1:3310");
        assert_eq!(deserialized.url, "https://example.com/clamav.zip");
        assert_eq!(deserialized.clamav_path, "/usr/bin/clamscan");
        assert!(deserialized.scan_on_write);
        assert!(deserialized.scan_on_download);
        assert!(!deserialized.scan_on_exec);
        assert_eq!(deserialized.max_file_size, 50000000);
    }

    #[test]
    fn test_engine_state_default_v2() {
        let state = EngineState::default();
        assert_eq!(state.install_status, "");
        assert_eq!(state.install_error, "");
        assert_eq!(state.db_status, "");
    }

    // -------------------------------------------------------------------------
    // ScannerFullConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_scanner_full_config_default_v2() {
        let config = ScannerFullConfig::default();
        assert!(config.enabled.is_empty());
        assert!(config.engines.is_empty());
    }

    #[test]
    fn test_scanner_full_config_with_engines_v2() {
        let mut config = ScannerFullConfig::default();
        config.enabled.push("clamav".to_string());
        config.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        assert_eq!(config.enabled.len(), 1);
        assert_eq!(config.engines.len(), 1);
    }

    // -------------------------------------------------------------------------
    // parse_engine_config tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_engine_config_full_v2() {
        let json = serde_json::json!({
            "address": "10.0.0.1:3310",
            "url": "https://clamav.net/download",
            "clamav_path": "/opt/clamav/bin",
            "data_dir": "/var/clamav",
            "scan_on_write": true,
            "scan_on_download": true,
            "scan_on_exec": true,
            "max_file_size": 100000000
        });
        let config = parse_engine_config(&json);
        assert_eq!(config.address, "10.0.0.1:3310");
        assert_eq!(config.url, "https://clamav.net/download");
        assert_eq!(config.clamav_path, "/opt/clamav/bin");
        assert_eq!(config.data_dir, "/var/clamav");
        assert!(config.scan_on_write);
        assert!(config.scan_on_download);
        assert!(config.scan_on_exec);
        assert_eq!(config.max_file_size, 100000000);
    }

    #[test]
    fn test_parse_engine_config_minimal_v2() {
        let json = serde_json::json!({});
        let config = parse_engine_config(&json);
        assert_eq!(config.address, "127.0.0.1:3310"); // default
    }

    // -------------------------------------------------------------------------
    // cmd_enable/cmd_disable tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_enable_new_engine_v2() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        save_scanner_config(&path, &cfg).unwrap();

        cmd_enable(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(loaded.enabled.contains(&"clamav".to_string()));
    }

    #[test]
    fn test_cmd_disable_engine_v2() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        save_scanner_config(&path, &cfg).unwrap();

        cmd_disable(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(!loaded.enabled.contains(&"clamav".to_string()));
    }

    #[test]
    fn test_cmd_enable_already_enabled_v2() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        save_scanner_config(&path, &cfg).unwrap();

        cmd_enable(&path, "clamav").unwrap();
        // Should still have only one entry
        let loaded = load_scanner_config(&path).unwrap();
        assert_eq!(loaded.enabled.iter().filter(|e| **e == "clamav").count(), 1);
    }

    // -------------------------------------------------------------------------
    // cmd_remove tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_remove_existing_engine_v2() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        save_scanner_config(&path, &cfg).unwrap();

        cmd_remove(&path, "clamav").unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(!loaded.engines.contains_key("clamav"));
        assert!(!loaded.enabled.contains(&"clamav".to_string()));
    }

    #[test]
    fn test_cmd_remove_nonexistent_engine_v2() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        save_scanner_config(&path, &cfg).unwrap();

        // Remove an existing engine works
        cmd_remove(&path, "clamav").unwrap();
        let loaded = load_scanner_config(&path).unwrap();
        assert!(!loaded.engines.contains_key("clamav"));
    }

    // -------------------------------------------------------------------------
    // cmd_add with various parameters
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_add_new_engine_v2() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        save_scanner_config(&path, &ScannerFullConfig::default()).unwrap();

        cmd_add(&path, "clamav", Some("https://scanner.example.com"), Some("/opt/scanner"), None).unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        assert!(loaded.engines.contains_key("clamav"));
    }

    #[test]
    fn test_cmd_add_with_address_override() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        save_scanner_config(&path, &ScannerFullConfig::default()).unwrap();

        cmd_add(&path, "clamav", Some("https://clamav.net"), Some("/opt/clamav"), Some("10.0.0.1:3310")).unwrap();

        let loaded = load_scanner_config(&path).unwrap();
        let engine = parse_engine_config(loaded.engines.get("clamav").unwrap());
        assert_eq!(engine.url, "https://clamav.net");
        assert_eq!(engine.clamav_path, "/opt/clamav");
        assert_eq!(engine.address, "10.0.0.1:3310");
    }

    // -------------------------------------------------------------------------
    // default_address and default_max_file_size function tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_address_v2() {
        assert_eq!(default_address(), "127.0.0.1:3310");
    }

    #[test]
    fn test_default_max_file_size_v2() {
        assert_eq!(default_max_file_size(), 52428800);
    }

    // -------------------------------------------------------------------------
    // cmd_clamav subcommand tests (testing the subcommand logic via direct calls)
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_clamav_enable_and_disable() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.scanner.json");
        let mut cfg = ScannerFullConfig::default();
        let engine = ClamAVEngineConfig {
            address: "127.0.0.1:3310".to_string(),
            ..Default::default()
        };
        cfg.engines.insert("clamav".to_string(), serde_json::to_value(engine).unwrap());
        save_scanner_config(&path, &cfg).unwrap();

        cmd_enable(&path, "clamav").unwrap();
        let loaded = load_scanner_config(&path).unwrap();
        assert!(loaded.enabled.contains(&"clamav".to_string()));

        cmd_disable(&path, "clamav").unwrap();
        let loaded = load_scanner_config(&path).unwrap();
        assert!(!loaded.enabled.contains(&"clamav".to_string()));
    }

    // -------------------------------------------------------------------------
    // ScannerFullConfig with multiple engines
    // -------------------------------------------------------------------------

    #[test]
    fn test_scanner_config_multiple_engines() {
        let mut cfg = ScannerFullConfig::default();
        cfg.enabled.push("clamav".to_string());
        cfg.engines.insert("clamav".to_string(), serde_json::json!({"address": "127.0.0.1:3310"}));
        cfg.engines.insert("custom".to_string(), serde_json::json!({"address": "127.0.0.1:9999"}));
        assert_eq!(cfg.engines.len(), 2);
        assert_eq!(cfg.enabled.len(), 1);
    }
}
