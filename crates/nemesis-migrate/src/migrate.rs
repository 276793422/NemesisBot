//! Full migration orchestration: plan, execute, confirm, print helpers.
//!
//! Mirrors Go migrate/migrate.go.

use crate::openclaw_config;
use crate::workspace::WorkspaceMigrator;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Migration options controlling what gets migrated and how.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrateOptions {
    pub dry_run: bool,
    pub config_only: bool,
    pub workspace_only: bool,
    pub force: bool,
    pub refresh: bool,
    pub openclaw_home: String,
    pub nemesisbot_home: String,
}

impl Default for MigrateOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            config_only: false,
            workspace_only: false,
            force: false,
            refresh: false,
            openclaw_home: String::new(),
            nemesisbot_home: String::new(),
        }
    }
}

/// Types of migration actions that can be performed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FullMigrationActionType {
    Copy,
    Skip,
    Backup,
    ConvertConfig,
    CreateDir,
    MergeConfig,
}

/// A single migration action in the full migration plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullMigrationAction {
    #[serde(rename = "type")]
    pub action_type: FullMigrationActionType,
    pub source: Option<String>,
    pub destination: String,
    pub description: String,
}

/// Result of executing a full migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullMigrationResult {
    pub files_copied: usize,
    pub files_skipped: usize,
    pub backups_created: usize,
    pub config_migrated: bool,
    pub dirs_created: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl Default for FullMigrationResult {
    fn default() -> Self {
        Self {
            files_copied: 0,
            files_skipped: 0,
            backups_created: 0,
            config_migrated: false,
            dirs_created: 0,
            warnings: Vec::new(),
            errors: Vec::new(),
        }
    }
}

/// Migrator handles data migration between versions.
pub struct Migrator {
    config: crate::config::MigrateConfig,
}

impl Migrator {
    pub fn new(config: crate::config::MigrateConfig) -> Self {
        Self { config }
    }

    /// Run all pending migrations.
    pub fn run(&self) -> Result<(), String> {
        let workspace = WorkspaceMigrator::new(&self.config.workspace_path);
        workspace.migrate()?;
        Ok(())
    }

    /// Check if migrations are needed.
    pub fn needs_migration(&self) -> bool {
        !self.config.workspace_path.is_empty()
            && std::path::Path::new(&self.config.workspace_path).exists()
    }

    /// Dry run: plan migration without executing.
    pub fn dry_run(
        &self,
        src: &str,
        dst: &str,
        force: bool,
    ) -> Result<crate::workspace::MigrationPlan, String> {
        WorkspaceMigrator::dry_run(src, dst, force)
    }
}

// ---------------------------------------------------------------------------
// Full migration orchestration (mirrors Go migrate/migrate.go Run/Plan/Execute)
// ---------------------------------------------------------------------------

/// Run a full migration from OpenClaw to NemesisBot.
///
/// This is the top-level entry point mirroring Go `Run()`.
/// It resolves home directories, plans actions, optionally confirms,
/// and executes.
pub fn run_full_migration(opts: &MigrateOptions) -> Result<FullMigrationResult, String> {
    if opts.config_only && opts.workspace_only {
        return Err("--config-only and --workspace-only are mutually exclusive".to_string());
    }

    let effective_opts = if opts.refresh {
        let mut o = opts.clone();
        o.workspace_only = true;
        o
    } else {
        opts.clone()
    };

    let openclaw_home = resolve_openclaw_home(&effective_opts.openclaw_home)?;
    let nemesisbot_home = resolve_nemesisbot_home(&effective_opts.nemesisbot_home)?;

    if !Path::new(&openclaw_home).exists() {
        return Err(format!(
            "OpenClaw installation not found at {}",
            openclaw_home
        ));
    }

    let (actions, warnings) = plan(&effective_opts, &openclaw_home, &nemesisbot_home)?;

    println!("Migrating from OpenClaw to NemesisBot");
    println!("  Source:      {}", openclaw_home);
    println!("  Destination: {}", nemesisbot_home);
    println!();

    if effective_opts.dry_run {
        print_plan(&actions, &warnings);
        return Ok(FullMigrationResult {
            warnings,
            ..Default::default()
        });
    }

    if !effective_opts.force {
        print_plan(&actions, &warnings);
        if !confirm() {
            println!("Aborted.");
            return Ok(FullMigrationResult {
                warnings,
                ..Default::default()
            });
        }
        println!();
    }

    let mut result = execute(&actions, &openclaw_home, &nemesisbot_home);
    result.warnings = warnings;
    Ok(result)
}

