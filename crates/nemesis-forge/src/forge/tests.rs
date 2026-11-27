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
    // F-: Forge::stop() must cleanly signal + flush + join background tasks
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
    // F-: a "---" horizontal rule NOT at content start must not be treated
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
    // F-: is_enabled() is the runtime gate the per-tool-call recording path
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
    // F- regression: a skill with dangerous commands must be rejected BEFORE
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
    assert!(forge.trace_store().is_none());

    let store = crate::trace_store::TraceStore::new(dir.path().join("traces.jsonl"));
    forge.init_trace(store);

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

/// Session-parameterized variant（B1 缺陷整改批次：检测器改会话级——
/// pattern 检测测试需要跨会话重复同形链才能过 min_freq 闸）。
fn make_session_experience(
    session: &str,
    tool: &str,
    success: bool,
    dur: u64,
) -> crate::types::CollectedExperience {
    crate::types::CollectedExperience {
        dedup_hash: format!("test-{}-{}-{}", session, tool, success),
        experience: nemesis_types::forge::Experience {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool.to_string(),
            input_summary: "test input".into(),
            output_summary: if success { "ok" } else { "error" }.into(),
            success,
            duration_ms: dur,
            timestamp: chrono::Local::now().to_rfc3339(),
            session_key: session.to_string(),
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

/// 语义反思接入：run_reflection_cycle 带 LLM provider 时，
/// 报告落盘「LLM Insights」节（活渲染链 = Reflector::format_report_markdown，
/// 反思 insights 非空即渲染；注意不是 report.rs 里只有测试消费的
/// format_report_from_report 的「LLM Deep Analysis」头）。
#[tokio::test]
async fn test_reflection_cycle_writes_llm_deep_analysis_section() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());
    forge.set_provider(std::sync::Arc::new(MockLLMCaller));

    write_test_experiences(forge.forge_dir(), 5).await;
    forge.run_reflection_cycle().await;

    let reflections_dir = dir.path().join("forge").join("reflections");
    // read_dir 顺序是平台实现细节（NTFS/ext4 不同）；本测试只取首个报告，
    // 显式排序消除平台差（多报告场景下断言稳定）。
    let mut reports: Vec<std::path::PathBuf> = std::fs::read_dir(&reflections_dir)
        .expect("reflections dir must exist after cycle")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .map(|e| e.path())
        .collect();
    reports.sort();
    assert!(!reports.is_empty(), "expected at least one report file");
    let content = std::fs::read_to_string(&reports[0]).unwrap();
    assert!(
        content.contains("## LLM Insights"),
        "report must carry the LLM Insights section; got:\n{content}"
    );
    assert!(
        content.contains("mock"),
        "insights body must be rendered into the report"
    );
}

/// A4+A5：报告 Period 行反映真实数据窗口（不再硬编码 "today"），
/// 头部带学习闭环开关状态行（审计教训：关着的闭环在报告里不可见，
/// 读者会误以为循环在跑）。
#[tokio::test]
async fn test_reflection_cycle_period_and_learning_state_line() {
    let dir = tempfile::tempdir().unwrap();
    let forge = create_integration_forge(dir.path());
    forge.set_learning_enabled(true);

    write_test_experiences(forge.forge_dir(), 5).await;
    forge.run_reflection_cycle().await;

    let reflections_dir = dir.path().join("forge").join("reflections");
    // read_dir 顺序是平台实现细节；单报告场景取排序首个即稳定。
    let mut reports: Vec<std::path::PathBuf> = std::fs::read_dir(&reflections_dir)
        .expect("reflections dir must exist after cycle")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .map(|e| e.path())
        .collect();
    reports.sort();
    assert!(!reports.is_empty(), "expected at least one report file");
    let content = std::fs::read_to_string(&reports[0]).unwrap();
    // A4：Period 行 = 真实条数 + 首末时间戳（RFC3339 前 19 字符）。
    assert!(
        content.contains("**Period:** 最近 5 条"),
        "Period 行应反映真实数据窗口; got:\n{content}"
    );
    assert!(
        !content.contains("Period: today") && !content.contains("**Period:** today"),
        "旧硬编码 period 不得再出现"
    );
    // A5：头部学习闭环状态行（显式开启 → enabled）。
    assert!(
        content.contains("**学习闭环:** enabled"),
        "报告头部应有学习闭环状态行且为 enabled; got:\n{content}"
    );
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

#[tokio::test]
async fn test_cleanup_cycle_removes_old_traces() {
    // 验证轮二回归锁：run_cleanup_cycle 必须按
    // trace.max_trace_age_days 滚动清理 traces JSONL（接线后盘上
    // 只增不删的缺口）。旧事件（2020）删、新事件（now）留。
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    // Wire the trace store exactly like the gateway assembly does.
    let store = crate::trace_store::TraceStore::new(forge.forge_dir().join("traces"));
    forge.init_trace(store);

    let store = crate::trace_store::TraceStore::new(forge.forge_dir().join("traces"));
    let old = crate::trace::TraceEvent {
        id: "t-old".into(),
        event_type: "tool_call".into(),
        session_key: "web:1".into(),
        timestamp: "2020-01-01T00:00:00Z".into(),
        data: serde_json::json!({"tool": "file_read", "ok": true}),
    };
    let recent = crate::trace::TraceEvent {
        id: "t-new".into(),
        event_type: "tool_call".into(),
        session_key: "web:2".into(),
        timestamp: chrono::Local::now().to_rfc3339(),
        data: serde_json::json!({"tool": "file_read", "ok": true}),
    };
    store.append(&old).await.unwrap();
    store.append(&recent).await.unwrap();

    forge.run_cleanup_cycle().await;

    let after = store.read_all().await.unwrap();
    assert_eq!(after.len(), 1, "old trace event should be removed");
    assert_eq!(after[0].id, "t-new", "recent trace event should survive");
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

    // B1 会话级语义：5 个会话重复同形 2 步链（min_pattern_frequency=5）。
    // 链频按跨会话聚合，单会话只数 1。
    let mut experiences = vec![];
    for s in 0..5 {
        experiences.push(make_session_experience(
            &format!("sess-{}", s),
            "file_read",
            true,
            100,
        ));
        experiences.push(make_session_experience(
            &format!("sess-{}", s),
            "file_write",
            true,
            100,
        ));
    }

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

    // B1 会话级语义：恢复链 = 失败工具 → （同会话内）不同工具成功。
    // 5 个会话重复 web_fetch 失败 → web_search 成功（freq 5 = min_freq）。
    let mut experiences = vec![];
    for s in 0..5 {
        experiences.push(make_session_experience(
            &format!("sess-{}", s),
            "web_fetch",
            false,
            500,
        ));
        experiences.push(make_session_experience(
            &format!("sess-{}", s),
            "web_search",
            true,
            200,
        ));
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

    // B1 会话级语义：慢会话（avg_dur > 2× 全局均值）× min_freq 重复。
    // 9 个快会话（2 步 @100ms）压低全局均值；5 个慢会话（同形 2 步链
    // @100000ms）聚合到 count=5 过 min_freq=5 闸。慢/全局 = 100000/35779
    // ≈ 2.8× > 2× ✓（快会话过半时慢会话会拉爆全局均值，必须保持快多慢少）。
    let mut experiences = vec![];
    for s in 0..9 {
        experiences.push(make_session_experience(
            &format!("fast-{}", s),
            "file_read",
            true,
            100,
        ));
        experiences.push(make_session_experience(
            &format!("fast-{}", s),
            "file_write",
            true,
            100,
        ));
    }
    for s in 0..5 {
        experiences.push(make_session_experience(
            &format!("slow-{}", s),
            "shell_exec",
            true,
            100000,
        ));
        experiences.push(make_session_experience(
            &format!("slow-{}", s),
            "git",
            true,
            100000,
        ));
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

    // B1 会话级语义：全成功会话的同形链跨会话重复 ≥ min_freq。
    let mut experiences = vec![];
    for s in 0..5 {
        experiences.push(make_session_experience(
            &format!("sess-{}", s),
            "file_read",
            true,
            100,
        ));
        experiences.push(make_session_experience(
            &format!("sess-{}", s),
            "file_write",
            true,
            100,
        ));
    }

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
    // （会话级语义：6 个会话各跑同形两步全成功链 → freq 6 ≥ 5）
    let experiences: Vec<_> = (0..6)
        .flat_map(|i| {
            vec![
                make_session_experience(&format!("s{i}"), "file_read", true, 100),
                make_session_experience(&format!("s{i}"), "file_write", true, 100),
            ]
        })
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
    let report = reflector.reflect(&experiences, &[], None, "today", "all", &[], &[]);
    // Should have stats even without LLM
    assert!(!report.date.is_empty());
    assert!(report.stats.total_records > 0);
    assert!(
        report.llm_insights.is_none(),
        "no LLM insights expected without LLM provider"
    );
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
// coverage batch (quality-hardening goal 冲刺)
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

/// Background loops (reflector/cleanup) fire once their interval elapses.
/// Real time with 1s intervals: the reflection tick must write a report
/// (observable proof the loop body ran); the cleanup tick runs with no data
/// (early return).（collector_loop 已随聚合线移除，——可观测信号
/// 从 aggregates.jsonl 换成反思报告。）
#[tokio::test]
async fn test_s8_background_loops_fire_intervals() {
    let _g = crate::test_support::quiet_trace_guard();
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.enabled = true; // 总闸开（C3 热生效语义后 cycle 前看闸，关=空转待命）
    config.reflection.interval_secs = 1;
    config.storage.cleanup_interval_secs = 1;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir.clone()));
    let forge = Arc::new(forge);

    // Seed experiences so the reflection tick has observable output (a report
    // under forge/reflections; s8 系列已证明 3 条经验即可写报告).
    write_test_experiences(forge.forge_dir(), 5).await;

    Arc::clone(&forge).start().await;
    assert!(forge.is_running());

    // Let both loops tick at least once (interval = 1s).
    tokio::time::sleep(std::time::Duration::from_millis(1800)).await;

    forge.stop().await;
    assert!(!forge.is_running());

    let reports = std::fs::read_dir(&reflections_dir)
        .map(|d| d.count())
        .unwrap_or(0);
    assert!(
        reports > 0,
        "reflection loop tick should have written a report"
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
            // 入口 slugify 是防穿越纵深的一部分——`_` 归一为 `-`，
            // 注册名与落盘目录都用归一后的 slug。
            "s8_auto_validate",
            content,
            "s8 test skill",
            vec!["read".into()],
        )
        .expect("create_skill with pipeline should succeed");
    assert!(artifact.id.starts_with("skill-"));
    assert_eq!(artifact.name, "s8-auto-validate");
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

/// 反思输入并集（BUG 批次 2026-11-14）：merged_artifacts_for_reflection
/// = 本库 ∪ 闭环库，按 id 去重——闭环侧产物对反思/蒸馏 LLM 可见（此前
/// 互盲，重复提名风险），同 id 双库在场时只计一次。
#[test]
fn test_merged_artifacts_for_reflection_unions_learning_registry() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    let now = chrono::Local::now().to_rfc3339();
    let make_artifact = |id: &str, name: &str| nemesis_types::forge::Artifact {
        id: id.to_string(),
        name: name.to_string(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".to_string(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "c".to_string(),
        tool_signature: vec![],
        created_at: now.clone(),
        updated_at: now.clone(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };

    // 本库：manual-a；闭环库：learning-b + 与本库同 id 的 dup-id。
    forge
        .registry()
        .add(make_artifact("skill-manual-a", "manual-a"));
    let learning = vec![
        make_artifact("skill-learning-b", "learning-b"),
        make_artifact("skill-manual-a", "dup-id"),
    ];
    let learning_path = dir.path().join("forge").join(crate::LEARNING_REGISTRY_FILE);
    std::fs::write(&learning_path, serde_json::to_string(&learning).unwrap()).unwrap();

    let merged = forge.merged_artifacts_for_reflection();
    let ids: Vec<&str> = merged.iter().map(|a| a.id.as_str()).collect();
    assert!(
        ids.contains(&"skill-manual-a") && ids.contains(&"skill-learning-b"),
        "并集必须同时含本库与闭环库产物，got {:?}",
        ids
    );
    assert_eq!(merged.len(), 2, "同 id 双库在场只计一次（去重）");
}

/// `merge_artifacts_by_id` 自由函数契约（BUG 批次 2026-11-15）：并集 +
/// id 去重 primary 优先 + 空 secondary 恒等。web 手动反思 handler 无
/// Forge 实例，直接消费本函数——契约在此钉死，两入口不再分叉。
#[test]
fn test_merge_artifacts_by_id_union_dedup_primary_wins() {
    let now = chrono::Local::now().to_rfc3339();
    let make_artifact = |id: &str, name: &str| nemesis_types::forge::Artifact {
        id: id.to_string(),
        name: name.to_string(),
        kind: nemesis_types::forge::ArtifactKind::Skill,
        version: "1.0".to_string(),
        status: nemesis_types::forge::ArtifactStatus::Active,
        content: "c".to_string(),
        tool_signature: vec![],
        created_at: now.clone(),
        updated_at: now.clone(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
        deployed_at: None,
        last_efficacy: None,
    };

    // 并集：primary 2 条 + secondary 独有 1 条 → 3 条。
    let primary = vec![make_artifact("a", "a"), make_artifact("b", "b")];
    let secondary = vec![make_artifact("c", "c")];
    let merged = crate::forge::merge_artifacts_by_id(primary, secondary);
    assert_eq!(merged.len(), 3);
    let ids: Vec<&str> = merged.iter().map(|a| a.id.as_str()).collect();
    assert!(ids.contains(&"a") && ids.contains(&"b") && ids.contains(&"c"));

    // id 冲突：primary 侧条目保留（name 不被 secondary 覆盖）。
    let primary = vec![make_artifact("dup", "manual-name")];
    let secondary = vec![make_artifact("dup", "learning-name")];
    let merged = crate::forge::merge_artifacts_by_id(primary, secondary);
    assert_eq!(merged.len(), 1, "同 id 只计一次");
    assert_eq!(merged[0].name, "manual-name", "primary 优先");

    // 空 secondary：恒等于 primary（无扩容无重排）。
    let primary = vec![make_artifact("a", "a")];
    let merged = crate::forge::merge_artifacts_by_id(primary, Vec::new());
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "a");

    // BUG-20261125-054 回归锁：secondary 内部互相重复的 id 也只计一次
    // （盘上注册表手工编辑/损坏可现双 id 行）。
    let primary = vec![make_artifact("a", "a")];
    let secondary = vec![
        make_artifact("dup", "first"),
        make_artifact("dup", "second"),
    ];
    let merged = crate::forge::merge_artifacts_by_id(primary, secondary);
    assert_eq!(merged.len(), 2, "secondary 内部重复 id 只保留首个");
    assert_eq!(merged[1].name, "first");
}

/// Background loops with interval = 2s: the first 1s tick must take the
/// "elapsed < interval" branch of each loop (the else-arms of the
/// `elapsed >= interval` checks) before the second tick fires the action.
/// Observable proof the ladder semantics hold: no report before the interval
/// elapses, a report after.（collector_loop 已随聚合线移除，。）
#[tokio::test]
async fn test_s8_background_loops_interval_ladder() {
    let _g = crate::test_support::quiet_trace_guard();
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.enabled = true; // 总闸开（C3 热生效语义后 cycle 前看闸，关=空转待命）
    config.reflection.interval_secs = 2;
    config.storage.cleanup_interval_secs = 2;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir.clone()));
    let forge = Arc::new(forge);

    write_test_experiences(forge.forge_dir(), 5).await;

    let report_count = || -> usize {
        std::fs::read_dir(&reflections_dir)
            .map(|d| d.count())
            .unwrap_or(0)
    };

    Arc::clone(&forge).start().await;
    // Tick 1 (~1s): all loops take the not-yet branch — no report yet.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert_eq!(
        report_count(),
        0,
        "reflection must not fire before its interval"
    );
    // Tick 2 (~2s total): the reflection tick fires.
    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
    forge.stop().await;
    assert!(!forge.is_running());
    assert!(
        report_count() > 0,
        "reflection tick should have written a report after its interval"
    );
}

// =========================================================================
// slug 卫生函数 + 学习对话接线（自学习铺路批次三 cycle Step 5）
// =========================================================================

#[test]
fn test_slugify_skill_name_normalization() {
    assert_eq!(
        slugify_skill_name("Install MCP Server (HTTP)"),
        "install-mcp-server-http"
    );
    // 连续分隔符压缩 + 首尾剥离
    assert_eq!(slugify_skill_name("__Weird--Name__"), "weird-name");
    // 全非法字符（中文）→ 空串，调用方跳过
    assert_eq!(slugify_skill_name("中文技能名"), "");
    assert_eq!(slugify_skill_name(""), "");
    // 48 字符截断
    assert_eq!(slugify_skill_name(&"a".repeat(60)).len(), 48);
}

#[test]
fn test_slugify_skill_name_traversal_forms() {
    // 毒名形态钉（resolve_artifact_path / exporter 两族修复的共同地基）：
    // 穿越序列经 slug 全部塌缩为安全单段或空串（空串由调用方回落
    // `_unnamed_` 占位，不得把穿越序列交还给 join）
    assert_eq!(slugify_skill_name("../../evil"), "evil");
    assert_eq!(slugify_skill_name("../escape"), "escape");
    assert_eq!(slugify_skill_name("../.."), "");
    assert_eq!(slugify_skill_name(".."), "");
    assert_eq!(slugify_skill_name("a/../../b"), "a-b");
    // 幂等：slug 产物再 slug 恒等（exporter/export_artifact 链的二次
    // 解析依赖该性质）
    let once = slugify_skill_name("Install MCP Server (HTTP)");
    assert_eq!(slugify_skill_name(&once), once);
}

#[test]
fn test_slug_or_placeholder_rule() {
    use super::slug_or_placeholder;
    // 单一真相源钉（exporter 写面 / forge_tools resolve_artifact_path /
    // learning_engine disable 面三族共用）：合法名 slug 恒等直通；空 slug
    // （全非法/纯符号毒名）回落受控占位段——调用方拿到的永远是安全单段。
    assert_eq!(slug_or_placeholder("my-skill"), "my-skill");
    assert_eq!(
        slug_or_placeholder("Install MCP Server (HTTP)"),
        "install-mcp-server-http"
    );
    assert_eq!(slug_or_placeholder("../../evil"), "evil");
    assert_eq!(slug_or_placeholder(""), "_unnamed_");
    assert_eq!(slug_or_placeholder(".."), "_unnamed_");
    assert_eq!(slug_or_placeholder("../.."), "_unnamed_");
}

#[test]
fn test_valid_skill_dir_name_fence() {
    use super::valid_skill_dir_name;
    // 合法域：slugify 产物形态放行。
    assert!(valid_skill_dir_name("install-http-mcp").is_ok());
    assert!(valid_skill_dir_name("a-b_c1").is_ok());
    // 穿越：`..` 形态直拼 join 出 workspace。
    assert!(valid_skill_dir_name("../evil").is_err());
    assert!(valid_skill_dir_name("..\\evil").is_err());
    assert!(valid_skill_dir_name("..").is_err());
    assert!(valid_skill_dir_name(".").is_err());
    assert!(valid_skill_dir_name("").is_err());
    // 分隔符（单段目录名要求）。
    assert!(valid_skill_dir_name("a/b").is_err());
    assert!(valid_skill_dir_name("a\\b").is_err());
    // Windows 盘符相对形态：is_absolute()==false 且无分隔符，但
    // Path::join 整段换基座（新增的拒绝面）。
    assert!(valid_skill_dir_name("C:evil").is_err());
    assert!(valid_skill_dir_name("c:skills").is_err());
    // Win32 归一化族：尾随点/空格在 Win32 路径解析中被剥——`.. ` ≡
    // `..`（父目录穿越等价形态）；首尾空白 trim 后可能落回 `.`/`..`。
    assert!(valid_skill_dir_name(".. ").is_err());
    assert!(valid_skill_dir_name(".. .").is_err());
    assert!(valid_skill_dir_name(". ").is_err());
    assert!(valid_skill_dir_name("a.").is_err());
    assert!(valid_skill_dir_name("a. ").is_err());
    assert!(valid_skill_dir_name(" a").is_err());
    // 中间点无归一化等价形态，仍放行（克制修复：只封穿越/歧义族）。
    assert!(valid_skill_dir_name("a.b").is_ok());
}

// BUG-20261123-035 回归锁：Windows 保留设备名族一律拒绝——裸名与带扩展名
// 形态（判定取 base，旧 Windows 全设备解析 / 新 Win11 裸名仍解析）；非保留
// 近邻（子串/超集/带前缀）是合法 slug 不误伤。
#[test]
fn test_valid_skill_dir_name_rejects_reserved_device_names() {
    use super::valid_skill_dir_name;
    // 22 名单抽样：裸名、大小写、带扩展名 base 形态
    for n in [
        "nul",
        "con",
        "aux",
        "prn",
        "com1",
        "com5",
        "lpt1",
        "lpt9",
        "NUL",
        "Con",
        "nul.md",
        "con.txt",
        "com1.skill",
    ] {
        assert!(
            valid_skill_dir_name(n).is_err(),
            "reserved device name must be rejected: {n}"
        );
    }
    // 近邻不误伤
    assert!(valid_skill_dir_name("null").is_ok());
    assert!(valid_skill_dir_name("nulx").is_ok());
    assert!(valid_skill_dir_name("console").is_ok());
    assert!(valid_skill_dir_name("auxiliary").is_ok());
    assert!(valid_skill_dir_name("nul-forge").is_ok());
    assert!(valid_skill_dir_name("1.0").is_ok()); // 版本段消费方基底恒不命中
}

// BUG-20261123-035 回归锁：create_skill 入口保留名诚实拒绝（此前 slugify
// 恒等放行 → Windows cryptic OS error / Linux 落盘跨节点死档）。
#[test]
fn test_create_skill_rejects_reserved_device_names() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    for n in ["nul", "con", "com1"] {
        let res = forge.create_skill(n, "content", "desc", vec![]);
        assert!(res.is_err(), "reserved device name must be rejected: {n}");
        // 围栏拒绝（明确信息），不是 create_dir 的 cryptic OS error
        assert!(
            res.unwrap_err().contains("invalid skill name"),
            "rejection must come from the name fence"
        );
    }
}

// （旧蒸馏 test_distill_and_store_draft_success_and_dedup 随线 B 蒸馏退役
// 删除——2026-11-24 批次三生成面改道学习对话，蒸馏函数已不存在。）

// ── 学习对话接线（自学习铺路批次三）──────────────────────────────────

/// 学习对话接线测试：脚本 caller 按序弹出回复（耗尽回落「没有值得学的」
/// 安全形态）。
struct LearnScriptMockLLM {
    steps: std::sync::Mutex<std::collections::VecDeque<String>>,
}

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for LearnScriptMockLLM {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Ok(self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| r#"{"proposals":[],"empty_reason":"脚本耗尽"}"#.to_string()))
    }
}

fn learn_nothing_script() -> LearnScriptMockLLM {
    LearnScriptMockLLM {
        steps: std::sync::Mutex::new(
            vec![r#"{"proposals":[],"empty_reason":"候选全是正常咨询"}"#.to_string()].into(),
        ),
    }
}

#[tokio::test]
async fn test_learn_dialogue_manual_no_caller_is_err() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    let err = forge.run_learn_dialogue("manual").await.unwrap_err();
    assert!(err.contains("未配置"), "got: {err}");
    assert!(
        !forge.forge_dir().join("learn").exists(),
        "caller 缺席时零留痕"
    );
}

#[tokio::test]
async fn test_learn_dialogue_manual_estop_is_err() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.set_learn_caller(Arc::new(learn_nothing_script()));
    forge.set_learn_estop_check(Arc::new(|| true));
    let err = forge.run_learn_dialogue("manual").await.unwrap_err();
    assert!(err.contains("急停"), "got: {err}");
    assert!(
        !forge.forge_dir().join("learn").exists(),
        "estop 入口即拒时零留痕"
    );
}

