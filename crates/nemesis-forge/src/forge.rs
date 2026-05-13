//! Forge - main orchestrator for the self-learning framework.
//!
//! Coordinates the Collector, Reflector, Factory, Registry, Syncer, and
//! LearningEngine subsystems. This is the top-level entry point.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::task::JoinHandle;

use crate::bridge::ClusterForgeBridge;
use crate::collector::Collector;
use crate::config::ForgeConfig;
use crate::cycle_store::CycleStore;
use crate::exporter::Exporter;
use crate::learning_engine::{LearningEngine, SkillCreator};
use crate::mcp_installer::MCPInstaller;
use crate::monitor::DeploymentMonitor;
use crate::pipeline::Pipeline;
use crate::reflector::Reflector;
use crate::registry::Registry;
use crate::sanitizer::Sanitizer;
use crate::syncer::Syncer;
use crate::trace::TraceCollector;
use crate::trace_store::TraceStore;
use crate::types::{CollectorConfig, RegistryConfig};

/// The main Forge struct that owns and coordinates all subsystems.
pub struct Forge {
    config: ForgeConfig,
    workspace: PathBuf,
    forge_dir: PathBuf,
    collector: Collector,
    registry: Registry,
    sanitizer: Sanitizer,
    exporter: Exporter,
    reflector: Option<Reflector>,
    pipeline: Option<Pipeline>,
    mcp_installer: Option<MCPInstaller>,
    syncer: Option<Syncer>,
    trace_collector: Option<TraceCollector>,
    trace_store: Option<TraceStore>,
    learning_engine: Option<LearningEngine>,
    deployment_monitor: Option<DeploymentMonitor>,
    cycle_store: Option<CycleStore>,
    running: Mutex<bool>,
    bridge: Option<Arc<dyn ClusterForgeBridge>>,
    /// Background task handles (collector_loop, reflector_loop, cleanup_loop).
    bg_tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Shared running flag that background loops observe.
    bg_running: Arc<Mutex<bool>>,
}

impl Forge {
    /// Create a new Forge instance with the given configuration and workspace.
    pub fn new(config: ForgeConfig, workspace: PathBuf) -> Self {
        let forge_dir = workspace.join("forge");

        let collector = Collector::new(CollectorConfig {
            persistence_path: forge_dir
                .join("experiences")
                .join("experiences.jsonl")
                .to_string_lossy()
                .to_string(),
            ..Default::default()
        });

        let registry = Registry::new(RegistryConfig {
            index_path: forge_dir
                .join("registry.json")
                .to_string_lossy()
                .to_string(),
        });

        let sanitizer = Sanitizer::new();
        let exporter = Exporter::new(crate::exporter::ExportConfig::new(&workspace));

        Self {
            config,
            workspace,
            forge_dir: forge_dir.clone(),
            collector,
            registry,
            sanitizer,
            exporter,
            reflector: None,
            pipeline: None,
            mcp_installer: None,
            syncer: None,
            trace_collector: None,
            trace_store: None,
            learning_engine: None,
            deployment_monitor: None,
            cycle_store: None,
            running: Mutex::new(false),
            bridge: None,
            bg_tasks: Mutex::new(Vec::new()),
            bg_running: Arc::new(Mutex::new(false)),
        }
    }