/// Plan migration actions without executing them.
///
/// Mirrors Go `Plan()`.
pub fn plan(
    opts: &MigrateOptions,
    openclaw_home: &str,
    nemesisbot_home: &str,
) -> Result<(Vec<FullMigrationAction>, Vec<String>), String> {
    let mut actions: Vec<FullMigrationAction> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let force = opts.force || opts.refresh;

    // Config migration
    if !opts.workspace_only {
        match openclaw_config::find_openclaw_config(Path::new(openclaw_home)) {
            Ok(config_path) => {
                actions.push(FullMigrationAction {
                    action_type: FullMigrationActionType::ConvertConfig,
                    source: Some(config_path.clone()),
                    destination: Path::new(nemesisbot_home)
                        .join("config.json")
                        .to_string_lossy()
                        .to_string(),
                    description: "convert OpenClaw config to NemesisBot format".to_string(),
                });

                if let Ok(data) = openclaw_config::load_openclaw_config(Path::new(&config_path)) {
                    let (_, config_warnings) = openclaw_config::convert_config(&data);
                    warnings.extend(config_warnings);
                }
            }
            Err(e) => {
                if opts.config_only {
                    return Err(e);
                }
                warnings.push(format!("Config migration skipped: {}", e));
            }
        }
    }

    // Workspace migration
    if !opts.config_only {
        let src_workspace = resolve_workspace(openclaw_home);
        let dst_workspace = resolve_workspace(nemesisbot_home);

        if Path::new(&src_workspace).exists() {
            let ws_actions = plan_workspace_migration(&src_workspace, &dst_workspace, force)?;
            actions.extend(ws_actions);
        } else {
            warnings.push(
                "OpenClaw workspace directory not found, skipping workspace migration".to_string(),
            );
        }
    }

    Ok((actions, warnings))
}

/// Execute a set of planned migration actions.
///
/// Mirrors Go `Execute()`.
pub fn execute(
    actions: &[FullMigrationAction],
    openclaw_home: &str,
    _nemesisbot_home: &str,
) -> FullMigrationResult {
    let mut result = FullMigrationResult::default();

    for action in actions {
        match action.action_type {
            FullMigrationActionType::ConvertConfig => {
                if let Some(ref src) = action.source {
                    match execute_config_migration(src, &action.destination) {
                        Ok(()) => {
                            result.config_migrated = true;
                            println!("  [ok] Converted config: {}", action.destination);
                        }
                        Err(e) => {
                            result.errors.push(format!("config migration: {}", e));
                            println!("  [fail] Config migration failed: {}", e);
                        }
                    }
                }
            }
            FullMigrationActionType::CreateDir => {
                if let Err(e) = fs::create_dir_all(&action.destination) {
                    result.errors.push(format!("{}", e));
                } else {
                    result.dirs_created += 1;
                }
            }
            FullMigrationActionType::Backup => {
                let bak_path = format!("{}.bak", action.destination);
                if let Some(ref src) = action.source {
                    match copy_file(&action.destination, &bak_path) {
                        Ok(()) => {
                            result.backups_created += 1;
                            let basename = Path::new(&action.destination)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy();
                            println!("  [ok] Backed up {}.bak", basename);

                            // Copy source to destination
                            if let Err(e) = fs::create_dir_all(
                                Path::new(&action.destination)
                                    .parent()
                                    .unwrap_or(Path::new(".")),
                            ) {
                                result.errors.push(format!("{}", e));
                                continue;
                            }
                            match copy_file(src, &action.destination) {
                                Ok(()) => {
                                    result.files_copied += 1;
                                    let rel = rel_path(src, openclaw_home);
                                    println!("  [ok] Copied {}", rel);
                                }
                                Err(e) => {
                                    result.errors.push(format!("copy {}: {}", src, e));
                                    println!("  [fail] Copy failed: {}", src);
                                }
                            }
                        }
                        Err(e) => {
                            result
                                .errors
                                .push(format!("backup {}: {}", action.destination, e));
                            println!("  [fail] Backup failed: {}", action.destination);
                        }
                    }
                }
            }
            FullMigrationActionType::Copy => {
                if let Some(ref src) = action.source {
                    if let Err(e) = fs::create_dir_all(
                        Path::new(&action.destination)
                            .parent()
                            .unwrap_or(Path::new(".")),
                    ) {
                        result.errors.push(format!("{}", e));
                        continue;
                    }
                    match copy_file(src, &action.destination) {
                        Ok(()) => {
                            result.files_copied += 1;
                            let rel = rel_path(src, openclaw_home);
                            println!("  [ok] Copied {}", rel);
                        }
                        Err(e) => {
                            result.errors.push(format!("copy {}: {}", src, e));
                            println!("  [fail] Copy failed: {}", src);
                        }
                    }
                }
            }
            FullMigrationActionType::Skip => {
                result.files_skipped += 1;
            }
            FullMigrationActionType::MergeConfig => {
                // MergeConfig actions are handled inside ConvertConfig execution
                result.files_skipped += 1;
            }
        }
    }

    result
}