#[tokio::test]
async fn test_learn_dialogue_manual_nothing_worth_learning_writes_trace() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.set_learn_caller(Arc::new(learn_nothing_script()));
    forge.set_learn_estop_check(Arc::new(|| false));
    let out = forge.run_learn_dialogue("manual").await.unwrap();
    assert_eq!(out.phases_completed, 1, "空提议后对话即时收场");
    assert_eq!(out.empty_reason.as_deref(), Some("候选全是正常咨询"));

    // 留痕：learn/<ts>_manual/ 目录带 outcome.json + transcript.md。
    let learn_dir = forge.forge_dir().join("learn");
    let entries: Vec<_> = std::fs::read_dir(&learn_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "恰好一个 trace 目录");
    let trace_dir = entries[0].as_ref().unwrap().path();
    assert!(
        trace_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("_manual"),
        "触发来源进目录名：{:?}",
        trace_dir
    );
    assert!(trace_dir.join("outcome.json").exists());
    assert!(trace_dir.join("transcript.md").exists());
}

#[tokio::test]
async fn test_learn_dialogue_cycle_gated_by_learn_enabled() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    // learn.enabled=false（默认）→ cycle Step 5 不跑对话（零留痕），
    // 即使 caller 在场——交付后行为零变化。
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    forge.set_learn_caller(Arc::new(learn_nothing_script()));
    let forge = Arc::new(forge);

    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    assert!(
        !forge.forge_dir().join("learn").exists(),
        "learn.enabled=false must not run the dialogue in the cycle"
    );
}

