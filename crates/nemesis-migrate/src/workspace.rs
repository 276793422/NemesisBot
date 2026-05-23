//! Workspace migration: MigrateConfig, MigrateWorkspace, DryRun.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Migration action types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    Skip,
    Backup,
    Copy,
    CreateDir,
}

/// A single migration action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationAction {
    #[serde(rename = "type")]
    pub action_type: ActionType,
    pub source: Option<String>,
    pub destination: String,
    pub description: String,
}

/// Migration plan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPlan {
    pub actions: Vec<MigrationAction>,
    pub total_files: usize,
    pub total_dirs: usize,
}

/// Workspace migrator handles data migration between versions.
pub struct WorkspaceMigrator {
    workspace_path: PathBuf,
}

/// Files that can be migrated.
const MIGRATEABLE_FILES: &[&str] = &[
    "AGENT.md",
    "SOUL.md",
    "USER.md",
    "TOOLS.md",
    "HEARTBEAT.md",
];

/// Directories that can be migrated.
const MIGRATEABLE_DIRS: &[&str] = &[
    "memory",
    "skills",
];

impl WorkspaceMigrator {
    pub fn new(workspace_path: &str) -> Self {
        Self { workspace_path: PathBuf::from(workspace_path) }
    }

    /// Run workspace migrations (create workspace if needed).
    pub fn migrate(&self) -> Result<(), String> {
        if !self.workspace_path.exists() {
            fs::create_dir_all(&self.workspace_path)
                .map_err(|e| format!("create workspace: {}", e))?;
        }
        Ok(())
    }

    /// Plan a migration from src to dst workspace.
    pub fn plan_migration(src: &str, dst: &str, force: bool) -> Result<MigrationPlan, String> {
        let mut actions = Vec::new();
        let mut total_files = 0;
        let mut total_dirs = 0;

        for filename in MIGRATEABLE_FILES {
            let src_path = Path::new(src).join(filename);
            let dst_path = Path::new(dst).join(filename);
            let action = plan_file_copy(&src_path, &dst_path, force);
            if action.action_type != ActionType::Skip {
                total_files += 1;
            }
            actions.push(action);
        }

        for dirname in MIGRATEABLE_DIRS {
            let src_dir = Path::new(src).join(dirname);
            if !src_dir.exists() {
                continue;
            }
            let dst_dir = Path::new(dst).join(dirname);
            let dir_actions = plan_dir_copy(&src_dir, &dst_dir, force)?;
            for action in &dir_actions {
                match action.action_type {
                    ActionType::CreateDir => total_dirs += 1,
                    ActionType::Copy | ActionType::Backup => total_files += 1,
                    _ => {}
                }
            }
            actions.extend(dir_actions);
        }

        Ok(MigrationPlan { actions, total_files, total_dirs })
    }

    /// Execute a migration plan.
    pub fn execute_plan(plan: &MigrationPlan) -> Result<MigrationResult, String> {
        let mut copied = 0;
        let mut backed_up = 0;
        let mut dirs_created = 0;
        let mut skipped = 0;

        for action in &plan.actions {
            match action.action_type {
                ActionType::Skip => {
                    skipped += 1;
                }
                ActionType::CreateDir => {
                    let path = Path::new(&action.destination);
                    if !path.exists() {
                        fs::create_dir_all(path)
                            .map_err(|e| format!("create dir {}: {}", action.destination, e))?;
                    }
                    dirs_created += 1;
                }
                ActionType::Copy => {
                    if let Some(ref src) = action.source {
                        copy_file_with_mkdir(src, &action.destination)?;
                        copied += 1;
                    }
                }
                ActionType::Backup => {
                    if let Some(ref src) = action.source {
                        // Backup existing destination
                        let backup_path = format!("{}.bak", action.destination);
                        let dst = Path::new(&action.destination);
                        if dst.exists() {
                            fs::copy(dst, &backup_path)
                                .map_err(|e| format!("backup {}: {}", action.destination, e))?;
                        }
                        copy_file_with_mkdir(src, &action.destination)?;
                        backed_up += 1;
                    }
                }
            }
        }

        Ok(MigrationResult {
            files_copied: copied,
            files_backed_up: backed_up,
            dirs_created,
            files_skipped: skipped,
        })
    }

    /// Dry run: plan and return what would happen without executing.
    pub fn dry_run(src: &str, dst: &str, force: bool) -> Result<MigrationPlan, String> {
        Self::plan_migration(src, dst, force)
    }
}

/// Migration execution result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationResult {
    pub files_copied: usize,
    pub files_backed_up: usize,
    pub dirs_created: usize,
    pub files_skipped: usize,
}

/// Plan a single file copy action.
fn plan_file_copy(src: &Path, dst: &Path, force: bool) -> MigrationAction {
    if !src.exists() {
        return MigrationAction {
            action_type: ActionType::Skip,
            source: Some(src.to_string_lossy().to_string()),
            destination: dst.to_string_lossy().to_string(),
            description: "source file not found".to_string(),
        };
    }

    if dst.exists() && !force {
        return MigrationAction {
            action_type: ActionType::Backup,
            source: Some(src.to_string_lossy().to_string()),
            destination: dst.to_string_lossy().to_string(),
            description: "destination exists, will backup and overwrite".to_string(),
        };
    }

    MigrationAction {
        action_type: ActionType::Copy,
        source: Some(src.to_string_lossy().to_string()),
        destination: dst.to_string_lossy().to_string(),
        description: "copy file".to_string(),
    }
}

/// Plan a directory copy action.
fn plan_dir_copy(src_dir: &Path, dst_dir: &Path, force: bool) -> Result<Vec<MigrationAction>, String> {
    let mut actions = Vec::new();

    actions.push(MigrationAction {
        action_type: ActionType::CreateDir,
        source: None,
        destination: dst_dir.to_string_lossy().to_string(),
        description: "create directory".to_string(),
    });

    let entries = fs::read_dir(src_dir)
        .map_err(|e| format!("read dir {}: {}", src_dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("read entry: {}", e))?;
        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default();
        let dst_path = dst_dir.join(file_name);

        if path.is_dir() {
            let sub_actions = plan_dir_copy(&path, &dst_path, force)?;
            actions.extend(sub_actions);
        } else {
            let action = plan_file_copy(&path, &dst_path, force);
            actions.push(action);
        }
    }

    Ok(actions)
}

/// Copy a file, creating parent directories as needed.
fn copy_file_with_mkdir(src: &str, dst: &str) -> Result<(), String> {
    let dst_path = Path::new(dst);
    if let Some(parent) = dst_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    fs::copy(src, dst)
        .map_err(|e| format!("copy {} -> {}: {}", src, dst, e))?;
    Ok(())
}

/// Run a full migration from src to dst workspace.
pub fn migrate_workspace(src: &str, dst: &str, force: bool) -> Result<MigrationResult, String> {
    let plan = WorkspaceMigrator::plan_migration(src, dst, force)?;
    WorkspaceMigrator::execute_plan(&plan)
}

#[cfg(test)]
mod tests;