/// Interactive confirmation prompt.
///
/// Mirrors Go `Confirm()`.
pub fn confirm() -> bool {
    print!("Proceed with migration? (y/n): ");
    let _ = io::stdout().flush();
    let mut response = String::new();
    if io::stdin().read_line(&mut response).is_err() {
        return false;
    }
    response.trim().to_lowercase() == "y"
}

/// Print the migration plan to stdout.
///
/// Mirrors Go `PrintPlan()`.
pub fn print_plan(actions: &[FullMigrationAction], warnings: &[String]) {
    println!("Planned actions:");
    let mut copies = 0usize;
    let mut skips = 0usize;
    let mut backups = 0usize;
    let mut config_count = 0usize;

    for action in actions {
        match action.action_type {
            FullMigrationActionType::ConvertConfig => {
                let src = action.source.as_deref().unwrap_or("?");
                println!("  [config]  {} -> {}", src, action.destination);
                config_count += 1;
            }
            FullMigrationActionType::Copy => {
                let basename = Path::new(action.source.as_deref().unwrap_or("?"))
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                println!("  [copy]    {}", basename);
                copies += 1;
            }
            FullMigrationActionType::Backup => {
                let basename = Path::new(&action.destination)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                println!(
                    "  [backup]  {} (exists, will backup and overwrite)",
                    basename
                );
                backups += 1;
                copies += 1;
            }
            FullMigrationActionType::Skip => {
                if !action.description.is_empty() {
                    let basename = Path::new(action.source.as_deref().unwrap_or("?"))
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    println!("  [skip]    {} ({})", basename, action.description);
                }
                skips += 1;
            }
            FullMigrationActionType::CreateDir => {
                println!("  [mkdir]   {}", action.destination);
            }
            FullMigrationActionType::MergeConfig => {
                let src = action.source.as_deref().unwrap_or("?");
                println!("  [merge]   {} -> {}", src, action.destination);
                config_count += 1;
            }
        }
    }

    if !warnings.is_empty() {
        println!();
        println!("Warnings:");
        for w in warnings {
            println!("  - {}", w);
        }
    }

    println!();
    println!(
        "{} files to copy, {} configs to convert, {} backups needed, {} skipped",
        copies, config_count, backups, skips
    );
}