#[tokio::test]
async fn test_learn_dialogue_cycle_wiring_writes_trace() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    // learn.enabled=true + use_llm=true → cycle Step 5 跑对话（留痕落
    // learn/<ts>_cycle/）。「没有值得学的」empty_reason 收场也走完整路径。
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learn.enabled = true;
    config.reflection.use_llm = true;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    forge.set_provider(Arc::new(learn_nothing_script()));
    let forge = Arc::new(forge);

    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    let learn_dir = forge.forge_dir().join("learn");
    let entries: Vec<_> = std::fs::read_dir(&learn_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "恰好一个 trace 目录");
    let trace_dir = entries[0].as_ref().unwrap().path();
    assert!(
        trace_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("_cycle"),
        "cycle 触发来源进目录名：{:?}",
        trace_dir
    );
    assert!(trace_dir.join("outcome.json").exists());
}

// ── 装前 tests 闸接线（自学习铺路批次四）──────────────────────────────

/// 全过执行器：所有用例回「done」final（与脚本草稿的 contains 断言对齐）。
struct AlwaysPassRunner;

#[async_trait::async_trait]
impl crate::skill_tests::SkillTestRunner for AlwaysPassRunner {
    async fn run_case(
        &self,
        _skill_content: &str,
        _case: &crate::skill_tests::TestCase,
    ) -> Result<crate::skill_tests::ExecutionTranscript, String> {
        Ok(crate::skill_tests::ExecutionTranscript {
            turns_executed: 1,
            duration_secs: 0,
            tools_called: Vec::new(),
            finals: vec!["done".into()],
            errors: Vec::new(),
            files: Vec::new(),
        })
    }
}

