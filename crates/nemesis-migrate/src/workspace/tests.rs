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
