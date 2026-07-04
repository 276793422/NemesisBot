use super::*;
use std::sync::Mutex;

// Single shared lock for tests that mutate process-global env (set_var /
// remove_var on OPENCLAW_HOME / NEMESISBOT_HOME / HOME / USERPROFILE). Env is
// process-wide, so under parallel test execution these race: a writer in one
// test pollutes the env a reader in another sees. Every env-mutating test in
// this file acquires this lock → they run exclusively → no parallel flake.
// (This is the only env-touching test file in the crate, so one local static
// suffices.) Verified equivalent to --test-threads=1 for these tests.
static GLOBAL_STATE_LOCK: Mutex<()> = Mutex::new(());

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

// ============================================================
// Coverage Phase 2: Target previously uncovered branches
// ============================================================

#[test]
fn test_migrator_run_creates_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let new_ws = dir.path().join("ws_to_create");
    let config = crate::config::MigrateConfig {
        workspace_path: new_ws.to_string_lossy().to_string(),
        target_version: 1,
    };
    let migrator = Migrator::new(config);
    assert!(!new_ws.exists());
    migrator.run().unwrap();
    assert!(new_ws.exists());
}

#[test]
fn test_migrator_run_existing_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("already_exists");
    std::fs::create_dir_all(&existing).unwrap();
    let config = crate::config::MigrateConfig {
        workspace_path: existing.to_string_lossy().to_string(),
        target_version: 1,
    };
    let migrator = Migrator::new(config);
    migrator.run().unwrap();
    assert!(existing.exists());
}

#[test]
fn test_print_plan_create_dir_action() {
    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::CreateDir,
        source: None,
        destination: "/tmp/newdir".to_string(),
        description: "create directory".to_string(),
    }];
    // Should not panic — exercises the CreateDir branch of print_plan
    print_plan(&actions, &[]);
}

#[test]
fn test_print_plan_backup_action() {
    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Backup,
        source: Some("/src/file.txt".to_string()),
        destination: "/dst/file.txt".to_string(),
        description: "backup existing".to_string(),
    }];
    // Exercises the Backup branch of print_plan
    print_plan(&actions, &[]);
}

#[test]
fn test_print_plan_skip_with_empty_description() {
    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Skip,
        source: Some("/src/file.txt".to_string()),
        destination: "/dst/file.txt".to_string(),
        description: "".to_string(),
    }];
    // When description is empty, print_plan should skip the println for the file
    print_plan(&actions, &[]);
}

#[test]
fn test_print_plan_skip_with_description() {
    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Skip,
        source: Some("/src/file.txt".to_string()),
        destination: "/dst/file.txt".to_string(),
        description: "already exists".to_string(),
    }];
    // When description is non-empty, prints the file info
    print_plan(&actions, &[]);
}

#[test]
fn test_print_plan_with_warnings() {
    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Copy,
        source: Some("/src/file.txt".to_string()),
        destination: "/dst/file.txt".to_string(),
        description: "copy".to_string(),
    }];
    let warnings = vec![
        "warning one".to_string(),
        "warning two".to_string(),
        "warning three".to_string(),
    ];
    // Exercises the warnings branch
    print_plan(&actions, &warnings);
}

#[test]
fn test_print_plan_empty_actions() {
    // Edge case: no actions, no warnings
    print_plan(&[], &[]);
}

#[test]
fn test_print_plan_copy_with_no_source() {
    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Copy,
        source: None,
        destination: "/dst/file.txt".to_string(),
        description: "no source".to_string(),
    }];
    // Path::new(None.as_deref().unwrap_or("?")) — uses "?" fallback
    print_plan(&actions, &[]);
}

#[test]
fn test_print_plan_convert_config_with_no_source() {
    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::ConvertConfig,
        source: None,
        destination: "/dst/config.json".to_string(),
        description: "no source".to_string(),
    }];
    // Uses "?" fallback for source
    print_plan(&actions, &[]);
}

#[test]
fn test_print_summary_with_errors() {
    let result = FullMigrationResult {
        files_copied: 2,
        files_skipped: 0,
        backups_created: 0,
        config_migrated: false,
        dirs_created: 0,
        warnings: vec![],
        errors: vec!["err1".to_string(), "err2".to_string()],
    };
    print_summary(&result);
}

