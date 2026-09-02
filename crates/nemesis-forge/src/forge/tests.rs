use super::*;

/// Shared mock LLM caller for tests.
struct MockLLMCaller;

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for MockLLMCaller {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Ok("mock".to_string())
    }
}

#[tokio::test]
async fn test_forge_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    assert!(!forge.is_running());
    let forge = Arc::new(forge);
    Arc::clone(&forge).start().await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_fl2_forge_stop_completes_cleanly() {
    // F-L2: Forge::stop() must cleanly signal + flush + join background tasks
    // without hanging. (The gateway calls this on graceful shutdown.)
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.enabled = true;
    let forge = Arc::new(Forge::new(config, dir.path().to_path_buf()));
    forge.clone().start().await;
    forge.stop().await; // If this hangs, test times out → fail.
}

#[test]
fn test_fm11_frontmatter_added_when_separator_not_at_start() {
    // F-M11: a "---" horizontal rule NOT at content start must not be treated
    // as existing frontmatter (old `contains("---")` skipped adding the wrapper).
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    let artifact = forge
        .create_skill(
            "fm11-test",
            "# Title\n\nSome text.\n\n---\n\nMore text here that is long enough.",
            "desc",
            vec![],
        )
        .unwrap();
    assert!(
        artifact.content.starts_with("---\nname: fm11-test"),
        "frontmatter should be prepended when content doesn't start with ---"
    );
}

#[test]
fn test_fp1_is_enabled_reflects_config() {
    // F-P1: is_enabled() is the runtime gate the per-tool-call recording path
    // checks (so forge.enabled=false actually stops collection).
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ForgeConfig::default();
    cfg.enabled = false;
    assert!(!Forge::new(cfg, dir.path().to_path_buf()).is_enabled());
    let dir2 = tempfile::tempdir().unwrap();
    let mut cfg2 = ForgeConfig::default();
    cfg2.enabled = true;
    assert!(Forge::new(cfg2, dir2.path().to_path_buf()).is_enabled());
}

#[test]
fn test_create_skill() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge
        .create_skill(
            "my-skill",
            "# My Skill\nThis is a test skill content.",
            "A test skill",
            vec!["file_read".to_string()],
        )
        .unwrap();

    assert_eq!(artifact.name, "my-skill");
    assert!(artifact.id.starts_with("skill-"));

    // Check the skill file was written in forge dir
    let skill_path = dir
        .path()
        .join("forge")
        .join("skills")
        .join("my-skill")
        .join("SKILL.md");
    assert!(skill_path.exists());

    // Check the workspace copy with -forge suffix
    let ws_skill_path = dir
        .path()
        .join("skills")
        .join("my-skill-forge")
        .join("SKILL.md");
    assert!(ws_skill_path.exists());

    // Check it was registered
    let registered = forge.registry.get(&artifact.id);
    assert!(registered.is_some());
}

#[test]
fn test_create_skill_rejects_dangerous_content() {
    // F-S1 regression: a skill with dangerous commands must be rejected BEFORE
    // the file is written — otherwise it becomes an agent instruction.
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let res = forge.create_skill(
        "bad-skill",
        "---\nname: bad\n---\nRun rm -rf / to clean everything up now.",
        "dangerous",
        vec![],
    );
    assert!(res.is_err(), "dangerous skill must be rejected");
    assert!(res.unwrap_err().contains("security validation"));

    // Rejected skill must NOT be written to disk.
    let skill_path = dir
        .path()
        .join("forge")
        .join("skills")
        .join("bad-skill")
        .join("SKILL.md");
    assert!(!skill_path.exists(), "rejected skill must not be written");

    // A safe (even if short) skill is still accepted — length is quality, not security.
    let ok = forge.create_skill(
        "good-skill",
        "---\nname: good\n---\nA perfectly safe skill.",
        "safe",
        vec![],
    );
    assert!(
        ok.is_ok(),
        "safe skill should be accepted, got: {:?}",
        ok.err()
    );
}

#[test]
fn test_create_skill_auto_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge
        .create_skill(
            "auto-frontmatter",
            "Just plain content without frontmatter.",
            "Auto frontmatter test",
            vec![],
        )
        .unwrap();

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
    let artifact = forge
        .create_skill("existing-fm", content, "Test", vec![])
        .unwrap();

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
    forge.init_pipeline(Arc::new(Pipeline::new(ForgeConfig::default(), registry)));
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
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
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
    let artifact = forge
        .create_skill(
            "signed-skill",
            "Content with signature",
            "Signed",
            vec!["file_read".to_string(), "file_write".to_string()],
        )
        .unwrap();
    assert_eq!(artifact.tool_signature.len(), 2);
    assert!(artifact.tool_signature.contains(&"file_read".to_string()));
    assert!(artifact.tool_signature.contains(&"file_write".to_string()));
}

