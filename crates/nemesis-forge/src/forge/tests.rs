use super::*;

/// Shared mock LLM caller for tests.
struct MockLLMCaller;

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for MockLLMCaller {
    async fn chat(&self, _system_prompt: &str, _user_prompt: &str, _max_tokens: Option<i64>) -> Result<String, String> {
        Ok("mock".to_string())
    }
}

#[tokio::test]
async fn test_forge_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    assert!(!forge.is_running());
    forge.start().await;
    assert!(forge.is_running());
    forge.stop().await;
    assert!(!forge.is_running());
}

#[test]
fn test_forge_components_accessible() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    // All core components should be accessible
    let _ = forge.collector();
    let _ = forge.registry();
    let _ = forge.sanitizer();
    let _ = forge.exporter();
    let _ = forge.config();
    let _ = forge.workspace();
    let _ = forge.forge_dir();
}

#[test]
fn test_forge_subsystem_accessors_none() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    // Optional subsystems should be None initially
    assert!(forge.reflector().is_none());
    assert!(forge.pipeline().is_none());
    assert!(forge.mcp_installer().is_none());
    assert!(forge.syncer().is_none());
    assert!(forge.trace_collector().is_none());
    assert!(forge.trace_store().is_none());
    assert!(forge.learning_engine().is_none());
    assert!(forge.deployment_monitor().is_none());
    assert!(forge.cycle_store().is_none());
    assert!(forge.bridge().is_none());
}

#[test]
fn test_forge_receive_reflection_without_syncer() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    let result = forge.receive_reflection(&serde_json::json!({"content": "test"}));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Syncer not initialized"));
}

#[test]
fn test_forge_reflect_now_without_reflector() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    let result = forge.reflect_now(&[]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Reflector not initialized"));
}

#[test]
fn test_create_skill() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge.create_skill(
        "my-skill",
        "# My Skill\nThis is a test skill content.",
        "A test skill",
        vec!["file_read".to_string()],
    ).unwrap();

    assert_eq!(artifact.name, "my-skill");
    assert!(artifact.id.starts_with("skill-"));

    // Check the skill file was written in forge dir
    let skill_path = dir.path().join("forge").join("skills").join("my-skill").join("SKILL.md");
    assert!(skill_path.exists());

    // Check the workspace copy with -forge suffix
    let ws_skill_path = dir.path().join("skills").join("my-skill-forge").join("SKILL.md");
    assert!(ws_skill_path.exists());

    // Check it was registered
    let registered = forge.registry.get(&artifact.id);
    assert!(registered.is_some());
}

#[test]
fn test_create_skill_auto_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge.create_skill(
        "auto-frontmatter",
        "Just plain content without frontmatter.",
        "Auto frontmatter test",
        vec![],
    ).unwrap();

    // Content should have frontmatter added
    assert!(artifact.content.contains("---"));
    assert!(artifact.content.contains("name: auto-frontmatter"));
}

#[test]
fn test_create_skill_existing_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let content = "---\nname: existing\n---\nCustom content.";
    let artifact = forge.create_skill(
        "existing-fm",
        content,
        "Test",
        vec![],
    ).unwrap();

    // Should not double-add frontmatter
    assert!(artifact.content.starts_with("---"));
    assert!(artifact.content.contains("Custom content."));
}

#[test]
fn test_cleanup_prompt_suggestions_removes_old() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    let prompts_dir = dir.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();

    // Create an old suggestion file
    let old_file = prompts_dir.join("old_suggestion.md");
    std::fs::write(&old_file, "old content").unwrap();

    // Set modification time to 10 days ago
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86400);
    let _ = filetime::set_file_mtime(&old_file, filetime::FileTime::from_system_time(old_time));

    // Create a recent suggestion file
    let recent_file = prompts_dir.join("recent_suggestion.md");
    std::fs::write(&recent_file, "recent content").unwrap();

    // Create a non-suggestion file (should be kept)
    let other_file = prompts_dir.join("notes.md");
    std::fs::write(&other_file, "other").unwrap();

    forge.cleanup_prompt_suggestions(7);

    assert!(!old_file.exists()); // Removed: too old
    assert!(recent_file.exists()); // Kept: recent
    assert!(other_file.exists()); // Kept: not a suggestion file
}

#[test]
fn test_cleanup_prompt_suggestions_no_dir() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    // Should not panic when prompts dir doesn't exist
    forge.cleanup_prompt_suggestions(7);
}

#[test]
fn test_forge_init_reflector() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    assert!(forge.reflector().is_none());
    forge.init_reflector(Reflector::new());
    assert!(forge.reflector().is_some());
}

#[test]
fn test_forge_init_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    assert!(forge.pipeline().is_none());
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    forge.init_pipeline(Pipeline::new(ForgeConfig::default(), registry));
    assert!(forge.pipeline().is_some());
}

#[test]
fn test_forge_init_mcp_installer() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    assert!(forge.mcp_installer().is_none());
    forge.init_mcp_installer(MCPInstaller::new(dir.path().to_path_buf()));
    assert!(forge.mcp_installer().is_some());
}

#[test]
fn test_forge_set_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    assert!(forge.bridge().is_none());
    let bridge = Arc::new(crate::bridge::NoOpBridge::new("node-1".into()));
    forge.set_bridge(bridge);
    assert!(forge.bridge().is_some());
}

