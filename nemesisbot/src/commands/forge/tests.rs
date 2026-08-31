use super::*;

#[test]
fn test_default_forge_config() {
    let cfg = default_forge_config();
    assert_eq!(
        cfg.get("collect_interval_sec").and_then(|v| v.as_u64()),
        Some(300)
    );
    assert_eq!(
        cfg.get("reflect_interval_sec").and_then(|v| v.as_u64()),
        Some(3600)
    );
    assert_eq!(cfg.get("min_experiences").and_then(|v| v.as_u64()), Some(5));
    assert_eq!(
        cfg.get("learning_enabled").and_then(|v| v.as_bool()),
        Some(false)
    );
}

#[test]
fn test_load_forge_config_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = load_forge_config(tmp.path());
    assert_eq!(
        cfg.get("collect_interval_sec").and_then(|v| v.as_u64()),
        Some(300)
    );
}

#[test]
fn test_save_and_load_forge_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cfg = default_forge_config();
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert(
            "learning_enabled".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    save_forge_config(tmp.path(), &cfg).unwrap();
    let loaded = load_forge_config(tmp.path());
    assert_eq!(
        loaded.get("learning_enabled").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn test_load_registry_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let reg = load_registry(tmp.path());
    assert!(reg.is_empty());
}

#[test]
fn test_load_registry_with_data() {
    let tmp = tempfile::TempDir::new().unwrap();
    let registry_path = tmp.path().join("registry.json");
    std::fs::write(
        &registry_path,
        r#"[{"id":"test-1","type":"skill","name":"test","status":"draft"}]"#,
    )
    .unwrap();
    let reg = load_registry(tmp.path());
    assert_eq!(reg.len(), 1);
    assert_eq!(reg[0].get("id").and_then(|v| v.as_str()), Some("test-1"));
}

// -------------------------------------------------------------------------
// default_forge_config comprehensive tests
// -------------------------------------------------------------------------

#[test]
fn test_default_forge_config_collect_interval() {
    let cfg = default_forge_config();
    assert_eq!(
        cfg.get("collect_interval_sec").and_then(|v| v.as_u64()),
        Some(300)
    );
}

#[test]
fn test_default_forge_config_reflect_interval() {
    let cfg = default_forge_config();
    assert_eq!(
        cfg.get("reflect_interval_sec").and_then(|v| v.as_u64()),
        Some(3600)
    );
}

#[test]
fn test_default_forge_config_min_experiences() {
    let cfg = default_forge_config();
    assert_eq!(cfg.get("min_experiences").and_then(|v| v.as_u64()), Some(5));
}

#[test]
fn test_default_forge_config_llm_semantic_analysis() {
    let cfg = default_forge_config();
    assert_eq!(
        cfg.get("llm_semantic_analysis").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn test_default_forge_config_default_artifact_status() {
    let cfg = default_forge_config();
    assert_eq!(
        cfg.get("default_artifact_status").and_then(|v| v.as_str()),
        Some("draft")
    );
}

#[test]
fn test_default_forge_config_trace_collection() {
    let cfg = default_forge_config();
    assert_eq!(
        cfg.get("trace_collection").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn test_default_forge_config_learning_enabled_false() {
    let cfg = default_forge_config();
    assert_eq!(
        cfg.get("learning_enabled").and_then(|v| v.as_bool()),
        Some(false)
    );
}

#[test]
fn test_default_forge_config_learning_subsection() {
    let cfg = default_forge_config();
    let learning = cfg.get("learning").unwrap();
    assert_eq!(
        learning
            .get("min_pattern_frequency")
            .and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(
        learning
            .get("high_confidence_threshold")
            .and_then(|v| v.as_f64()),
        Some(0.8)
    );
    assert_eq!(
        learning.get("max_auto_creates").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(
        learning.get("max_refine_rounds").and_then(|v| v.as_u64()),
        Some(3)
    );
    assert_eq!(
        learning.get("min_outcome_samples").and_then(|v| v.as_u64()),
        Some(5)
    );
    assert_eq!(
        learning.get("monitor_window_days").and_then(|v| v.as_u64()),
        Some(7)
    );
    assert_eq!(
        learning.get("degrade_threshold").and_then(|v| v.as_f64()),
        Some(-0.2)
    );
    assert_eq!(
        learning
            .get("degrade_cooldown_days")
            .and_then(|v| v.as_u64()),
        Some(7)
    );
    assert_eq!(
        learning.get("llm_budget_tokens").and_then(|v| v.as_u64()),
        Some(8000)
    );
}

// -------------------------------------------------------------------------
// load_forge_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_load_forge_config_invalid_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    // Write invalid JSON
    std::fs::write(forge_dir.join("forge.json"), "not valid json {{{").unwrap();
    let cfg = load_forge_config(&forge_dir);
    // Should fall back to defaults
    assert_eq!(
        cfg.get("collect_interval_sec").and_then(|v| v.as_u64()),
        Some(300)
    );
}

// -------------------------------------------------------------------------
// save_forge_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_save_forge_config_creates_directory() {
    let tmp = tempfile::TempDir::new().unwrap();
    let new_dir = tmp.path().join("new_forge_dir");
    assert!(!new_dir.exists());
    save_forge_config(&new_dir, &default_forge_config()).unwrap();
    assert!(new_dir.exists());
    assert!(new_dir.join("forge.json").exists());
}

#[test]
fn test_save_forge_config_overwrites() {
    let tmp = tempfile::TempDir::new().unwrap();
    save_forge_config(tmp.path(), &default_forge_config()).unwrap();

    let mut custom = default_forge_config();
    if let Some(obj) = custom.as_object_mut() {
        obj.insert(
            "collect_interval_sec".to_string(),
            serde_json::Value::Number(600.into()),
        );
    }
    save_forge_config(tmp.path(), &custom).unwrap();

    let loaded = load_forge_config(tmp.path());
    assert_eq!(
        loaded.get("collect_interval_sec").and_then(|v| v.as_u64()),
        Some(600)
    );
}

// -------------------------------------------------------------------------
// load_registry edge cases
// -------------------------------------------------------------------------

#[test]
fn test_load_registry_invalid_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let registry_path = tmp.path().join("registry.json");
    std::fs::write(&registry_path, "invalid json").unwrap();
    let reg = load_registry(tmp.path());
    assert!(reg.is_empty());
}

#[test]
fn test_load_registry_empty_array() {
    let tmp = tempfile::TempDir::new().unwrap();
    let registry_path = tmp.path().join("registry.json");
    std::fs::write(&registry_path, "[]").unwrap();
    let reg = load_registry(tmp.path());
    assert!(reg.is_empty());
}

#[test]
fn test_load_registry_multiple_artifacts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let registry_path = tmp.path().join("registry.json");
    let data = serde_json::json!([
        {"id": "a1", "type": "skill", "name": "skill1", "status": "active", "version": "1.0"},
        {"id": "a2", "type": "script", "name": "script1", "status": "draft", "version": "0.1"},
        {"id": "a3", "type": "mcp", "name": "mcp1", "status": "active", "version": "2.0"}
    ]);
    std::fs::write(&registry_path, serde_json::to_string(&data).unwrap()).unwrap();
    let reg = load_registry(tmp.path());
    assert_eq!(reg.len(), 3);
    assert_eq!(reg[0].get("type").and_then(|v| v.as_str()), Some("skill"));
    assert_eq!(reg[1].get("type").and_then(|v| v.as_str()), Some("script"));
    assert_eq!(reg[2].get("type").and_then(|v| v.as_str()), Some("mcp"));
}

// -------------------------------------------------------------------------
// cmd_status (requires config file)
// -------------------------------------------------------------------------

#[test]
fn test_cmd_status_no_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    // Don't create config file, should report disabled
    cmd_status(&home, &cfg_path, &forge_dir).unwrap();
}

#[test]
fn test_cmd_status_with_forge_enabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({"forge": {"enabled": true}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    cmd_status(&home, &cfg_path, &forge_dir).unwrap();
}

#[test]
fn test_cmd_status_with_forge_disabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({"forge": {"enabled": false}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    cmd_status(&home, &cfg_path, &forge_dir).unwrap();
}

// -------------------------------------------------------------------------
// cmd_list tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_list_empty_registry() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    cmd_list(&forge_dir, "all").unwrap();
}

#[test]
fn test_cmd_list_with_registry_artifacts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let registry = serde_json::json!([
        {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"},
        {"id": "a2", "type": "script", "name": "Test Script", "version": "0.5", "status": "draft"}
    ]);
    std::fs::write(
        forge_dir.join("registry.json"),
        serde_json::to_string(&registry).unwrap(),
    )
    .unwrap();
    cmd_list(&forge_dir, "all").unwrap();
}

#[test]
fn test_cmd_list_filter_by_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let registry = serde_json::json!([
        {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"},
        {"id": "a2", "type": "script", "name": "Test Script", "version": "0.5", "status": "draft"}
    ]);
    std::fs::write(
        forge_dir.join("registry.json"),
        serde_json::to_string(&registry).unwrap(),
    )
    .unwrap();
    // Filter by type "skill" - should only show skill artifacts
    cmd_list(&forge_dir, "skill").unwrap();
}

#[test]
fn test_cmd_list_filter_no_match() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let registry = serde_json::json!([
        {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"}
    ]);
    std::fs::write(
        forge_dir.join("registry.json"),
        serde_json::to_string(&registry).unwrap(),
    )
    .unwrap();
    // Filter by non-existent type
    cmd_list(&forge_dir, "nonexistent").unwrap();
}

// -------------------------------------------------------------------------
// cmd_evaluate tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_evaluate_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let registry = serde_json::json!([
        {"id": "art-1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active", "score": 0.95, "usage_count": 42}
    ]);
    std::fs::write(
        forge_dir.join("registry.json"),
        serde_json::to_string(&registry).unwrap(),
    )
    .unwrap();
    cmd_evaluate(&forge_dir, "art-1").unwrap();
}