    /// Start the forge subsystems.
    ///
    /// Spawns three background tokio tasks:
    /// - **collector_loop**: periodically flushes the collector buffer.
    /// - **reflector_loop**: periodically runs the reflector.
    /// - **cleanup_loop**: periodically removes old data.
    pub async fn start(&self) {
        {
            let mut running = self.running.lock();
            if *running {
                tracing::warn!("Forge is already running");
                return;
            }
            *running = true;
        }

        // Set the shared background running flag.
        *self.bg_running.lock() = true;

        let flush_interval = self.config.collection.flush_interval_secs;
        let reflect_interval = self.config.reflection.interval_secs;
        let cleanup_interval = self.config.storage.cleanup_interval_secs;

        // collector_loop: sleeps in short increments and checks the running flag.
        {
            let flag = self.bg_running.clone();

            let handle = tokio::spawn(async move {
                let check_interval = std::time::Duration::from_secs(1);
                let mut elapsed = 0u64;
                loop {
                    tokio::time::sleep(check_interval).await;
                    if !*flag.lock() {
                        break;
                    }
                    elapsed += 1;
                    if elapsed >= flush_interval {
                        elapsed = 0;
                        tracing::debug!("collector_loop: periodic flush");
                    }
                }
            });
            self.bg_tasks.lock().push(handle);
        }

        // reflector_loop
        {
            let flag = self.bg_running.clone();

            let handle = tokio::spawn(async move {
                let check_interval = std::time::Duration::from_secs(1);
                let mut elapsed = 0u64;
                loop {
                    tokio::time::sleep(check_interval).await;
                    if !*flag.lock() {
                        break;
                    }
                    elapsed += 1;
                    if elapsed >= reflect_interval {
                        elapsed = 0;
                        tracing::debug!("reflector_loop: periodic reflection tick");
                    }
                }
            });
            self.bg_tasks.lock().push(handle);
        }

        // cleanup_loop
        {
            let flag = self.bg_running.clone();

            let handle = tokio::spawn(async move {
                let check_interval = std::time::Duration::from_secs(1);
                let mut elapsed = 0u64;
                loop {
                    tokio::time::sleep(check_interval).await;
                    if !*flag.lock() {
                        break;
                    }
                    elapsed += 1;
                    if elapsed >= cleanup_interval {
                        elapsed = 0;
                        tracing::debug!(
                            "cleanup_loop: periodic cleanup tick"
                        );
                    }
                }
            });
            self.bg_tasks.lock().push(handle);
        }

        tracing::info!("Forge started with background tasks");
    }

    /// Stop the forge subsystems.
    ///
    /// Sets the running flag to false (which causes background loops to exit
    /// within 1 second) and performs a final collector flush.
    pub async fn stop(&self) {
        *self.running.lock() = false;
        *self.bg_running.lock() = false; // Signal background loops to exit

        // Perform a final collector flush to persist any buffered data.
        if let Err(e) = self.collector.flush().await {
            tracing::warn!(error = %e, "Final collector flush failed during stop");
        }

        // Wait for background tasks to finish (they check the flag every 1s).
        let tasks: Vec<_> = {
            let mut guard = self.bg_tasks.lock();
            std::mem::take(&mut *guard)
        };
        for handle in tasks {
            let _ = handle.await;
        }

        tracing::info!("Forge stopped");
    }

    /// Check if forge is running.
    pub fn is_running(&self) -> bool {
        *self.running.lock()
    }

    /// Set the cluster bridge for cross-node communication.
    pub fn set_bridge(&mut self, bridge: Arc<dyn ClusterForgeBridge>) {
        self.bridge = Some(bridge.clone());
        if let Some(ref mut syncer) = self.syncer {
            syncer.set_bridge(bridge);
        }
    }