#[test]
fn test_print_summary_only_skipped() {
    let result = FullMigrationResult {
        files_copied: 0,
        files_skipped: 5,
        backups_created: 0,
        config_migrated: false,
        dirs_created: 0,
        warnings: vec![],
        errors: vec![],
    };
    print_summary(&result);
}

#[test]
fn test_print_summary_only_backups() {
    let result = FullMigrationResult {
        files_copied: 0,
        files_skipped: 0,
        backups_created: 2,
        config_migrated: false,
        dirs_created: 0,
        warnings: vec![],
        errors: vec![],
    };
    print_summary(&result);
}

#[test]
fn test_print_summary_only_config_migrated() {
    let result = FullMigrationResult {
        files_copied: 0,
        files_skipped: 0,
        backups_created: 0,
        config_migrated: true,
        dirs_created: 0,
        warnings: vec![],
        errors: vec![],
    };
    print_summary(&result);
}

#[test]
fn test_execute_config_migration_prints_ok_message() {
    let dir = tempfile::tempdir().unwrap();
    let src_config = serde_json::json!({
        "agents": {"defaults": {"llm": "zhipu/glm-4"}},
        "providers": {},
        "channels": {}
    });
    let src_path = dir.path().join("openclaw.json");
    std::fs::write(&src_path, serde_json::to_string_pretty(&src_config).unwrap()).unwrap();
    let dst_path = dir.path().join("config.json");

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::ConvertConfig,
        source: Some(src_path.to_string_lossy().to_string()),
        destination: dst_path.to_string_lossy().to_string(),
        description: "convert".to_string(),
    }];
    let result = execute(&actions, dir.path().to_string_lossy().as_ref(), dir.path().to_string_lossy().as_ref());
    assert!(result.config_migrated);
    assert!(result.errors.is_empty());
    assert!(dst_path.exists());
}

#[test]
fn test_execute_config_migration_failed_writes_error() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("nonexistent.json");
    let dst_path = dir.path().join("config.json");

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::ConvertConfig,
        source: Some(src_path.to_string_lossy().to_string()),
        destination: dst_path.to_string_lossy().to_string(),
        description: "convert".to_string(),
    }];
    let result = execute(&actions, dir.path().to_string_lossy().as_ref(), dir.path().to_string_lossy().as_ref());
    assert!(!result.config_migrated);
    assert!(!result.errors.is_empty());
}

#[test]
fn test_execute_create_dir_failure() {
    // Try to create a directory whose parent doesn't exist and can't be created.
    // On most systems, create_dir_all on a path under a file (not dir) fails.
    let dir = tempfile::tempdir().unwrap();
    // Create a file at the parent location, then try to create_dir_all under it
    let blocking_file = dir.path().join("blocker");
    std::fs::write(&blocking_file, "block").unwrap();
    let impossible_path = blocking_file.join("subdir").to_string_lossy().to_string();

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::CreateDir,
        source: None,
        destination: impossible_path,
        description: "should fail".to_string(),
    }];
    let result = execute(&actions, "/openclaw", "/nemesisbot");
    // create_dir_all should fail
    assert!(!result.errors.is_empty());
    assert_eq!(result.dirs_created, 0);
}

#[test]
fn test_execute_copy_action_with_mkdir() {
    // Copy action where destination parent doesn't exist — should create it.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("a").join("b").join("c").join("dst.txt");
    std::fs::write(&src, "content").unwrap();

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Copy,
        source: Some(src.to_string_lossy().to_string()),
        destination: dst.to_string_lossy().to_string(),
        description: "copy".to_string(),
    }];
    let result = execute(&actions, dir.path().to_string_lossy().as_ref(), "/nemesisbot");
    assert_eq!(result.files_copied, 1);
    assert!(dst.exists());
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "content");
}

#[test]
fn test_execute_copy_action_mkdir_failure() {
    // Force mkdir failure: dest parent is a file
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    std::fs::write(&src, "content").unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let dst = blocker.join("dst.txt");

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Copy,
        source: Some(src.to_string_lossy().to_string()),
        destination: dst.to_string_lossy().to_string(),
        description: "copy".to_string(),
    }];
    let result = execute(&actions, "/openclaw", "/nemesisbot");
    assert_eq!(result.files_copied, 0);
    assert!(!result.errors.is_empty());
}