#[test]
fn test_cmd_evaluate_not_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let registry = serde_json::json!([
        {"id": "art-1", "type": "skill", "name": "Test Skill"}
    ]);
    std::fs::write(
        forge_dir.join("registry.json"),
        serde_json::to_string(&registry).unwrap(),
    )
    .unwrap();
    cmd_evaluate(&forge_dir, "nonexistent-id").unwrap();
}

// -------------------------------------------------------------------------
// cmd_learning_status tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_learning_status_defaults() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    cmd_learning_status(&forge_dir).unwrap();
}

#[test]
fn test_cmd_learning_status_custom_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let mut cfg = default_forge_config();
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert(
            "learning_enabled".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    save_forge_config(&forge_dir, &cfg).unwrap();
    cmd_learning_status(&forge_dir).unwrap();
}

// -------------------------------------------------------------------------
// cmd_enable tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_enable_creates_directories() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    cmd_enable(&cfg_path, &forge_dir).unwrap();

    // Check all 7 + prompts directories were created
    for d in &[
        "experiences",
        "reflections",
        "skills",
        "scripts",
        "mcp",
        "traces",
        "learning",
    ] {
        assert!(forge_dir.join(d).exists(), "Directory '{}' should exist", d);
    }
    assert!(forge_dir.join("prompts").exists());
    assert!(forge_dir.join("forge.json").exists());
    assert!(forge_dir.join("registry.json").exists());
}

#[test]
fn test_cmd_enable_preserves_existing_forge_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({"forge": {"some_field": "preserved"}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    cmd_enable(&cfg_path, &forge_dir).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(
        loaded
            .get("forge")
            .and_then(|f| f.get("enabled"))
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        loaded
            .get("forge")
            .and_then(|f| f.get("some_field"))
            .and_then(|v| v.as_str()),
        Some("preserved")
    );
}

// -------------------------------------------------------------------------
// cmd_disable tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_disable_sets_false() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({"forge": {"enabled": true, "some_field": "kept"}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    cmd_disable(&cfg_path).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(
        loaded
            .get("forge")
            .and_then(|f| f.get("enabled"))
            .and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        loaded
            .get("forge")
            .and_then(|f| f.get("some_field"))
            .and_then(|v| v.as_str()),
        Some("kept")
    );
}

