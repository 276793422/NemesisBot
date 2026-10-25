//! Migrate command - migrate from OpenClaw format to NemesisBot.
//!
//! Uses nemesis_migrate crate for config conversion and workspace migration.

use crate::common;
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(clap::Parser)]
pub struct MigrateOptions {
    /// Show what would be migrated without making changes
    #[arg(long)]
    pub dry_run: bool,

    /// Only migrate configuration files
    #[arg(long)]
    pub config_only: bool,

    /// Only migrate workspace files (prompts, skills, etc.)
    #[arg(long)]
    pub workspace_only: bool,

    /// Skip confirmation prompt
    #[arg(long)]
    pub force: bool,

    /// Path to OpenClaw home directory (default: auto-detect)
    #[arg(long)]
    pub openclaw_home: Option<String>,

    /// Re-sync workspace files even if target exists
    #[arg(long)]
    pub refresh: bool,

    /// Override NemesisBot home directory
    #[arg(long)]
    pub nemesisbot_home: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Detect the OpenClaw installation directory.
fn detect_openclaw_home(override_path: &Option<String>) -> Option<PathBuf> {
    if let Some(path) = override_path {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
        return None;
    }

    // Check OPENCLAW_HOME env var first
    if let Ok(home) = std::env::var("OPENCLAW_HOME") {
        let p = PathBuf::from(&home);
        if p.exists() {
            return Some(p);
        }
    }

    // Check ~/.openclaw
    if let Some(home) = dirs::home_dir() {
        let openclaw = home.join(".openclaw");
        if openclaw.exists() {
            return Some(openclaw);
        }
    }

    None
}

/// Backup a file by copying to .bak
fn backup_file(path: &Path) -> Result<()> {
    if path.exists() {
        let bak = path.with_extension(
            path.extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
                + ".bak",
        );
        std::fs::copy(path, &bak)?;
    }
    Ok(())
}

/// Write the converted config to `cfg_path`.
///
/// BUG-20261025-66：既有 config 不再被整体覆盖——先读现有配置，经
/// `nemesis_migrate::merge_config` 按「现有优先」合并（模型按 model_name
/// 去重保留现有、已启用通道不动、禁用通道由迁移值回填），再落盘合并
/// 结果。旧形态把转换结果直接写盘，用户迁移前配好的模型/密钥/通道全部
/// 丢失（只剩 .bak 可手工捞回）——「Try merge」注释是空 stub。
/// BUG-20261025-67：落盘统一走 `write_file_atomic`（唯一临时名 + rename），
/// 半路崩溃不再留半截 config.json。
/// 返回 `Ok(true)` = 与既有配置合并过；`Ok(false)` = 全新写入（或既有文件
/// 不是合法 JSON，警告后由迁移值覆盖，.bak 已兜底）。
fn write_migrated_config(cfg_path: &Path, incoming: &serde_json::Value) -> Result<bool> {
    let mut merged = incoming.clone();
    let mut merged_with_existing = false;
    if cfg_path.exists() {
        let existing_content = std::fs::read_to_string(cfg_path).map_err(|e| {
            anyhow::anyhow!("reading existing config {}: {}", cfg_path.display(), e)
        })?;
        match serde_json::from_str::<serde_json::Value>(&existing_content) {
            Ok(mut existing) => {
                // BUG-20261025-69：merge_config 契约是「只并入既有结构，不补建
                // 缺失键」（crate 覆盖测试钉死）。最小化既有配置（无 model_list
                // 键/键为 null）会让全部迁移模型静默丢失——迁移承诺是「模型跟着
                // 迁移走」，这层是调用方策略：合并前把 incoming 的模型预填进
                // 缺失的 model_list。channels 刻意不预填：给无 channels 段的
                // config 强插 enabled:true 的 web 通道 = 替用户开 8080 端口。
                let models_missing = existing
                    .get("model_list")
                    .map(|v| v.is_null())
                    .unwrap_or(true);
                if models_missing
                    && let Some(models) = merged.get("model_list").and_then(|v| v.as_array())
                    && !models.is_empty()
                {
                    existing["model_list"] = merged["model_list"].clone();
                }
                nemesis_migrate::merge_config(&mut existing, &merged);
                merged = existing;
                merged_with_existing = true;
            }
            Err(_) => {
                println!("  Warning: existing config is not valid JSON, overwritten (backup kept)");
            }
        }
    }
    let json = serde_json::to_string_pretty(&merged)
        .map_err(|e| anyhow::anyhow!("serializing migrated config: {e}"))?;
    nemesis_utils::file::write_file_atomic(&cfg_path.to_string_lossy(), json.as_bytes(), 0o644)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(merged_with_existing)
}

/// Copy a directory tree from src to dst.
/// If refresh is false, skip files that already exist in dst.
fn copy_dir_recursive(src: &Path, dst: &Path, refresh: bool) -> Result<u32> {
    let mut count = 0u32;
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            count += copy_dir_recursive(&src_path, &dst_path, refresh)?;
        } else {
            // Skip if target exists and not refreshing
            if !refresh && dst_path.exists() {
                continue;
            }
            std::fs::copy(&src_path, &dst_path)?;
            count += 1;
        }
    }

    Ok(count)
}