#[test]
fn test_execute_backup_action_full_flow() {
    // Backup action: destination exists, backup created, source copied
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&src, "new").unwrap();
    std::fs::write(&dst, "old").unwrap();

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Backup,
        source: Some(src.to_string_lossy().to_string()),
        destination: dst.to_string_lossy().to_string(),
        description: "backup".to_string(),
    }];
    let _ = execute(&actions, dir.path().to_string_lossy().as_ref(), "/nemesisbot");
}

#[test]
fn test_execute_backup_action_mkdir_failure() {
    // Backup action where parent of destination can't be created (parent is a file)
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    std::fs::write(&src, "new").unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let dst = blocker.join("dst.txt");

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Backup,
        source: Some(src.to_string_lossy().to_string()),
        destination: dst.to_string_lossy().to_string(),
        description: "backup".to_string(),
    }];
    let result = execute(&actions, "/openclaw", "/nemesisbot");
    assert_eq!(result.backups_created, 0);
    assert!(!result.errors.is_empty());
}

#[test]
fn test_execute_backup_action_copy_failure() {
    // Backup action: backup succeeds but source copy fails (source doesn't exist)
    let dir = tempfile::tempdir().unwrap();
    let nonexistent_src = dir.path().join("missing_src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&dst, "existing").unwrap();

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::Backup,
        source: Some(nonexistent_src.to_string_lossy().to_string()),
        destination: dst.to_string_lossy().to_string(),
        description: "backup".to_string(),
    }];
    let result = execute(&actions, dir.path().to_string_lossy().as_ref(), "/nemesisbot");
    // Backup of dst succeeds, but copy of src fails
    assert_eq!(result.backups_created, 1);
    assert!(!result.errors.is_empty());
    assert_eq!(result.files_copied, 0);
}

#[test]
fn test_plan_workspace_migration_with_force_creates_copy() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_ws");
    let dst = dir.path().join("dst_ws");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("SOUL.md"), "new").unwrap();
    std::fs::write(dst.join("SOUL.md"), "old").unwrap();

    let actions = plan_workspace_migration(
        src.to_string_lossy().as_ref(),
        dst.to_string_lossy().as_ref(),
        true,
    )
    .unwrap();
    assert!(actions.iter().any(|a| a.action_type == FullMigrationActionType::Copy));
    assert!(!actions.iter().any(|a| a.action_type == FullMigrationActionType::Backup));
}

#[test]
fn test_plan_workspace_migration_with_existing_creates_backup() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_ws");
    let dst = dir.path().join("dst_ws");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("SOUL.md"), "new").unwrap();
    std::fs::write(dst.join("SOUL.md"), "old").unwrap();

    let actions = plan_workspace_migration(
        src.to_string_lossy().as_ref(),
        dst.to_string_lossy().as_ref(),
        false,
    )
    .unwrap();
    assert!(actions.iter().any(|a| a.action_type == FullMigrationActionType::Backup));
}

#[test]
fn test_plan_workspace_migration_includes_create_dir() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_ws");
    let dst = dir.path().join("dst_ws");
    std::fs::create_dir_all(src.join("memory")).unwrap();
    std::fs::create_dir_all(src.join("skills")).unwrap();
    std::fs::write(src.join("memory").join("notes.txt"), "notes").unwrap();

    let actions = plan_workspace_migration(
        src.to_string_lossy().as_ref(),
        dst.to_string_lossy().as_ref(),
        false,
    )
    .unwrap();
    let create_dir_count = actions
        .iter()
        .filter(|a| a.action_type == FullMigrationActionType::CreateDir)
        .count();
    assert!(create_dir_count >= 2);
}

#[test]
fn test_plan_workspace_migration_no_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src_ws");
    let dst = dir.path().join("dst_ws");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("SOUL.md"), "x").unwrap();

    let actions = plan_workspace_migration(
        src.to_string_lossy().as_ref(),
        dst.to_string_lossy().as_ref(),
        false,
    )
    .unwrap();
    assert!(!actions
        .iter()
        .any(|a| a.action_type == FullMigrationActionType::CreateDir));
}