// -------------------------------------------------------------------------
// cmd_learning_enable / cmd_learning_disable tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_learning_enable() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    save_forge_config(&forge_dir, &default_forge_config()).unwrap();

    cmd_learning_enable(&forge_dir).unwrap();

    let loaded = load_forge_config(&forge_dir);
    assert_eq!(
        loaded.get("learning_enabled").and_then(|v| v.as_bool()),
        Some(true)
    );
    // Should also auto-enable trace collection
    assert_eq!(
        loaded.get("trace_collection").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn test_cmd_learning_disable() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let mut cfg = default_forge_config();
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert(
            "learning_enabled".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    save_forge_config(&forge_dir, &cfg).unwrap();

    cmd_learning_disable(&forge_dir).unwrap();

    let loaded = load_forge_config(&forge_dir);
    assert_eq!(
        loaded.get("learning_enabled").and_then(|v| v.as_bool()),
        Some(false)
    );
}

// -------------------------------------------------------------------------
// cmd_learning_history tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_learning_history_no_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    cmd_learning_history(&forge_dir, 10).unwrap();
}

#[test]
fn test_cmd_learning_history_with_entries() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    let learning_dir = forge_dir.join("learning");
    std::fs::create_dir_all(&learning_dir).unwrap();
    let entries = serde_json::json!([
        {"timestamp": "2026-01-01T00:00:00Z", "patterns_found": 5, "actions_generated": 3, "actions_deployed": 2},
        {"timestamp": "2026-01-02T00:00:00Z", "patterns_found": 8, "actions_generated": 6, "actions_deployed": 4}
    ]);
    let jsonl: String = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(learning_dir.join("learning_cycles.jsonl"), jsonl).unwrap();

    cmd_learning_history(&forge_dir, 10).unwrap();
}

// -------------------------------------------------------------------------
// cmd_reflect edge cases (non-runtime parts)
// -------------------------------------------------------------------------

#[test]
fn test_cmd_reflect_forge_not_enabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({"forge": {"enabled": false}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    // Should print "not enabled" and return Ok
    // This doesn't need tokio runtime since it returns early
    cmd_reflect(&cfg_path, &forge_dir).unwrap();
}

#[test]
fn test_cmd_reflect_forge_dir_not_exists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({"forge": {"enabled": true}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    // forge_dir doesn't exist - should print error and return Ok
    cmd_reflect(&cfg_path, &forge_dir).unwrap();
}

// -------------------------------------------------------------------------
// Additional forge tests for coverage
// -------------------------------------------------------------------------

#[test]
fn test_cmd_enable_no_existing_config_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    // Don't create config file - cmd_enable only writes if config exists
    std::fs::create_dir_all(&home).unwrap();

    cmd_enable(&cfg_path, &forge_dir).unwrap();

    // Directories should still be created
    assert!(forge_dir.join("experiences").exists());
    assert!(forge_dir.join("forge.json").exists());
    assert!(forge_dir.join("registry.json").exists());
}

#[test]
fn test_cmd_disable_no_config_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    // Don't create config file - should be a no-op
    cmd_disable(&cfg_path).unwrap();
}

#[test]
fn test_cmd_status_with_forge_directories() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({"forge": {"enabled": true}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    // Create some forge directories with content
    std::fs::create_dir_all(forge_dir.join("experiences")).unwrap();
    std::fs::write(forge_dir.join("experiences").join("exp1.json"), "{}").unwrap();
    std::fs::create_dir_all(forge_dir.join("reflections")).unwrap();

    cmd_status(&home, &cfg_path, &forge_dir).unwrap();
}

#[test]
fn test_cmd_status_with_registry_artifacts_and_types() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    let forge_dir = home.join("workspace").join("forge");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&forge_dir).unwrap();
    let cfg = serde_json::json!({"forge": {"enabled": true}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    // Create registry with various types and statuses
    let registry = serde_json::json!([
        {"id": "a1", "type": "skill", "status": "active"},
        {"id": "a2", "type": "skill", "status": "draft"},
        {"id": "a3", "type": "script", "status": "active"},
        {"id": "a4", "type": "mcp", "status": "deprecated"}
    ]);
    std::fs::write(
        forge_dir.join("registry.json"),
        serde_json::to_string(&registry).unwrap(),
    )
    .unwrap();

    cmd_status(&home, &cfg_path, &forge_dir).unwrap();
}

#[test]
fn test_cmd_evaluate_with_score_and_usage() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    let registry = serde_json::json!([
        {"id": "eval-1", "type": "skill", "name": "Scored Skill", "version": "2.0", "status": "active", "score": 0.85, "usage_count": 150}
    ]);
    std::fs::write(
        forge_dir.join("registry.json"),
        serde_json::to_string(&registry).unwrap(),
    )
    .unwrap();
    cmd_evaluate(&forge_dir, "eval-1").unwrap();
}

#[test]
fn test_cmd_list_fallback_directory_scan() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    // Don't create registry.json - should fall back to directory scan
    std::fs::create_dir_all(forge_dir.join("skills")).unwrap();
    std::fs::write(forge_dir.join("skills").join("skill1.json"), "{}").unwrap();
    std::fs::create_dir_all(forge_dir.join("scripts")).unwrap();
    // scripts dir is empty

    cmd_list(&forge_dir, "all").unwrap();
}

#[test]
fn test_cmd_list_fallback_specific_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    // No registry, scan specific type directory
    std::fs::create_dir_all(forge_dir.join("mcp")).unwrap();
    std::fs::write(forge_dir.join("mcp").join("server1.json"), "{}").unwrap();

    cmd_list(&forge_dir, "mcp").unwrap();
}

#[test]
fn test_cmd_learning_history_with_limit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    let learning_dir = forge_dir.join("learning");
    std::fs::create_dir_all(&learning_dir).unwrap();

    // Create 5 entries, limit to 2
    let entries: Vec<String> = (0..5)
        .map(|i| {
            serde_json::json!({
                "timestamp": format!("2026-01-0{}T00:00:00Z", i + 1),
                "patterns_found": i,
                "actions_generated": i * 2,
                "actions_deployed": i
            })
            .to_string()
        })
        .collect();
    std::fs::write(
        learning_dir.join("learning_cycles.jsonl"),
        entries.join("\n"),
    )
    .unwrap();

    cmd_learning_history(&forge_dir, 2).unwrap();
}

#[test]
fn test_cmd_learning_history_invalid_jsonl_line() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    let learning_dir = forge_dir.join("learning");
    std::fs::create_dir_all(&learning_dir).unwrap();

    // Mix of valid and invalid JSON lines
    let jsonl = r#"{"timestamp":"2026-01-01","patterns_found":1,"actions_generated":1,"actions_deployed":1}