/// 四阶段脚本（产出一条 build-recipe skill 草稿，tests 段合法）。
const GATE_P1: &str = r#"{"proposals":[
  {"id":"p1","title":"构建配方","observation":"多次成功序列","lesson":"完整操作步骤"}]}"#;
const GATE_P2: &str = r#"{"items":[{"id":"p1","form":"skill","reason":"完整配方"}]}"#;
const GATE_P3: &str = r#"{"items":[{"id":"p1","verdict":"keep","target":"","note":""}]}"#;
const GATE_P4: &str = r#"{"insights":[],
 "skills":[
  {"id":"p1","slug":"build-recipe","skill_md":"---\nname: build-recipe\ndescription: 构建配方\nversion: \"1.0\"\ntests:\n  - name: t1\n    turns:\n      - user: \"构建\"\n    expect:\n      contains: [\"done\"]\n---\n# 构建配方\n\n1. cargo build"}]}"#;

fn gate_script() -> LearnScriptMockLLM {
    LearnScriptMockLLM {
        steps: std::sync::Mutex::new(
            vec![
                GATE_P1.to_string(),
                GATE_P2.to_string(),
                GATE_P3.to_string(),
                GATE_P4.to_string(),
            ]
            .into(),
        ),
    }
}

#[tokio::test]
async fn test_learn_dialogue_gate_installs_skill_to_workspace() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    // pipeline + 全过执行器（单一注入点 set_skill_test_runner 的前置）。
    let registry = Arc::new(crate::registry::Registry::new(
        crate::types::RegistryConfig::default(),
    ));
    forge.init_pipeline(Arc::new(crate::pipeline::Pipeline::new(
        ForgeConfig::default(),
        registry,
    )));
    forge.set_skill_test_runner(Arc::new(AlwaysPassRunner));
    forge.set_provider(Arc::new(gate_script()));

    let outcome = forge.run_learn_dialogue("manual").await.unwrap();
    assert_eq!(outcome.skills.len(), 1);
    assert_eq!(outcome.gate.len(), 1, "逐稿闸记录必须随 outcome 交付");
    assert!(
        matches!(
            &outcome.gate[0].verdict,
            crate::skill_gate::GateVerdict::Installed
        ),
        "{:?}",
        outcome.gate[0]
    );
    assert_eq!(outcome.gate[0].refine_rounds, 0);
    assert!(outcome.gate[0].slug.ends_with("-forge"));
    let installed = dir
        .path()
        .join("skills")
        .join(&outcome.gate[0].slug)
        .join("SKILL.md");
    let on_disk = std::fs::read_to_string(&installed).unwrap();
    assert!(
        on_disk.contains("build-recipe"),
        "落盘 = 草稿全文：{on_disk}"
    );
}

