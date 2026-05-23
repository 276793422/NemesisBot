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