invalid json line
{"timestamp":"2026-01-02","patterns_found":2,"actions_generated":2,"actions_deployed":2}"#;
    std::fs::write(learning_dir.join("learning_cycles.jsonl"), jsonl).unwrap();

    cmd_learning_history(&forge_dir, 10).unwrap();
}

#[test]
fn test_learning_enable_creates_directories() {
    let tmp = tempfile::TempDir::new().unwrap();
    let forge_dir = tmp.path().join("forge");
    std::fs::create_dir_all(&forge_dir).unwrap();
    save_forge_config(&forge_dir, &default_forge_config()).unwrap();

    cmd_learning_enable(&forge_dir).unwrap();

    assert!(forge_dir.join("learning").exists());
    assert!(forge_dir.join("traces").exists());
}

#[test]
fn test_forge_config_round_trip_preserves_custom_values() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cfg = default_forge_config();
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert(
            "custom_field".to_string(),
            serde_json::json!("custom_value"),
        );
        obj.insert("collect_interval_sec".to_string(), serde_json::json!(600));
    }
    save_forge_config(tmp.path(), &cfg).unwrap();

    let loaded = load_forge_config(tmp.path());
    assert_eq!(loaded["custom_field"], "custom_value");
    assert_eq!(loaded["collect_interval_sec"], 600);
}

// ===========================================================================
// R7（coverage-95 goal，2026-08-27）：cmd_* 命令体 + run() dispatch 全链路。
// 全部经 env home（NEMESISBOT_HOME → tempdir）走 run()，覆盖 dispatch
// （909-935）与各命令的成功/错误臂；cmd_reflect/cmd_export 内部用
// block_in_place，必须显式 multi_thread flavor（plain #[tokio::test] 是
// current_thread，block_in_place 直接 panic "can call blocking only when
// running on the multi-threaded runtime"，tokio 1.52.1 探针实证）。
// ===========================================================================

mod r7_cmd_paths {
    use super::*;