/// Prompt the user for confirmation. Returns true if confirmed.
fn confirm(prompt: &str) -> bool {
    use std::io::{self, Write};
    print!("{} (y/N): ", prompt);
    io::stdout().flush().ok();

    let mut input = String::new();
    // If stdin is not a tty (piped), default to no unless --force.
    if atty_isnt() {
        println!("n (non-interactive)");
        return false;
    }

    io::stdin().read_line(&mut input).ok();
    let answer = input.trim().to_lowercase();
    answer == "y" || answer == "yes"
}

/// Check if stdin is not a terminal (non-interactive mode).
///
/// BUG-20261025-68：旧实现用 `PROMPT`/`TERM` 环境变量猜测 tty——PowerShell
/// 交互态两个变量都没有（PROMPT 是 CMD 的环境变量/PS 里是函数，TERM 通常
/// 不设），恒判「非交互」→ confirm 自动拒，交互用户被迫 --force；而管道
/// 运行若继承了父 shell 的 TERM 又被误判「交互」。改用 std 的 stdin 真探测。
fn atty_isnt() -> bool {
    use std::io::IsTerminal;
    !std::io::stdin().is_terminal()
}

// ---------------------------------------------------------------------------
// Config conversion using nemesis_migrate crate
// ---------------------------------------------------------------------------

/// Convert OpenClaw config to NemesisBot config using the crate's conversion.
fn convert_config_with_crate(openclaw_home: &Path) -> Result<(serde_json::Value, Vec<String>)> {
    // Try to find and load config using crate functions
    match nemesis_migrate::find_openclaw_config(openclaw_home) {
        Ok(config_path) => {
            let path = Path::new(&config_path);
            match nemesis_migrate::load_openclaw_config(path) {
                Ok(data) => {
                    let (config, warnings) = nemesis_migrate::convert_config(&data);
                    Ok((config, warnings))
                }
                Err(e) => {
                    // Fall back to manual conversion
                    eprintln!("Warning: crate load failed ({}), using fallback.", e);
                    convert_config_fallback(openclaw_home)
                }
            }
        }
        Err(_) => {
            // No config file found, try manual YAML extraction
            convert_config_fallback(openclaw_home)
        }
    }
}