#[test]
fn test_forge_init_syncer_without_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.init_syncer();
    // No bridge set, so syncer should remain None
    assert!(forge.syncer().is_none());
}

#[test]
fn test_forge_init_syncer_with_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    let bridge = Arc::new(crate::bridge::NoOpBridge::new("node-1".into()));
    forge.set_bridge(bridge);
    forge.init_syncer();
    assert!(forge.syncer().is_some());
}

#[test]
fn test_forge_reflect_now_with_reflector() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.init_reflector(Reflector::new());
    let result = forge.reflect_now(&[]);
    assert!(result.is_ok());
    let reflection = result.unwrap();
    assert!(!reflection.id.is_empty());
}

#[test]
fn test_create_skill_with_tool_signature() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    let artifact = forge.create_skill(
        "signed-skill",
        "Content with signature",
        "Signed",
        vec!["file_read".to_string(), "file_write".to_string()],
    ).unwrap();
    assert_eq!(artifact.tool_signature.len(), 2);
    assert!(artifact.tool_signature.contains(&"file_read".to_string()));
    assert!(artifact.tool_signature.contains(&"file_write".to_string()));
}

#[test]
fn test_create_skill_registry_updated() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.create_skill("reg-test", "content", "desc", vec![]).unwrap();
    let all = forge.registry.list(None, None);
    assert!(all.iter().any(|a| a.name == "reg-test"));
}

#[test]
fn test_create_skill_file_content() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.create_skill(
        "file-check",
        "---\nname: file-check\n---\nContent here",
        "desc",
        vec![],
    ).unwrap();
    let skill_path = dir.path().join("forge").join("skills").join("file-check").join("SKILL.md");
    let content = std::fs::read_to_string(&skill_path).unwrap();
    assert!(content.contains("Content here"));
}

#[test]
fn test_create_skill_with_active_default_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.artifacts.default_status = "active".to_string();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge.create_skill(
        "active-skill",
        "Content",
        "Active skill",
        vec![],
    ).unwrap();

    assert_eq!(artifact.status, nemesis_types::forge::ArtifactStatus::Active);
}

#[test]
fn test_create_skill_with_observing_default_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.artifacts.default_status = "observing".to_string();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge.create_skill(
        "observing-skill",
        "Content",
        "Observing skill",
        vec![],
    ).unwrap();

    assert_eq!(artifact.status, nemesis_types::forge::ArtifactStatus::Observing);
}

#[test]
fn test_create_skill_with_unknown_default_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.artifacts.default_status = "unknown_status".to_string();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge.create_skill(
        "unknown-skill",
        "Content",
        "Unknown status skill",
        vec![],
    ).unwrap();

    assert_eq!(artifact.status, nemesis_types::forge::ArtifactStatus::Draft);
}

#[test]
fn test_forge_set_provider_without_subsystems() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    // Should not panic even without reflector/pipeline
    forge.set_provider(Arc::new(MockLLMCaller));
}

#[test]
fn test_forge_init_trace() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    assert!(forge.trace_collector().is_none());
    assert!(forge.trace_store().is_none());

    let collector = crate::trace::TraceCollector::new();
    let store = crate::trace_store::TraceStore::new(dir.path().join("traces.jsonl"));
    forge.init_trace(collector, store);

    assert!(forge.trace_collector().is_some());
    assert!(forge.trace_store().is_some());
}

#[test]
fn test_forge_init_learning() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    assert!(forge.learning_engine().is_none());
    assert!(forge.deployment_monitor().is_none());
    assert!(forge.cycle_store().is_none());

    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = CycleStore::from_base(dir.path());
    let engine = LearningEngine::new(ForgeConfig::default(), registry.clone(), cycle_store);
    let monitor = DeploymentMonitor::new(ForgeConfig::default(), registry);
    let store = CycleStore::from_base(dir.path().join("cycles"));

    forge.init_learning(engine, monitor, store);

    assert!(forge.learning_engine().is_some());
    assert!(forge.deployment_monitor().is_some());
    assert!(forge.cycle_store().is_some());
}

#[tokio::test]
async fn test_forge_double_start_warning() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    forge.start().await;
    assert!(forge.is_running());

    // Second start should be idempotent (returns early)
    forge.start().await;
    assert!(forge.is_running());

    forge.stop().await;
    assert!(!forge.is_running());
}

#[test]
fn test_forge_workspace_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    let forge = Forge::new(ForgeConfig::default(), ws.clone());
    assert_eq!(*forge.workspace(), ws);
}

#[test]
fn test_forge_forge_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    let forge = Forge::new(ForgeConfig::default(), ws.clone());
    assert_eq!(*forge.forge_dir(), ws.join("forge"));
}

#[test]
fn test_create_skill_without_auto_validate() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.validation.auto_validate = false;
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge.create_skill(
        "no-validate",
        "Content",
        "No validation",
        vec!["file_read".to_string()],
    ).unwrap();

    assert_eq!(artifact.name, "no-validate");
    // Should be draft since auto_validate is off
    assert_eq!(artifact.status, nemesis_types::forge::ArtifactStatus::Draft);
}

#[test]
fn test_forge_set_provider_with_reflector() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.init_reflector(Reflector::new());
    forge.set_provider(Arc::new(MockLLMCaller));
}

#[test]
fn test_forge_set_provider_with_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    forge.init_pipeline(Pipeline::new(ForgeConfig::default(), registry));
    forge.set_provider(Arc::new(MockLLMCaller));
}