#[tokio::test]
async fn test_learn_dialogue_gate_without_runner_rejects_fail_closed() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    // pipeline 在场但执行器未注入（agent_factory 不在的装配形态）。
    let registry = Arc::new(crate::registry::Registry::new(
        crate::types::RegistryConfig::default(),
    ));
    forge.init_pipeline(Arc::new(crate::pipeline::Pipeline::new(
        ForgeConfig::default(),
        registry,
    )));
    forge.set_provider(Arc::new(gate_script()));

    let outcome = forge.run_learn_dialogue("manual").await.unwrap();
    assert_eq!(outcome.skills.len(), 1, "草稿照常随 outcome 留痕");
    assert!(
        matches!(
            &outcome.gate[0].verdict,
            crate::skill_gate::GateVerdict::Rejected(r) if r.contains("执行器未装配")
        ),
        "{:?}",
        outcome.gate[0]
    );
    assert!(
        !dir.path()
            .join("skills")
            .join(&outcome.gate[0].slug)
            .join("SKILL.md")
            .exists(),
        "fail-closed：草稿不得落盘"
    );
}

// ---------------------------------------------------------------------------
// use_llm 接线咬合：该键此前只在 CLI 消费，gateway
// 的 run_reflection_cycle 无条件走 reflect_with_llm——设 false 省
// 成本的意图被静默无视。修复后 false = 循环内零 LLM 调用（统计反思照跑、
// 报告照落盘但无 LLM Insights 节；学习对话一并关）。
// ---------------------------------------------------------------------------

/// 计数 mock：每次 LLM 调用 +1，返回 has_skill=false（Step 5 安全形态）。
struct CountingMockLLM(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for CountingMockLLM {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(r#"{"has_skill": false, "name": "", "skill_md": ""}"#.to_string())
    }
}

#[tokio::test]
async fn test_reflection_cycle_use_llm_false_makes_no_llm_calls() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.reflection.use_llm = false;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir.clone()));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // provider 在场（旧实现会无条件调它）——计数必须保持 0
    forge.set_provider(Arc::new(CountingMockLLM(Arc::clone(&calls))));
    let forge = Arc::new(forge);

    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "use_llm=false must make zero LLM calls in the cycle (provider present)"
    );

    // 统计反思照常产出报告，但无 LLM Insights 节（统计分支 llm_insights=None）
    let mut reports: Vec<std::path::PathBuf> = std::fs::read_dir(&reflections_dir)
        .expect("statistical reflection must still write a report")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .map(|e| e.path())
        .collect();
    reports.sort();
    assert!(!reports.is_empty(), "expected at least one report file");
    let content = std::fs::read_to_string(&reports[0]).unwrap();
    assert!(
        !content.contains("## LLM Insights"),
        "use_llm=false report must not carry LLM Insights; got:\n{content}"
    );
}