#[test]
fn test_create_skill_registry_updated() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge
        .create_skill("reg-test", "content", "desc", vec![])
        .unwrap();
    let all = forge.registry.list(None, None);
    assert!(all.iter().any(|a| a.name == "reg-test"));
}

#[test]
fn test_create_skill_file_content() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge
        .create_skill(
            "file-check",
            "---\nname: file-check\n---\nContent here",
            "desc",
            vec![],
        )
        .unwrap();
    let skill_path = dir
        .path()
        .join("forge")
        .join("skills")
        .join("file-check")
        .join("SKILL.md");
    let content = std::fs::read_to_string(&skill_path).unwrap();
    assert!(content.contains("Content here"));
}

#[test]
fn test_create_skill_with_active_default_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.artifacts.default_status = "active".to_string();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge
        .create_skill("active-skill", "Content", "Active skill", vec![])
        .unwrap();

    assert_eq!(
        artifact.status,
        nemesis_types::forge::ArtifactStatus::Active
    );
}

#[test]
fn test_create_skill_with_observing_default_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.artifacts.default_status = "observing".to_string();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge
        .create_skill("observing-skill", "Content", "Observing skill", vec![])
        .unwrap();

    assert_eq!(
        artifact.status,
        nemesis_types::forge::ArtifactStatus::Observing
    );
}

#[test]
fn test_create_skill_with_unknown_default_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.artifacts.default_status = "unknown_status".to_string();
    let forge = Forge::new(config, dir.path().to_path_buf());

    let artifact = forge
        .create_skill("unknown-skill", "Content", "Unknown status skill", vec![])
        .unwrap();

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
    let monitor = Arc::new(DeploymentMonitor::new(ForgeConfig::default(), registry));
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

    let forge = Arc::new(forge);
    Arc::clone(&forge).start().await;
    assert!(forge.is_running());

    // Second start should be idempotent (returns early)
    Arc::clone(&forge).start().await;
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

    let artifact = forge
        .create_skill(
            "no-validate",
            "Content",
            "No validation",
            vec!["file_read".to_string()],
        )
        .unwrap();

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
    forge.init_pipeline(Arc::new(Pipeline::new(ForgeConfig::default(), registry)));
    forge.set_provider(Arc::new(MockLLMCaller));
}

// =========================================================================
// Integration test helpers
// =========================================================================

/// Mock LLM caller that returns a programmable response (async, for Reflector).
struct ProgrammableMockLLM {
    response: String,
}

impl ProgrammableMockLLM {
    fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for ProgrammableMockLLM {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Ok(self.response.clone())
    }
}

/// Mock LLM caller that always returns an error.
struct ErrorMockLLM;

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for ErrorMockLLM {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Err("LLM unavailable".to_string())
    }
}

/// Mock LLM caller (for LearningEngine).
#[allow(dead_code)]
struct MockSyncProvider {
    response: String,
}

#[allow(dead_code)]
impl MockSyncProvider {
    fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for MockSyncProvider {
    async fn chat(
        &self,
        _system: &str,
        _user: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Ok(self.response.clone())
    }
}

/// Mock LLM caller that always returns an error.
struct ErrorSyncProvider;

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for ErrorSyncProvider {
    async fn chat(
        &self,
        _system: &str,
        _user: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Err("LLM provider unavailable".to_string())
    }
}

/// Build a test CollectedExperience.
fn make_collected_experience(
    tool: &str,
    success: bool,
    dur: u64,
) -> crate::types::CollectedExperience {
    crate::types::CollectedExperience {
        dedup_hash: format!("test-{}-{}", tool, success),
        experience: nemesis_types::forge::Experience {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool.to_string(),
            input_summary: "test input".into(),
            output_summary: if success { "ok" } else { "error" }.into(),
            success,
            duration_ms: dur,
            timestamp: chrono::Local::now().to_rfc3339(),
            session_key: "test:session".into(),
        },
    }
}

/// Write experiences directly to the ExperienceStore for a forge directory.
async fn write_test_experiences(forge_dir: &std::path::Path, count: usize) {
    let store = crate::experience_store::ExperienceStore::from_forge_dir(forge_dir);
    for i in 0..count {
        let exp = make_collected_experience(
            if i % 2 == 0 {
                "file_read"
            } else {
                "shell_exec"
            },
            true,
            100 + i as u64 * 50,
        );
        store.append(&exp).await.unwrap();
    }
}

/// Create a fully initialized Forge with Reflector + LearningEngine for integration tests.
fn create_integration_forge(dir: &std::path::Path) -> Arc<Forge> {
    let mut config = ForgeConfig::default();
    config.learning.enabled = true;
    let mut forge = Forge::new(config.clone(), dir.to_path_buf());

    // Reflector with a reflections dir for disk writes
    let reflections_dir = dir.join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));

    // LearningEngine — use the same base dir as Forge's cycle_store
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_dir = dir.join("forge");
    let engine = crate::learning_engine::LearningEngine::new(
        config.clone(),
        registry.clone(),
        crate::cycle_store::CycleStore::new(&cycle_dir),
    );
    let monitor = Arc::new(crate::monitor::DeploymentMonitor::new(
        config.clone(),
        registry,
    ));
    let cs = crate::cycle_store::CycleStore::new(&cycle_dir);
    forge.init_learning(engine, monitor, cs);

    Arc::new(forge)
}

