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
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_plan_empty_src() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        let plan = WorkspaceMigrator::plan_migration(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            false,
        ).unwrap();
        // All actions should be Skip since no source files exist
        assert!(plan.actions.iter().all(|a| a.action_type == ActionType::Skip));
    }

    #[test]
    fn test_plan_with_files() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("SOUL.md"), "test soul content").unwrap();

        let plan = WorkspaceMigrator::plan_migration(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            false,
        ).unwrap();
        assert!(plan.total_files >= 1);
    }

    #[test]
    fn test_execute_migration() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("SOUL.md"), "test content").unwrap();
        fs::write(src.join("USER.md"), "user content").unwrap();

        let result = migrate_workspace(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            false,
        ).unwrap();
        assert_eq!(result.files_copied, 2);
        assert!(dst.join("SOUL.md").exists());
        assert!(dst.join("USER.md").exists());
    }

    #[test]
    fn test_backup_on_conflict() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("SOUL.md"), "new content").unwrap();
        fs::write(dst.join("SOUL.md"), "old content").unwrap();

        let result = migrate_workspace(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            false,
        ).unwrap();
        assert_eq!(result.files_backed_up, 1);
        assert!(dst.join("SOUL.md.bak").exists());
        let content = fs::read_to_string(dst.join("SOUL.md")).unwrap();
        assert_eq!(content, "new content");
    }

    #[test]
    fn test_dry_run() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("SOUL.md"), "test").unwrap();

        let plan = WorkspaceMigrator::dry_run(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            false,
        ).unwrap();
        assert!(plan.total_files >= 1);
        // Dry run should NOT actually copy files
        assert!(!dst.join("SOUL.md").exists());
    }

    #[test]
    fn test_migration_with_dirs() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(src.join("memory")).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("memory/notes.txt"), "test notes").unwrap();
        fs::write(src.join("SOUL.md"), "soul content").unwrap();

        let result = migrate_workspace(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            false,
        ).unwrap();
        assert!(result.files_copied >= 2);
        assert!(result.dirs_created >= 1);
        assert!(dst.join("memory/notes.txt").exists());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_migrate_creates_workspace() {
        let dir = TempDir::new().unwrap();
        let ws_path = dir.path().join("new_workspace");
        let migrator = WorkspaceMigrator::new(ws_path.to_str().unwrap());
        assert!(!ws_path.exists());
        migrator.migrate().unwrap();
        assert!(ws_path.exists());
    }

    #[test]
    fn test_migrate_existing_workspace_ok() {
        let dir = TempDir::new().unwrap();
        let ws_path = dir.path().join("existing");
        fs::create_dir_all(&ws_path).unwrap();
        let migrator = WorkspaceMigrator::new(ws_path.to_str().unwrap());
        migrator.migrate().unwrap();
    }

    #[test]
    fn test_plan_with_force() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("SOUL.md"), "new content").unwrap();
        fs::write(dst.join("SOUL.md"), "existing content").unwrap();

        let plan = WorkspaceMigrator::plan_migration(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            true, // force
        ).unwrap();

        // With force, should be Copy (not Backup) since we force overwrite
        let soul_action = plan.actions.iter().find(|a| a.destination.contains("SOUL.md")).unwrap();
        assert_eq!(soul_action.action_type, ActionType::Copy);
    }

    #[test]
    fn test_plan_nonexistent_src_dir() {
        let dir = TempDir::new().unwrap();
        let dst = dir.path().join("dst");
        fs::create_dir_all(&dst).unwrap();

        let plan = WorkspaceMigrator::plan_migration(
            "/nonexistent/src",
            dst.to_str().unwrap(),
            false,
        ).unwrap();
        // All actions should be Skip since no source files exist
        assert!(plan.actions.iter().all(|a| a.action_type == ActionType::Skip));
        assert_eq!(plan.total_files, 0);
    }

    #[test]
    fn test_execute_plan_skip_actions() {
        let dir = TempDir::new().unwrap();
        let dst = dir.path().join("dst");
        fs::create_dir_all(&dst).unwrap();

        let plan = MigrationPlan {
            actions: vec![
                MigrationAction {
                    action_type: ActionType::Skip,
                    source: None,
                    destination: "/tmp/skip".to_string(),
                    description: "test skip".to_string(),
                },
            ],
            total_files: 0,
            total_dirs: 0,
        };

        let result = WorkspaceMigrator::execute_plan(&plan).unwrap();
        assert_eq!(result.files_skipped, 1);
        assert_eq!(result.files_copied, 0);
    }

    #[test]
    fn test_execute_plan_create_dir_action() {
        let dir = TempDir::new().unwrap();
        let new_dir = dir.path().join("new_subdir");

        let plan = MigrationPlan {
            actions: vec![
                MigrationAction {
                    action_type: ActionType::CreateDir,
                    source: None,
                    destination: new_dir.to_str().unwrap().to_string(),
                    description: "create dir".to_string(),
                },
            ],
            total_dirs: 1,
            total_files: 0,
        };

        let result = WorkspaceMigrator::execute_plan(&plan).unwrap();
        assert_eq!(result.dirs_created, 1);
        assert!(new_dir.exists());
    }

    #[test]
    fn test_migration_with_nested_dirs() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::create_dir_all(src.join("skills").join("subdir")).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join("skills/subdir/skill.md"), "skill content").unwrap();
        fs::write(src.join("SOUL.md"), "soul").unwrap();

        let result = migrate_workspace(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            false,
        ).unwrap();

        assert!(result.dirs_created >= 2);
        assert!(dst.join("skills/subdir/skill.md").exists());
    }

    #[test]
    fn test_migration_result_serialization() {
        let result = MigrationResult {
            files_copied: 3,
            files_backed_up: 1,
            dirs_created: 2,
            files_skipped: 5,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: MigrationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.files_copied, 3);
        assert_eq!(deserialized.files_backed_up, 1);
    }

    #[test]
    fn test_action_type_equality() {
        assert_eq!(ActionType::Copy, ActionType::Copy);
        assert_ne!(ActionType::Copy, ActionType::Skip);
        assert_ne!(ActionType::Backup, ActionType::CreateDir);
    }

    #[test]
    fn test_migration_plan_serialization() {
        let plan = MigrationPlan {
            actions: vec![MigrationAction {
                action_type: ActionType::Copy,
                source: Some("/src/file.txt".to_string()),
                destination: "/dst/file.txt".to_string(),
                description: "copy file".to_string(),
            }],
            total_files: 1,
            total_dirs: 0,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let deserialized: MigrationPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total_files, 1);
    }
}