#[tokio::test]
async fn test_learn_dialogue_skipped_when_use_llm_false() {
    // learn.enabled=true 但 use_llm=false → Step 5 对话关（LLM 工作随语义
    // 分析一并停），即使 caller 在场——零 LLM 调用、零 learn 留痕。
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learn.enabled = true;
    config.reflection.use_llm = false;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    forge.set_provider(Arc::new(CountingMockLLM(Arc::clone(&calls))));
    let forge = Arc::new(forge);

    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "learn dialogue must not invoke the LLM when use_llm=false"
    );
    assert!(
        !forge.forge_dir().join("learn").exists(),
        "use_llm=false must skip Step 5 dialogue entirely (no learn trace dir)"
    );
}

// ── 缝一：ack 责任分流（双缝互通 2026-11-25）──────────────────────────

/// 种 pending 教训入队（bridge 内部分配 id/kind/status），返回落盘实况
/// （真实 id——断言必须用分配后的 id，不能用调用侧占位值）。
fn seed_lessons(
    ws: &std::path::Path,
    contents: &[String],
) -> Vec<crate::userprofile::bridge::LessonEntry> {
    let seed: Vec<crate::userprofile::bridge::LessonEntry> = contents
        .iter()
        .map(|c| crate::userprofile::bridge::LessonEntry {
            id: String::new(),
            ts_ms: 0,
            kind: String::new(),
            category: "workflow".into(),
            content: c.clone(),
            confidence: 0.9,
            evidence: Vec::new(),
            session_count: 0,
            status: String::new(),
        })
        .collect();
    crate::userprofile::bridge::append_lessons(ws, &seed);
    crate::userprofile::bridge::read_pending_lessons(ws, 100)
}

/// 队列状态盘点：(pending, acked)。
fn lesson_status_counts(ws: &std::path::Path) -> (usize, usize) {
    let content = std::fs::read_to_string(crate::userprofile::bridge::outbound_lessons_path(ws))
        .unwrap_or_default();
    let mut pending = 0;
    let mut acked = 0;
    for line in content.lines() {
        if let Ok(l) = serde_json::from_str::<crate::userprofile::bridge::LessonEntry>(line) {
            match l.status.as_str() {
                "pending" => pending += 1,
                "acked" => acked += 1,
                _ => {}
            }
        }
    }
    (pending, acked)
}

#[tokio::test]
async fn test_dialogue_owns_ack_unpacked_lessons_stay_pending() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    // learn.enabled + use_llm → 对话是唯一 ack 责任人：只 ack 实际进包的
    // 条目（节帽 8）；反思循环不 ack——旧口径「读到的全 ack」会让超帽
    // 2 条被静默标记消费（未进包即 ack 的 BUG-1 变体）。
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learn.enabled = true;
    config.reflection.use_llm = true;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    forge.set_provider(Arc::new(learn_nothing_script()));
    let forge = Arc::new(forge);

    let ws = dir.path().to_path_buf();
    let seeded = seed_lessons(
        &ws,
        &(0..10).map(|i| format!("教训{i:02}")).collect::<Vec<_>>(),
    );
    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    let (pending, acked) = lesson_status_counts(&ws);
    assert_eq!((pending, acked), (2, 8), "超包帽 2 条留 pending 下轮重放");
    // pending 的恰是包帽外尾部两条（读全 10 → 装箱前 8）。
    let still_pending = crate::userprofile::bridge::read_pending_lessons(&ws, 100);
    let pending_ids: std::collections::HashSet<&str> =
        still_pending.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(pending_ids.len(), 2);
    assert!(
        seeded[8..]
            .iter()
            .all(|l| pending_ids.contains(l.id.as_str())),
        "留 pending 的 = 第 9、10 条（超帽未进包）"
    );
    assert!(
        seeded[..8]
            .iter()
            .all(|l| !pending_ids.contains(l.id.as_str())),
        "进包 8 条已 ack"
    );
}

#[tokio::test]
async fn test_learn_disabled_reflection_acks_fallback() {
    // learn.enabled=false → 对话不跑，反思循环保持批 D 原行为：报告落盘
    // 成功后 ack 全部读到的教训（兜底，防 pending 永积）。
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.reflection.use_llm = false; // 统计反思即可，无需 provider
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    let forge = Arc::new(forge);

    let ws = dir.path().to_path_buf();
    seed_lessons(&ws, &["教训A".into(), "教训B".into(), "教训C".into()]);
    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    assert!(
        !forge.forge_dir().join("learn").exists(),
        "对话关（零 learn 留痕）"
    );
    assert_eq!(
        lesson_status_counts(&ws),
        (0, 3),
        "反思兜底 ack 全部读到的教训"
    );
}

#[tokio::test]
async fn test_use_llm_false_reflection_acks_fallback() {
    // use_llm=false → 对话随之关（dialogue_owns=false），反思 ack 兜底——
    // 防止 pending 只进不出（对话不跑时无人 ack = 永积）。
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learn.enabled = true;
    config.reflection.use_llm = false;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    let forge = Arc::new(forge);

    let ws = dir.path().to_path_buf();
    seed_lessons(&ws, &["教训A".into(), "教训B".into()]);
    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    assert!(
        !forge.forge_dir().join("learn").exists(),
        "use_llm=false 对话关（零 learn 留痕）"
    );
    assert_eq!(
        lesson_status_counts(&ws),
        (0, 2),
        "反思兜底 ack（dialogue_owns = enabled && use_llm = false）"
    );
}

/// 语义/对话消费分流计数 caller（按 system prompt 归类）：
/// 2026-11-27 语义消费收敛的断言面——深学对话开启时 cycle 的语义分析
/// （SEMANTIC_ANALYSIS_SYSTEM_PROMPT）必须零调用，语义判断只发生在
/// 对话（LEARN_DIALOGUE_SYSTEM_PROMPT）侧。
struct CountingCaller {
    semantic: std::sync::atomic::AtomicUsize,
    dialogue: std::sync::atomic::AtomicUsize,
    other: std::sync::atomic::AtomicUsize,
}