// =========================================================================
// Layer 1A: Reflection cycle integration tests
// =========================================================================

#[tokio::test]
async fn test_reflection_cycle_writes_report() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    // Pre-write 5 experiences
    write_test_experiences(forge.forge_dir(), 5).await;

    // Run the reflection cycle
    forge.run_reflection_cycle().await;

    // Check report was written
    let reflections_dir = dir.path().join("forge").join("reflections");
    if reflections_dir.exists() {
        let reports: Vec<_> = std::fs::read_dir(&reflections_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
            .collect();
        assert!(!reports.is_empty(), "expected at least one report file");
    }

    // Check learning cycle was recorded
    if let Some(cycle_store) = forge.cycle_store() {
        let cycles = cycle_store.read_all().await.unwrap();
        assert!(!cycles.is_empty(), "expected at least one learning cycle");
        assert_eq!(
            cycles[0].status,
            nemesis_types::forge::CycleStatus::Completed
        );
    }
}

#[tokio::test]
async fn test_reflection_cycle_empty_experiences() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    // No experiences written
    forge.run_reflection_cycle().await;

    // Should return early — no reports
    let reflections_dir = dir.path().join("forge").join("reflections");
    if reflections_dir.exists() {
        let count = std::fs::read_dir(&reflections_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
            .count();
        assert_eq!(count, 0, "expected no reports with empty experiences");
    }
}

#[tokio::test]
async fn test_reflection_cycle_without_reflector() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let forge = Forge::new(config, dir.path().to_path_buf());
    let forge = Arc::new(forge);

    // Write experiences but no reflector initialized
    write_test_experiences(forge.forge_dir(), 3).await;

    // Should return early without panic
    forge.run_reflection_cycle().await;
}

#[tokio::test]
async fn test_reflection_cycle_without_learning_engine() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let mut forge = Forge::new(config, dir.path().to_path_buf());

    // Only init reflector, no learning engine
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));

    let forge = Arc::new(forge);
    write_test_experiences(forge.forge_dir(), 5).await;

    forge.run_reflection_cycle().await;

    // Report should still be written (reflector works)
    let reflections_dir = dir.path().join("forge").join("reflections");
    let count = std::fs::read_dir(&reflections_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .count();
    assert!(count > 0, "expected report without learning engine");
}

#[tokio::test]
async fn test_reflection_cycle_without_syncer() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    // No syncer initialized (create_integration_forge doesn't init syncer)
    write_test_experiences(forge.forge_dir(), 5).await;

    // Should not panic
    forge.run_reflection_cycle().await;
}

// =========================================================================
// Layer 1B: Cleanup cycle tests
// =========================================================================

#[tokio::test]
async fn test_cleanup_cycle_removes_old_experiences() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    let store = crate::experience_store::ExperienceStore::from_forge_dir(forge.forge_dir());

    // Write a recent experience (goes to today's file)
    let recent_exp = make_collected_experience("new_tool", true, 50);
    store.append(&recent_exp).await.unwrap();

    // Manually create an old file (before cleanup cutoff)
    let base_dir = forge.forge_dir().join("experiences");
    let old_month_dir = base_dir.join("202001");
    std::fs::create_dir_all(&old_month_dir).unwrap();
    let old_file = old_month_dir.join("20200101.jsonl");
    let old_exp = crate::types::CollectedExperience {
        dedup_hash: "old-exp".to_string(),
        experience: nemesis_types::forge::Experience {
            id: "exp-old".to_string(),
            tool_name: "old_tool".to_string(),
            input_summary: "old".into(),
            output_summary: "old".into(),
            success: true,
            duration_ms: 100,
            timestamp: "2020-01-01T00:00:00Z".to_string(),
            session_key: "test".into(),
        },
    };
    let json = serde_json::to_string(&old_exp).unwrap();
    std::fs::write(&old_file, json).unwrap();

    assert!(old_file.exists());

    forge.run_cleanup_cycle().await;

    // Old file should be removed
    assert!(!old_file.exists(), "old experience file should be removed");

    // Recent should still be readable
    let after = store.read_all().await.unwrap();
    assert!(
        after.iter().any(|e| e.experience.tool_name == "new_tool"),
        "recent experience should survive"
    );
}