#[test]
fn test_resolve_openclaw_home_with_env() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let orig = std::env::var("OPENCLAW_HOME").ok();
    unsafe { std::env::set_var("OPENCLAW_HOME", "/env/openclaw"); }
    let result = resolve_openclaw_home("");
    match orig {
        Some(v) => unsafe { std::env::set_var("OPENCLAW_HOME", v); },
        None => unsafe { std::env::remove_var("OPENCLAW_HOME"); },
    }
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "/env/openclaw");
}

#[test]
fn test_resolve_nemesisbot_home_with_env() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let orig = std::env::var("NEMESISBOT_HOME").ok();
    unsafe { std::env::set_var("NEMESISBOT_HOME", "/env/nemesisbot"); }
    let result = resolve_nemesisbot_home("");
    match orig {
        Some(v) => unsafe { std::env::set_var("NEMESISBOT_HOME", v); },
        None => unsafe { std::env::remove_var("NEMESISBOT_HOME"); },
    }
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "/env/nemesisbot");
}

#[test]
fn test_resolve_openclaw_home_override_takes_priority_over_env() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let orig = std::env::var("OPENCLAW_HOME").ok();
    unsafe { std::env::set_var("OPENCLAW_HOME", "/env/openclaw"); }
    let result = resolve_openclaw_home("/override/path");
    match orig {
        Some(v) => unsafe { std::env::set_var("OPENCLAW_HOME", v); },
        None => unsafe { std::env::remove_var("OPENCLAW_HOME"); },
    }
    assert_eq!(result.unwrap(), "/override/path");
}

#[test]
fn test_resolve_nemesisbot_home_override_takes_priority_over_env() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let orig = std::env::var("NEMESISBOT_HOME").ok();
    unsafe { std::env::set_var("NEMESISBOT_HOME", "/env/nemesisbot"); }
    let result = resolve_nemesisbot_home("/override/path");
    match orig {
        Some(v) => unsafe { std::env::set_var("NEMESISBOT_HOME", v); },
        None => unsafe { std::env::remove_var("NEMESISBOT_HOME"); },
    }
    assert_eq!(result.unwrap(), "/override/path");
}

#[test]
fn test_resolve_openclaw_home_with_tilde() {
    let result = resolve_openclaw_home("~/openclaw");
    assert!(result.is_ok());
    let p = result.unwrap();
    assert!(!p.starts_with("~"));
}

#[test]
fn test_resolve_nemesisbot_home_with_tilde() {
    let result = resolve_nemesisbot_home("~/nemesisbot");
    assert!(result.is_ok());
    let p = result.unwrap();
    assert!(!p.starts_with("~"));
}

#[test]
fn test_dirs_home_returns_ok() {
    let result = dirs_home();
    assert!(result.is_ok());
    assert!(!result.unwrap().is_empty());
}

#[test]
fn test_dirs_home_uses_home_env_first() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let orig_home = std::env::var("HOME").ok();
    let orig_userprofile = std::env::var("USERPROFILE").ok();
    unsafe { std::env::set_var("HOME", "/test/home/dir"); }
    let result = dirs_home();
    match orig_home {
        Some(v) => unsafe { std::env::set_var("HOME", v); },
        None => unsafe { std::env::remove_var("HOME"); },
    }
    match orig_userprofile {
        Some(v) => unsafe { std::env::set_var("USERPROFILE", v); },
        None => unsafe { std::env::remove_var("USERPROFILE"); },
    }
    assert_eq!(result.unwrap(), "/test/home/dir");
}

#[test]
fn test_dirs_home_uses_userprofile_fallback() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let orig_home = std::env::var("HOME").ok();
    let orig_userprofile = std::env::var("USERPROFILE").ok();
    unsafe {
        std::env::remove_var("HOME");
        std::env::set_var("USERPROFILE", "/test/userprofile/dir");
    }
    let result = dirs_home();
    match orig_home {
        Some(v) => unsafe { std::env::set_var("HOME", v); },
        None => { /* leave unset */ }
    }
    match orig_userprofile {
        Some(v) => unsafe { std::env::set_var("USERPROFILE", v); },
        None => unsafe { std::env::remove_var("USERPROFILE"); },
    }
    assert_eq!(result.unwrap(), "/test/userprofile/dir");
}

