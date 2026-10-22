//! Forge - main orchestrator for the self-learning framework.
//!
//! Coordinates the Collector, Reflector, Registry, Syncer, and
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
use crate::learning_engine::LearningEngine;
use crate::mcp_installer::MCPInstaller;
use crate::monitor::DeploymentMonitor;
use crate::pipeline::Pipeline;
use crate::reflector::Reflector;
use crate::registry::Registry;
use crate::sanitizer::Sanitizer;
use crate::syncer::Syncer;
use crate::trace::TraceEvent;
use crate::trace_store::TraceStore;
use crate::types::{CollectedExperience, CollectorConfig, RegistryConfig};

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
    pipeline: Option<Arc<Pipeline>>,
    mcp_installer: Option<MCPInstaller>,
    syncer: Option<Syncer>,
    trace_store: Option<TraceStore>,
    learning_engine: Option<LearningEngine>,
    deployment_monitor: Option<Arc<DeploymentMonitor>>,
    cycle_store: Option<CycleStore>,
    running: Mutex<bool>,
    bridge: Mutex<Option<Arc<dyn ClusterForgeBridge>>>,
    /// Background task handles (reflector_loop, cleanup_loop).
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

    /// Whether the forge system is enabled (the master switch from config).
    /// This is the truth source for "should forge collect experiences / run
    /// learning" — the per-tool-call recording path gates on it so that
    /// `forge.enabled = false` actually stops collection (not just the
    /// background loops).
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Start the forge subsystems.
    ///
    /// Spawns two background tokio tasks:
    /// - **reflector_loop**: periodically runs reflection + learning.
    /// - **cleanup_loop**: periodically removes old data.
    ///
    /// The former collector_loop (periodic buffer flush) was removed together
    /// with the aggregate line (2026-10-22): `record()` writes through to disk
    /// directly, so there is no buffered channel left to drain.
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

        let reflect_interval = self.config.reflection.interval_secs;
        let cleanup_interval = self.config.storage.cleanup_interval_secs;

        // 0=关 惯例（2026-10-16 R16 缺陷 22）：reflector/cleanup 循环对
        // interval=0 不 spawn——旧实现 `elapsed >= 0` 恒真，反思（use_llm
        // =true 时含 LLM 调用）与清理每秒各跑一轮，成本与磁盘 churn 失控；
        // 本仓 0=关 惯例（board.budget/sweep 等同款）下用户设 0 的意图是
        // 关闭循环，旧行为恰好相反。
        if reflect_interval == 0 {
            tracing::warn!("[Forge] reflection.interval_secs = 0，反思循环已关闭（0=关）");
        }
        if cleanup_interval == 0 {
            tracing::warn!("[Forge] storage.cleanup_interval_secs = 0，清理循环已关闭（0=关）");
        }

        // reflector_loop（interval=0 = 关闭，不 spawn）
        if reflect_interval > 0 {
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

        // cleanup_loop（interval=0 = 关闭，不 spawn）
        if cleanup_interval > 0 {
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
        // F-P2: parse only the most-recent experiences (bounded by
        // collection.max_experiences) instead of the whole file each cycle.
        // (File size itself is age-bounded by cleanup — F-D2.)
        let limit = self.config.collection.max_experiences.max(1);
        let experiences = match store.read_recent(limit).await {
            Ok(exps) if !exps.is_empty() => exps,
            _ => return,
        };

        tracing::info!(
            count = experiences.len(),
            "[Forge] Running reflection cycle"
        );

        let Some(ref reflector) = self.reflector else {
            return;
        };

        // Step 1: Learning cycle first（语义反思接入 2026-09-28 时序修正）：
        // 先跑学习闭环（模式检出 + 技能生成），本轮 LearningCycle 产出后
        // 再做语义反思——报告首次携带**本轮**闭环状态（旧序传 None，语义
        // 分析看到的永远是上一轮的闭环，时序错位）。
        let mut learning_cycle = None;
        if let Some(ref learning_engine) = self.learning_engine
            && self.is_learning_enabled()
        {
            let cycle = learning_engine.run_cycle(&experiences).await;
            tracing::info!(cycle_id = %cycle.id, patterns = cycle.patterns_found, actions = cycle.actions_taken, "[Forge] learning cycle completed");
            learning_cycle = Some(cycle);
        }

        // Step 2: Reflect（统计 + use_llm 时 LLM 语义分析）。语义反思带：全量 artifacts
        // （旧调用面只传统计不传产物——LLM 看不到已部署技能会重复提名）、
        // 本轮 learning_cycle、统一杂务预算（thinking 系防烧穿）。桥接护栏
        // 空输出 → Err → warn+skip，报告无 insights 节，诚实降级不炸循环。
        // use_llm=false = 纯统计反思（2026-10-15 第十一轮修复：该键文档契约
        // 「是否用 LLM 做语义分析」，此前 gateway 无条件走 LLM、键只在 CLI
        // 被消费——用户设 false 省成本的意图被静默无视）。
        let report = if self.config.reflection.use_llm {
            reflector
                .reflect_with_llm(
                    &experiences,
                    &self.registry().list(None, None),
                    learning_cycle.as_ref(),
                    "today",
                    "all",
                    Some(crate::reflector_llm::AUX_FORGE_MAX_TOKENS),
                )
                .await
        } else {
            reflector.reflect(&experiences, learning_cycle.as_ref(), "today", "all")
        };

        // Step 3: Write report
        if let Ok(path) = reflector.write_report(&report) {
            tracing::info!(path = %path.display(), "[Forge] reflection report written");

            // Step 4: Cluster share
            if let Some(ref syncer) = self.syncer
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                let json = serde_json::json!({
                    "content": content,
                    "filename": path.file_name().map(|n| n.to_string_lossy().to_string()),
                });
                if let Err(e) = syncer.share_reflection(json).await {
                    tracing::warn!(error = %e, "[Forge] cluster share failed");
                }
            }
        } else {
            // P2 诚实化（2026-10-16）：报告落盘失败此前静默——反思明明跑完，
            // 盘上却无新报告，用户只看到「latest report 停在旧日期」。loud warn。
            tracing::warn!("[Forge] failed to write reflection report (see prior errors)");
        }

        // Step 5: auto_skill 蒸馏（2026-10-15 W2 生成面）。`learning.enabled`
        // 保持关（自动部署闭环风险面不动）；本步只产出 **draft** 状态草稿
        // （落 forge skills 目录 + 注册表），不自动启用——启用/部署是人工
        // 评审后的显式动作。Arc 锁内取即放（guard 不跨 await，F-M7 同款）。
        // use_llm=false 时蒸馏一并关（2026-10-15 第十一轮）：蒸馏是 LLM 工作，
        // 与 CLI 同语义（CLI use_llm=false 不挂 provider → 蒸馏诚实报
        // no LLM provider）——语义分析关而蒸馏开会自相矛盾；caller=None
        // 从静默跳过改诚实 WARN（配置陷阱此前不可见）。
        let caller = self.reflector.as_ref().and_then(|r| r.llm_caller().clone());
        if self.config.artifacts.auto_skill {
            if !self.config.reflection.use_llm {
                tracing::warn!(
                    "[Forge] auto_skill enabled but reflection.use_llm=false — distillation requires LLM, skipping"
                );
            } else if let Some(caller) = caller {
                match crate::reflector_llm::distill_skill_draft(
                    caller.as_ref(),
                    &experiences,
                    &self.registry().list(None, None),
                    Some(crate::reflector_llm::AUX_FORGE_MAX_TOKENS),
                )
                .await
                {
                    Ok(Some(distilled)) => {
                        self.store_distilled_draft(&distilled);
                    }
                    Ok(None) => {
                        tracing::info!("[Forge] auto_skill: no distillable pattern this cycle");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "[Forge] auto_skill distillation failed, skipping");
                    }
                }
            } else {
                tracing::warn!(
                    "[Forge] auto_skill enabled but reflector has no LLM provider — skipping distillation"
                );
            }
        }
    }

    /// Run the auto_skill distillation standalone and return the draft slug.
    ///
    /// CLI `forge reflect` 消费（网关外重放通道）：与
    /// [`Self::run_reflection_cycle`] Step 5 同一蒸馏+落盘路径，但要求
    /// reflector 已挂 LLM provider（未挂 = Err 诚实报因）。
    pub async fn distill_and_store_draft(
        &self,
        experiences: &[CollectedExperience],
    ) -> Result<Option<String>, String> {
        let caller = self.reflector.as_ref().and_then(|r| r.llm_caller().clone());
        let Some(caller) = caller else {
            return Err("forge reflector has no LLM provider configured".into());
        };
        match crate::reflector_llm::distill_skill_draft(
            caller.as_ref(),
            experiences,
            &self.registry().list(None, None),
            Some(crate::reflector_llm::AUX_FORGE_MAX_TOKENS),
        )
        .await?
        {
            Some(distilled) => Ok(self.store_distilled_draft(&distilled)),
            None => Ok(None),
        }
    }

    /// Persist a distilled skill draft: write `{forge_dir}/skills/<slug>/SKILL.md`
    /// + register as Draft (dedup by name, slug-normalized). Returns the slug.
    ///
    /// 落盘/注册失败只 warn 不炸反思循环（产物是本轮增益，不是关键路径）；
    /// 返回 None 表示被跳过（名字非法或同题材已存在）。
    fn store_distilled_draft(
        &self,
        distilled: &crate::reflector_llm::DistilledSkill,
    ) -> Option<String> {
        let slug = slugify_skill_name(&distilled.name);
        if slug.is_empty() {
            tracing::warn!(name = %distilled.name, "[Forge] auto_skill: draft name slugified to empty, skipping");
            return None;
        }
        // dedup：同名**非降级**产物已存在则跳过（反思 6h 一轮，同题材不
        // 重复出草稿）。2026-10-16 复查（F-R4）：此前 find_by_name 匹配任意
        // 状态——Degraded/Archived 的旧产物也会挡住重蒸馏，与
        // disable_degraded_skills 的「降级产物应能被替代品顶替」语义矛盾
        // （旧技能已被禁用却永远阻止新版本诞生）。现对齐
        // LearningEngine::find_artifact_by_fingerprint 口径：只挡
        // Draft/Active/Observing/Negative 活跃产物。
        if let Some(existing) = self
            .registry()
            .find_by_name(&slug, nemesis_types::forge::ArtifactKind::Skill)
        {
            use nemesis_types::forge::ArtifactStatus;
            if !matches!(
                existing.status,
                ArtifactStatus::Degraded | ArtifactStatus::Archived
            ) {
                tracing::info!(name = %slug, "[Forge] auto_skill: artifact already exists, skipping");
                return None;
            }
            tracing::info!(
                name = %slug,
                status = ?existing.status,
                "[Forge] auto_skill: existing artifact is {} — allowing re-distillation",
                if existing.status == ArtifactStatus::Degraded {
                    "degraded"
                } else {
                    "archived"
                }
            );
        }
        // F-S1 安全闸（2026-10-16 R16 缺陷 20）：LLM 蒸馏产物写盘前过
        // security_errors——蒸馏提示词喂的是经验摘要（外部交互的衍生物，
        // 潜在注入面），产出的 SKILL.md 可能被引导写入危险命令/硬编码密钥。
        // 与 `create_skill` 同一闸同一规则（F-S1 承诺覆盖所有 artifact kind
        // 的写盘点，此前本路径漏配）。拦下 = 本轮放弃（与名字非法/skip 同面，
        // 6h 后下轮反思可能产出不同草稿）。
        let sec_errors =
            crate::validator::StaticValidator::new().security_errors(&distilled.skill_md);
        if !sec_errors.is_empty() {
            tracing::warn!(name = %slug, errors = ?sec_errors, "[Forge] auto_skill: draft failed security gate, skipping");
            return None;
        }
        let artifact_dir = self.forge_dir.join("skills").join(&slug);
        if let Err(e) = std::fs::create_dir_all(&artifact_dir)
            .and_then(|_| std::fs::write(artifact_dir.join("SKILL.md"), &distilled.skill_md))
        {
            tracing::warn!(error = %e, name = %slug, "[Forge] auto_skill: failed to write draft SKILL.md");
            return None;
        }
        let artifact_id = format!("skill-{}", slug);
        let now = chrono::Local::now().to_rfc3339();
        self.registry().add(nemesis_types::forge::Artifact {
            id: artifact_id.clone(),
            name: slug.clone(),
            kind: nemesis_types::forge::ArtifactKind::Skill,
            version: "1.0".to_string(),
            status: nemesis_types::forge::ArtifactStatus::Draft,
            content: distilled.skill_md.clone(),
            tool_signature: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
            deployed_at: None,
            last_efficacy: None,
        });
        tracing::info!(name = %slug, rationale = %distilled.rationale, "[Forge] auto_skill: draft skill created");
        Some(slug)
    }

    /// Run a single cleanup cycle for old data.
    async fn run_cleanup_cycle(&self) {
        let max_age = self.config.storage.max_experience_age_days as i64;

        let store = crate::experience_store::ExperienceStore::from_forge_dir(&self.forge_dir);
        if let Ok(removed) = store.cleanup(max_age).await
            && removed > 0
        {
            tracing::info!(removed, "[Forge] cleaned up old experiences");
        }

        if let Some(ref reflector) = self.reflector {
            let removed = reflector.cleanup_reports(max_age as u64);
            if removed > 0 {
                tracing::info!(removed, "[Forge] cleaned up old reports");
            }
        }

        if let Some(ref cycle_store) = self.cycle_store
            && let Ok(removed) = cycle_store.cleanup(max_age).await
            && removed > 0
        {
            tracing::info!(removed, "[Forge] cleaned up old learning cycles");
        }

        // Trace events (P0-F1 接线的盘上产物)：按 trace.max_trace_age_days
        // 滚动清理 JSONL（读-滤-重写）。验证轮二接线——此前该配置键零消费者，
        // traces 目录只增不删。清理 Forge 实际持有的那个 store（未 init 时
        // 无写入，无需清理）。
        if let Some(store) = &self.trace_store
            && let Ok(removed) = store.cleanup(self.config.trace.max_trace_age_days).await
            && removed > 0
        {
            tracing::info!(removed, "[Forge] cleaned up old trace events");
        }
    }

    /// Stop the forge subsystems.
    ///
    /// Sets the running flag to false (which causes background loops to exit
    /// within 1 second). No final flush is needed: `record()` writes through
    /// to disk directly (the buffered collector was removed 2026-10-22).
    pub async fn stop(&self) {
        *self.running.lock() = false;
        *self.bg_running.lock() = false; // Signal background loops to exit
        *self.started_at.lock() = None;

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
        self.learning_enabled
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
        tracing::info!(enabled, "[Forge] Learning flag updated at runtime");
    }

    /// Check whether learning is currently enabled.
    pub fn is_learning_enabled(&self) -> bool {
        self.learning_enabled
            .load(std::sync::atomic::Ordering::SeqCst)
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

    /// Set the skill behavior-test runner (A 线回归闸执行器，2026-10-14)。
    /// Pipeline 在 Skill artifact 校验时调用；未注入 = 带 tests 段的
    /// artifact 诚实失败（fail-closed，不允许无执行器的静默跳过）。
    pub fn set_skill_test_runner(
        &self,
        runner: std::sync::Arc<dyn crate::skill_tests::SkillTestRunner>,
    ) {
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_skill_test_runner(runner);
        }
    }

    /// Set the Stage 2 judge-layer caller（杂务模型回落链注入点，2026-10-14
    /// 三轮复查对齐 CLI/WSAPI/部署闸三入口口径）。级联到 pipeline 的
    /// judge 专用槽；Stage 3 质量评估维持主模型语义不受影响。
    pub fn set_judge_caller(&self, caller: std::sync::Arc<dyn crate::reflector_llm::LLMCaller>) {
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_judge_caller(caller);
        }
    }

    /// Get the pipeline (if initialized).
    pub fn pipeline(&self) -> Option<&Pipeline> {
        self.pipeline.as_deref()
    }

    /// Get the MCP installer (if initialized).
    pub fn mcp_installer(&self) -> Option<&MCPInstaller> {
        self.mcp_installer.as_ref()
    }

    /// Get the syncer (if initialized).
    pub fn syncer(&self) -> Option<&Syncer> {
        self.syncer.as_ref()
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
        self.deployment_monitor.as_deref()
    }

    /// Get the cycle store (if initialized).
    pub fn cycle_store(&self) -> Option<&CycleStore> {
        self.cycle_store.as_ref()
    }

    // ----- Trace recording -----

    /// Record a trace event: direct JSONL append to the TraceStore.
    ///
    /// Tool-dispatch 侧的低频喂入口（审计 P0-F1 修复，2026-10-22：
    /// TraceStore 原先零生产调用）。trace 是观测数据——写失败只 warn，
    /// 绝不反过来影响工具执行链。
    ///
    /// 2026-10-22 验证轮二：不再同步喂 in-memory `TraceCollector`——该
    /// collector 全仓零读者（getter 无生产调用方），喂它等于制造无上界
    /// write-only 内存增长（每笔工具调用永久驻留 Vec）；盘上 TraceStore
    /// 是学习引擎的唯一输入真相源（learning_engine 轨迹臂读盘）。
    pub async fn record_trace_event(&self, event: TraceEvent) {
        if let Some(store) = &self.trace_store
            && let Err(e) = store.append(&event).await
        {
            tracing::warn!(
                error = %e,
                event_type = %event.event_type,
                "[Forge] Failed to append trace event"
            );
        }
    }

    // ----- Initialization methods -----

    /// Initialize the reflector subsystem.
    pub fn init_reflector(&mut self, reflector: Reflector) {
        tracing::info!("[Forge] Reflector subsystem initialized");
        self.reflector = Some(reflector);
    }

    /// Initialize the pipeline subsystem.
    pub fn init_pipeline(&mut self, pipeline: Arc<Pipeline>) {
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
    ///
    /// 2026-10-22 验证轮二：只收 TraceStore。原 collector 参数随 write-only
    /// 内存面移除（见 `record_trace_event` 注记）。
    pub fn init_trace(&mut self, store: TraceStore) {
        tracing::info!("[Forge] Trace subsystem initialized");
        self.trace_store = Some(store);
    }

    /// Initialize the learning subsystem.
    pub fn init_learning(
        &mut self,
        engine: LearningEngine,
        monitor: Arc<DeploymentMonitor>,
        store: CycleStore,
    ) {
        // Inject pipeline + monitor into the learning engine so the Phase 6
        // closed loop (learn -> validate -> deploy -> monitor -> feedback)
        // actually runs. Previously never injected (F-C1).（LE3：skill_creator
        // 注入已随 write-only 死接线删除，无需补线。）
        if let Some(p) = &self.pipeline {
            engine.set_pipeline(p.clone());
        }
        engine.set_monitor(monitor.clone());
        tracing::info!("[Forge] Learning subsystem initialized (engine + monitor + cycle store)");
        self.learning_engine = Some(engine);
        self.deployment_monitor = Some(monitor);
        self.cycle_store = Some(store);
    }

    // ----- High-level methods -----

    /// Trigger a manual reflection cycle.
    pub fn reflect_now(
        &self,
        experiences: &[crate::types::CollectedExperience],
    ) -> Result<nemesis_types::forge::Reflection, String> {
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
            } else if let Err(e) = &result {
                tracing::error!(error = %e, "[Forge] Failed to store remote reflection");
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

        // 防御纵深（2026-10-16 R18 路径穿越修复）：name 是路径段
        // （`skills/{name}` 与下方 `skills/{name}-forge` 两处 join），在
        // 本体入口归一，覆盖所有调用方（工具执行器已在入口 slugify，这里
        // 兜底 learning_engine 等其余 caller 与历史回归）。空名诚实报错。
        let name = slugify_skill_name(name);
        if name.is_empty() {
            return Err("skill name contains no usable characters (need [a-z0-9_-])".into());
        }

        // F-M11: frontmatter must be at the very start — `contains("---")`
        // treated any Markdown horizontal rule as existing frontmatter and
        // skipped adding the wrapper.
        let skill_content = if content.starts_with("---") {
            content.to_string()
        } else {
            format!(
                "---\nname: {}\ndescription: {}\n---\n\n{}",
                name, description, content
            )
        };

        // Security gate (F-S1): block dangerous commands (rm -rf /, curl|bash)
        // and hardcoded secrets BEFORE writing the file — previously the file
        // was written first with only a weak post-write check, so such content
        // became an active agent instruction. Length/frontmatter are quality,
        // not security, so they don't block here.
        let sec_errors = crate::validator::StaticValidator::new().security_errors(&skill_content);
        if !sec_errors.is_empty() {
            return Err(format!(
                "Skill content failed security validation: {}",
                sec_errors.join("; ")
            ));
        }

        // 1. Write content to forge skills directory
        let artifact_dir = self.forge_dir.join("skills").join(&name);
        let artifact_path = artifact_dir.join("SKILL.md");
        std::fs::create_dir_all(&artifact_dir).map_err(|e| format!("create dir failed: {}", e))?;
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
            deployed_at: None,
            last_efficacy: None,
        };

        self.registry.add(artifact.clone());

        // 3. Copy to workspace/skills/ with -forge suffix
        let workspace_skill_dir = self
            .workspace
            .join("skills")
            .join(format!("{}-forge", name));
        if std::fs::create_dir_all(&workspace_skill_dir).is_ok() {
            let _ = std::fs::write(workspace_skill_dir.join("SKILL.md"), &skill_content);
        }

        // 4. Auto-validate if configured
        let mut final_artifact = artifact;
        if self.config.validation.auto_validate
            && let Some(ref pipeline) = self.pipeline
        {
            let validation = pipeline.validate(ArtifactKind::Skill, &name, &skill_content);
            let new_status = pipeline.determine_status(&validation);
            self.registry.update(&artifact_id, |a| {
                a.status = new_status;
            });
            if let Some(updated) = self.registry.get(&artifact_id) {
                final_artifact = updated;
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
            if let Ok(metadata) = entry.metadata()
                && let Ok(modified) = metadata.modified()
            {
                let modified_time: chrono::DateTime<chrono::Local> = modified.into();
                if modified_time < cutoff {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// Normalize an LLM-produced skill name to a filesystem-safe kebab-case slug.
///
/// 蒸馏草稿名是 LLM 自由文本，落盘前归一：小写、非 `[a-z0-9_-]` 折叠为
/// `-`、压缩连续 `-`、去首尾 `-`、截断 48 字符。空名/全非法字符返回空串
/// （调用方据以跳过，不猜不救）。pub(crate)：learning_engine 的
/// execute_create_skill 落盘点共用同一规则（R17 防御纵深）。
pub(crate) fn slugify_skill_name(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            let c = c.to_ascii_lowercase();
            let is_dash = c == '-' || c == '_';
            if is_dash && (slug.is_empty() || last_dash) {
                continue;
            }
            slug.push(if is_dash { '-' } else { c });
            last_dash = is_dash;
        } else {
            if !slug.is_empty() && !last_dash {
                slug.push('-');
                last_dash = true;
            }
        }
        if slug.len() >= 48 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// 技能目录名合法性围栏（跨入口单一真相源，R18）：必须是单段 kebab 目录
/// 名。`../x` 直拼 join 有穿越面；`C:evil` 盘符相对形态 `is_absolute()==false`
/// 且无分隔符，但 join 会整段换基座（Windows）；`.`/`..`/空串同理拒。
/// 合法域 = `slugify_skill_name` 的产物域。web `forge.test` 与 CLI
/// `forge test` / `forge optimize` 三入口共用同一判定。
pub fn valid_skill_dir_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name == "."
        || name == ".."
    {
        return Err(format!("invalid skill name: {name}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