#[tokio::test]
async fn test_cleanup_cycle_removes_old_reports() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();

    // Create an old report
    let old_report = reflections_dir.join("2020-01-01_report.md");
    std::fs::write(&old_report, "old report").unwrap();
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(365 * 86400);
    let _ = filetime::set_file_mtime(&old_report, filetime::FileTime::from_system_time(old_time));

    // Create a recent report
    let recent_report = reflections_dir.join("2099-12-31_report.md");
    std::fs::write(&recent_report, "recent report").unwrap();

    forge.run_cleanup_cycle().await;

    assert!(!old_report.exists(), "old report should be removed");
    assert!(recent_report.exists(), "recent report should be kept");
}

#[tokio::test]
async fn test_cleanup_cycle_empty_state() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    // No data at all — should not panic
    forge.run_cleanup_cycle().await;
}

// =========================================================================
// Layer 2A: Pattern detection tests
// =========================================================================

#[tokio::test]
async fn test_pattern_tool_chain_detection() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = crate::cycle_store::CycleStore::new(dir.path());
    let engine = crate::learning_engine::LearningEngine::new(config, registry, cycle_store);

    // 6 experiences with the same tool (≥5 triggers tool_chain, default min_pattern_frequency=5)
    let experiences: Vec<_> = (0..6)
        .map(|_| make_collected_experience("file_read", true, 100))
        .collect();

    let patterns = engine.extract_patterns(&experiences);
    assert!(!patterns.is_empty(), "expected at least one pattern");
    assert!(
        patterns.iter().any(|p| p.pattern_type == "tool_chain"),
        "expected tool_chain pattern, got: {:?}",
        patterns.iter().map(|p| &p.pattern_type).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_pattern_error_recovery_detection() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = crate::cycle_store::CycleStore::new(dir.path());
    let engine = crate::learning_engine::LearningEngine::new(config, registry, cycle_store);

    // Same tool: 5 failures + 5 successes (frequency = error_count, must be ≥5)
    let mut experiences = vec![];
    for _ in 0..5 {
        experiences.push(make_collected_experience("web_fetch", false, 500));
    }
    for _ in 0..5 {
        experiences.push(make_collected_experience("web_fetch", true, 200));
    }

    let patterns = engine.extract_patterns(&experiences);
    assert!(
        patterns.iter().any(|p| p.pattern_type == "error_recovery"),
        "expected error_recovery pattern, got: {:?}",
        patterns.iter().map(|p| &p.pattern_type).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_pattern_efficiency_issue_detection() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = crate::cycle_store::CycleStore::new(dir.path());
    let engine = crate::learning_engine::LearningEngine::new(config, registry, cycle_store);

    // Most tools fast, one tool very slow — need enough fast tools to keep overall avg low
    let mut experiences = vec![];
    for _ in 0..10 {
        experiences.push(make_collected_experience("file_read", true, 100));
    }
    for _ in 0..5 {
        experiences.push(make_collected_experience("shell_exec", true, 50000));
    }

    let patterns = engine.extract_patterns(&experiences);
    assert!(
        patterns
            .iter()
            .any(|p| p.pattern_type == "efficiency_issue"),
        "expected efficiency_issue pattern, got: {:?}",
        patterns.iter().map(|p| &p.pattern_type).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_pattern_success_template_detection() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = crate::cycle_store::CycleStore::new(dir.path());
    let engine = crate::learning_engine::LearningEngine::new(config, registry, cycle_store);

    // 6 experiences all successful with the same tool (≥5 for min_pattern_frequency)
    let experiences: Vec<_> = (0..6)
        .map(|_| make_collected_experience("file_read", true, 100))
        .collect();

    let patterns = engine.extract_patterns(&experiences);
    assert!(
        patterns
            .iter()
            .any(|p| p.pattern_type == "success_template"),
        "expected success_template pattern, got: {:?}",
        patterns.iter().map(|p| &p.pattern_type).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_no_patterns_insufficient_data() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = crate::cycle_store::CycleStore::new(dir.path());
    let engine = crate::learning_engine::LearningEngine::new(config, registry, cycle_store);

    // Only 1 experience — below min_pattern_frequency of 3
    let experiences = vec![make_collected_experience("file_read", true, 100)];

    let patterns = engine.extract_patterns(&experiences);
    assert!(
        patterns.is_empty(),
        "expected no patterns with insufficient data"
    );
}

#[tokio::test]
async fn test_run_cycle_full_with_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = crate::cycle_store::CycleStore::new(dir.path());
    let engine = crate::learning_engine::LearningEngine::new(config, registry, cycle_store);

    // No LLM provider — execute_create_skill will skip but pattern detection still works
    let experiences: Vec<_> = (0..6)
        .map(|_| make_collected_experience("file_read", true, 100))
        .collect();

    let cycle = engine.run_cycle(&experiences).await;
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);
    assert!(cycle.patterns_found > 0, "expected patterns detected");
}

// =========================================================================
// Layer 2B: Pipeline validation tests
// =========================================================================

#[test]
fn test_pipeline_validation_passes_with_mock() {
    let _dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let pipeline = Pipeline::new(ForgeConfig::default(), registry);
    pipeline.set_provider(Arc::new(ProgrammableMockLLM::new(
        r#"{"score": 0.9, "feedback": "Good quality"}"#,
    )));

    let content = "---\nname: test-skill\n---\n# Test Skill\nThis is a well-structured skill.";
    let validation = pipeline.validate(
        nemesis_types::forge::ArtifactKind::Skill,
        "test-skill",
        content,
    );
    // Pipeline should complete without error
    let status = pipeline.determine_status(&validation);
    assert_ne!(
        status,
        nemesis_types::forge::ArtifactStatus::Negative,
        "valid content should not be Negative"
    );
}

#[test]
fn test_pipeline_validation_with_llm_failure() {
    let _dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let pipeline = Pipeline::new(ForgeConfig::default(), registry);
    pipeline.set_provider(Arc::new(ErrorMockLLM));

    let content = "---\nname: test-skill\n---\n# Test Skill";
    let validation = pipeline.validate(
        nemesis_types::forge::ArtifactKind::Skill,
        "test-skill",
        content,
    );
    // Should degrade to static validation, not panic
    let status = pipeline.determine_status(&validation);
    // With LLM failure, falls back to basic validation
    assert_ne!(status, nemesis_types::forge::ArtifactStatus::Archived);
}

// =========================================================================
// Layer 3A: LLM failure tests
// =========================================================================

#[test]
fn test_reflector_with_llm_failure() {
    let reflector = Reflector::new();
    // Reflector::new() creates a basic reflector without LLM
    // reflect() should still produce statistical analysis
    let experiences = vec![
        make_collected_experience("file_read", true, 100),
        make_collected_experience("shell_exec", false, 200),
    ];
    let report = reflector.reflect(&experiences, None, "today", "all");
    // Should have stats even without LLM
    assert!(!report.date.is_empty());
    assert!(report.stats.total_records > 0);
    assert!(
        report.llm_insights.is_none(),
        "no LLM insights expected without LLM provider"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_learning_engine_with_llm_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = ForgeConfig::default();
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = crate::cycle_store::CycleStore::new(dir.path());
    let engine = crate::learning_engine::LearningEngine::new(config, registry, cycle_store);

    // Set error provider
    engine.set_provider(Arc::new(ErrorSyncProvider));

    let experiences: Vec<_> = (0..6)
        .map(|_| make_collected_experience("file_read", true, 100))
        .collect();

    // Should not panic
    let cycle = engine.run_cycle(&experiences).await;
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);
    assert!(
        cycle.patterns_found > 0,
        "patterns should still be detected"
    );
    // actions_taken may be 0 because LLM failed for skill generation
}

#[tokio::test]
async fn test_reflection_cycle_with_llm_errors() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    // Set error LLM on all subsystems
    forge.set_provider(Arc::new(ErrorMockLLM));

    write_test_experiences(forge.forge_dir(), 5).await;

    // Should not panic despite LLM failures
    forge.run_reflection_cycle().await;
}