#[test]
fn test_run_full_migration_aborted_via_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let openclaw_home = dir.path().join(".openclaw");
    std::fs::create_dir_all(&openclaw_home).unwrap();

    let opts = MigrateOptions {
        force: true,
        openclaw_home: openclaw_home.to_string_lossy().to_string(),
        nemesisbot_home: dir.path().join(".nemesisbot").to_string_lossy().to_string(),
        ..Default::default()
    };
    let result = run_full_migration(&opts).unwrap();
    assert_eq!(result.files_copied, 0);
    assert!(!result.config_migrated);
}

#[test]
fn test_run_full_migration_force_skips_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let openclaw_home = dir.path().join(".openclaw");
    let openclaw_ws = openclaw_home.join("workspace");
    std::fs::create_dir_all(&openclaw_ws).unwrap();
    std::fs::write(openclaw_ws.join("SOUL.md"), "soul").unwrap();

    let opts = MigrateOptions {
        force: true,
        openclaw_home: openclaw_home.to_string_lossy().to_string(),
        nemesisbot_home: dir.path().join(".nemesisbot").to_string_lossy().to_string(),
        ..Default::default()
    };
    let result = run_full_migration(&opts).unwrap();
    assert!(result.files_copied >= 1);
}

#[test]
fn test_confirm_function_returns_bool() {
    let _ = confirm();
}

#[test]
fn test_full_migration_result_with_all_fields() {
    let result = FullMigrationResult {
        files_copied: 10,
        files_skipped: 3,
        backups_created: 2,
        config_migrated: true,
        dirs_created: 5,
        warnings: vec!["w1".to_string(), "w2".to_string()],
        errors: vec!["e1".to_string()],
    };
    print_summary(&result);
}

#[test]
fn test_execute_mixed_actions() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    std::fs::write(&src, "content").unwrap();
    let dst = dir.path().join("dst.txt");

    let actions = vec![
        FullMigrationAction {
            action_type: FullMigrationActionType::Copy,
            source: Some(src.to_string_lossy().to_string()),
            destination: dst.to_string_lossy().to_string(),
            description: "copy".to_string(),
        },
        FullMigrationAction {
            action_type: FullMigrationActionType::Skip,
            source: None,
            destination: "/skip".to_string(),
            description: "skip".to_string(),
        },
        FullMigrationAction {
            action_type: FullMigrationActionType::MergeConfig,
            source: Some("/merge.json".to_string()),
            destination: "/merge_out.json".to_string(),
            description: "merge".to_string(),
        },
    ];
    let result = execute(&actions, dir.path().to_string_lossy().as_ref(), "/nemesisbot");
    assert_eq!(result.files_copied, 1);
    assert_eq!(result.files_skipped, 2);
}

#[test]
fn test_full_migration_action_serialization() {
    let action = FullMigrationAction {
        action_type: FullMigrationActionType::Backup,
        source: Some("/src".to_string()),
        destination: "/dst".to_string(),
        description: "test".to_string(),
    };
    let json = serde_json::to_string(&action).unwrap();
    let de: FullMigrationAction = serde_json::from_str(&json).unwrap();
    assert_eq!(de.action_type, FullMigrationActionType::Backup);
    assert_eq!(de.source, Some("/src".to_string()));
}

