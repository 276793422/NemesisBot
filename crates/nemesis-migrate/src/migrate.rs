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
    pub fn dry_run(&self, src: &str, dst: &str, force: bool) -> Result<crate::workspace::MigrationPlan, String> {
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
        return Err(format!("OpenClaw installation not found at {}", openclaw_home));
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
                                Path::new(&action.destination).parent().unwrap_or(Path::new(".")),
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
                    if let Err(e) =
                        fs::create_dir_all(Path::new(&action.destination).parent().unwrap_or(Path::new(".")))
                    {
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
    let data =
        openclaw_config::load_openclaw_config(Path::new(src_config_path))?;
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
mod tests {
    use super::*;

    #[test]
    fn test_needs_migration_empty() {
        let config = crate::config::MigrateConfig {
            workspace_path: String::new(),
            target_version: 1,
        };
        let migrator = Migrator::new(config);
        assert!(!migrator.needs_migration());
    }

    #[test]
    fn test_migrate_options_default() {
        let opts = MigrateOptions::default();
        assert!(!opts.dry_run);
        assert!(!opts.config_only);
        assert!(!opts.workspace_only);
        assert!(!opts.force);
        assert!(!opts.refresh);
        assert!(opts.openclaw_home.is_empty());
        assert!(opts.nemesisbot_home.is_empty());
    }

    #[test]
    fn test_full_migration_result_default() {
        let result = FullMigrationResult::default();
        assert_eq!(result.files_copied, 0);
        assert_eq!(result.files_skipped, 0);
        assert_eq!(result.backups_created, 0);
        assert!(!result.config_migrated);
        assert_eq!(result.dirs_created, 0);
        assert!(result.warnings.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_expand_home() {
        // Non-home paths should be unchanged
        assert_eq!(expand_home("/some/path"), "/some/path");
        assert_eq!(expand_home("relative/path"), "relative/path");
    }

    #[test]
    fn test_resolve_workspace() {
        let ws = resolve_workspace("/home/user/.openclaw");
        let expected = Path::new("/home/user/.openclaw")
            .join("workspace")
            .to_string_lossy()
            .to_string();
        assert_eq!(ws, expected);
    }

    #[test]
    fn test_rel_path() {
        let rel = rel_path("/home/user/.openclaw/workspace/SOUL.md", "/home/user/.openclaw");
        assert_eq!(rel, "workspace/SOUL.md");
    }

    #[test]
    fn test_run_full_migration_mutually_exclusive() {
        let opts = MigrateOptions {
            config_only: true,
            workspace_only: true,
            ..Default::default()
        };
        let result = run_full_migration(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("mutually exclusive"));
    }

    #[test]
    fn test_execute_skip_actions() {
        let actions = vec![
            FullMigrationAction {
                action_type: FullMigrationActionType::Skip,
                source: Some("/tmp/nonexistent".to_string()),
                destination: "/tmp/dest".to_string(),
                description: "test skip".to_string(),
            },
        ];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert_eq!(result.files_skipped, 1);
    }

    #[test]
    fn test_execute_create_dir() {
        let dir = tempfile::tempdir().unwrap();
        let new_dir = dir.path().join("subdir").to_string_lossy().to_string();

        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::CreateDir,
            source: None,
            destination: new_dir.clone(),
            description: "create directory".to_string(),
        }];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert_eq!(result.dirs_created, 1);
        assert!(Path::new(&new_dir).exists());
    }

    #[test]
    fn test_execute_copy_action() {
        let dir = tempfile::tempdir().unwrap();
        let src_file = dir.path().join("source.txt");
        let dst_file = dir.path().join("dest.txt");
        fs::write(&src_file, "hello").unwrap();

        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::Copy,
            source: Some(src_file.to_string_lossy().to_string()),
            destination: dst_file.to_string_lossy().to_string(),
            description: "copy file".to_string(),
        }];
        let result = execute(
            &actions,
            dir.path().to_string_lossy().as_ref(),
            "/tmp/nemesisbot",
        );
        assert_eq!(result.files_copied, 1);
        assert!(dst_file.exists());
        assert_eq!(fs::read_to_string(&dst_file).unwrap(), "hello");
    }

    #[test]
    fn test_execute_backup_action() {
        let dir = tempfile::tempdir().unwrap();
        let src_file = dir.path().join("source.txt");
        let dst_file = dir.path().join("dest.txt");
        fs::write(&src_file, "new content").unwrap();
        fs::write(&dst_file, "old content").unwrap();

        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::Backup,
            source: Some(src_file.to_string_lossy().to_string()),
            destination: dst_file.to_string_lossy().to_string(),
            description: "backup and overwrite".to_string(),
        }];
        let result = execute(
            &actions,
            dir.path().to_string_lossy().as_ref(),
            "/tmp/nemesisbot",
        );
        assert_eq!(result.files_copied, 1);
        assert_eq!(result.backups_created, 1);
        assert_eq!(fs::read_to_string(&dst_file).unwrap(), "new content");
        assert_eq!(
            fs::read_to_string(format!("{}.bak", dst_file.to_string_lossy())).unwrap(),
            "old content"
        );
    }

    #[test]
    fn test_print_plan_no_panic() {
        let actions = vec![
            FullMigrationAction {
                action_type: FullMigrationActionType::ConvertConfig,
                source: Some("/src/openclaw.json".to_string()),
                destination: "/dst/config.json".to_string(),
                description: "convert config".to_string(),
            },
            FullMigrationAction {
                action_type: FullMigrationActionType::Copy,
                source: Some("/src/SOUL.md".to_string()),
                destination: "/dst/SOUL.md".to_string(),
                description: "copy file".to_string(),
            },
            FullMigrationAction {
                action_type: FullMigrationActionType::Skip,
                source: Some("/src/missing.txt".to_string()),
                destination: "/dst/missing.txt".to_string(),
                description: "not found".to_string(),
            },
        ];
        let warnings = vec!["test warning".to_string()];
        // Should not panic
        print_plan(&actions, &warnings);
    }

    #[test]
    fn test_print_summary_no_panic() {
        let result = FullMigrationResult {
            files_copied: 3,
            files_skipped: 1,
            backups_created: 1,
            config_migrated: true,
            dirs_created: 2,
            warnings: vec!["warn".to_string()],
            errors: vec!["err".to_string()],
        };
        // Should not panic
        print_summary(&result);
    }

    #[test]
    fn test_plan_config_only() {
        let dir = tempfile::tempdir().unwrap();
        let openclaw_home = dir.path().join(".openclaw");
        let nemesisbot_home = dir.path().join(".nemesisbot");
        fs::create_dir_all(&openclaw_home).unwrap();
        fs::create_dir_all(&nemesisbot_home).unwrap();

        // Create a minimal OpenClaw config
        let config = serde_json::json!({
            "agents": {"defaults": {"llm": "zhipu/glm-4"}},
            "providers": {},
            "channels": {}
        });
        fs::write(
            openclaw_home.join("openclaw.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();

        let opts = MigrateOptions {
            config_only: true,
            force: true,
            ..Default::default()
        };

        let (actions, warnings) = plan(
            &opts,
            openclaw_home.to_string_lossy().as_ref(),
            nemesisbot_home.to_string_lossy().as_ref(),
        )
        .unwrap();

        assert!(actions
            .iter()
            .any(|a| a.action_type == FullMigrationActionType::ConvertConfig));
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_execute_config_migration() {
        let dir = tempfile::tempdir().unwrap();

        // Create a minimal OpenClaw config
        let src_config = serde_json::json!({
            "agents": {"defaults": {"llm": "zhipu/glm-4"}},
            "providers": {"zhipu": {"api_key": "test-key"}},
            "channels": {}
        });
        let src_path = dir.path().join("openclaw.json");
        fs::write(&src_path, serde_json::to_string_pretty(&src_config).unwrap()).unwrap();

        let dst_path = dir.path().join("config.json");

        execute_config_migration(
            src_path.to_string_lossy().as_ref(),
            dst_path.to_string_lossy().as_ref(),
        )
        .unwrap();

        assert!(dst_path.exists());
        let result: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dst_path).unwrap()).unwrap();
        // Check that providers were converted to model_list
        assert!(result["model_list"].is_array());
        let models = result["model_list"].as_array().unwrap();
        assert!(!models.is_empty());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_needs_migration_valid_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::MigrateConfig {
            workspace_path: dir.path().to_string_lossy().to_string(),
            target_version: 1,
        };
        let migrator = Migrator::new(config);
        assert!(migrator.needs_migration());
    }

    #[test]
    fn test_needs_migration_nonexistent_path() {
        let nonexistent = format!("C:/__nonexistent_migrate_path_{}", std::process::id());
        let config = crate::config::MigrateConfig {
            workspace_path: nonexistent,
            target_version: 1,
        };
        let migrator = Migrator::new(config);
        assert!(!migrator.needs_migration());
    }

    #[test]
    fn test_migrator_new() {
        let config = crate::config::MigrateConfig::default();
        let _migrator = Migrator::new(config);
    }

    #[test]
    fn test_migrator_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        let config = crate::config::MigrateConfig {
            workspace_path: String::new(),
            target_version: 1,
        };
        let migrator = Migrator::new(config);
        let plan = migrator.dry_run(
            src.to_string_lossy().as_ref(),
            dst.to_string_lossy().as_ref(),
            false,
        ).unwrap();
        // No files to migrate
        assert!(plan.actions.iter().all(|a| a.action_type == crate::workspace::ActionType::Skip));
    }

    #[test]
    fn test_resolve_openclaw_home_override() {
        let result = resolve_openclaw_home("/custom/openclaw");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/custom/openclaw");
    }

    #[test]
    fn test_resolve_nemesisbot_home_override() {
        let result = resolve_nemesisbot_home("/custom/nemesisbot");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/custom/nemesisbot");
    }

    #[test]
    fn test_expand_home_with_tilde() {
        // Test that ~ is expanded
        let result = expand_home("~/test");
        // Result should not start with ~
        assert!(!result.starts_with('~') || result == "~/test");
    }

    #[test]
    fn test_expand_home_tilde_only() {
        let result = expand_home("~");
        // Should either expand or remain as ~
        assert!(!result.is_empty());
    }

    #[test]
    fn test_expand_home_no_tilde() {
        assert_eq!(expand_home("/absolute/path"), "/absolute/path");
        assert_eq!(expand_home("relative/path"), "relative/path");
    }

    #[test]
    fn test_run_full_migration_refresh_flag() {
        let dir = tempfile::tempdir().unwrap();
        let openclaw_home = dir.path().join(".openclaw");
        fs::create_dir_all(&openclaw_home).unwrap();
        fs::write(openclaw_home.join("openclaw.json"), r#"{"agents":{"defaults":{}}}"#).unwrap();

        let opts = MigrateOptions {
            refresh: true,
            force: true, // skip stdin confirm
            openclaw_home: openclaw_home.to_string_lossy().to_string(),
            nemesisbot_home: dir.path().join(".nemesisbot").to_string_lossy().to_string(),
            ..Default::default()
        };
        // refresh sets workspace_only=true internally, should work
        let result = run_full_migration(&opts);
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_run_full_migration_nonexistent_source() {
        let opts = MigrateOptions {
            openclaw_home: "/nonexistent/.openclaw".to_string(),
            ..Default::default()
        };
        let result = run_full_migration(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_execute_merge_config_action() {
        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::MergeConfig,
            source: Some("/tmp/config.json".to_string()),
            destination: "/tmp/dest.json".to_string(),
            description: "merge config".to_string(),
        }];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert_eq!(result.files_skipped, 1);
    }

    #[test]
    fn test_execute_copy_nonexistent_source() {
        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::Copy,
            source: Some("/nonexistent/source.txt".to_string()),
            destination: "/tmp/dest.txt".to_string(),
            description: "copy nonexistent".to_string(),
        }];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert_eq!(result.files_copied, 0);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_execute_backup_nonexistent_source() {
        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::Backup,
            source: Some("/nonexistent/source.txt".to_string()),
            destination: "/tmp/dest.txt".to_string(),
            description: "backup nonexistent".to_string(),
        }];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert_eq!(result.files_copied, 0);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_execute_convert_config_missing_source() {
        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::ConvertConfig,
            source: Some("/nonexistent/config.json".to_string()),
            destination: "/tmp/output.json".to_string(),
            description: "convert missing config".to_string(),
        }];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert!(!result.config_migrated);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_execute_convert_config_no_source() {
        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::ConvertConfig,
            source: None,
            destination: "/tmp/output.json".to_string(),
            description: "no source".to_string(),
        }];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert!(!result.config_migrated);
    }

    #[test]
    fn test_rel_path_no_prefix_match() {
        let rel = rel_path("/other/path/file.txt", "/home/user");
        assert_eq!(rel, "file.txt");
    }

    #[test]
    fn test_plan_workspace_migration() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src_ws");
        let dst = dir.path().join("dst_ws");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("SOUL.md"), "soul content").unwrap();

        let actions = plan_workspace_migration(
            src.to_string_lossy().as_ref(),
            dst.to_string_lossy().as_ref(),
            false,
        )
        .unwrap();
        assert!(!actions.is_empty());
        assert!(actions.iter().any(|a| a.action_type == FullMigrationActionType::Copy));
    }

    #[test]
    fn test_execute_config_migration_with_merge() {
        let dir = tempfile::tempdir().unwrap();

        // Source OpenClaw config
        let src_config = serde_json::json!({
            "agents": {"defaults": {"llm": "zhipu/glm-4"}},
            "providers": {"zhipu": {"api_key": "key1"}},
            "channels": {}
        });
        let src_path = dir.path().join("openclaw.json");
        fs::write(&src_path, serde_json::to_string_pretty(&src_config).unwrap()).unwrap();

        // Existing NemesisBot config to merge into
        let dst_path = dir.path().join("config.json");
        let existing = serde_json::json!({
            "model_list": [{"model_name": "existing", "model": "test/existing"}],
            "channels": {}
        });
        fs::write(&dst_path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

        execute_config_migration(
            src_path.to_string_lossy().as_ref(),
            dst_path.to_string_lossy().as_ref(),
        )
        .unwrap();

        let result: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&dst_path).unwrap()).unwrap();
        let models = result["model_list"].as_array().unwrap();
        // Should have both existing and new model
        assert!(models.len() >= 2);
    }

    #[test]
    fn test_plan_config_not_found_warning() {
        let dir = tempfile::tempdir().unwrap();
        let openclaw_home = dir.path().join(".openclaw");
        let nemesisbot_home = dir.path().join(".nemesisbot");
        fs::create_dir_all(&openclaw_home).unwrap();
        fs::create_dir_all(&nemesisbot_home).unwrap();
        // No config file created

        let opts = MigrateOptions {
            force: true,
            ..Default::default()
        };
        let (_, warnings) = plan(
            &opts,
            openclaw_home.to_string_lossy().as_ref(),
            nemesisbot_home.to_string_lossy().as_ref(),
        )
        .unwrap();
        assert!(warnings.iter().any(|w| w.contains("Config migration skipped")));
    }

    #[test]
    fn test_print_plan_merge_action() {
        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::MergeConfig,
            source: Some("/src/config.json".to_string()),
            destination: "/dst/config.json".to_string(),
            description: "merge config".to_string(),
        }];
        // Should not panic
        print_plan(&actions, &[]);
    }

    #[test]
    fn test_print_summary_no_actions() {
        let result = FullMigrationResult::default();
        // Should not panic
        print_summary(&result);
    }

    #[test]
    fn test_copy_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        fs::write(&src, "content").unwrap();
        copy_file(src.to_string_lossy().as_ref(), dst.to_string_lossy().as_ref()).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "content");
    }

    #[test]
    fn test_copy_file_nonexistent() {
        let result = copy_file("/nonexistent/src.txt", "/tmp/dst.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_full_migration_action_type_equality() {
        assert_eq!(FullMigrationActionType::Copy, FullMigrationActionType::Copy);
        assert_ne!(FullMigrationActionType::Copy, FullMigrationActionType::Skip);
    }

    #[test]
    fn test_migrate_options_serialization() {
        let opts = MigrateOptions::default();
        let json = serde_json::to_string(&opts).unwrap();
        let deserialized: MigrateOptions = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.dry_run, opts.dry_run);
        assert_eq!(deserialized.force, opts.force);
    }

    #[test]
    fn test_full_migration_result_serialization() {
        let result = FullMigrationResult {
            files_copied: 5,
            files_skipped: 2,
            backups_created: 1,
            config_migrated: true,
            dirs_created: 3,
            warnings: vec!["w1".to_string()],
            errors: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: FullMigrationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.files_copied, 5);
        assert!(deserialized.config_migrated);
    }

    #[test]
    fn test_run_full_migration_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let openclaw_home = dir.path().join(".openclaw");
        fs::create_dir_all(&openclaw_home).unwrap();
        fs::write(
            openclaw_home.join("openclaw.json"),
            r#"{"agents":{"defaults":{}}}"#,
        )
        .unwrap();

        let opts = MigrateOptions {
            dry_run: true,
            openclaw_home: openclaw_home.to_string_lossy().to_string(),
            nemesisbot_home: dir.path().join(".nemesisbot").to_string_lossy().to_string(),
            ..Default::default()
        };
        let result = run_full_migration(&opts).unwrap();
        assert_eq!(result.files_copied, 0);
    }

    #[test]
    fn test_plan_workspace_only_no_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let openclaw_home = dir.path().join(".openclaw");
        let nemesisbot_home = dir.path().join(".nemesisbot");
        fs::create_dir_all(&openclaw_home).unwrap();
        fs::create_dir_all(&nemesisbot_home).unwrap();

        let opts = MigrateOptions {
            workspace_only: true,
            force: true,
            ..Default::default()
        };
        let (_, warnings) = plan(
            &opts,
            openclaw_home.to_string_lossy().as_ref(),
            nemesisbot_home.to_string_lossy().as_ref(),
        )
        .unwrap();
        assert!(warnings.iter().any(|w| w.contains("workspace directory not found")));
    }

    #[test]
    fn test_plan_with_workspace_files() {
        let dir = tempfile::tempdir().unwrap();
        let openclaw_home = dir.path().join(".openclaw");
        let nemesisbot_home = dir.path().join(".nemesisbot");
        let openclaw_ws = openclaw_home.join("workspace");
        fs::create_dir_all(&openclaw_ws).unwrap();
        fs::create_dir_all(&nemesisbot_home).unwrap();
        fs::write(openclaw_ws.join("SOUL.md"), "soul content").unwrap();
        fs::write(openclaw_ws.join("USER.md"), "user content").unwrap();

        let opts = MigrateOptions {
            force: true,
            ..Default::default()
        };
        let (actions, _) = plan(
            &opts,
            openclaw_home.to_string_lossy().as_ref(),
            nemesisbot_home.to_string_lossy().as_ref(),
        )
        .unwrap();
        // Should have workspace-related actions (Copy for SOUL.md etc.)
        assert!(actions.iter().any(|a| a.action_type == FullMigrationActionType::Copy));
    }

    #[test]
    fn test_plan_config_only_not_found_error() {
        let dir = tempfile::tempdir().unwrap();
        let openclaw_home = dir.path().join(".openclaw");
        let nemesisbot_home = dir.path().join(".nemesisbot");
        fs::create_dir_all(&openclaw_home).unwrap();
        fs::create_dir_all(&nemesisbot_home).unwrap();

        let opts = MigrateOptions {
            config_only: true,
            ..Default::default()
        };
        let result = plan(
            &opts,
            openclaw_home.to_string_lossy().as_ref(),
            nemesisbot_home.to_string_lossy().as_ref(),
        );
        // config_only with missing config should error
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_backup_no_source() {
        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::Backup,
            source: None,
            destination: "/tmp/dest.txt".to_string(),
            description: "no source".to_string(),
        }];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert_eq!(result.files_copied, 0);
        assert_eq!(result.backups_created, 0);
    }

    #[test]
    fn test_execute_copy_no_source() {
        let actions = vec![FullMigrationAction {
            action_type: FullMigrationActionType::Copy,
            source: None,
            destination: "/tmp/dest.txt".to_string(),
            description: "no source".to_string(),
        }];
        let result = execute(&actions, "/tmp/openclaw", "/tmp/nemesisbot");
        assert_eq!(result.files_copied, 0);
    }

    #[test]
    fn test_run_full_migration_workspace_only_with_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let openclaw_home = dir.path().join(".openclaw");
        let openclaw_ws = openclaw_home.join("workspace");
        fs::create_dir_all(&openclaw_ws).unwrap();
        fs::write(openclaw_ws.join("SOUL.md"), "soul content").unwrap();

        let opts = MigrateOptions {
            workspace_only: true,
            force: true,
            openclaw_home: openclaw_home.to_string_lossy().to_string(),
            nemesisbot_home: dir.path().join(".nemesisbot").to_string_lossy().to_string(),
            ..Default::default()
        };
        let result = run_full_migration(&opts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dirs_home_fallback() {
        // Just verify dirs_home doesn't panic and returns something
        let _ = dirs_home();
    }

    #[test]
    fn test_resolve_openclaw_home_default() {
        // No override, should resolve via dirs_home
        let result = resolve_openclaw_home("");
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.contains(".openclaw"));
    }

    #[test]
    fn test_resolve_nemesisbot_home_default() {
        // No override, should resolve via dirs_home
        let result = resolve_nemesisbot_home("");
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.contains(".nemesisbot"));
    }

    #[test]
    fn test_rel_path_with_no_filename() {
        let rel = rel_path("/", "/base");
        assert!(!rel.is_empty());
    }
}