// =========================================================================
// Layer 3B: Missing subsystem tests
// =========================================================================

#[tokio::test]
async fn test_reflection_cycle_learning_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learning.enabled = false;
    let mut forge = Forge::new(config, dir.path().to_path_buf());

    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));

    // LearningEngine exists but disabled
    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    let cycle_store = crate::cycle_store::CycleStore::new(dir.path().join("forge"));
    let engine =
        crate::learning_engine::LearningEngine::new(ForgeConfig::default(), registry, cycle_store);
    let monitor = Arc::new(crate::monitor::DeploymentMonitor::new(
        ForgeConfig::default(),
        Arc::new(Registry::new(RegistryConfig::default())),
    ));
    let cs = crate::cycle_store::CycleStore::new(dir.path().join("forge").join("cycles"));
    forge.init_learning(engine, monitor, cs);

    let forge = Arc::new(forge);
    write_test_experiences(forge.forge_dir(), 5).await;

    forge.run_reflection_cycle().await;

    // Report should still be written (reflector works)
    let reflections_dir = dir.path().join("forge").join("reflections");
    let count = std::fs::read_dir(&reflections_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .count();
    assert!(
        count > 0,
        "report should be written even with learning disabled"
    );
}

// =========================================================================
// S8 coverage batch (quality-hardening goal 冲刺 S8)
// =========================================================================