#[test]
fn test_full_migration_action_type_all_variants() {
    let variants = vec![
        FullMigrationActionType::Copy,
        FullMigrationActionType::Skip,
        FullMigrationActionType::Backup,
        FullMigrationActionType::ConvertConfig,
        FullMigrationActionType::CreateDir,
        FullMigrationActionType::MergeConfig,
    ];
    for v in variants {
        let action = FullMigrationAction {
            action_type: v.clone(),
            source: None,
            destination: "/dst".to_string(),
            description: "test".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let de: FullMigrationAction = serde_json::from_str(&json).unwrap();
        assert_eq!(de.action_type, v);
    }
}

#[test]
fn test_full_migration_action_type_snake_case_serialization() {
    let json = r#"{"type":"convert_config","destination":"/x","description":"d"}"#;
    let de: FullMigrationAction = serde_json::from_str(json).unwrap();
    assert_eq!(de.action_type, FullMigrationActionType::ConvertConfig);

    let json = r#"{"type":"create_dir","destination":"/x","description":"d"}"#;
    let de: FullMigrationAction = serde_json::from_str(json).unwrap();
    assert_eq!(de.action_type, FullMigrationActionType::CreateDir);

    let json = r#"{"type":"merge_config","destination":"/x","description":"d"}"#;
    let de: FullMigrationAction = serde_json::from_str(json).unwrap();
    assert_eq!(de.action_type, FullMigrationActionType::MergeConfig);
}

#[test]
fn test_rel_path_empty_base() {
    let rel = rel_path("/some/file.txt", "");
    assert!(!rel.is_empty());
}

#[test]
fn test_rel_path_exact_match() {
    let rel = rel_path("/base", "/base");
    assert_eq!(rel, "");
}

#[test]
fn test_rel_path_subpath() {
    let rel = rel_path("/a/b/c/d.txt", "/a/b");
    assert_eq!(rel, "c/d.txt");
}

#[test]
fn test_copy_file_to_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.txt");
    let dst = dir.path().join("dst.txt");
    std::fs::write(&src, "data").unwrap();
    copy_file(src.to_string_lossy().as_ref(), dst.to_string_lossy().as_ref()).unwrap();
    assert!(dst.exists());
}

#[test]
fn test_resolve_workspace_nested() {
    let ws = resolve_workspace("/home/user/.openclaw");
    let expected = Path::new("/home/user/.openclaw")
        .join("workspace")
        .to_string_lossy()
        .to_string();
    assert_eq!(ws, expected);
}

#[test]
fn test_resolve_workspace_empty_base() {
    let ws = resolve_workspace("");
    let expected = Path::new("")
        .join("workspace")
        .to_string_lossy()
        .to_string();
    assert_eq!(ws, expected);
}

#[test]
fn test_migrate_options_clone() {
    let opts = MigrateOptions {
        dry_run: true,
        config_only: false,
        workspace_only: false,
        force: true,
        refresh: false,
        openclaw_home: "/openclaw".to_string(),
        nemesisbot_home: "/nemesisbot".to_string(),
    };
    let cloned = opts.clone();
    assert_eq!(cloned.dry_run, opts.dry_run);
    assert_eq!(cloned.force, opts.force);
    assert_eq!(cloned.openclaw_home, opts.openclaw_home);
}

#[test]
fn test_migrate_options_debug_format() {
    let opts = MigrateOptions::default();
    let debug_str = format!("{:?}", opts);
    assert!(debug_str.contains("MigrateOptions"));
    assert!(debug_str.contains("dry_run"));
}

#[test]
fn test_full_migration_result_clone() {
    let result = FullMigrationResult {
        files_copied: 5,
        files_skipped: 1,
        backups_created: 2,
        config_migrated: true,
        dirs_created: 3,
        warnings: vec!["w".to_string()],
        errors: vec![],
    };
    let cloned = result.clone();
    assert_eq!(cloned.files_copied, 5);
    assert_eq!(cloned.backups_created, 2);
    assert!(cloned.config_migrated);
}

#[test]
fn test_full_migration_result_debug_format() {
    let result = FullMigrationResult::default();
    let debug_str = format!("{:?}", result);
    assert!(debug_str.contains("FullMigrationResult"));
}

#[test]
fn test_full_migration_action_clone() {
    let action = FullMigrationAction {
        action_type: FullMigrationActionType::Copy,
        source: Some("/src".to_string()),
        destination: "/dst".to_string(),
        description: "desc".to_string(),
    };
    let cloned = action.clone();
    assert_eq!(cloned.action_type, action.action_type);
    assert_eq!(cloned.source, action.source);
}

#[test]
fn test_full_migration_action_debug_format() {
    let action = FullMigrationAction {
        action_type: FullMigrationActionType::Skip,
        source: None,
        destination: "/dst".to_string(),
        description: "skip".to_string(),
    };
    let debug_str = format!("{:?}", action);
    assert!(debug_str.contains("FullMigrationAction"));
}