    fn env_home() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        (tmp, home)
    }

    fn clear_env() {
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    fn write_config(home: &std::path::Path, forge_enabled: Option<bool>) {
        let mut cfg = serde_json::json!({"version": "1.0", "default_model": "m"});
        if let Some(b) = forge_enabled {
            cfg["forge"] = serde_json::json!({"enabled": b, "collect_interval_sec": 60});
        }
        std::fs::write(
            home.join("config.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        )
        .unwrap();
    }

    fn seed_experiences(forge_dir: &std::path::Path) {
        let dir = forge_dir.join("experiences").join("202608");
        std::fs::create_dir_all(&dir).unwrap();
        let rows = [serde_json::json!({
                "pattern_hash": "r7hash1", "tool_name": "read_file", "count": 10,
                "avg_duration_ms": 12, "success_rate": 0.9,
                "last_seen": "2026-08-27T00:00:00+08:00"
            }),
            serde_json::json!({
                "pattern_hash": "r7hash2", "tool_name": "exec", "count": 4,
                "avg_duration_ms": 340, "success_rate": 0.25,
                "last_seen": "2026-08-27T01:00:00+08:00"
            })];
        let body: String = rows
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("20260827.jsonl"), body + "\n").unwrap();
    }

    fn seed_registry(forge_dir: &std::path::Path) {
        std::fs::create_dir_all(forge_dir).unwrap();
        // status 必须 PascalCase：ArtifactStatus 无 rename_all，serde 反序列化
        // 只认 `Draft`/`Active`/…（首版小写 "active" 被 registry.load 拒收）。
        // Artifact 无 serde(default)：id/name/kind/version/status/content/
        // tool_signature/created_at/updated_at/usage_count/last_degraded_at/
        // consecutive_observing_rounds 全必填（success_rate 除外）。
        let artifacts = serde_json::json!([
            {"id": "r7a1", "type": "tool", "name": "读文件优化", "kind": "Skill",
             "version": "1", "status": "Active", "content": "# skill",
             "tool_signature": [], "created_at": "2026-08-27T00:00:00Z",
             "updated_at": "2026-08-27T00:00:00Z", "usage_count": 3,
             "success_rate": 0.9, "last_degraded_at": null,
             "consecutive_observing_rounds": 0},
            {"id": "r7a2", "type": "skill", "name": "技能甲", "kind": "Skill",
             "version": "2", "status": "Draft", "content": "# draft",
             "tool_signature": [], "created_at": "2026-08-27T00:00:00Z",
             "updated_at": "2026-08-27T00:00:00Z", "usage_count": 1,
             "success_rate": 0.0, "last_degraded_at": null,
             "consecutive_observing_rounds": 0}
        ]);
        std::fs::write(
            forge_dir.join("registry.json"),
            serde_json::to_string(&artifacts).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn run_status_without_config_prints_disabled() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (_tmp, home) = env_home();
        let r = run(ForgeAction::Status, false);
        clear_env();
        r.expect("status on fresh home ok");
        assert!(!home.join("config.json").exists(), "status must not create config");
    }

    #[test]
    fn run_status_enabled_with_registry_prints_type_counts() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (_tmp, home) = env_home();
        write_config(&home, Some(true));
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(forge_dir.join("experiences")).unwrap();
        seed_registry(&forge_dir);
        // forge.json 存在 → “Config: <path>” 臂；experiences 目录有 1 个
        // 非目录 entry（202608 子目录）→ count>0。
        std::fs::write(
            forge_dir.join("forge.json"),
            serde_json::to_string(&default_forge_config()).unwrap(),
        )
        .unwrap();
        let r = run(ForgeAction::Status, false);
        clear_env();
        r.expect("status with registry ok");
    }

    #[test]
    fn run_enable_creates_dirs_and_config_then_disable_flips_flag() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (_tmp, home) = env_home();
        // 无 forge 键 → 插入 {"forge":{"enabled":true}} 臂。
        write_config(&home, None);
        run(ForgeAction::Enable, false).expect("enable ok");
        let forge_dir = home.join("workspace").join("forge");
        for d in ["experiences", "reflections", "skills", "scripts", "mcp", "traces", "learning", "prompts"] {
            assert!(forge_dir.join(d).is_dir(), "enable must create {d}");
        }
        assert!(forge_dir.join("forge.json").exists());
        assert!(forge_dir.join("registry.json").exists());
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["forge"]["enabled"], true);

        // 已有 forge 对象 → 只改 enabled=false。注意：本测试的 config 是
        // Enable 时从「无 forge 键」臂新建的（只含 enabled），sibling 保留
        // 语义由下一个测试（已有 forge 对象）钉住。
        run(ForgeAction::Disable, false).expect("disable ok");
        let cfg2: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg2["forge"]["enabled"], false);
        clear_env();
    }

    #[test]
    fn run_enable_on_existing_forge_object_preserves_fields() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (_tmp, home) = env_home();
        write_config(&home, Some(false));
        run(ForgeAction::Enable, false).expect("enable with existing forge obj ok");
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["forge"]["enabled"], true);
        assert_eq!(cfg["forge"]["collect_interval_sec"], 60);

        // disable 同样保留 sibling 字段（337-343 臂）。
        run(ForgeAction::Disable, false).expect("disable with existing forge obj ok");
        let cfg2: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg2["forge"]["enabled"], false);
        assert_eq!(
            cfg2["forge"]["collect_interval_sec"], 60,
            "disable must preserve sibling fields"
        );
        clear_env();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_reflect_guard_arms_disabled_and_uninitialized_and_empty() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        // 臂 1：config 存在但 forge.enabled=false → 提示先 enable。
        let (_t1, h1) = env_home();
        write_config(&h1, Some(false));
        let r1 = run(ForgeAction::Reflect, false);
        clear_env();

        // 臂 2：enabled=true 但 forge 目录不存在。
        let (_t2, h2) = env_home();
        write_config(&h2, Some(true));
        let r2 = run(ForgeAction::Reflect, false);
        clear_env();

        // 臂 3：目录在但 experiences 空（0 文件）。
        let (_t3, h3) = env_home();
        write_config(&h3, Some(true));
        let forge_dir = h3.join("workspace").join("forge");
        std::fs::create_dir_all(forge_dir.join("experiences")).unwrap();
        let r3 = run(ForgeAction::Reflect, false);
        clear_env();

        r1.expect("reflect disabled-guard returns Ok");
        r2.expect("reflect uninitialized-guard returns Ok");
        r3.expect("reflect empty-experiences returns Ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_reflect_full_path_produces_report() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (_tmp, home) = env_home();
        write_config(&home, Some(true));
        let forge_dir = home.join("workspace").join("forge");
        seed_experiences(&forge_dir);
        let r = run(ForgeAction::Reflect, false);
        clear_env();
        r.expect("reflect with seeded experiences ok");
        // 报告写入 reflections 目录（write_report 成功臂）。
        let reflect_dir = forge_dir.join("reflections");
        let has_report = std::fs::read_dir(&reflect_dir)
            .map(|rd| rd.filter_map(|e| e.ok()).count() > 0)
            .unwrap_or(false);
        assert!(has_report, "reflection report must be written to {:?}", reflect_dir);
    }

    #[test]
    fn run_list_registry_filter_and_fallback_scan_arms() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (_tmp, home) = env_home();
        let forge_dir = home.join("workspace").join("forge");
        seed_registry(&forge_dir);
        // registry 非空 + all → 明细打印臂。
        let r1 = run(ForgeAction::List { r#type: "all".into() }, false);
        // registry 非空 + 类型过滤命中。
        let r2 = run(ForgeAction::List { r#type: "skill".into() }, false);
        // registry 非空 + 类型过滤无命中 → “(no artifacts matching type)” 臂。
        let r3 = run(ForgeAction::List { r#type: "mcp".into() }, false);
        clear_env();
        r1.expect("list all ok");
        r2.expect("list skill ok");
        r3.expect("list no-match type ok");

        // registry 为空 → 目录扫描 fallback 臂。
        let (_t2, h2) = env_home();
        let fd2 = h2.join("workspace").join("forge");
        std::fs::create_dir_all(fd2.join("skills")).unwrap();
        std::fs::write(fd2.join("skills").join("s.md"), "x").unwrap();
        let r4 = run(ForgeAction::List { r#type: "all".into() }, false);
        let r5 = run(ForgeAction::List { r#type: "scripts".into() }, false);
        clear_env();
        r4.expect("list fallback scan all ok");
        r5.expect("list fallback scan single missing dir ok");
    }

    #[test]
    fn run_evaluate_found_and_not_found() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (_tmp, home) = env_home();
        let forge_dir = home.join("workspace").join("forge");
        seed_registry(&forge_dir);
        let ok = run(ForgeAction::Evaluate { id: "r7a1".into() }, false);
        let missing = run(ForgeAction::Evaluate { id: "zzz".into() }, false);
        clear_env();
        ok.expect("evaluate found ok");
        missing.expect("evaluate not-found still Ok (prints message)");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_export_guard_and_specific_artifact() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        // 守卫臂：forge 目录不存在。
        let (_t1, _h1) = env_home();
        let r1 = run(
            ForgeAction::Export { id: None, output: None, all: false },
            false,
        );
        clear_env();

        // 空 registry 臂。
        let (_t2, h2) = env_home();
        let fd2 = h2.join("workspace").join("forge");
        std::fs::create_dir_all(&fd2).unwrap();
        std::fs::write(fd2.join("registry.json"), "[]").unwrap();
        let r2 = run(
            ForgeAction::Export { id: None, output: None, all: false },
            false,
        );
        clear_env();

        // 指定 id 找不到 → 提示 not found。
        let (_t3, h3) = env_home();
        let fd3 = h3.join("workspace").join("forge");
        seed_registry(&fd3);
        let r3 = run(
            ForgeAction::Export { id: Some("zzz".into()), output: None, all: false },
            false,
        );
        clear_env();

        // 指定 id 命中 → 真导出到 exports/。
        let (_t4, h4) = env_home();
        let fd4 = h4.join("workspace").join("forge");
        seed_registry(&fd4);
        let r4 = run(
            ForgeAction::Export { id: Some("r7a1".into()), output: None, all: false },
            false,
        );
        clear_env();

        r1.expect("export uninitialized returns Ok");
        r2.expect("export empty registry returns Ok");
        r3.expect("export unknown id returns Ok");
        r4.expect("export specific artifact ok");
    }

    #[test]
    fn run_learning_lifecycle_and_history() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (_tmp, home) = env_home();
        let forge_dir = home.join("workspace").join("forge");
        // status：默认配置（无 forge.json）→ 学习配置打印臂走 unwrap_or 默认。
        run(ForgeAction::Learning { action: None }, false)
            .expect("learning status (bare) ok");
        run(ForgeAction::Learning { action: Some(LearningAction::Status) }, false)
            .expect("learning status ok");

        // enable → forge.json 落盘 learning_enabled=true + 目录创建。
        run(ForgeAction::Learning { action: Some(LearningAction::Enable) }, false)
            .expect("learning enable ok");
        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(forge_dir.join("forge.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["learning_enabled"], true);
        assert_eq!(cfg["trace_collection"], true, "auto-enable trace collection");
        assert!(forge_dir.join("learning").is_dir());

        // history：无 cycles 文件 → “No learning history”。
        run(ForgeAction::Learning { action: Some(LearningAction::History { limit: 5 }) }, false)
            .expect("learning history (no file) ok");

        // 播种 cycles（好行 + 坏行 + 空行）→ 解析打印臂。
        let cycles = "{\"timestamp\":\"2026-08-27T01:00:00+08:00\",\"patterns_found\":2,\"actions_generated\":1,\"actions_deployed\":1}\n\nnot-json-line\n";
        std::fs::create_dir_all(forge_dir.join("learning")).unwrap();
        std::fs::write(forge_dir.join("learning").join("learning_cycles.jsonl"), cycles).unwrap();
        run(ForgeAction::Learning { action: Some(LearningAction::History { limit: 10 }) }, false)
            .expect("learning history with cycles ok");

        // disable → learning_enabled=false。
        run(ForgeAction::Learning { action: Some(LearningAction::Disable) }, false)
            .expect("learning disable ok");
        let cfg2: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(forge_dir.join("forge.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg2["learning_enabled"], false);
        clear_env();
    }
}

// ===========================================================================
// wave_b（coverage 补测，2026-08-27）：miss 行清零补洞。与 r7_cmd_paths 平级，
// 看不到其私有 helper，下列 env/config/registry 工具为本模块复制版（语义一致
// 并加 prev-value 恢复）。涉及 cmd_reflect/cmd_export（内部 block_in_place）
// 的用例一律 multi_thread flavor，理由见上方 r7 头注释（current_thread 直接
// panic）。
//
// 目标行（forge.rs）与本模块的对应关系：
//  - 106-110 save_forge_config 的 fs::write ?——把 forge.json 预建成【目录】
//    （load_forge_config 读目录失败静默回默认值，save 时 write 打不开目录 =>
//    Err 冒泡）→ wave_b_learning_enable_fails_when_forge_json_unwritable；
//  - 341-343 cmd_disable 的「config 无 forge 键」插入臂（r7 两测都走已有的
//    forge 对象臂）→ wave_b_disable_inserts_forge_key_when_missing；
//  - 384-386 experiences 子目录缺席 => exp_count=0 早退提示 →
//    wave_b_reflect_without_experiences_subdir_returns_hint；
//  - 434-437 read_aggregated Err 臂——月目录下放名为 x.jsonl 的【目录】逼
//    walk 的 read_to_string 失败传播 →
//    wave_b_reflect_aggregate_read_error_on_jsonl_as_directory；
//  - 440-443 Ok 但空聚合早退臂——月目录存在且无任何 .jsonl →
//    wave_b_reflect_empty_aggregation_stops_early；
//  - 459-461 write_report Err 警告臂——把 reflections 预建成普通【文件】逼
//    create_dir_all 失败（Reflector 继续打印统计，仅告警）→
//    wave_b_reflect_warns_when_report_target_is_a_file；
//  - 489-500 low_success 打印臂——threshold 反映的是【记录条数】口径：
//    Reflector 按 experience 条目计数（reflector.rs:582-583 entry.0 +=
// 1/记录）而非聚合行的 count 字段，故需 3 条独立记录全败才触发（这解释了
//    r7 两条聚合行不达标为何此块漏测）→
//    wave_b_reflect_reports_low_success_patterns；
//  - 695-718 导出全量双态：Draft-only => count=0 + 「--all」提示（706-710）；
//    Active => count>0 + Exported 行（711-717）；all:false/all:true 两种头
//    都打一遍（695-699）→ wave_b_export_draft_only_hints_all_flag /
//    wave_b_export_active_artifacts_report_count；
//  - 794-801 learning 明细块关闭沿——forge.json 无 learning 键整块跳过 →
//    wave_b_learning_status_skips_details_without_learning_key；
//  - 869-871 history 文件存在但只有空白行 => 内层 No-history 臂 →
//    wave_b_history_blank_cycles_file_prints_no_history；
//  - 867/898 cycles 路径是【目录】 => read_to_string Err 被外层 if-let 静默
//    吞掉返回 Ok → wave_b_history_cycles_as_directory_skips_silently。
//
// ALREADY（既有测试名证据，不重复覆盖）：95/97（load_forge_config 合法解析
// 返回体）＝run_learning_lifecycle_and_history 反复读合法 forge.json；
// 118-123（load_registry 合法返回体）＝run_list_registry_filter_and_
// fallback_scan_arms / run_evaluate_found_and_not_found；294-295/316/322
// （enable 的写盘 ? 与 forge.json/registry.json 首建 ?）＝run_enable_creates_
// dirs_and_config_then_disable_flips_flag + run_enable_on_existing_forge_
// object_preserves_fields；347-348（disable 写盘尾部）＝同上两例；368-370
// （reflect 守卫直落沿）＝run_reflect_full_path_produces_report；487
// （top_patterns 块尾沿）＝同上报告产出自证。
//
// EXEMPT：本文件全部命令均为本地 FS/打印路径，无 probe/下载/freshclam 类
// 真外部交互臂，无结构性豁免条目。
//
// 生产可疑点（记录不改码）：cmd_export(695-698) 的 `--all` 只切换提示文案，
// 实际导出恒为 exporter.export_all 的「Active 过滤」结果——`--all` 承诺的
// 「包含非 Active 制品」并未生效（与 709 行提示语自相矛盾）。
// ===========================================================================

mod wave_b {
    use super::*;

    /// NEMESISBOT_HOME → tempdir 守卫（RAII，Drop 按进入前快照恢复/移除）。
    /// 所有用例必须持 crate::GLOBAL_STATE_LOCK。
    struct WbEnvGuard {
        _tmp: tempfile::TempDir,
        home: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
    }

    impl WbEnvGuard {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join(".nemesisbot");
            std::fs::create_dir_all(&home).unwrap();
            let prev = std::env::var_os("NEMESISBOT_HOME");
            unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
            Self { _tmp: tmp, home, prev }
        }

        fn forge_dir(&self) -> std::path::PathBuf {
            self.home.join("workspace").join("forge")
        }
    }

    impl Drop for WbEnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("NEMESISBOT_HOME", v),
                    None => std::env::remove_var("NEMESISBOT_HOME"),
                }
            }
        }
    }

    fn wb_write_config(home: &std::path::Path, forge_enabled: bool) {
        std::fs::write(
            home.join("config.json"),
            serde_json::json!({"version": "1.0", "forge": {"enabled": forge_enabled}})
                .to_string(),
        )
        .unwrap();
    }

    /// 当前月的 experiences/<yyyymm>/wb.jsonl 播种（时间取运行时刻，
    /// 不依赖固定日期，避免跨月漂移失效）。
    fn wb_seed_experience_rows(forge_dir: &std::path::Path, rows: &[serde_json::Value]) {
        let month = chrono::Local::now().format("%Y%m").to_string();
        let dir = forge_dir.join("experiences").join(month);
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = rows
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("wb.jsonl"), body + "\n").unwrap();
    }

    fn wb_row(hash: &str, tool: &str, count: u32, ms: i64, sr: f64) -> serde_json::Value {
        serde_json::json!({
            "pattern_hash": hash, "tool_name": tool, "count": count,
            "avg_duration_ms": ms, "success_rate": sr,
            "last_seen": chrono::Local::now().to_rfc3339()
        })
    }

    /// Artifact 全必填字段样例（ArtifactStatus 仅认 PascalCase，见 r7 注释）。
    fn wb_artifact(id: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "type": "tool", "name": format!("wb-{id}"), "kind": "Skill",
            "version": "1", "status": status, "content": "# wb",
            "tool_signature": [], "created_at": "2026-08-27T00:00:00Z",
            "updated_at": "2026-08-27T00:00:00Z", "usage_count": 1,
            "success_rate": 0.9, "last_degraded_at": null,
            "consecutive_observing_rounds": 0
        })
    }

    fn wb_write_registry(forge_dir: &std::path::Path, artifacts: &[serde_json::Value]) {
        std::fs::create_dir_all(forge_dir).unwrap();
        std::fs::write(
            forge_dir.join("registry.json"),
            serde_json::to_string(&artifacts).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn wave_b_learning_enable_fails_when_forge_json_unwritable() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        // forge.json 预建成目录：load 静默回默认，save 的 fs::write 必然失败。
        std::fs::create_dir_all(g.forge_dir().join("forge.json")).unwrap();
        let r = run(
            ForgeAction::Learning { action: Some(LearningAction::Enable) },
            false,
        );
        let err = r.expect_err("unwritable forge.json must propagate the IO error");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn wave_b_disable_inserts_forge_key_when_missing() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        // 存量配置带无关 sibling 字段、没有 forge 键 → Disable 走插入臂。
        std::fs::write(
            g.home.join("config.json"),
            serde_json::json!({"version": "1.0", "default_model": "m"}).to_string(),
        )
        .unwrap();

        run(ForgeAction::Disable, false).expect("disable without forge key ok");

        let cfg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(g.home.join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["forge"]["enabled"], false, "插入的 forge 对象应为 disabled");
        assert_eq!(cfg["default_model"], "m", "存量 sibling 字段必须保留");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_reflect_without_experiences_subdir_returns_hint() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        wb_write_config(&g.home, true);
        // forge 目录本身存在（过 uninitialized 守卫），但没有 experiences 子目录
        // → exp_count 走 else 0 值臂 → 提示采集为空后返回。
        std::fs::create_dir_all(g.forge_dir()).unwrap();
        run(ForgeAction::Reflect, false).expect("reflect without experiences ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_reflect_aggregate_read_error_on_jsonl_as_directory() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        wb_write_config(&g.home, true);
        // 月目录下的“数据文件”实际是个目录 → walk 的 read_to_string 失败并
        // 传播为 read_aggregated 的 Err → cmd_reflect 走 Err 提示臂。
        let month = chrono::Local::now().format("%Y%m").to_string();
        std::fs::create_dir_all(g.forge_dir().join("experiences").join(month).join("x.jsonl"))
            .unwrap();
        run(ForgeAction::Reflect, false).expect("aggregate error arm returns Ok with hint");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_reflect_empty_aggregation_stops_early() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        wb_write_config(&g.home, true);
        // experiences/<month>/ 存在但没有任何 .jsonl → read_aggregated 返回
        // Ok(vec![]) → “No experiences loaded.” 早退臂（区别于 store 层 Err）。
        let month = chrono::Local::now().format("%Y%m").to_string();
        std::fs::create_dir_all(g.forge_dir().join("experiences").join(&month)).unwrap();
        std::fs::write(g.forge_dir().join("experiences").join(&month).join("keep.txt"), "-")
            .unwrap(); // 非 .jsonl，保证不被扫描
        run(ForgeAction::Reflect, false).expect("empty aggregation early-return ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_reflect_reports_low_success_patterns() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        wb_write_config(&g.home, true);
        // 三条独立记录同一工具全败（sr=0.0）：low_success 的 count 口径是
        // 记录条数（>=3）且均值 <0.7 → 触发打印臂；外加一条健康记录扰动。
        wb_seed_experience_rows(
            &g.forge_dir(),
            &[
                wb_row("wbh1", "exec", 1, 10, 0.0),
                wb_row("wbh2", "exec", 1, 20, 0.0),
                wb_row("wbh3", "exec", 1, 30, 0.0),
                wb_row("wbh4", "read_file", 5, 5, 1.0),
            ],
        );
        run(ForgeAction::Reflect, false).expect("reflect with failing tool ok");
        // 报告仍应正常落盘（write_report 成功臂并行自证）。
        assert!(
            g.forge_dir().join("reflections").read_dir().map(|d| d.count()).unwrap_or(0) > 0,
            "reflection report must be written"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_reflect_warns_when_report_target_is_a_file() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        wb_write_config(&g.home, true);
        wb_seed_experience_rows(&g.forge_dir(), &[wb_row("wbh1", "exec", 2, 15, 1.0)]);
        // reflections 被一个普通文件占位 → Reflector 的 create_dir_all 失败
        // → write_report Err → 只告警，统计照常打印，整体 Ok。
        std::fs::create_dir_all(g.forge_dir()).unwrap();
        std::fs::write(g.forge_dir().join("reflections"), "not a dir").unwrap();
        run(ForgeAction::Reflect, false).expect("report failure must degrade to warning");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_export_draft_only_hints_all_flag() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        wb_write_config(&g.home, true);
        wb_write_registry(g.forge_dir().as_path(), &[wb_artifact("wbd1", "Draft")]);

        run(
            ForgeAction::Export { id: None, output: None, all: false },
            false,
        )
        .expect("draft-only export ok");

        // count=0 分支：目录已被建立但没有任何产物。
        let exports = g.forge_dir().join("exports");
        assert!(exports.is_dir(), "export_all 先建目标目录再过滤");
        assert_eq!(
            exports.read_dir().map(|d| d.count()).unwrap_or(usize::MAX),
            0,
            "Draft-only 不应产生导出物"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_export_active_artifacts_report_count() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        wb_write_config(&g.home, true);
        wb_write_registry(
            g.forge_dir().as_path(),
            &[wb_artifact("wba1", "Active"), wb_artifact("wbd2", "Draft")],
        );

        // 默认（active 文案头）与 --all（all 文案头）两种头部臂各跑一次。
        run(
            ForgeAction::Export { id: None, output: None, all: false },
            false,
        )
        .expect("active export (default header) ok");
        run(
            ForgeAction::Export { id: None, output: None, all: true },
            false,
        )
        .expect("active export (--all header) ok");

        let exports = g.forge_dir().join("exports");
        assert!(
            exports.read_dir().map(|d| d.count()).unwrap_or(0) > 0,
            "Active 制品应真正导出到 exports/"
        );
    }

    #[test]
    fn wave_b_learning_status_skips_details_without_learning_key() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        // forge.json 存在但无 learning 键 → 明细打印块整段跳过，
        // trace_collection 取默认 true 的兜底打印仍要走到。
        std::fs::create_dir_all(g.forge_dir()).unwrap();
        std::fs::write(g.forge_dir().join("forge.json"), "{}").unwrap();
        run(ForgeAction::Learning { action: Some(LearningAction::Status) }, false)
            .expect("status without learning key ok");
    }

    #[test]
    fn wave_b_history_blank_cycles_file_prints_no_history() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        // cycles 文件存在但内容全是空白行 → 过滤后 lines 为空 → 内层
        // “No learning history found.” 臂（区别于文件不存在的外层臂）。
        std::fs::create_dir_all(g.forge_dir().join("learning")).unwrap();
        std::fs::write(g.forge_dir().join("learning").join("learning_cycles.jsonl"), "\n \n\t\n")
            .unwrap();
        run(
            ForgeAction::Learning { action: Some(LearningAction::History { limit: 5 }) },
            false,
        )
        .expect("blank cycles history ok");
    }

    #[test]
    fn wave_b_history_cycles_as_directory_skips_silently() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let g = WbEnvGuard::new();
        // cycles 路径是目录：exists() 通过、read_to_string 失败 → 外层
        // if-let Ok 静默跳过，函数仍返回 Ok（该容错语义的行为钉死）。
        std::fs::create_dir_all(
            g.forge_dir().join("learning").join("learning_cycles.jsonl"),
        )
        .unwrap();
        run(
            ForgeAction::Learning { action: Some(LearningAction::History { limit: 5 }) },
            false,
        )
        .expect("cycles-as-directory is silently skipped, not an error");
    }
}

// ===========================================================================
// r10（覆盖率 A 类 miss 补充）：fresh enable 的产物做「内容级」断言 +
// enable→disable 往返。既有批次只断言文件存在性；这里钉：
// - 全新安装写入的 forge.json == 完整默认配置（313-316）；
// - registry.json 恰为空数组文本 "[]"（319-322）；
// - disable 在 config.json 上只翻 forge.enabled、其余键原样（331-352
//   含 347-348 写回臂）。
// 全部直调 cmd_* 函数：不触 env / 端口 / 网络，无需全局锁。
// ===========================================================================

#[test]
fn r10_fresh_enable_writes_default_config_empty_registry_then_disable_roundtrip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(&cfg_path, r#"{"agents":{"defaults":{"llm":"r10-x"}}}"#).unwrap();
    let forge_dir = home.join("workspace").join("forge");

    cmd_enable(&cfg_path, &forge_dir).unwrap();

    // forge.json 内容必须等于完整默认配置。
    let forged: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(forge_dir.join("forge.json")).unwrap())
            .unwrap();
    assert_eq!(
        forged,
        serde_json::to_value(default_forge_config()).unwrap(),
        "全新 enable 必须落一份完整默认 forge.json"
    );
    // registry.json 恰为空数组文本（不是缺失、不是对象）。
    let reg = std::fs::read_to_string(forge_dir.join("registry.json")).unwrap();
    assert_eq!(reg.trim(), "[]", "全新 enable 的 registry.json 必须恰为 \"[]\"");

    // 二次 enable 不重写既有 forge.json/registry.json（幂等保用户数据）。
    let forged_before = std::fs::read_to_string(forge_dir.join("forge.json")).unwrap();
    cmd_enable(&cfg_path, &forge_dir).unwrap();
    assert_eq!(
        std::fs::read_to_string(forge_dir.join("forge.json")).unwrap(),
        forged_before,
        "再次 enable 不得覆盖已有 forge.json"
    );

    // disable 写回：enabled=false，config.json 其它键原样保留。
    cmd_disable(&cfg_path).unwrap();
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(cfg["forge"]["enabled"], false);
    assert_eq!(
        cfg["agents"]["defaults"]["llm"], "r10-x",
        "非 forge 键不得被 disable 触碰"
    );
}