/// Bridge whose share_reflection always fails — drives the cluster-share
/// error branch of run_reflection_cycle.
struct S8FailingBridge;

#[async_trait::async_trait]
impl crate::bridge::ClusterForgeBridge for S8FailingBridge {
    async fn share_reflection(&self, _report_json: serde_json::Value) -> Result<usize, String> {
        Err("share boom".to_string())
    }
    async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
        Ok(vec![])
    }
    async fn get_online_peers(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
    fn local_node_id(&self) -> &str {
        "s8-node"
    }
    fn is_cluster_enabled(&self) -> bool {
        false
    }
}

/// Build a Forge with reflector + syncer (bridge-controlled) and pre-written
/// experiences — the minimal setup for the run_reflection_cycle cluster-share
/// step.
async fn s8_forge_with_syncer(
    dir: &std::path::Path,
    bridge: Arc<dyn crate::bridge::ClusterForgeBridge>,
) -> Arc<Forge> {
    let mut forge = Forge::new(ForgeConfig::default(), dir.to_path_buf());
    let reflections_dir = dir.join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    forge.set_bridge(bridge);
    forge.init_syncer();
    let forge = Arc::new(forge);
    write_test_experiences(forge.forge_dir(), 3).await;
    forge
}

/// Forge::new tracing field expressions (workspace/forge_dir display).
#[test]
fn test_s8_forge_new_tracing_fields() {
    let _g = crate::test_support::quiet_trace_guard();
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    assert!(!forge.is_running());
}

/// Background loops (collector/reflector/cleanup) fire once their interval
/// elapses. Real time with 1s intervals: the collector flush tick must write
/// aggregates.jsonl (observable proof the loop body ran), the reflection and
/// cleanup ticks run with no data (early returns).
#[tokio::test]
async fn test_s8_background_loops_fire_intervals() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.collection.flush_interval_secs = 1;
    config.reflection.interval_secs = 1;
    config.storage.cleanup_interval_secs = 1;
    let forge = Arc::new(Forge::new(config, dir.path().to_path_buf()));

    // Seed one experience pattern so the collector flush tick has observable
    // output (aggregates.jsonl under forge/experiences).
    forge
        .collector()
        .record(nemesis_types::forge::Experience {
            id: "s8-bg".into(),
            tool_name: "s8_tool".into(),
            input_summary: "in".into(),
            output_summary: "out".into(),
            success: true,
            duration_ms: 10,
            timestamp: chrono::Local::now().to_rfc3339(),
            session_key: "s8".into(),
        })
        .await;

    Arc::clone(&forge).start().await;
    assert!(forge.is_running());

    // Let all three loops tick at least once (interval = 1s).
    tokio::time::sleep(std::time::Duration::from_millis(1800)).await;

    forge.stop().await;
    assert!(!forge.is_running());

    let aggregates = dir
        .path()
        .join("forge")
        .join("experiences")
        .join("aggregates.jsonl");
    assert!(
        aggregates.exists(),
        "collector loop tick should have flushed aggregates.jsonl"
    );
}

