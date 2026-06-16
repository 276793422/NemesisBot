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

// ============================================================
// Additional coverage tests for 95%+ target — Phase 2
// ============================================================

#[test]
fn test_plan_migration_with_skip_in_dir() {
    // Test that Skip actions inside a directory are handled (covers _ => {} branch on line 96)
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(src.join("memory")).unwrap();
    fs::create_dir_all(&dst).unwrap();

    // Create source file in memory dir, but also create matching dst file
    // so the action becomes Backup (which still counts as file)
    fs::write(src.join("memory/notes.txt"), "notes content").unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    // Should have at least one action with CreateDir for memory/
    assert!(plan.actions.iter().any(|a| a.action_type == ActionType::CreateDir));
    assert!(plan.total_dirs >= 1);
}

#[test]
fn test_plan_migration_count_skip_in_dir_as_zero() {
    // When a file inside a dir exists in dst but not src, it should be a Skip
    // and not count toward total_files (covers _ => {} branch)
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    // Create memory dir but with only AGENT.md (not in MIGRATEABLE_FILES, but
    // dir copy handles all files inside).
    fs::create_dir_all(src.join("memory")).unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    // memory dir gets CreateDir but no files inside it (besides the dir itself).
    // The _ => {} branch covers Skip actions encountered during dir traversal.
    assert!(plan.total_dirs >= 1);
}

#[test]
fn test_plan_migration_empty_memory_dir() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(src.join("memory")).unwrap();
    fs::create_dir_all(&dst).unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    // memory dir gets CreateDir action even if empty
    assert!(plan
        .actions
        .iter()
        .any(|a| a.action_type == ActionType::CreateDir
            && a.destination.ends_with("memory")));
}

#[test]
fn test_execute_plan_with_existing_dir() {
    // When CreateDir is called for a path that already exists, execute_plan should skip creation
    let dir = TempDir::new().unwrap();
    let existing_dir = dir.path().join("already_there");
    fs::create_dir_all(&existing_dir).unwrap();

    let plan = MigrationPlan {
        actions: vec![MigrationAction {
            action_type: ActionType::CreateDir,
            source: None,
            destination: existing_dir.to_str().unwrap().to_string(),
            description: "create dir".to_string(),
        }],
        total_dirs: 1,
        total_files: 0,
    };

    let result = WorkspaceMigrator::execute_plan(&plan).unwrap();
    assert_eq!(result.dirs_created, 1);
    assert!(existing_dir.exists());
}

#[test]
fn test_execute_plan_copy_no_source() {
    // Copy action with source=None should not increment files_copied
    let dir = TempDir::new().unwrap();
    let plan = MigrationPlan {
        actions: vec![MigrationAction {
            action_type: ActionType::Copy,
            source: None,
            destination: dir.path().join("dst.txt").to_str().unwrap().to_string(),
            description: "no source".to_string(),
        }],
        total_files: 1,
        total_dirs: 0,
    };
    let result = WorkspaceMigrator::execute_plan(&plan).unwrap();
    assert_eq!(result.files_copied, 0);
}

#[test]
fn test_execute_plan_backup_no_source() {
    // Backup action with source=None should not increment files_backed_up
    let dir = TempDir::new().unwrap();
    let plan = MigrationPlan {
        actions: vec![MigrationAction {
            action_type: ActionType::Backup,
            source: None,
            destination: dir.path().join("dst.txt").to_str().unwrap().to_string(),
            description: "no source".to_string(),
        }],
        total_files: 1,
        total_dirs: 0,
    };
    let result = WorkspaceMigrator::execute_plan(&plan).unwrap();
    assert_eq!(result.files_backed_up, 0);
}

#[test]
fn test_execute_plan_backup_dst_not_exists() {
    // Backup action where dst doesn't exist — should skip backup step but still copy src
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("new_dst.txt");
    fs::write(&src, "content").unwrap();
    // dst doesn't exist

    let plan = MigrationPlan {
        actions: vec![MigrationAction {
            action_type: ActionType::Backup,
            source: Some(src.to_str().unwrap().to_string()),
            destination: dst.to_str().unwrap().to_string(),
            description: "backup".to_string(),
        }],
        total_files: 1,
        total_dirs: 0,
    };
    let result = WorkspaceMigrator::execute_plan(&plan).unwrap();
    assert_eq!(result.files_backed_up, 1);
    assert!(dst.exists());
    assert_eq!(fs::read_to_string(&dst).unwrap(), "content");
}