/// Print a summary of the migration result.
///
/// Mirrors Go `PrintSummary()`.
pub fn print_summary(result: &FullMigrationResult) {
    println!();
    let mut parts: Vec<String> = Vec::new();
    if result.files_copied > 0 {
        parts.push(format!("{} files copied", result.files_copied));
    }
    if result.config_migrated {
        parts.push("1 config converted".to_string());
    }
    if result.backups_created > 0 {
        parts.push(format!("{} backups created", result.backups_created));
    }
    if result.files_skipped > 0 {
        parts.push(format!("{} files skipped", result.files_skipped));
    }

    if !parts.is_empty() {
        println!("Migration complete! {}.", parts.join(", "));
    } else {
        println!("Migration complete! No actions taken.");
    }

    if !result.errors.is_empty() {
        println!();
        println!("{} errors occurred:", result.errors.len());
        for e in &result.errors {
            println!("  - {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Execute config migration: load OpenClaw config, convert, optionally merge, save.
fn execute_config_migration(src_config_path: &str, dst_config_path: &str) -> Result<(), String> {
    let data = openclaw_config::load_openclaw_config(Path::new(src_config_path))?;
    let (mut incoming, _warnings) = openclaw_config::convert_config(&data);

    // If destination config exists, merge
    if Path::new(dst_config_path).exists() {
        let existing_content = fs::read_to_string(dst_config_path)
            .map_err(|e| format!("loading existing NemesisBot config: {}", e))?;
        let mut existing: serde_json::Value = serde_json::from_str(&existing_content)
            .map_err(|e| format!("parsing existing NemesisBot config: {}", e))?;
        openclaw_config::merge_config(&mut existing, &incoming);
        incoming = existing;
    }

    if let Some(parent) = Path::new(dst_config_path).parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
    }

    let json_str = serde_json::to_string_pretty(&incoming)
        .map_err(|e| format!("serializing config: {}", e))?;
    fs::write(dst_config_path, json_str).map_err(|e| format!("writing config: {}", e))?;

    Ok(())
}

/// Plan workspace migration actions (mirrors Go PlanWorkspaceMigration).
fn plan_workspace_migration(
    src_workspace: &str,
    dst_workspace: &str,
    force: bool,
) -> Result<Vec<FullMigrationAction>, String> {
    let ws_plan = WorkspaceMigrator::plan_migration(src_workspace, dst_workspace, force)?;

    // Convert workspace MigrationAction to FullMigrationAction
    let actions: Vec<FullMigrationAction> = ws_plan
        .actions
        .into_iter()
        .map(|a| FullMigrationAction {
            action_type: match a.action_type {
                crate::workspace::ActionType::Copy => FullMigrationActionType::Copy,
                crate::workspace::ActionType::Skip => FullMigrationActionType::Skip,
                crate::workspace::ActionType::Backup => FullMigrationActionType::Backup,
                crate::workspace::ActionType::CreateDir => FullMigrationActionType::CreateDir,
            },
            source: a.source,
            destination: a.destination,
            description: a.description,
        })
        .collect();

    Ok(actions)
}

/// Resolve the OpenClaw home directory.
fn resolve_openclaw_home(override_path: &str) -> Result<String, String> {
    if !override_path.is_empty() {
        return Ok(expand_home(override_path));
    }
    if let Ok(env_home) = std::env::var("OPENCLAW_HOME") {
        return Ok(expand_home(&env_home));
    }
    let home = dirs_home()?;
    Ok(Path::new(&home)
        .join(".openclaw")
        .to_string_lossy()
        .to_string())
}

/// Resolve the NemesisBot home directory.
fn resolve_nemesisbot_home(override_path: &str) -> Result<String, String> {
    if !override_path.is_empty() {
        return Ok(expand_home(override_path));
    }
    if let Ok(env_home) = std::env::var("NEMESISBOT_HOME") {
        return Ok(expand_home(&env_home));
    }
    let home = dirs_home()?;
    Ok(Path::new(&home)
        .join(".nemesisbot")
        .to_string_lossy()
        .to_string())
}

/// Get the user's home directory.
fn dirs_home() -> Result<String, String> {
    // Try HOME env var first (Unix-style), then USERPROFILE (Windows)
    if let Ok(h) = std::env::var("HOME") {
        return Ok(h);
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return Ok(h);
    }
    Err("resolving home directory: no HOME or USERPROFILE set".to_string())
}

/// Expand ~ to home directory.
fn expand_home(path: &str) -> String {
    if path.starts_with("~/") || path == "~" {
        if let Ok(home) = dirs_home() {
            if path == "~" {
                return home;
            }
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}

/// Resolve workspace subdirectory.
fn resolve_workspace(home_dir: &str) -> String {
    Path::new(home_dir)
        .join("workspace")
        .to_string_lossy()
        .to_string()
}

/// Copy a file from src to dst.
fn copy_file(src: &str, dst: &str) -> Result<(), String> {
    fs::copy(src, dst).map_err(|e| format!("copy {} -> {}: {}", src, dst, e))?;
    Ok(())
}

/// Compute relative path from base.
fn rel_path(path: &str, base: &str) -> String {
    match Path::new(path).strip_prefix(Path::new(base)) {
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string()),
    }
}

#[cfg(test)]
mod tests;