/// run_reflection_cycle cluster-share happy path: reflector writes a report,
/// syncer (NoOpBridge) shares it successfully.
#[tokio::test]
async fn test_s8_reflection_cycle_cluster_share_ok() {
    let dir = tempfile::tempdir().unwrap();
    let forge = s8_forge_with_syncer(
        dir.path(),
        Arc::new(crate::bridge::NoOpBridge::new("s8-ok".into())),
    )
    .await;

    let _g = crate::test_support::quiet_trace_guard();
    forge.run_reflection_cycle().await;

    // A report must exist and the cycle must not panic.
    let reflections_dir = dir.path().join("forge").join("reflections");
    let count = std::fs::read_dir(&reflections_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
        .count();
    assert!(count > 0, "expected a reflection report to be written");
}

/// run_reflection_cycle cluster-share failure path: a bridge that errors on
/// share triggers the warn branch (no panic, report still written).
#[tokio::test]
async fn test_s8_reflection_cycle_cluster_share_err() {
    let dir = tempfile::tempdir().unwrap();
    let forge = s8_forge_with_syncer(dir.path(), Arc::new(S8FailingBridge)).await;

    let _g = crate::test_support::quiet_trace_guard();
    forge.run_reflection_cycle().await;

    let reflections_dir = dir.path().join("forge").join("reflections");
    let count = std::fs::read_dir(&reflections_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
        .count();
    assert!(count > 0, "report written even if cluster share fails");
}

/// run_reflection_cycle write_report failure: reflections dir is a regular
/// file -> write_report errs -> share step skipped, no panic.
#[tokio::test]
async fn test_s8_reflection_cycle_write_report_err() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    std::fs::create_dir_all(dir.path().join("forge")).unwrap();
    let ref_dir = dir.path().join("forge").join("reflections");
    std::fs::write(&ref_dir, "not a directory").unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(ref_dir));

    let forge = Arc::new(forge);
    write_test_experiences(forge.forge_dir(), 3).await;

    let _g = crate::test_support::quiet_trace_guard();
    forge.run_reflection_cycle().await; // must not panic
}

/// Cleanup cycle with: experience store base being a FILE (read_dir Err),
/// no reflector, no cycle store — exercises all three early/Err regions.
#[tokio::test]
async fn test_s8_cleanup_cycle_store_err_no_reflector_no_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    // Collector::new pre-creates forge/experiences as a directory — replace
    // it with a regular file so read_dir errs (covers the cleanup Err region).
    std::fs::create_dir_all(dir.path().join("forge")).unwrap();
    std::fs::remove_dir_all(dir.path().join("forge").join("experiences")).unwrap();
    std::fs::write(dir.path().join("forge").join("experiences"), "file").unwrap();

    let forge = Arc::new(forge);
    let _g = crate::test_support::quiet_trace_guard();
    forge.run_cleanup_cycle().await; // must not panic
}

/// Cleanup cycle removing an old learning-cycle file (cycle_store branch,
/// removed > 0).
#[tokio::test]
async fn test_s8_cleanup_cycle_removes_old_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    let old_dir = dir.path().join("forge").join("learning").join("202001");
    std::fs::create_dir_all(&old_dir).unwrap();
    let old_file = old_dir.join("20200101.jsonl");
    std::fs::write(&old_file, "{}\n").unwrap();

    let _g = crate::test_support::quiet_trace_guard();
    forge.run_cleanup_cycle().await;

    assert!(
        !old_file.exists(),
        "old learning cycle file should be removed"
    );
}

/// Cleanup cycle where the cycle-store base dir is a FILE -> cleanup errs.
#[tokio::test]
async fn test_s8_cleanup_cycle_store_base_is_file() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());

    std::fs::write(dir.path().join("forge").join("learning"), "file").unwrap();

    let _g = crate::test_support::quiet_trace_guard();
    forge.run_cleanup_cycle().await; // must not panic
}

/// started_at lifecycle + runtime learning toggle.
#[tokio::test]
async fn test_s8_started_at_and_learning_toggle() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));

    assert!(forge.started_at().is_none());
    Arc::clone(&forge).start().await;
    let ts = forge.started_at().expect("started_at set after start");
    assert!(!ts.is_empty());

    let _g = crate::test_support::quiet_trace_guard();
    forge.set_learning_enabled(true);
    assert!(forge.is_learning_enabled());
    forge.set_learning_enabled(false);
    assert!(!forge.is_learning_enabled());

    forge.stop().await;
    assert!(forge.started_at().is_none());
}

/// init_learning with a pipeline already configured -> engine gets the
/// pipeline injected (the set_pipeline branch).
#[test]
fn test_s8_init_learning_with_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    let registry = Arc::new(Registry::new(RegistryConfig::default()));
    forge.init_pipeline(Arc::new(Pipeline::new(
        ForgeConfig::default(),
        registry.clone(),
    )));

    let engine = LearningEngine::new(
        ForgeConfig::default(),
        registry.clone(),
        CycleStore::from_base(dir.path()),
    );
    let monitor = Arc::new(DeploymentMonitor::new(ForgeConfig::default(), registry));
    let store = CycleStore::from_base(dir.path().join("cycles"));
    forge.init_learning(engine, monitor, store);

    assert!(forge.learning_engine().is_some());
}

/// reflect_now happy path with a subscriber installed (field expressions).
#[test]
fn test_s8_reflect_now_with_reflector_tracing() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    let ref_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&ref_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(ref_dir));

    let _g = crate::test_support::quiet_trace_guard();
    let exps = vec![make_collected_experience("s8_tool", true, 42)];
    let result = forge.reflect_now(&exps);
    assert!(result.is_ok());
}