impl CountingCaller {
    fn new() -> Self {
        Self {
            semantic: std::sync::atomic::AtomicUsize::new(0),
            dialogue: std::sync::atomic::AtomicUsize::new(0),
            other: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn semantic(&self) -> usize {
        self.semantic.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn dialogue(&self) -> usize {
        self.dialogue.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn other(&self) -> usize {
        self.other.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl crate::reflector_llm::LLMCaller for CountingCaller {
    async fn chat(
        &self,
        system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        if system_prompt == nemesis_prompts::forge::SEMANTIC_ANALYSIS_SYSTEM_PROMPT {
            self.semantic
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        } else if system_prompt == nemesis_prompts::forge::LEARN_DIALOGUE_SYSTEM_PROMPT {
            self.dialogue
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        } else {
            self.other.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        // 空提议安全形态：对话第一阶段即收束（只调一次）。
        Ok(r#"{"proposals":[],"empty_reason":"计数桩：无候选"}"#.to_string())
    }
}

#[tokio::test]
async fn test_learn_enabled_cycle_skips_semantic_llm_call() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    // learn.enabled + use_llm → 对话是本周期唯一语义消费方：Step 2 落纯
    // 统计反思（语义分析零调用），语义判断只发生在对话侧。回归钉咬合：
    // 摘掉 Step 2 的 !dialogue_owns_semantics 条件 → semantic 计数 ≥1 本
    // 测试必红。
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learn.enabled = true;
    config.reflection.use_llm = true;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    let caller = Arc::new(CountingCaller::new());
    forge.set_provider(caller.clone() as Arc<dyn crate::reflector_llm::LLMCaller>);
    let forge = Arc::new(forge);

    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    assert_eq!(caller.semantic(), 0, "对话开：语义分析必须零调用");
    assert_eq!(
        caller.dialogue(),
        1,
        "对话跑且空提议第一阶段收束（恰一次调用）"
    );
    assert_eq!(caller.other(), 0, "无第三类消费方（pipeline 未装配）");
    assert!(
        forge.forge_dir().join("learn").exists(),
        "对话留痕存在（learn/ 目录）"
    );
}

#[tokio::test]
async fn test_learn_disabled_cycle_keeps_semantic_call() {
    // learn.enabled=false → 对话不跑，Step 2 的语义分析是本周期唯一语义
    // 路径：保持旧行为（use_llm=true 时恰一次语义调用）。语义消费收敛
    // 不得波及 learn 关的部署形态（默认部署零变化）。
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learn.enabled = false;
    config.reflection.use_llm = true;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir));
    let caller = Arc::new(CountingCaller::new());
    forge.set_provider(caller.clone() as Arc<dyn crate::reflector_llm::LLMCaller>);
    let forge = Arc::new(forge);

    write_test_experiences(forge.forge_dir(), 3).await;
    forge.run_reflection_cycle().await;

    assert_eq!(
        caller.semantic(),
        1,
        "对话关：语义分析是唯一语义路径（恰一次调用）"
    );
    assert_eq!(caller.dialogue(), 0, "对话关零对话调用");
    assert!(
        !forge.forge_dir().join("learn").exists(),
        "对话关（零 learn 留痕）"
    );
}

// ---------------------------------------------------------------------------
// 0=关 惯例（复查）
// ---------------------------------------------------------------------------

/// 0=关 惯例（缺陷 22）：reflection.interval_secs=0 → 反思循环不 spawn。
/// 旧实现 `elapsed >= 0` 恒真 = 每秒一轮反思（use_llm=true 时每秒一次 LLM
/// 调用）——本仓 0=关 惯例下用户设 0 的意图是关闭循环。回归钉咬合：摘掉
/// start() 的 interval==0 守卫，本测试必红（~1.2s 首轮反思即写报告）。
#[tokio::test]
async fn test_start_reflect_interval_zero_disables_reflection_loop() {
    let _g = crate::test_support::quiet_trace_guard();
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.reflection.interval_secs = 0; // 0=关
    config.storage.cleanup_interval_secs = 3600;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    // 显式挂带 reflections 目录的 reflector（s8 系列同款夹具）——默认
    // reflector 的报告目录不指向测试目录，报告写失败会让断言失去可观测信号。
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir.clone()));
    let forge = Arc::new(forge);

    // 预写足量经验，确保旧缺陷路径首轮反思必然产出报告文件（可观测信号；
    // s8 系列已证明 3 条经验即可写报告）。
    write_test_experiences(forge.forge_dir(), 12).await;

    Arc::clone(&forge).start().await;
    // 旧缺陷：check_interval 1s，首轮反思在 ~1.0-1.2s 落报告；2.2s 窗口
    // 足以让旧路径必红、新路径（无循环）恒静默。
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    forge.stop().await;

    let reports = std::fs::read_dir(&reflections_dir)
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(
        reports, 0,
        "interval_secs=0 必须关闭反思循环（0=关），实际产出 {reports} 份报告"
    );
}

// ── 总闸热读（2026-11-20 C3 热生效语义）──

/// path 缺席 = 构造期值（旧行为）：config.enabled 镜像直读，零文件 IO。
#[test]
fn master_switch_none_uses_construction_value() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ForgeConfig::default();
    cfg.enabled = false;
    let forge = Forge::new(cfg, dir.path().to_path_buf());
    assert!(!forge.is_enabled());

    let mut cfg2 = ForgeConfig::default();
    cfg2.enabled = true;
    let forge2 = Forge::new(cfg2, dir.path().to_path_buf());
    assert!(forge2.is_enabled());
}

/// path 在场 = mtime 现读：首读磁盘值胜过构造期 mirror；改 config.json
/// 后下次 is_enabled() 即新值（热翻，无需重建 Forge）。
#[test]
fn master_switch_hot_read_flips() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"forge":{"enabled":true}}"#).unwrap();

    let mut cfg = ForgeConfig::default();
    cfg.enabled = false; // 构造期 mirror 与磁盘不一致：热读首读即磁盘值
    let forge = Forge::new(cfg, dir.path().to_path_buf()).with_master_switch_path(cfg_path.clone());
    assert!(forge.is_enabled(), "首读 = 磁盘 forge.enabled=true");

    // 热翻关：写 enabled=false → 下次调用即 false。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&cfg_path, r#"{"forge":{"enabled":false}}"#).unwrap();
    assert!(!forge.is_enabled(), "mtime 变化 → 热翻 false");
    // 缓存命中（mtime 未变）→ 同值。
    assert!(!forge.is_enabled());

    // 再热翻开。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&cfg_path, r#"{"forge":{"enabled":true}}"#).unwrap();
    assert!(forge.is_enabled(), "热翻 true");
}

/// config 读坏 = 诚实降级：保持上次生效值；首读就坏 = 构造期值。
#[test]
fn master_switch_broken_config_keeps_last_value() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"forge":{"enabled":true}}"#).unwrap();

    let mut cfg = ForgeConfig::default();
    cfg.enabled = false;
    let forge = Forge::new(cfg, dir.path().to_path_buf()).with_master_switch_path(cfg_path.clone());
    assert!(forge.is_enabled(), "首读磁盘 true");

    // 写坏 → 保持上次热读值 true，不 panic 不翻 false。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&cfg_path, "{broken json").unwrap();
    assert!(forge.is_enabled(), "读坏 → 保持上次生效值 true");

    // 全新 Forge 首读就坏 = 构造期值 false。
    let mut cfg2 = ForgeConfig::default();
    cfg2.enabled = false;
    let forge2 =
        Forge::new(cfg2, dir.path().to_path_buf()).with_master_switch_path(cfg_path.clone());
    assert!(!forge2.is_enabled(), "首读失败 = 构造期值");
}