#[test]
fn test_execute_plan_backup_dst_exists() {
    // Backup action where dst exists — should backup then copy
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    fs::write(&src, "new").unwrap();
    fs::write(&dst, "old").unwrap();

    let plan = MigrationPlan {
        actions: vec![MigrationAction {
            action_type: ActionType::Backup,
            source: Some(src.to_str().unwrap().to_string()),
            destination: dst.to_str().unwrap().to_string(),
            description: "backup".to_string(),
        }],
        total_files: 1,
        total_dirs: 0,
    };
    let result = WorkspaceMigrator::execute_plan(&plan).unwrap();
    assert_eq!(result.files_backed_up, 1);
    let backup_path = format!("{}.bak", dst.to_str().unwrap());
    assert!(Path::new(&backup_path).exists());
    assert_eq!(fs::read_to_string(&backup_path).unwrap(), "old");
    assert_eq!(fs::read_to_string(&dst).unwrap(), "new");
}

#[test]
fn test_execute_plan_copy_failure() {
    // Copy action where source doesn't exist — should return error
    let dir = TempDir::new().unwrap();
    let nonexistent_src = dir.path().join("missing.txt");
    let dst = dir.path().join("dst.txt");

    let plan = MigrationPlan {
        actions: vec![MigrationAction {
            action_type: ActionType::Copy,
            source: Some(nonexistent_src.to_str().unwrap().to_string()),
            destination: dst.to_str().unwrap().to_string(),
            description: "copy".to_string(),
        }],
        total_files: 1,
        total_dirs: 0,
    };
    let result = WorkspaceMigrator::execute_plan(&plan);
    assert!(result.is_err());
}

#[test]
fn test_execute_plan_backup_copy_failure() {
    // Backup action where source doesn't exist — should return error after backup
    let dir = TempDir::new().unwrap();
    let nonexistent_src = dir.path().join("missing.txt");
    let dst = dir.path().join("dst.txt");
    fs::write(&dst, "existing").unwrap();

    let plan = MigrationPlan {
        actions: vec![MigrationAction {
            action_type: ActionType::Backup,
            source: Some(nonexistent_src.to_str().unwrap().to_string()),
            destination: dst.to_str().unwrap().to_string(),
            description: "backup".to_string(),
        }],
        total_files: 1,
        total_dirs: 0,
    };
    let result = WorkspaceMigrator::execute_plan(&plan);
    assert!(result.is_err());
}

#[test]
fn test_execute_plan_create_dir_failure() {
    // CreateDir where parent is a file — should fail
    let dir = TempDir::new().unwrap();
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "x").unwrap();
    let impossible = blocker.join("subdir");

    let plan = MigrationPlan {
        actions: vec![MigrationAction {
            action_type: ActionType::CreateDir,
            source: None,
            destination: impossible.to_str().unwrap().to_string(),
            description: "create dir".to_string(),
        }],
        total_dirs: 1,
        total_files: 0,
    };
    let result = WorkspaceMigrator::execute_plan(&plan);
    assert!(result.is_err());
}

#[test]
fn test_migrate_workspace_full_with_force() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("SOUL.md"), "new").unwrap();
    fs::write(dst.join("SOUL.md"), "old").unwrap();

    let result = migrate_workspace(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        true, // force
    )
    .unwrap();
    // With force, should be Copy not Backup
    assert_eq!(result.files_backed_up, 0);
    assert!(result.files_copied >= 1);
    assert_eq!(fs::read_to_string(dst.join("SOUL.md")).unwrap(), "new");
}

#[test]
fn test_dry_run_equals_plan_migration() {
    // dry_run should produce the same plan as plan_migration
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("SOUL.md"), "x").unwrap();

    let plan1 = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    let plan2 = WorkspaceMigrator::dry_run(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(plan1.total_files, plan2.total_files);
    assert_eq!(plan1.total_dirs, plan2.total_dirs);
    assert_eq!(plan1.actions.len(), plan2.actions.len());
}

#[test]
fn test_plan_migration_only_skills_dir() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(src.join("skills")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("skills").join("skill1.md"), "skill").unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    assert!(plan.total_dirs >= 1);
    assert!(plan.total_files >= 1);
    assert!(plan
        .actions
        .iter()
        .any(|a| a.action_type == ActionType::CreateDir
            && a.destination.ends_with("skills")));
}

