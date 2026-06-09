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
    bridge: Mutex<Option<Arc<dyn ClusterForgeBridge>>>,
    /// Background task handles (collector_loop, reflector_loop, cleanup_loop).
    bg_tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Shared running flag that background loops observe.
    bg_running: Arc<Mutex<bool>>,
    /// Timestamp when Forge was last started (RFC3339).
    started_at: Mutex<Option<String>>,
    /// Runtime toggle for learning (mirrors config.learning.enabled, mutable at runtime).
    learning_enabled: std::sync::atomic::AtomicBool,
}

impl Forge {
    /// Create a new Forge instance with the given configuration and workspace.
    pub fn new(config: ForgeConfig, workspace: PathBuf) -> Self {
        let forge_dir = workspace.join("forge");

        tracing::info!(
            workspace = %workspace.display(),
            forge_dir = %forge_dir.display(),
            "[Forge] Instance created"
        );

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

        let learning_enabled = config.learning.enabled;

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
            bridge: Mutex::new(None),
            bg_tasks: Mutex::new(Vec::new()),
            bg_running: Arc::new(Mutex::new(false)),
            started_at: Mutex::new(None),
            learning_enabled: std::sync::atomic::AtomicBool::new(learning_enabled),
        }
    }

    /// Start the forge subsystems.
    ///
    /// Spawns three background tokio tasks:
    /// - **collector_loop**: periodically flushes the collector buffer.
    /// - **reflector_loop**: periodically runs reflection + learning.
    /// - **cleanup_loop**: periodically removes old data.
    pub async fn start(self: Arc<Self>) {
        {
            let mut running = self.running.lock();
            if *running {
                tracing::warn!("[Forge] Already running");
                return;
            }
            *running = true;
        }

        *self.bg_running.lock() = true;
        *self.started_at.lock() = Some(chrono::Local::now().to_rfc3339());

        let flush_interval = self.config.collection.flush_interval_secs;
        let reflect_interval = self.config.reflection.interval_secs;
        let cleanup_interval = self.config.storage.cleanup_interval_secs;

        // collector_loop
        {
            let forge = Arc::clone(&self);
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
                        if let Err(e) = forge.collector.flush().await {
                            tracing::warn!(error = %e, "[Forge] collector flush failed");
                        }
                    }
                }
            });
            self.bg_tasks.lock().push(handle);
        }

        // reflector_loop
        {
            let forge = Arc::clone(&self);
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
                        forge.run_reflection_cycle().await;
                    }
                }
            });
            self.bg_tasks.lock().push(handle);
        }

        // cleanup_loop
        {
            let forge = Arc::clone(&self);
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
                        forge.run_cleanup_cycle().await;
                    }
                }
            });
            self.bg_tasks.lock().push(handle);
        }

        tracing::info!("[Forge] Started with background tasks");
    }

    /// Run a single reflection cycle: read experiences → reflect → learn → write report → share.
    async fn run_reflection_cycle(&self) {
        let store = crate::experience_store::ExperienceStore::from_forge_dir(&self.forge_dir);
        let experiences = match store.read_all().await {
            Ok(exps) if !exps.is_empty() => exps,
            _ => return,
        };

        tracing::info!(count = experiences.len(), "[Forge] Running reflection cycle");

        // Step 1: Reflect (statistical analysis)
        let report = if let Some(ref reflector) = self.reflector {
            reflector.reflect(&experiences, None, "today", "all")
        } else {
            return;
        };

        // Step 2: Learning cycle (pattern detection + skill generation)
        if let Some(ref learning_engine) = self.learning_engine {
            if self.is_learning_enabled() {
                let cycle = learning_engine.run_cycle(&experiences).await;
                tracing::info!(cycle_id = %cycle.id, patterns = cycle.patterns_found, actions = cycle.actions_taken, "[Forge] learning cycle completed");
            }
        }

        // Step 3: Write report
        if let Some(ref reflector) = self.reflector {
            if let Ok(path) = reflector.write_report(&report) {
                tracing::info!(path = %path.display(), "[Forge] reflection report written");

                // Step 4: Cluster share
                if let Some(ref syncer) = self.syncer {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let json = serde_json::json!({
                            "content": content,
                            "filename": path.file_name().map(|n| n.to_string_lossy().to_string()),
                        });
                        if let Err(e) = syncer.share_reflection(json).await {
                            tracing::warn!(error = %e, "[Forge] cluster share failed");
                        }
                    }
                }
            }
        }
    }

    /// Run a single cleanup cycle for old data.
    async fn run_cleanup_cycle(&self) {
        let max_age = self.config.storage.max_experience_age_days as i64;

        let store = crate::experience_store::ExperienceStore::from_forge_dir(&self.forge_dir);
        if let Ok(removed) = store.cleanup(max_age).await {
            if removed > 0 {
                tracing::info!(removed, "[Forge] cleaned up old experiences");
            }
        }

        if let Some(ref reflector) = self.reflector {
            let removed = reflector.cleanup_reports(max_age as u64);
            if removed > 0 {
                tracing::info!(removed, "[Forge] cleaned up old reports");
            }
        }

        if let Some(ref cycle_store) = self.cycle_store {
            if let Ok(removed) = cycle_store.cleanup(max_age).await {
                if removed > 0 {
                    tracing::info!(removed, "[Forge] cleaned up old learning cycles");
                }
            }
        }
    }

    /// Stop the forge subsystems.
    ///
    /// Sets the running flag to false (which causes background loops to exit
    /// within 1 second) and performs a final collector flush.
    pub async fn stop(&self) {
        *self.running.lock() = false;
        *self.bg_running.lock() = false; // Signal background loops to exit
        *self.started_at.lock() = None;

        // Perform a final collector flush to persist any buffered data.
        if let Err(e) = self.collector.flush().await {
            tracing::warn!(error = %e, "[Forge] Final collector flush failed during stop");
        }

        // Wait for background tasks to finish (they check the flag every 1s).
        let tasks: Vec<_> = {
            let mut guard = self.bg_tasks.lock();
            std::mem::take(&mut *guard)
        };
        for handle in tasks {
            let _ = handle.await;
        }

        tracing::info!("[Forge] Stopped");
    }

    /// Check if forge is running.
    pub fn is_running(&self) -> bool {
        *self.running.lock()
    }

    /// Set the cluster bridge for cross-node communication.
    pub fn set_bridge(&self, bridge: Arc<dyn ClusterForgeBridge>) {
        tracing::info!("[Forge] Cluster bridge configured");
        *self.bridge.lock() = Some(bridge);
    }

    /// Cascade-set the LLM provider to all subsystems that need it:
    /// reflector, pipeline, and learning engine (all via LLMCaller).
    pub fn set_provider(&self, provider: Arc<dyn crate::reflector_llm::LLMCaller>) {
        tracing::info!("[Forge] Setting LLM provider on subsystems");
        // Reflector gets the LLM caller for semantic analysis (Stage 2 LLM).
        if let Some(ref reflector) = self.reflector {
            reflector.set_provider(provider.clone());
        }
        // Pipeline gets the LLM caller for quality evaluation (Stage 3).
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_provider(provider.clone());
        }
        // Learning engine gets the LLM caller for skill draft generation.
        if let Some(ref le) = self.learning_engine {
            le.set_provider(provider.clone());
        }
        drop(provider);
    }

    // ----- Accessor methods (mirrors Go Forge.GetXxx) -----

    /// Get a reference to the configuration.
    pub fn config(&self) -> &ForgeConfig {
        &self.config
    }

    /// Get the timestamp when Forge was last started (RFC3339).
    pub fn started_at(&self) -> Option<String> {
        self.started_at.lock().clone()
    }

    /// Set the learning enabled flag at runtime.
    /// Takes effect on the next reflection cycle — no restart needed.
    pub fn set_learning_enabled(&self, enabled: bool) {
        self.learning_enabled.store(enabled, std::sync::atomic::Ordering::SeqCst);
        tracing::info!(enabled, "[Forge] Learning flag updated at runtime");
    }

    /// Check whether learning is currently enabled.
    pub fn is_learning_enabled(&self) -> bool {
        self.learning_enabled.load(std::sync::atomic::Ordering::SeqCst)
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
    pub fn bridge(&self) -> Option<Arc<dyn ClusterForgeBridge>> {
        let guard = self.bridge.lock();
        guard.clone()
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
        tracing::info!("[Forge] Reflector subsystem initialized");
        self.reflector = Some(reflector);
    }

    /// Initialize the pipeline subsystem.
    pub fn init_pipeline(&mut self, pipeline: Pipeline) {
        tracing::info!("[Forge] Pipeline subsystem initialized");
        self.pipeline = Some(pipeline);
    }

    /// Initialize the MCP installer.
    pub fn init_mcp_installer(&mut self, installer: MCPInstaller) {
        tracing::info!("[Forge] MCP installer initialized");
        self.mcp_installer = Some(installer);
    }

    /// Initialize the syncer with the current bridge.
    pub fn init_syncer(&mut self) {
        let bridge_opt = self.bridge.lock().clone();
        if let Some(bridge) = bridge_opt {
            tracing::info!("[Forge] Syncer initialized with cluster bridge");
            self.syncer = Some(Syncer::with_forge_dir(bridge, self.forge_dir.clone()));
        } else {
            tracing::debug!("[Forge] Syncer not initialized: no bridge configured");
        }
    }

    /// Initialize the trace subsystem.
    pub fn init_trace(&mut self, collector: TraceCollector, store: TraceStore) {
        tracing::info!("[Forge] Trace subsystem initialized");
        self.trace_collector = Some(collector);
        self.trace_store = Some(store);
    }

    /// Initialize the learning subsystem.
    pub fn init_learning(&mut self, engine: LearningEngine, monitor: DeploymentMonitor, store: CycleStore) {
        tracing::info!("[Forge] Learning subsystem initialized (engine + monitor + cycle store)");
        self.learning_engine = Some(engine);
        self.deployment_monitor = Some(monitor);
        self.cycle_store = Some(store);
    }

    // ----- High-level methods -----

    /// Trigger a manual reflection cycle.
    pub fn reflect_now(&self, experiences: &[crate::types::CollectedExperience]) -> Result<nemesis_types::forge::Reflection, String> {
        tracing::info!(
            experience_count = experiences.len(),
            "[Forge] Manual reflection triggered"
        );
        if let Some(ref reflector) = self.reflector {
            let result = reflector.generate_reflection(experiences);
            tracing::info!(
                insight_count = result.insights.len(),
                recommendation_count = result.recommendations.len(),
                "[Forge] Reflection completed"
            );
            Ok(result)
        } else {
            tracing::error!("[Forge] Reflection failed: reflector not initialized");
            Err("Reflector not initialized".to_string())
        }
    }

    /// Receive a remote reflection report and store it.
    pub fn receive_reflection(&self, payload: &serde_json::Value) -> Result<(), String> {
        tracing::info!("[Forge] Receiving remote reflection report");
        if let Some(ref syncer) = self.syncer {
            let result = syncer.receive_reflection(payload);
            if result.is_ok() {
                tracing::info!("[Forge] Remote reflection report stored successfully");
            } else {
                tracing::error!(error = %result.as_ref().unwrap_err(), "[Forge] Failed to store remote reflection");
            }
            result
        } else {
            tracing::error!("[Forge] Receive reflection failed: syncer not initialized");
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
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
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
            "[Forge] Created and registered skill artifact"
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

        let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days);

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
                    let modified_time: chrono::DateTime<chrono::Local> = modified.into();
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
mod tests;