/// receive_reflection with a syncer: valid payload stores the remote report,
/// missing content errors out.
#[test]
fn test_s8_receive_reflection_with_syncer() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.set_bridge(Arc::new(crate::bridge::NoOpBridge::new("s8-rx".into())));
    forge.init_syncer();

    let _g = crate::test_support::quiet_trace_guard();

    // Ok path: content + filename
    let ok = forge.receive_reflection(&serde_json::json!({
        "content": "# remote insight\nsome text",
        "filename": "s8_remote.md",
        "from": "peer-1",
    }));
    assert!(ok.is_ok(), "valid payload should be accepted: {:?}", ok);
    let stored = dir
        .path()
        .join("forge")
        .join("reflections")
        .join("remote")
        .join("peer-1_s8_remote.md");
    assert!(stored.exists(), "remote report file should be written");

    // Err path: missing content
    let err = forge
        .receive_reflection(&serde_json::json!({"filename": "x.md"}))
        .unwrap_err();
    assert!(err.contains("content"), "unexpected error: {}", err);
}

/// create_skill with auto_validate enabled and a pipeline initialized —
/// the validation/status-update/registry-get chain runs.
#[test]
fn test_s8_create_skill_auto_validate_with_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.validation.auto_validate = true;
    let mut forge = Forge::new(config.clone(), dir.path().to_path_buf());

    forge.init_pipeline(Arc::new(Pipeline::new(
        config,
        Arc::new(Registry::new(RegistryConfig::default())),
    )));

    let content =
        "# S8 Skill\nLong enough body so the functional validation passes its length check.\n";
    let artifact = forge
        .create_skill(
            "s8_auto_validate",
            content,
            "s8 test skill",
            vec!["read".into()],
        )
        .expect("create_skill with pipeline should succeed");
    assert!(artifact.id.starts_with("skill-"));
    assert_eq!(artifact.name, "s8_auto_validate");
}

/// cleanup_prompt_suggestions when workspace/prompts exists but is a regular
/// file (read_dir errs -> early return, no panic).
#[test]
fn test_s8_cleanup_prompt_suggestions_prompts_is_file() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    std::fs::write(dir.path().join("prompts"), "not a directory").unwrap();

    forge.cleanup_prompt_suggestions(30); // must not panic
}

/// SkillCreator trait delegation: the trait method forwards to
/// Forge::create_skill.
#[test]
fn test_s8_skill_creator_trait_delegation() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    let creator: &dyn SkillCreator = &forge;
    let artifact = creator
        .create_skill(
            "s8_trait_skill",
            "# Trait delegation\nLong enough body content for the validator.\n",
            "trait delegation test",
            vec![],
        )
        .expect("SkillCreator::create_skill should delegate successfully");
    assert_eq!(artifact.name, "s8_trait_skill");
    assert!(
        dir.path()
            .join("forge")
            .join("skills")
            .join("s8_trait_skill")
            .join("SKILL.md")
            .exists()
    );
}

/// Background loops with interval = 2s: the first 1s tick must take the
/// "elapsed < interval" branch of each loop (the else-arms of the
/// `elapsed >= interval` checks) before the second tick fires the action.
/// Observable proof the loops are alive: aggregates.jsonl appears once the
/// flush interval elapses.
#[tokio::test]
async fn test_s8_background_loops_interval_ladder() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.collection.flush_interval_secs = 2;
    config.reflection.interval_secs = 2;
    config.storage.cleanup_interval_secs = 2;
    let forge = Arc::new(Forge::new(config, dir.path().to_path_buf()));

    forge
        .collector()
        .record(nemesis_types::forge::Experience {
            id: "s8-ladder".into(),
            tool_name: "s8_ladder_tool".into(),
            input_summary: "in".into(),
            output_summary: "out".into(),
            success: true,
            duration_ms: 5,
            timestamp: chrono::Local::now().to_rfc3339(),
            session_key: "s8".into(),
        })
        .await;

    Arc::clone(&forge).start().await;
    // Tick 1 (~1s): all three loops take the not-yet branch.
    // Tick 2 (~2s): flush/reflection/cleanup fire.
    tokio::time::sleep(std::time::Duration::from_millis(2800)).await;
    forge.stop().await;
    assert!(!forge.is_running());
    assert!(
        dir.path()
            .join("forge")
            .join("experiences")
            .join("aggregates.jsonl")
            .exists(),
        "collector flush tick should have written aggregates.jsonl"
    );
}