/// Fallback config conversion when crate can't find the config.
fn convert_config_fallback(openclaw_home: &Path) -> Result<(serde_json::Value, Vec<String>)> {
    let mut warnings = Vec::new();

    let mut config = serde_json::json!({
        "version": "1.0",
        "default_model": "",
        "model_list": [],
        "channels": {
            "web": {"enabled": true, "host": "127.0.0.1", "port": 8080},
            "websocket": {"enabled": false},
        },
        "security": {"enabled": true},
        "forge": {"enabled": false},
    });

    // Try YAML-based extraction for common fields
    for config_name in &["config.yaml", "config.yml", "openclaw.yaml", "openclaw.yml"] {
        let yaml_path = openclaw_home.join(config_name);
        if yaml_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&yaml_path) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if let Some(model) = trimmed.strip_prefix("default_model:") {
                        let model = model.trim().trim_matches('"').trim_matches('\'');
                        if !model.is_empty()
                            && let Some(obj) = config.as_object_mut()
                        {
                            obj.insert(
                                "default_model".to_string(),
                                serde_json::Value::String(model.to_string()),
                            );
                        }
                    }
                    if let Some(port_str) = trimmed.strip_prefix("port:")
                        && let Ok(port) = port_str.trim().parse::<u64>()
                        && let Some(web) = config.pointer_mut("/channels/web")
                        && let Some(obj) = web.as_object_mut()
                    {
                        obj.insert("port".to_string(), serde_json::Value::Number(port.into()));
                    }
                }
            }
            break;
        }
    }

    // Extract models from models.yaml
    let models_yaml = openclaw_home.join("models.yaml");
    if models_yaml.exists()
        && let Ok(models_content) = std::fs::read_to_string(&models_yaml)
    {
        let mut model_list = Vec::new();
        for line in models_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- name:") || trimmed.starts_with("- model:") {
                let model_name = trimmed
                    .strip_prefix("- name:")
                    .or_else(|| trimmed.strip_prefix("- model:"))
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                if !model_name.is_empty() {
                    model_list.push(serde_json::json!({ "model": model_name, "key": "" }));
                }
            }
        }
        if !model_list.is_empty()
            && let Some(obj) = config.as_object_mut()
        {
            obj.insert(
                "model_list".to_string(),
                serde_json::Value::Array(model_list),
            );
        }
    }

    warnings.push("Used fallback config conversion (no OpenClaw config file found)".to_string());
    Ok((config, warnings))
}

// ---------------------------------------------------------------------------
// Main command
// ---------------------------------------------------------------------------