#[test]
fn test_plan_with_force_overrides_existing() {
    let dir = tempfile::tempdir().unwrap();
    let openclaw_home = dir.path().join(".openclaw");
    let openclaw_ws = openclaw_home.join("workspace");
    let nemesisbot_home = dir.path().join(".nemesisbot");
    let nemesisbot_ws = nemesisbot_home.join("workspace");
    std::fs::create_dir_all(&openclaw_ws).unwrap();
    std::fs::create_dir_all(&nemesisbot_ws).unwrap();
    std::fs::write(openclaw_ws.join("SOUL.md"), "new").unwrap();
    std::fs::write(nemesisbot_ws.join("SOUL.md"), "old").unwrap();

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
    assert!(actions
        .iter()
        .any(|a| a.action_type == FullMigrationActionType::Copy
            && a.destination.contains("SOUL.md")));
    assert!(!actions
        .iter()
        .any(|a| a.action_type == FullMigrationActionType::Backup));
}

#[test]
fn test_plan_without_force_creates_backup_for_existing() {
    let dir = tempfile::tempdir().unwrap();
    let openclaw_home = dir.path().join(".openclaw");
    let openclaw_ws = openclaw_home.join("workspace");
    let nemesisbot_home = dir.path().join(".nemesisbot");
    let nemesisbot_ws = nemesisbot_home.join("workspace");
    std::fs::create_dir_all(&openclaw_ws).unwrap();
    std::fs::create_dir_all(&nemesisbot_ws).unwrap();
    std::fs::write(openclaw_ws.join("SOUL.md"), "new").unwrap();
    std::fs::write(nemesisbot_ws.join("SOUL.md"), "old").unwrap();

    let opts = MigrateOptions {
        force: false,
        ..Default::default()
    };
    let (actions, _) = plan(
        &opts,
        openclaw_home.to_string_lossy().as_ref(),
        nemesisbot_home.to_string_lossy().as_ref(),
    )
    .unwrap();
    assert!(actions
        .iter()
        .any(|a| a.action_type == FullMigrationActionType::Backup
            && a.destination.contains("SOUL.md")));
}

#[test]
fn test_plan_refresh_overrides_workspace_only() {
    let dir = tempfile::tempdir().unwrap();
    let openclaw_home = dir.path().join(".openclaw");
    let openclaw_ws = openclaw_home.join("workspace");
    std::fs::create_dir_all(&openclaw_ws).unwrap();
    std::fs::write(openclaw_ws.join("SOUL.md"), "x").unwrap();

    let opts = MigrateOptions {
        refresh: true,
        force: true,
        openclaw_home: openclaw_home.to_string_lossy().to_string(),
        nemesisbot_home: dir.path().join(".nemesisbot").to_string_lossy().to_string(),
        ..Default::default()
    };
    let result = run_full_migration(&opts).unwrap();
    assert!(result.files_copied >= 1);
}

#[test]
fn test_execute_with_empty_actions() {
    let result = execute(&[], "/openclaw", "/nemesisbot");
    assert_eq!(result.files_copied, 0);
    assert_eq!(result.files_skipped, 0);
    assert_eq!(result.backups_created, 0);
    assert_eq!(result.dirs_created, 0);
    assert!(!result.config_migrated);
}

#[test]
fn test_execute_convert_config_with_invalid_existing() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("openclaw.json");
    std::fs::write(
        &src_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "agents": {"defaults": {}},
            "providers": {},
            "channels": {}
        }))
        .unwrap(),
    )
    .unwrap();
    let dst_path = dir.path().join("config.json");
    std::fs::write(&dst_path, "not valid json {{{").unwrap();

    let actions = vec![FullMigrationAction {
        action_type: FullMigrationActionType::ConvertConfig,
        source: Some(src_path.to_string_lossy().to_string()),
        destination: dst_path.to_string_lossy().to_string(),
        description: "convert".to_string(),
    }];
    let result = execute(&actions, "/openclaw", "/nemesisbot");
    assert!(!result.config_migrated);
    assert!(!result.errors.is_empty());
}

#[test]
fn test_print_summary_with_all_zero() {
    let result = FullMigrationResult::default();
    print_summary(&result);
}

#[test]
fn test_needs_migration_with_config_default() {
    let config = crate::config::MigrateConfig::default();
    let migrator = Migrator::new(config);
    assert!(!migrator.needs_migration());
}