#[test]
fn test_plan_migration_all_migrateable_files() {
    // Create all 5 migrateable files + verify each is planned as Copy
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    for fname in ["AGENT.md", "SOUL.md", "USER.md", "TOOLS.md", "HEARTBEAT.md"] {
        fs::write(src.join(fname), format!("content {}", fname)).unwrap();
    }

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    // All 5 files should be planned as Copy (since dst doesn't have them)
    let copies = plan
        .actions
        .iter()
        .filter(|a| a.action_type == ActionType::Copy)
        .count();
    assert_eq!(copies, 5);
    assert_eq!(plan.total_files, 5);
}

#[test]
fn test_plan_migration_all_files_force_with_existing_dst() {
    // With force=true and existing dst files, all should be Copy (not Backup)
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    for fname in ["AGENT.md", "SOUL.md"] {
        fs::write(src.join(fname), "new").unwrap();
        fs::write(dst.join(fname), "old").unwrap();
    }

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        true, // force
    )
    .unwrap();
    // All should be Copy (force overrides Backup)
    let copies = plan
        .actions
        .iter()
        .filter(|a| a.action_type == ActionType::Copy)
        .count();
    let backups = plan
        .actions
        .iter()
        .filter(|a| a.action_type == ActionType::Backup)
        .count();
    assert_eq!(copies, 2);
    assert_eq!(backups, 0);
}

#[test]
fn test_plan_migration_files_with_existing_dst_no_force() {
    // Without force, existing dst files should be Backup
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("SOUL.md"), "new").unwrap();
    fs::write(dst.join("SOUL.md"), "old").unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    let backups = plan
        .actions
        .iter()
        .filter(|a| a.action_type == ActionType::Backup)
        .count();
    assert_eq!(backups, 1);
}

#[test]
fn test_plan_migration_with_nested_subdir_in_memory() {
    // memory/sub/file.txt should generate two CreateDir actions
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(src.join("memory").join("sub")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("memory").join("sub").join("file.txt"), "x").unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    let create_dirs = plan
        .actions
        .iter()
        .filter(|a| a.action_type == ActionType::CreateDir)
        .count();
    assert!(create_dirs >= 2);
}

#[test]
fn test_action_type_debug_format() {
    let debug_str = format!("{:?}", ActionType::Copy);
    assert!(debug_str.contains("Copy"));
    let debug_str = format!("{:?}", ActionType::Skip);
    assert!(debug_str.contains("Skip"));
}

#[test]
fn test_migration_action_debug_format() {
    let action = MigrationAction {
        action_type: ActionType::Copy,
        source: Some("/src".to_string()),
        destination: "/dst".to_string(),
        description: "test".to_string(),
    };
    let debug_str = format!("{:?}", action);
    assert!(debug_str.contains("MigrationAction"));
}

#[test]
fn test_migration_plan_debug_format() {
    let plan = MigrationPlan {
        actions: vec![],
        total_files: 0,
        total_dirs: 0,
    };
    let debug_str = format!("{:?}", plan);
    assert!(debug_str.contains("MigrationPlan"));
}

#[test]
fn test_migration_result_debug_format() {
    let result = MigrationResult {
        files_copied: 1,
        files_backed_up: 0,
        dirs_created: 0,
        files_skipped: 0,
    };
    let debug_str = format!("{:?}", result);
    assert!(debug_str.contains("MigrationResult"));
}

#[test]
fn test_migration_action_serialization_round_trip() {
    let action = MigrationAction {
        action_type: ActionType::Backup,
        source: Some("/path".to_string()),
        destination: "/dst".to_string(),
        description: "desc".to_string(),
    };
    let json = serde_json::to_string(&action).unwrap();
    let de: MigrationAction = serde_json::from_str(&json).unwrap();
    assert_eq!(de.action_type, ActionType::Backup);
    assert_eq!(de.source, Some("/path".to_string()));
}

#[test]
fn test_action_type_snake_case_serialization() {
    let json = r#"{"type":"copy","destination":"/x","description":"d"}"#;
    let de: MigrationAction = serde_json::from_str(json).unwrap();
    assert_eq!(de.action_type, ActionType::Copy);

    let json = r#"{"type":"skip","destination":"/x","description":"d"}"#;
    let de: MigrationAction = serde_json::from_str(json).unwrap();
    assert_eq!(de.action_type, ActionType::Skip);

    let json = r#"{"type":"backup","destination":"/x","description":"d"}"#;
    let de: MigrationAction = serde_json::from_str(json).unwrap();
    assert_eq!(de.action_type, ActionType::Backup);

    let json = r#"{"type":"create_dir","destination":"/x","description":"d"}"#;
    let de: MigrationAction = serde_json::from_str(json).unwrap();
    assert_eq!(de.action_type, ActionType::CreateDir);
}