pub fn run(options: MigrateOptions, local: bool) -> Result<()> {
    let nemesis_home = if let Some(ref home) = options.nemesisbot_home {
        PathBuf::from(home)
    } else {
        common::resolve_home(local)
    };

    // Step 1: Detect OpenClaw installation.
    let openclaw_home = match detect_openclaw_home(&options.openclaw_home) {
        Some(p) => p,
        None => {
            println!("OpenClaw installation not found.");
            if let Some(ref path) = options.openclaw_home {
                println!("  Checked: {}", path);
            } else {
                if let Some(home) = dirs::home_dir() {
                    println!("  Checked: {}", home.join(".openclaw").display());
                }
                println!("  Checked: $OPENCLAW_HOME environment variable");
            }
            println!();
            println!("Specify a path with: nemesisbot migrate --openclaw-home <path>");
            return Ok(());
        }
    };

    println!("OpenClaw Migration");
    println!("==================");
    println!("  Source: {}", openclaw_home.display());
    println!("  Target: {}", nemesis_home.display());
    println!();

    // Step 2: Build migration plan.
    let workspace_src = openclaw_home.join("workspace");
    let prompts_src = openclaw_home.join("prompts");
    let skills_src = openclaw_home.join("skills");
    let identity_src = openclaw_home.join("IDENTITY.md");
    let soul_src = openclaw_home.join("SOUL.md");
    let user_src = openclaw_home.join("USER.md");

    let has_config = nemesis_migrate::find_openclaw_config(&openclaw_home).is_ok()
        || openclaw_home.join("config.yaml").exists()
        || openclaw_home.join("config.yml").exists()
        || openclaw_home.join("openclaw.yaml").exists();

    let migrate_config = !options.workspace_only && has_config;
    let migrate_workspace = !options.config_only
        && (workspace_src.exists()
            || prompts_src.exists()
            || skills_src.exists()
            || identity_src.exists()
            || soul_src.exists()
            || user_src.exists());

    // Step 3: Show migration plan.
    println!("Migration plan:");
    if migrate_config {
        println!(
            "  [Config] {} -> {}",
            openclaw_home.display(),
            common::config_path(&nemesis_home).display()
        );
    }
    if workspace_src.exists() && !options.config_only {
        println!(
            "  [Workspace] {} -> {}",
            workspace_src.display(),
            common::workspace_path(&nemesis_home).display()
        );
    }
    if prompts_src.exists() && !options.config_only {
        println!(
            "  [Prompts] {} -> {}",
            prompts_src.display(),
            nemesis_home.join("workspace").join("prompts").display()
        );
    }
    if skills_src.exists() && !options.config_only {
        println!(
            "  [Skills] {} -> {}",
            skills_src.display(),
            nemesis_home.join("workspace").join("skills").display()
        );
    }
    for (label, src) in [
        ("Identity", &identity_src),
        ("Soul", &soul_src),
        ("User", &user_src),
    ] {
        if src.exists() && !options.config_only {
            println!(
                "  [{}] {} -> {}",
                label,
                src.display(),
                nemesis_home.join(src.file_name().unwrap()).display()
            );
        }
    }

    if !migrate_config && !migrate_workspace {
        println!("  Nothing to migrate.");
        return Ok(());
    }

    println!();

    // Step 4: Dry run mode.
    if options.dry_run {
        // Use crate for dry-run if possible
        if migrate_config {
            match convert_config_with_crate(&openclaw_home) {
                Ok((config, warnings)) => {
                    println!("Config preview:");
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&config).unwrap_or_default()
                    );
                    for w in &warnings {
                        println!("  Warning: {}", w);
                    }
                }
                Err(e) => println!("  Config conversion error: {}", e),
            }
        }
        println!();
        println!("Dry run complete. No changes were made.");
        return Ok(());
    }

    // Step 5: Confirmation.
    if !options.force && !confirm("Proceed with migration?") {
        println!("Migration cancelled.");
        return Ok(());
    }

    // Step 6: Perform migration.
    let mut migrated_files: u32 = 0;
    let mut migrated_dirs: u32 = 0;

    // Migrate config.
    if migrate_config {
        let cfg_path = common::config_path(&nemesis_home);
        let config_dir = cfg_path.parent().unwrap();
        let _ = std::fs::create_dir_all(config_dir);

        // Backup existing config
        if cfg_path.exists() {
            backup_file(&cfg_path)?;
        }

        match convert_config_with_crate(&openclaw_home) {
            Ok((new_config, warnings)) => {
                match write_migrated_config(&cfg_path, &new_config) {
                    Ok(true) => {
                        migrated_files += 1;
                        println!(
                            "  Migrated config -> {} (merged with existing, existing takes priority)",
                            cfg_path.display()
                        );
                    }
                    Ok(false) => {
                        migrated_files += 1;
                        println!("  Migrated config -> {}", cfg_path.display());
                    }
                    Err(e) => {
                        println!("  Error writing config: {e}");
                    }
                }
                for w in &warnings {
                    println!("  Warning: {}", w);
                }
            }
            Err(e) => {
                println!("  Error converting config: {}", e);
            }
        }
    }

    // Migrate workspace files.
    if !options.config_only {
        let ws_dst = common::workspace_path(&nemesis_home);
        let _ = std::fs::create_dir_all(&ws_dst);

        // Workspace directory.
        if workspace_src.exists() {
            let count = copy_dir_recursive(&workspace_src, &ws_dst, options.refresh)?;
            migrated_files += count;
            migrated_dirs += 1;
            println!(
                "  Migrated workspace ({} files{}) -> {}",
                count,
                if options.refresh { ", refreshed" } else { "" },
                ws_dst.display()
            );
        }

        // Prompts directory.
        if prompts_src.exists() {
            let dst = ws_dst.join("prompts");
            let count = copy_dir_recursive(&prompts_src, &dst, options.refresh)?;
            migrated_files += count;
            migrated_dirs += 1;
            println!("  Migrated prompts ({} files) -> {}", count, dst.display());
        }

        // Skills directory.
        if skills_src.exists() {
            let dst = ws_dst.join("skills");
            let count = copy_dir_recursive(&skills_src, &dst, options.refresh)?;
            migrated_files += count;
            migrated_dirs += 1;
            println!("  Migrated skills ({} files) -> {}", count, dst.display());
        }

        // Individual files (IDENTITY.md, SOUL.md, USER.md).
        for src in [&identity_src, &soul_src, &user_src] {
            if src.exists() {
                let file_name = src.file_name().unwrap();
                let dst = nemesis_home.join(file_name);
                std::fs::copy(src, &dst)?;
                migrated_files += 1;
                println!("  Migrated {} -> {}", src.display(), dst.display());
            }
        }
    }

    // Step 7: Print summary.
    println!();
    println!("Migration complete.");
    println!("  Files migrated: {}", migrated_files);
    if migrated_dirs > 0 {
        println!("  Directories migrated: {}", migrated_dirs);
    }
    println!();
    println!("Next steps:");
    println!("  1. Review configuration: nemesisbot log config");
    println!(
        "  2. Set your model key: nemesisbot model add --model <vendor/model> --key <key> --default"
    );
    println!("  3. Start the gateway: nemesisbot gateway");

    Ok(())
}

#[cfg(test)]
mod tests;