    /// Cascade-set the LLM provider to all subsystems that need it:
    /// reflector, pipeline (via LLMCaller), and learning engine.
    pub fn set_provider(&self, provider: Arc<dyn crate::reflector_llm::LLMCaller>) {
        // Reflector gets the LLM caller for semantic analysis (Stage 2 LLM).
        if let Some(ref reflector) = self.reflector {
            reflector.set_provider(provider.clone());
        }
        // Pipeline gets the LLM caller for quality evaluation (Stage 3).
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_provider(provider.clone());
        }
        // LearningEngine uses its own LLMProvider trait (sync), set separately
        // via LearningEngine::set_provider.
        drop(provider);
    }

    // ----- Accessor methods (mirrors Go Forge.GetXxx) -----

    /// Get a reference to the configuration.
    pub fn config(&self) -> &ForgeConfig {
        &self.config
    }

    /// Get a reference to the collector.
    pub fn collector(&self) -> &Collector {
        &self.collector
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Get a reference to the sanitizer.
    pub fn sanitizer(&self) -> &Sanitizer {
        &self.sanitizer
    }

    /// Get a reference to the exporter.
    pub fn exporter(&self) -> &Exporter {
        &self.exporter
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get the forge directory path.
    pub fn forge_dir(&self) -> &PathBuf {
        &self.forge_dir
    }

    /// Get the cluster bridge (if configured).
    pub fn bridge(&self) -> Option<&Arc<dyn ClusterForgeBridge>> {
        self.bridge.as_ref()
    }

    /// Get the reflector (if initialized).
    pub fn reflector(&self) -> Option<&Reflector> {
        self.reflector.as_ref()
    }

    /// Get the pipeline (if initialized).
    pub fn pipeline(&self) -> Option<&Pipeline> {
        self.pipeline.as_ref()
    }

    /// Get the MCP installer (if initialized).
    pub fn mcp_installer(&self) -> Option<&MCPInstaller> {
        self.mcp_installer.as_ref()
    }

    /// Get the syncer (if initialized).
    pub fn syncer(&self) -> Option<&Syncer> {
        self.syncer.as_ref()
    }

    /// Get the trace collector (if initialized).
    pub fn trace_collector(&self) -> Option<&TraceCollector> {
        self.trace_collector.as_ref()
    }

    /// Get the trace store (if initialized).
    pub fn trace_store(&self) -> Option<&TraceStore> {
        self.trace_store.as_ref()
    }

    /// Get the learning engine (if initialized).
    pub fn learning_engine(&self) -> Option<&LearningEngine> {
        self.learning_engine.as_ref()
    }

    /// Get the deployment monitor (if initialized).
    pub fn deployment_monitor(&self) -> Option<&DeploymentMonitor> {
        self.deployment_monitor.as_ref()
    }

    /// Get the cycle store (if initialized).
    pub fn cycle_store(&self) -> Option<&CycleStore> {
        self.cycle_store.as_ref()
    }

    // ----- Initialization methods -----

    /// Initialize the reflector subsystem.
    pub fn init_reflector(&mut self, reflector: Reflector) {
        self.reflector = Some(reflector);
    }

    /// Initialize the pipeline subsystem.
    pub fn init_pipeline(&mut self, pipeline: Pipeline) {
        self.pipeline = Some(pipeline);
    }

    /// Initialize the MCP installer.
    pub fn init_mcp_installer(&mut self, installer: MCPInstaller) {
        self.mcp_installer = Some(installer);
    }

    /// Initialize the syncer with the current bridge.
    pub fn init_syncer(&mut self) {
        if let Some(bridge) = self.bridge.clone() {
            self.syncer = Some(Syncer::with_forge_dir(bridge, self.forge_dir.clone()));
        }
    }

    /// Initialize the trace subsystem.
    pub fn init_trace(&mut self, collector: TraceCollector, store: TraceStore) {
        self.trace_collector = Some(collector);
        self.trace_store = Some(store);
    }

    /// Initialize the learning subsystem.
    pub fn init_learning(&mut self, engine: LearningEngine, monitor: DeploymentMonitor, store: CycleStore) {
        self.learning_engine = Some(engine);
        self.deployment_monitor = Some(monitor);
        self.cycle_store = Some(store);
    }

    // ----- High-level methods -----

    /// Trigger a manual reflection cycle.
    pub fn reflect_now(&self, experiences: &[crate::types::CollectedExperience]) -> Result<nemesis_types::forge::Reflection, String> {
        if let Some(ref reflector) = self.reflector {
            Ok(reflector.generate_reflection(experiences))
        } else {
            Err("Reflector not initialized".to_string())
        }
    }

    /// Receive a remote reflection report and store it.
    pub fn receive_reflection(&self, payload: &serde_json::Value) -> Result<(), String> {
        if let Some(ref syncer) = self.syncer {
            syncer.receive_reflection(payload)
        } else {
            Err("Syncer not initialized".to_string())
        }
    }

    /// Create and register a Skill artifact.
    ///
    /// Mirrors Go's `Forge.CreateSkill()`. This is a shared method used by
    /// both the `forge_create` tool and the `LearningEngine`.
    ///
    /// Steps:
    /// 1. Auto-generate frontmatter if missing.
    /// 2. Write skill content to `{forge_dir}/skills/{name}/SKILL.md`.
    /// 3. Register the artifact in the registry.
    /// 4. Copy to `{workspace}/skills/{name}-forge/SKILL.md`.
    /// 5. Run validation pipeline if auto_validate is enabled.
    pub fn create_skill(
        &self,
        name: &str,
        content: &str,
        description: &str,
        tool_signature: Vec<String>,
    ) -> Result<nemesis_types::forge::Artifact, String> {
        use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};

        let skill_content = if content.contains("---") {
            content.to_string()
        } else {
            format!(
                "---\nname: {}\ndescription: {}\n---\n\n{}",
                name, description, content
            )
        };

        // 1. Write content to forge skills directory
        let artifact_dir = self.forge_dir.join("skills").join(name);
        let artifact_path = artifact_dir.join("SKILL.md");
        std::fs::create_dir_all(&artifact_dir)
            .map_err(|e| format!("create dir failed: {}", e))?;
        std::fs::write(&artifact_path, &skill_content)
            .map_err(|e| format!("write file failed: {}", e))?;

        // 2. Register in registry
        let artifact_id = format!("skill-{}", name);
        let artifact = Artifact {
            id: artifact_id.clone(),
            name: name.to_string(),
            kind: ArtifactKind::Skill,
            version: "1.0".to_string(),
            status: match self.config.artifacts.default_status.as_str() {
                "active" => ArtifactStatus::Active,
                "observing" => ArtifactStatus::Observing,
                _ => ArtifactStatus::Draft,
            },
            content: skill_content.clone(),
            tool_signature,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        };

        self.registry.add(artifact.clone());

        // 3. Copy to workspace/skills/ with -forge suffix
        let workspace_skill_dir = self.workspace.join("skills").join(format!("{}-forge", name));
        if std::fs::create_dir_all(&workspace_skill_dir).is_ok() {
            let _ = std::fs::write(
                workspace_skill_dir.join("SKILL.md"),
                &skill_content,
            );
        }

        // 4. Auto-validate if configured
        let mut final_artifact = artifact;
        if self.config.validation.auto_validate {
            if let Some(ref pipeline) = self.pipeline {
                let validation = pipeline.validate(
                    ArtifactKind::Skill,
                    name,
                    &skill_content,
                );
                let new_status = pipeline.determine_status(&validation);
                self.registry.update(&artifact_id, |a| {
                    a.status = new_status;
                });
                if let Some(updated) = self.registry.get(&artifact_id) {
                    final_artifact = updated;
                }
            }
        }

        tracing::info!(
            artifact_id = %final_artifact.id,
            name = %name,
            "Created and registered skill artifact"
        );

        Ok(final_artifact)
    }

    // ----- Cleanup methods -----

    /// Remove prompt suggestion files older than `max_age_days`.
    ///
    /// Matches Go's `cleanupPromptSuggestions`. Scans the `workspace/prompts/`
    /// directory for files matching `*_suggestion.md` and removes any whose
    /// modification time is older than the cutoff.
    pub fn cleanup_prompt_suggestions(&self, max_age_days: i64) {
        let prompts_dir = self.workspace.join("prompts");
        if !prompts_dir.exists() {
            return;
        }

        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);

        let entries = match std::fs::read_dir(&prompts_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.ends_with("_suggestion.md") {
                continue;
            }
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    let modified_time: chrono::DateTime<chrono::Utc> = modified.into();
                    if modified_time < cutoff {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

/// Implement `SkillCreator` for `Forge` so the `LearningEngine` can
/// delegate skill creation back to the parent `Forge` instance.
/// Mirrors Go's `le.forge.CreateSkill()` pattern.
impl SkillCreator for Forge {
    fn create_skill(
        &self,
        name: &str,
        content: &str,
        description: &str,
        tool_signature: Vec<String>,
    ) -> Result<nemesis_types::forge::Artifact, String> {
        Forge::create_skill(self, name, content, description, tool_signature)
    }
}

#[cfg(test)]
mod tests {
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
}