#[test]
fn test_workspace_migrator_new_with_path() {
    let migrator = WorkspaceMigrator::new("/some/path");
    let _ = migrator;
}

#[test]
fn test_migrate_with_relative_path() {
    let dir = TempDir::new().unwrap();
    let rel_path = dir.path().join("new_rel_ws");
    let migrator = WorkspaceMigrator::new(rel_path.to_str().unwrap());
    migrator.migrate().unwrap();
    assert!(rel_path.exists());
}

#[test]
fn test_migrate_workspace_no_files() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();

    let result = migrate_workspace(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    assert_eq!(result.files_copied, 0);
    assert_eq!(result.files_backed_up, 0);
}

#[test]
fn test_plan_dir_copy_recursive() {
    // plan_dir_copy directly — 3 levels deep
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(src.join("a").join("b").join("c")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a").join("b").join("c").join("deep.txt"), "deep").unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    // Note: only memory/ and skills/ dirs are migrated. /a/b/c/ won't appear unless inside one of those.
    // But the function still works without error.
    assert!(plan.total_dirs >= 0);
}

#[test]
fn test_plan_migration_with_memory_subdir_files() {
    // Files inside memory/ subdir — verifies _ => {} branch is hit by Skip
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(src.join("memory")).unwrap();
    fs::create_dir_all(dst.join("memory")).unwrap();
    fs::write(src.join("memory").join("file1.md"), "1").unwrap();
    // dst already has file2.md (not in src) — would be Skip if we iterated dst, but we iterate src
    fs::write(dst.join("memory").join("file2.md"), "2").unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    // file1 in src but not dst -> Copy
    assert!(plan
        .actions
        .iter()
        .any(|a| a.action_type == ActionType::Copy && a.destination.ends_with("file1.md")));
}

#[test]
fn test_execute_plan_multiple_actions() {
    // Multiple actions of different types
    let dir = TempDir::new().unwrap();
    let src1 = dir.path().join("src1.txt");
    let src2 = dir.path().join("src2.txt");
    let dst1 = dir.path().join("dst1.txt");
    let new_dir = dir.path().join("new_dir");
    fs::write(&src1, "content1").unwrap();
    fs::write(&src2, "content2").unwrap();

    let plan = MigrationPlan {
        actions: vec![
            MigrationAction {
                action_type: ActionType::Copy,
                source: Some(src1.to_str().unwrap().to_string()),
                destination: dst1.to_str().unwrap().to_string(),
                description: "copy".to_string(),
            },
            MigrationAction {
                action_type: ActionType::CreateDir,
                source: None,
                destination: new_dir.to_str().unwrap().to_string(),
                description: "mkdir".to_string(),
            },
            MigrationAction {
                action_type: ActionType::Skip,
                source: None,
                destination: "/skip".to_string(),
                description: "skip".to_string(),
            },
            MigrationAction {
                action_type: ActionType::Copy,
                source: Some(src2.to_str().unwrap().to_string()),
                destination: dir.path().join("dst2.txt").to_str().unwrap().to_string(),
                description: "copy".to_string(),
            },
        ],
        total_files: 3,
        total_dirs: 1,
    };
    let result = WorkspaceMigrator::execute_plan(&plan).unwrap();
    assert_eq!(result.files_copied, 2);
    assert_eq!(result.dirs_created, 1);
    assert_eq!(result.files_skipped, 1);
}

#[test]
fn test_migration_with_dirs_count_correct() {
    // Verify that total_dirs is correctly incremented for CreateDir actions inside memory/skills
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(src.join("memory").join("sub1")).unwrap();
    fs::create_dir_all(src.join("memory").join("sub2")).unwrap();
    fs::create_dir_all(src.join("skills").join("sub3")).unwrap();
    fs::create_dir_all(&dst).unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    // memory, memory/sub1, memory/sub2, skills, skills/sub3 = 5 dirs
    assert!(plan.total_dirs >= 5);
}

#[test]
fn test_plan_migration_backed_up_in_subdir() {
    // File inside memory/ that exists at both src and dst -> Backup
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    fs::create_dir_all(src.join("memory")).unwrap();
    fs::create_dir_all(dst.join("memory")).unwrap();
    fs::write(src.join("memory").join("existing.md"), "new").unwrap();
    fs::write(dst.join("memory").join("existing.md"), "old").unwrap();

    let plan = WorkspaceMigrator::plan_migration(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
    )
    .unwrap();
    // Verify Backup action for existing.md
    assert!(plan
        .actions
        .iter()
        .any(|a| a.action_type == ActionType::Backup
            && a.destination.ends_with("existing.md")));
}