/// 缺 forge 段的 config = enabled=false（与 gateway 装配臂
/// `cfg.forge.as_ref().map(|f| f.enabled).unwrap_or(false)` 同源）。
#[test]
fn master_switch_missing_forge_section_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"agents":{"defaults":{}}}"#).unwrap();

    let mut cfg = ForgeConfig::default();
    cfg.enabled = true; // 构造期 true，磁盘缺段 = false：热读胜出
    let forge = Forge::new(cfg, dir.path().to_path_buf()).with_master_switch_path(cfg_path);
    assert!(!forge.is_enabled(), "缺 forge 段 = disabled（同源装配臂）");
}

/// 总闸关（C3 热生效语义）：start() 循环照常 spawn 但每 cycle 前看闸空转
/// ——同窗口不写 report；config.json 热翻开后到点即恢复产出，无需重启
/// （带 master_switch_path 的真实接线形态）。
#[tokio::test]
async fn test_master_switch_off_loops_idle_then_hot_on_resumes() {
    let _g = crate::test_support::quiet_trace_guard();
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"forge":{"enabled":false}}"#).unwrap();
    let mut config = ForgeConfig::default();
    config.enabled = false;
    config.reflection.interval_secs = 1;
    let mut forge =
        Forge::new(config, dir.path().to_path_buf()).with_master_switch_path(cfg_path.clone());
    let reflections_dir = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    forge.init_reflector(Reflector::with_reflections_dir(reflections_dir.clone()));
    let forge = Arc::new(forge);
    write_test_experiences(forge.forge_dir(), 5).await;

    Arc::clone(&forge).start().await;
    assert!(forge.is_running(), "总闸关也照常 spawn 循环（待命形态）");
    let report_count = || -> usize {
        std::fs::read_dir(&reflections_dir)
            .map(|d| d.count())
            .unwrap_or(0)
    };
    // 总闸关窗口：cycle 到点被闸掉，不写 report。
    tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
    assert_eq!(report_count(), 0, "总闸关 = cycle 看闸空转，不写 report");

    // 热翻开：改盘 → is_enabled() 现读 true → 下一到点 tick 恢复产出。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&cfg_path, r#"{"forge":{"enabled":true}}"#).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    assert!(
        report_count() > 0,
        "热翻开后 reflection tick 应恢复写 report（无需重启）"
    );

    forge.stop().await;
    assert!(!forge.is_running());
}

// ── 场景拆分 + 扫描路径修复（洞察页与场景拆分 2026-11-24）─────────────

fn scope_cand(id: &str, scope: Option<&str>) -> crate::session_evidence::CorrectionCandidate {
    crate::session_evidence::CorrectionCandidate {
        id: id.into(),
        draft: crate::session_evidence::CandidateDraft {
            session_key: "s".into(),
            ts: "2026-11-24T10:00:00+08:00".into(),
            ask: "a".into(),
            reply: "r".into(),
            goal: "g".into(),
            result: "x".into(),
            fp: id.into(),
            scope: scope.map(|s| s.into()),
            kind: crate::session_evidence::CandidateKind::QuestionAnswer,
        },
    }
}

#[test]
fn test_unanimous_scope_three_states() {
    // 全同一项目 → 继承
    assert_eq!(
        super::unanimous_scope(&[
            scope_cand("a", Some("prj-1")),
            scope_cand("b", Some("prj-1"))
        ])
        .as_deref(),
        Some("prj-1")
    );
    // 混合项目 → 对话
    assert_eq!(
        super::unanimous_scope(&[
            scope_cand("a", Some("prj-1")),
            scope_cand("b", Some("prj-2"))
        ]),
        None
    );
    // 项目 + 对话混批 → 对话（任一 None 即破一致）
    assert_eq!(
        super::unanimous_scope(&[scope_cand("a", Some("prj-1")), scope_cand("b", None)]),
        None
    );
    // 全对话 → 对话
    assert_eq!(
        super::unanimous_scope(&[scope_cand("a", None), scope_cand("b", None)]),
        None
    );
    // 空批 → 对话
    assert_eq!(super::unanimous_scope(&[]), None);
    // 单条项目候选 → 继承
    assert_eq!(
        super::unanimous_scope(&[scope_cand("a", Some("prj-9"))]).as_deref(),
        Some("prj-9")
    );
}

/// 扫描路径回归锁（BUG 2026-11-24 证据扫描目录指错）：会话日志在
/// `logs/session_logs/` 子目录——生产路径布局必须扫得到候选。
#[tokio::test]
async fn test_learn_dialogue_scans_session_logs_subdir() {
    let _serial = crate::learn_dialogue::TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let logs = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    // 对话会话（无 sidecar）+ 项目会话（sidecar 带 project_id）
    let rows = r#"{"role":"user","timestamp":"2026-11-24T10:00:00+08:00","content":"帮我修测试"}
{"role":"assistant","timestamp":"2026-11-24T10:01:00+08:00","content":"已修复","work":{"rounds":[{"tools":[{"tool":"question","args_preview":"{\"question\":\"先跑哪个测试？\"}","result_preview":"User selected: 全部"}]}]}}"#;
    std::fs::write(logs.join("chat_a.jsonl"), rows).unwrap();
    std::fs::write(logs.join("proj_b.jsonl"), rows).unwrap();
    std::fs::write(
        logs.join("proj_b.meta.json"),
        r#"{"project_id":"prj-7","project_path":"D:/w"}"#,
    )
    .unwrap();

    let forge = Forge::new(ForgeConfig::default(), ws);
    forge.set_learn_caller(Arc::new(learn_nothing_script()));
    forge.set_learn_estop_check(Arc::new(|| false));
    let out = forge.run_learn_dialogue("manual").await.unwrap();
    assert_eq!(out.phases_completed, 1);

    // 候选落台账（扫描路径修复前 = 0 行：扫描指错父目录）
    let ledger =
        std::fs::read_to_string(forge.forge_dir().join("evidence").join("candidates.jsonl"))
            .unwrap();
    let count = ledger.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(count, 2, "两个会话各产一条候选（旧路径扫描恒空）");
    // 场景戳随行
    let store = crate::session_evidence::CandidateStore::from_forge_dir(forge.forge_dir());
    let cands = store.read_all();
    let scope_of = |k: &str| {
        cands
            .iter()
            .find(|c| c.draft.session_key == k)
            .unwrap()
            .draft
            .scope
            .clone()
    };
    assert_eq!(scope_of("chat_a"), None, "无 sidecar = 对话洞察");
    assert_eq!(
        scope_of("proj_b").as_deref(),
        Some("prj-7"),
        "sidecar 盖项目戳"
    );
}
