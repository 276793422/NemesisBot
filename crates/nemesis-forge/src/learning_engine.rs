//! Learning engine - closed-loop self-learning.
//!
//! Extracts patterns from collected experiences, generates learning actions
//! (skill creation, prompt suggestions), and evaluates deployment outcomes.
//!
//! Supports four pattern detectors:
//! - tool_chain: High-frequency tool sequences
//! - error_recovery: Tools that fail then succeed
//! - efficiency_issue: Slow or wasteful tool usage
//! - success_template: Consistently successful tool patterns
//!
//! LLM-driven skill generation:
//! - `generate_skill_draft` uses LLM to create SKILL.md content
//! - `refine_skill_draft` uses LLM to fix failed drafts
//! - `execute_create_skill` runs the full generation + validation + deploy loop
//! - `execute_suggest_prompt` writes prompt suggestions to workspace/prompts/

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::ForgeConfig;
use crate::cycle_store::CycleStore;
use crate::monitor::DeploymentMonitor;
use crate::pipeline::{ArtifactValidation, Pipeline};
use crate::registry::Registry;
use crate::types::CollectedExperience;
use nemesis_types::forge::LearningCycle;

// 复查（C2/C3 合一，缺陷整改批次 2026-11-14）：技能创建只有一条实现——
// 公开的 `execute_create_skill_action`（slug 化 → 查重 → LLM 生成 →
// validate + refine 循环 → 安全闸 → forge/skills 落盘 → registry 登记 →
// workspace/skills `-forge` 副本）。私有 `execute_create_skill` 是薄委托
// （调公开实现 + 按结果状态打日志），run_cycle ⑥ 消费其返回值真实记账
// （executed/failed/skipped 分开计数，failed 不再虚计为 executed）。
// 模式检测单一来源：`crate::pattern::extract_conversation_patterns`
// （B1 全换——四个弱内联检测器已删）。

/// Detected pattern from experience analysis.
#[derive(Debug, Clone)]
pub struct DetectedPattern {
    /// Pattern type (tool_chain, error_recovery, efficiency_issue, success_template).
    pub pattern_type: String,
    /// Frequency of occurrence.
    pub frequency: u32,
    /// Confidence score [0, 1].
    pub confidence: f64,
    /// Human-readable description.
    pub description: String,
    /// Associated tool names.
    pub tools: Vec<String>,
    /// C1（缺陷整改批次）：脱敏证据块（链序列 + 失败样例 + 统计）——
    /// LLM 草稿提示词的素材从频次数字升级为可判断形态的真实证据。
    pub evidence: Option<String>,
}

/// A learning action to be executed.
#[derive(Debug, Clone)]
pub struct LearningAction {
    /// Unique action ID.
    pub id: String,
    /// Action type (create_skill, suggest_prompt, deprecate_artifact).
    pub action_type: String,
    /// Priority (high, medium, low).
    pub priority: String,
    /// Description of the action.
    pub description: String,
    /// Status (pending, executed, skipped, failed).
    pub status: String,
    /// Error message if status is "failed".
    pub error_msg: Option<String>,
    /// Name for the draft skill.
    pub draft_name: Option<String>,
    /// Rationale for the action.
    pub rationale: Option<String>,
    /// Confidence score from the pattern.
    pub confidence: f64,
    /// Pattern ID that triggered this action.
    pub pattern_id: Option<String>,
    /// Artifact ID (set after execution).
    pub artifact_id: Option<String>,
    /// Creation timestamp (ISO 8601).
    pub created_at: Option<String>,
    /// Execution timestamp (ISO 8601).
    pub executed_at: Option<String>,
    /// Associated tool names from the triggering pattern（复查
    /// 部署时 tool_signature 的直接来源——此前从 description 散文
    /// 按 `→` 反解析，而 pattern 描述是英文散文不含箭头，落库的是垃圾
    /// 尾词。pattern.tools 由检测器结构化携带，不再二次解析）。
    pub tools: Vec<String>,
    /// C1：触发该 action 的模式证据块（脱敏）——透传给草稿生成提示词。
    pub evidence: Option<String>,
}

impl LearningAction {
    /// Create a new pending action.
    pub fn new(action_type: &str, priority: &str, description: &str) -> Self {
        Self {
            id: format!(
                "la-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("0")
            ),
            action_type: action_type.to_string(),
            priority: priority.to_string(),
            description: description.to_string(),
            status: "pending".to_string(),
            error_msg: None,
            draft_name: None,
            rationale: None,
            confidence: 0.0,
            pattern_id: None,
            artifact_id: None,
            created_at: Some(chrono::Local::now().to_rfc3339()),
            executed_at: None,
            tools: Vec::new(),
            evidence: None,
        }
    }
}

/// Outcome of a deployed artifact evaluation.
#[derive(Debug, Clone)]
pub struct DeploymentOutcome {
    /// Artifact ID.
    pub artifact_id: String,
    /// Verdict (positive, negative, observing).
    pub verdict: String,
    /// Improvement score.
    pub improvement_score: f64,
    /// Sample size for evaluation.
    pub sample_size: usize,
}

/// Pattern summary for cycle storage.
#[derive(Debug, Clone)]
pub struct PatternSummary {
    /// Pattern ID.
    pub id: String,
    /// Pattern type.
    pub pattern_type: String,
    /// Fingerprint hash.
    pub fingerprint: String,
    /// Frequency.
    pub frequency: u32,
    /// Confidence score.
    pub confidence: f64,
}

/// Action summary for cycle storage.
#[derive(Debug, Clone)]
pub struct ActionSummary {
    /// Action ID.
    pub id: String,
    /// Action type.
    pub action_type: String,
    /// Priority.
    pub priority: String,
    /// Status.
    pub status: String,
    /// Artifact ID (if created).
    pub artifact_id: Option<String>,
}

/// The learning engine drives the closed-loop learning cycle.
pub struct LearningEngine {
    config: ForgeConfig,
    forge_dir: PathBuf,
    registry: Arc<Registry>,
    cycle_store: CycleStore,
    pipeline: Mutex<Option<Arc<Pipeline>>>,
    monitor: Mutex<Option<Arc<DeploymentMonitor>>>,
    provider: Mutex<Option<Arc<dyn crate::reflector_llm::LLMCaller>>>,
    latest_cycle: Mutex<Option<LearningCycle>>,
}

impl LearningEngine {
    /// Create a new learning engine.
    pub fn new(config: ForgeConfig, registry: Arc<Registry>, cycle_store: CycleStore) -> Self {
        tracing::info!("[Forge/LearningEngine] Created");
        Self {
            forge_dir: PathBuf::new(),
            config,
            registry,
            cycle_store,
            pipeline: Mutex::new(None),
            monitor: Mutex::new(None),
            provider: Mutex::new(None),
            latest_cycle: Mutex::new(None),
        }
    }

    /// Create a new learning engine with forge directory.
    pub fn with_forge_dir(
        config: ForgeConfig,
        forge_dir: PathBuf,
        registry: Arc<Registry>,
        cycle_store: CycleStore,
    ) -> Self {
        tracing::info!(
            forge_dir = %forge_dir.display(),
            "[Forge/LearningEngine] Created with forge directory"
        );
        Self {
            forge_dir,
            config,
            registry,
            cycle_store,
            pipeline: Mutex::new(None),
            monitor: Mutex::new(None),
            provider: Mutex::new(None),
            latest_cycle: Mutex::new(None),
        }
    }

    /// Set the LLM provider for skill draft generation.
    pub fn set_provider(&self, provider: Arc<dyn crate::reflector_llm::LLMCaller>) {
        tracing::info!("[Forge/LearningEngine] LLM provider set for skill generation");
        *self.provider.lock() = Some(provider);
    }

    /// Call an async LLMCaller from sync context.
    /// Mirrors the block_in_place pattern in pipeline.rs:evaluate_quality_sync.
    fn call_llm_sync(
        caller: &dyn crate::reflector_llm::LLMCaller,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> Result<String, String> {
        let future = caller.chat(system, user, Some(max_tokens as i64));
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
            Err(_) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| format!("failed to create runtime: {}", e))?;
                rt.block_on(future)
            }
        }
    }

    /// Set the pipeline for validation.
    pub fn set_pipeline(&self, pipeline: Arc<Pipeline>) {
        tracing::info!("[Forge/LearningEngine] Pipeline set for validation");
        *self.pipeline.lock() = Some(pipeline);
    }

    /// Set the deployment monitor for outcome evaluation.
    pub fn set_monitor(&self, monitor: Arc<DeploymentMonitor>) {
        tracing::info!("[Forge/LearningEngine] Deployment monitor set");
        *self.monitor.lock() = Some(monitor);
    }

    /// Run a single learning cycle.
    ///
    /// Mirrors Go's `LearningEngine.RunCycle`:
    /// 1. Evaluate previous deployment outcomes
    /// 2. Adjust confidence based on feedback
    /// 3. Extract patterns from experiences
    /// 4. Check suggestion adoption status
    /// 5. Generate actions from patterns
    /// 6. Execute actions (with auto-create limit)
    /// 7. Save cycle record
    pub async fn run_cycle(&self, experiences: &[CollectedExperience]) -> LearningCycle {
        let cycle_id = uuid::Uuid::new_v4().to_string();
        tracing::info!(
            cycle_id = %cycle_id,
            experience_count = experiences.len(),
            "[Forge/LearningEngine] Starting learning cycle"
        );
        let mut cycle = LearningCycle {
            id: cycle_id,
            started_at: chrono::Local::now().to_rfc3339(),
            completed_at: None,
            patterns_found: 0,
            actions_taken: 0,
            status: nemesis_types::forge::CycleStatus::Running,
        };

        // Step 1: Evaluate previous deployment outcomes
        // （短作用域取 evaluate_all 结果并放锁；轨迹臂的 await 不得持有
        // parking_lot guard——首版写法 guard 跨 await 被编译器打回。）
        let (previous_outcomes, monitor_for_traces) = match self.monitor.lock().as_ref() {
            Some(monitor) => {
                let results = monitor.evaluate_all();
                (results, Some(Arc::clone(monitor)))
            }
            None => (Vec::new(), None),
        };
        if let Some(monitor) = monitor_for_traces {
            // 审计 -：轨迹臂补全。TraceStore 里工具派发侧
            // 喂的 tool_call 事件此前零读者——在此读出并转成回话轨迹，走
            // evaluate_outcomes 的 before/after 实效对比（含 handle_verdict
            // 降级 + last_efficacy 写回）。与上面 evaluate_all 简模式并存：
            // 前者按 success_rate 粗判，轨迹臂按真实工具链细判。store 路径
            // 形态与 gateway 装配点（ctx.rs TraceStore::new(forge_dir/"traces")）
            // 同源；读失败 = 诚实跳过轨迹臂，不阻塞学习周期。
            let store = crate::trace_store::TraceStore::new(self.forge_dir.join("traces"));
            match store.read_all().await {
                Ok(events) => {
                    let traces = crate::monitor::build_conversation_traces(&events);
                    let outcomes = monitor.evaluate_outcomes(&traces);
                    let active = outcomes
                        .iter()
                        .filter(|o| o.verdict != "insufficient_data")
                        .count();
                    tracing::debug!(
                        events = events.len(),
                        traces = traces.len(),
                        active_outcomes = active,
                        "[Forge/LearningEngine] Trace-based outcome evaluation done"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "[Forge/LearningEngine] Failed to read traces — skipping trace arm this cycle"
                    );
                }
            }
        }

        // Step 2: Adjust confidence based on feedback
        self.adjust_confidence_from_outcomes(&previous_outcomes);
        // F-: take monitoring effect — disable the deployed skill file of any
        // artifact the monitor marked Degraded so the skills loader stops using
        // it. Without this the monitor's verdict had no teeth.
        self.disable_degraded_skills();

        // Step 3: Extract patterns from experiences
        let patterns = self.extract_patterns(experiences);
        cycle.patterns_found = patterns.len() as u32;

        // Step 4: Check suggestion adoption status
        self.check_suggestion_adoption(&patterns);

        // Step 5: Generate actions from patterns
        let actions = self.generate_actions(&patterns);

        // Step 6: Execute actions with limit
        let max_auto = if self.config.learning.max_auto_creates > 0 {
            self.config.learning.max_auto_creates
        } else {
            3
        };
        let mut auto_count = 0u32;
        let mut actions_executed = 0u32;
        let mut actions_skipped = 0u32;
        let mut actions_failed = 0u32;

        for mut action in actions {
            match action.action_type.as_str() {
                "create_skill" => {
                    if auto_count >= max_auto {
                        action.status = "skipped".to_string();
                        actions_skipped += 1;
                    } else {
                        auto_count += 1;
                        // C2/C3 合一（缺陷整改批次）：create 单一实现，
                        // 返回值按真实状态记账——failed 不再虚计为
                        // executed（旧实现拿不到返回值，early-return
                        // 全部记成已执行，actions_taken 说谎）。
                        let result = self.execute_create_skill(&action);
                        match result.status.as_str() {
                            "executed" => actions_executed += 1,
                            "failed" => actions_failed += 1,
                            _ => actions_skipped += 1,
                        }
                    }
                }
                "suggest_prompt" => {
                    self.execute_suggest_prompt(&mut action);
                    match action.status.as_str() {
                        "executed" => actions_executed += 1,
                        "failed" => actions_failed += 1,
                        _ => actions_skipped += 1,
                    }
                }
                _ => {
                    actions_skipped += 1;
                }
            }
        }

        cycle.actions_taken = actions_executed;
        if actions_skipped > 0 || actions_failed > 0 {
            tracing::info!(
                actions_executed,
                actions_failed,
                actions_skipped,
                "[LearningEngine] Learning cycle action accounting (executed/failed/skipped)"
            );
        }

        cycle.status = nemesis_types::forge::CycleStatus::Completed;
        cycle.completed_at = Some(chrono::Local::now().to_rfc3339());

        // Persist cycle
        if let Err(e) = self.cycle_store.append(&cycle).await {
            tracing::warn!(error = %e, "[LearningEngine] Failed to persist learning cycle");
        }

        *self.latest_cycle.lock() = Some(cycle.clone());
        cycle
    }

    /// Extract patterns from collected experiences.
    ///
    /// B1（缺陷整改批次 2026-11-14）：检测逻辑单一来源 =
    /// `crate::pattern::extract_conversation_patterns`（会话级检测器，
    /// 镜像 Go 的 ConversationPattern：SHA256 指纹去重、first/last_seen、
    /// 参数键聚合、类型专属字段）。本方法只剩频率阈值下传 +
    /// ConversationPattern → DetectedPattern 适配层 + C1 证据块挂载
    /// （[`build_pattern_evidence`]）。此前四个内联检测器是「工具级
    /// 计数」伪模式（单工具出现次数当链、失败率当恢复），已整体删除。
    pub fn extract_patterns(&self, experiences: &[CollectedExperience]) -> Vec<DetectedPattern> {
        if experiences.is_empty() {
            return Vec::new();
        }

        tracing::debug!(
            experience_count = experiences.len(),
            "[Forge/LearningEngine] Extracting patterns from experiences"
        );

        // 频率下限 fallback（0=未配置 → 缺省 3）；extract_conversation_patterns
        // 对 min_freq==0 诚实返回空，配置缺省不能依赖检测器兜底。
        let min_freq = if self.config.learning.min_pattern_frequency > 0 {
            self.config.learning.min_pattern_frequency
        } else {
            3
        };

        let mut patterns: Vec<DetectedPattern> =
            crate::pattern::extract_conversation_patterns(experiences, min_freq)
                .into_iter()
                .map(|cp| {
                    // 结构化 tools（B1）：链型按 → 拆链；error_recovery 用
                    // error_tool + recovery_tool 双工具（失败后恢复的完整链）。
                    let tools: Vec<String> = match cp.pattern_type {
                        crate::pattern::ConversationPatternType::ErrorRecovery => {
                            [cp.error_tool.clone(), cp.recovery_tool.clone()]
                                .into_iter()
                                .flatten()
                                .filter(|t| !t.is_empty())
                                .collect()
                        }
                        _ => cp
                            .tool_chain
                            .as_deref()
                            .unwrap_or_default()
                            .split('\u{2192}')
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .map(String::from)
                            .collect(),
                    };
                    DetectedPattern {
                        pattern_type: cp.pattern_type.as_str().to_string(),
                        frequency: cp.frequency,
                        confidence: cp.confidence,
                        description: cp.description.clone(),
                        tools,
                        evidence: build_pattern_evidence(&cp, experiences),
                    }
                })
                .collect();

        // Sort by confidence descending
        patterns.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        tracing::info!(
            pattern_count = patterns.len(),
            "[Forge/LearningEngine] Patterns extracted"
        );

        patterns
    }

    /// Generate learning actions from detected patterns.
    ///
    /// B2（缺陷整改批次 2026-11-14）决策表：
    /// - tool_chain：conf ≥ high_threshold 且 freq ≥ 10 → create，否则 suggest
    /// - error_recovery：三臂——conf ≥ high_threshold → create；
    ///   conf ≥ error_recovery_suggest_min → suggest（低置信经验也值得提示，
    ///   旧实现直接静默丢弃）；conf < suggest_min → 无动作（debug 留痕）
    /// - efficiency_issue：恒 suggest（优化建议不占自动创建名额）
    /// - success_template：freq ≥ success_template_skill_threshold → create，
    ///   否则 suggest（会话级检测器 confidence ≡ 1.0，置信度闸恒真无区分度，
    ///   频率才是真闸）
    ///
    /// 未知 pattern 类型 = loud warn（旧静默 `_` 臂改诚实告警——新增枚举值
    /// 漏接决策表时第一时间暴露）。每臂透传 pattern.evidence（C1）。
    pub fn generate_actions(&self, patterns: &[DetectedPattern]) -> Vec<LearningAction> {
        let mut actions = Vec::new();
        let high_conf = if self.config.learning.high_conf_threshold > 0.0 {
            self.config.learning.high_conf_threshold
        } else {
            0.8
        };
        let er_suggest_min = if self.config.learning.error_recovery_suggest_min > 0.0 {
            self.config.learning.error_recovery_suggest_min
        } else {
            0.2
        };
        let st_threshold = if self.config.learning.success_template_skill_threshold > 0 {
            self.config.learning.success_template_skill_threshold
        } else {
            5
        };

        for pattern in patterns {
            let pattern_id = format!(
                "p-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("0")
            );

            let mut action = match pattern.pattern_type.as_str() {
                "tool_chain" => {
                    if pattern.confidence >= high_conf && pattern.frequency >= 10 {
                        let mut action =
                            LearningAction::new("create_skill", "high", &pattern.description);
                        action.rationale = Some(format!(
                            "High-confidence tool chain ({:.2}) with frequency {}",
                            pattern.confidence, pattern.frequency
                        ));
                        action
                    } else {
                        let mut action =
                            LearningAction::new("suggest_prompt", "medium", &pattern.description);
                        action.rationale = Some(format!(
                            "Tool chain below threshold ({:.2} < {:.2}), suggest prompt",
                            pattern.confidence, high_conf
                        ));
                        action
                    }
                }

                "error_recovery" => {
                    if pattern.confidence >= high_conf {
                        let mut action =
                            LearningAction::new("create_skill", "high", &pattern.description);
                        action.rationale = Some(format!(
                            "High-confidence error recovery ({:.2}): {}",
                            pattern.confidence, pattern.description
                        ));
                        action
                    } else if pattern.confidence >= er_suggest_min {
                        let mut action =
                            LearningAction::new("suggest_prompt", "medium", &pattern.description);
                        action.rationale = Some(format!(
                            "Error recovery below create threshold ({:.2} < {:.2}), suggest prompt",
                            pattern.confidence, high_conf
                        ));
                        action
                    } else {
                        tracing::debug!(
                            pattern_type = %pattern.pattern_type,
                            confidence = pattern.confidence,
                            "[LearningEngine] error_recovery below suggest floor, no action"
                        );
                        continue;
                    }
                }

                "efficiency_issue" => {
                    let mut action =
                        LearningAction::new("suggest_prompt", "medium", &pattern.description);
                    action.rationale = Some(format!(
                        "Efficiency issue ({:.2} confidence), suggest optimization",
                        pattern.confidence
                    ));
                    action
                }

                "success_template" => {
                    if pattern.frequency >= st_threshold {
                        let mut action =
                            LearningAction::new("create_skill", "high", &pattern.description);
                        action.rationale = Some(format!(
                            "Success template ({:.2} confidence, frequency {}), automate as Skill",
                            pattern.confidence, pattern.frequency
                        ));
                        action
                    } else {
                        let mut action =
                            LearningAction::new("suggest_prompt", "medium", &pattern.description);
                        action.rationale = Some(format!(
                            "Success template below skill threshold ({} < {}), suggest prompt",
                            pattern.frequency, st_threshold
                        ));
                        action
                    }
                }

                other => {
                    tracing::warn!(
                        pattern_type = %other,
                        "[LearningEngine] Unknown pattern type reached action generation, skipped"
                    );
                    continue;
                }
            };

            action.confidence = pattern.confidence;
            action.pattern_id = Some(pattern_id);
            action.draft_name = Some(if pattern.pattern_type.as_str() == "error_recovery" {
                format!(
                    "{}-error-handler",
                    pattern.tools.first().unwrap_or(&"unknown".to_string())
                )
            } else {
                generate_skill_name(&pattern.tools.join("->"))
            });
            action.tools = pattern.tools.clone();
            action.evidence = pattern.evidence.clone();
            actions.push(action);
        }

        // Sort by priority (high first) then confidence descending
        Self::sort_actions(&mut actions);

        actions
    }

    /// Adjust confidence based on deployment outcomes (mirrors Go's adjustConfidence).
    fn adjust_confidence_from_outcomes(&self, outcomes: &[crate::monitor::EvaluationResult]) {
        for outcome in outcomes {
            if outcome.artifact_id.is_empty() {
                continue;
            }
            let delta = match outcome.verdict.as_str() {
                "positive" => Some(0.1),
                "negative" => Some(-0.2),
                _ => None,
            };
            if let Some(delta) = delta {
                self.registry.update(&outcome.artifact_id, |a| {
                    // F-: adjust the dedicated success_rate field, NOT
                    // usage_count — the old code treated the integer count as
                    // success_rate*100, corrupting its semantics.
                    a.success_rate = (a.success_rate + delta).clamp(0.0, 1.0);
                });
            }
        }
    }

    /// Disable the deployed skill file for any Degraded artifact (F-). The
    /// skills loader scans `workspace/skills/*/SKILL.md`, so renaming a
    /// degraded skill's `SKILL.md` → `SKILL.md.disabled` hides it from the
    /// agent (reversible: rename back to re-enable). This is the "rollback"
    /// endpoint of the monitor→feedback loop — without it a bad learned
    /// skill stayed an active instruction forever.
    fn disable_degraded_skills(&self) {
        self.disable_degraded_skills_impl()
    }

    /// Same as `disable_degraded_skills`, exposed for tests (F- verification).
    pub(crate) fn disable_degraded_skills_impl(&self) {
        use nemesis_types::forge::{ArtifactKind, ArtifactStatus};
        let workspace = match self.forge_dir.parent() {
            Some(w) => w,
            None => return,
        };
        let skills_root = workspace.join("skills");
        for a in self.registry.list(None, None) {
            if !matches!(a.kind, ArtifactKind::Skill) {
                continue;
            }
            if !matches!(a.status, ArtifactStatus::Degraded) {
                continue;
            }
            // name 过统一 slug+占位规则（forge::slug_or_placeholder，N 轮
            // 复查补配）：a.name 来自磁盘态注册表，历史遗留中毒名直拼 join
            // 会让 rename 面越出 workspace/skills（把外面 `<名>-forge` 目录
            // 的 SKILL.md 禁用掉）。合法名 slugify 恒等，禁用行为不变；毒名
            // 塌缩为安全单段（穿越序列）或占位段（纯符号名）= 目录不存在
            // 诚实跳过（与 exporter 读路径同语义）。
            let dir = skills_root.join(format!(
                "{}-forge",
                crate::forge::slug_or_placeholder(&a.name)
            ));
            let active = dir.join("SKILL.md");
            if active.exists() {
                let disabled = dir.join("SKILL.md.disabled");
                match std::fs::rename(&active, &disabled) {
                    Ok(_) => tracing::info!(
                        artifact = %a.id,
                        "[LearningEngine] Disabled degraded skill — rolled back from agent"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        artifact = %a.id,
                        "[LearningEngine] Failed to disable degraded skill file"
                    ),
                }
            }
        }
    }

    /// Public test-only wrapper for `adjust_confidence_from_outcomes`.
    ///
    /// Mirrors Go's `LearningEngine.AdjustConfidenceForTest` — exposes the
    /// private `adjustConfidence` logic for testing purposes.
    pub fn adjust_confidence_for_test(&self, outcomes: &[crate::monitor::EvaluationResult]) {
        self.adjust_confidence_from_outcomes(outcomes);
    }

    /// Execute create_skill action（run_cycle 内部入口，C2/C3 合一薄委托）。
    ///
    /// 单一实现 = [`Self::execute_create_skill_action`]；本方法只委托 +
    /// 按结果状态打日志。run_cycle ⑥ 消费返回值真实记账（executed /
    /// failed / skipped 分开计数，failed 不再虚计为 executed）。
    fn execute_create_skill(&self, action: &LearningAction) -> LearningAction {
        let result = self.execute_create_skill_action(action);
        match result.status.as_str() {
            "executed" => tracing::info!(
                artifact_id = ?result.artifact_id,
                "[LearningEngine] Created skill from learning cycle"
            ),
            "failed" => tracing::warn!(
                error = ?result.error_msg,
                "[LearningEngine] Skill creation failed"
            ),
            _ => tracing::debug!(
                status = %result.status,
                error = ?result.error_msg,
                "[LearningEngine] Skill creation skipped"
            ),
        }
        result
    }

    /// Execute suggest_prompt action: write a prompt suggestion to workspace/prompts/.
    fn execute_suggest_prompt(&self, action: &mut LearningAction) {
        let workspace_dir = if self.forge_dir.as_os_str().is_empty() {
            return;
        } else {
            self.forge_dir
                .parent()
                .unwrap_or(&self.forge_dir)
                .join("prompts")
        };

        if let Err(e) = std::fs::create_dir_all(&workspace_dir) {
            action.status = "failed".to_string();
            action.error_msg = Some(format!("Failed to create prompts dir: {}", e));
            return;
        }

        let draft_name = action.draft_name.as_deref().unwrap_or("unknown");
        // 防御纵深（BUG 批次 2026-11-15）：suggest 文件名与 create/蒸馏
        // 路径同源同形——入口 slugify（小写 + 非 [a-z0-9_-] 折叠为 `-` +
        // 压缩连续 `-` + 48 上限），空产出回落 "unknown"（对齐 no_draft_name
        // 臂语义；slugify 空串 = 全非法输入，不得落空文件名）。draft_name
        // 现来源（trace 工具名经 generate_skill_name）受控，但同一「自由
        // 文本落盘文件名」字段三落盘点应一套规则，防御纵深不依赖上游
        // 受控假设。
        let mut filename = crate::forge::slugify_skill_name(draft_name);
        if filename.is_empty() {
            filename = "unknown".to_string();
        }

        let content = format!(
            "# Prompt Suggestion: {}\n\n\
             ## Rationale\n{}\n\n\
             ## Pattern Description\n{}\n\n\
             ## Confidence\n{:.2}\n\n\
             ## Suggested Action\nConsider creating a Skill or improving the workflow for this pattern.\n\
             Generated: {}\n",
            draft_name,
            action.rationale.as_deref().unwrap_or("N/A"),
            action.description,
            action.confidence,
            chrono::Local::now().to_rfc3339()
        );

        let path = workspace_dir.join(format!("{}_suggestion.md", filename));
        if let Err(e) = std::fs::write(&path, &content) {
            action.status = "failed".to_string();
            action.error_msg = Some(format!("Failed to write suggestion: {}", e));
            return;
        }

        action.status = "executed".to_string();
        action.executed_at = Some(chrono::Local::now().to_rfc3339());
        action.artifact_id = Some(path.to_string_lossy().to_string());
    }

    /// Check if previously suggested prompts have been adopted (mirrors Go's checkSuggestionAdoption).
    /// Test-only: probe whether the pipeline lock is currently free (F-).
    #[cfg(test)]
    pub(crate) fn pipeline_try_lock_for_test(&self) -> bool {
        self.pipeline.try_lock().is_some()
    }

    /// Test-only wrapper for check_suggestion_adoption (F- verification).
    #[cfg(test)]
    pub(crate) fn check_suggestion_adoption_for_test(&self, patterns: &[DetectedPattern]) {
        self.check_suggestion_adoption(patterns);
    }

    fn check_suggestion_adoption(&self, _patterns: &[DetectedPattern]) {
        let prompts_dir = if self.forge_dir.as_os_str().is_empty() {
            return;
        } else {
            self.forge_dir
                .parent()
                .unwrap_or(&self.forge_dir)
                .join("prompts")
        };

        // F-: only retire prompt suggestions once at least one skill has
        // actually been deployed (Active/Observing). The old code wiped every
        // *_suggestion.md on each cycle, so suggestions never survived long
        // enough to be adopted. No precise adoption signal exists, so gate on
        // deployed-skill existence.
        let has_deployed_skill = self.registry.list(None, None).iter().any(|a| {
            use nemesis_types::forge::{ArtifactKind, ArtifactStatus};
            matches!(a.kind, ArtifactKind::Skill)
                && matches!(a.status, ArtifactStatus::Active | ArtifactStatus::Observing)
        });
        if !has_deployed_skill {
            return;
        }

        let entries = match std::fs::read_dir(&prompts_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with("_suggestion.md") {
                continue;
            }
            // If any high-confidence pattern matches, remove the suggestion
            // （清理面语义，横扫族 C 三轮：删除失败 WARN 留痕——静默滞留
            // 会让同一建议下轮重复提出；下轮重试自愈）
            nemesis_utils::fsutil::remove_file_resilient(entry.path());
        }
    }

    /// Public test-only wrapper for `execute_suggest_prompt`.
    pub fn execute_suggest_prompt_for_test(&self, action: &mut LearningAction) {
        self.execute_suggest_prompt(action);
    }

    /// Generate a skill draft using LLM (mirrors Go's generateSkillDraft).
    fn generate_skill_draft(
        &self,
        provider: &dyn crate::reflector_llm::LLMCaller,
        action: &LearningAction,
    ) -> Result<String, String> {
        let draft_name = action.draft_name.as_deref().unwrap_or("unknown-skill");
        // C1（缺陷整改批次）：把模式证据块（链序列 + 统计 + 失败样例，
        // 采集期已脱敏）随提示词下发——LLM 拿到的不再是频次数字而是
        // 可判断形态的真实证据，草稿内容与实际失败形态对齐。
        let evidence_block = match action.evidence.as_deref() {
            Some(ev) if !ev.is_empty() => format!("\n模式证据：\n{}", ev),
            _ => String::new(),
        };
        let prompt = format!(
            "请为 Forge 自学习技能生成完整的 SKILL.md，规格如下：\n\n\
             名称：{}\n\
             描述：{}\n\
             缘由：{}\n\
             {}\n\
             SKILL.md 必须包含以 --- 定界的 YAML frontmatter，字段：\n\
             - name：技能名\n\
             - description：技能功能\n\
             - version：\"1.0\"\n\n\
             随后用 Markdown 给出技能指令，定义智能代理可遵循的清晰步骤。\n\
             聚焦已识别的工具使用模式，保持简明、可执行。",
            draft_name,
            action.description,
            action.rationale.as_deref().unwrap_or("无"),
            evidence_block
        );

        // 预算统一走 forge 侧常量（reflector_llm::AUX_FORGE_MAX_TOKENS，
        // 从 500 上调：旧值被 thinking 系模型烧穿，真机两稿全空）。
        let budget = crate::reflector_llm::AUX_FORGE_MAX_TOKENS as u32;
        Self::call_llm_sync(
            provider,
            // 文本单一真相源在 `nemesis-prompts`（集中化）。
            nemesis_prompts::forge::SKILL_GENERATOR_SYSTEM_PROMPT,
            &prompt,
            budget,
        )
    }

    /// Refine a skill draft using LLM based on validation diagnosis (mirrors Go's refineSkillDraft).
    fn refine_skill_draft(
        &self,
        provider: &dyn crate::reflector_llm::LLMCaller,
        action: &LearningAction,
        previous_content: &str,
        diagnosis: &str,
    ) -> Result<String, String> {
        let draft_name = action.draft_name.as_deref().unwrap_or("unknown-skill");
        let prompt = format!(
            "以下技能草稿未通过校验。请根据诊断意见修复。\n\n\
             技能名称：{}\n\
             原始描述：{}\n\n\
             先前内容：\n{}\n\n\
             校验诊断：\n{}\n\n\
             请生成修正后的完整 SKILL.md（含以 --- 定界的 YAML frontmatter），\
             修复诊断中指出的全部问题。",
            draft_name, action.description, previous_content, diagnosis
        );

        let budget = crate::reflector_llm::AUX_FORGE_MAX_TOKENS as u32;
        Self::call_llm_sync(
            provider,
            // 文本单一真相源在 `nemesis-prompts`（集中化）。
            nemesis_prompts::forge::SKILL_FIXER_SYSTEM_PROMPT,
            &prompt,
            budget,
        )
    }

    /// Check if an artifact with the given name already exists (mirrors Go's findArtifactByFingerprint).
    fn find_artifact_by_fingerprint(&self, name: &str) -> bool {
        let artifacts = self.registry.list(None, None);
        artifacts.iter().any(|a| {
            a.name == name
                && a.status != nemesis_types::forge::ArtifactStatus::Degraded
                && a.status != nemesis_types::forge::ArtifactStatus::Archived
        })
    }

    /// Read-only peek over the manual/distill registry (`forge/registry.json`,
    /// [`crate::MANUAL_REGISTRY_FILE`]) for cross-registry dedup.
    ///
    /// 跨注册表互查面（BUG 批次 2026-11-14）：ctx.rs 装配刻意分库——本库
    /// learning_registry.json 之外，手动/蒸馏产物在 registry.json。peek 是
    /// 防御性只读（缺失 = 空库、损坏 = WARN 不阻塞），零副作用。
    fn peek_manual_registry(&self) -> crate::registry::Registry {
        crate::registry::Registry::peek_sync(&self.forge_dir.join(crate::MANUAL_REGISTRY_FILE))
    }

    /// Get the latest completed cycle (from in-memory cache).
    pub fn get_latest_cycle(&self) -> Option<LearningCycle> {
        self.latest_cycle.lock().clone()
    }

    /// Get the latest cycle from the persistent cycle store (last 30 days).
    ///
    /// This reads from disk (JSONL files) and returns the most recent cycle
    /// recorded within the last 30 days. Falls back to the in-memory cache
    /// if the store read fails.
    pub async fn get_latest_cycle_from_store(&self) -> Option<LearningCycle> {
        let since = chrono::Local::now() - chrono::Duration::days(30);

        match self.cycle_store.read_cycles(Some(since)).await {
            Ok(cycles) => cycles.into_iter().last(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "[LearningEngine] Failed to read cycles from store, falling back to in-memory cache"
                );
                self.latest_cycle.lock().clone()
            }
        }
    }

    /// Evaluate previous deployment outcomes and adjust confidence.
    pub fn evaluate_outcomes(&self, outcomes: &[DeploymentOutcome]) {
        for outcome in outcomes {
            if outcome.artifact_id.is_empty() {
                continue;
            }

            let delta = match outcome.verdict.as_str() {
                "positive" => Some(0.1),
                "negative" => Some(-0.2),
                _ => None,
            };

            if let Some(delta) = delta {
                self.registry.update(&outcome.artifact_id, |a| {
                    // 复查：原实现算出 new_rate 后 `let _ =`
                    // 丢弃——空转假成功（调用方以为反馈闭环生效，实际什么都没
                    // 改）。现真正落字段：正评 +0.1 / 负评 -0.2，钳制 0-1，
                    // 与 monitor 的 adjust_confidence_from_outcomes 同语义
                    // （那里调 success_rate，此处同字段收口，两处不冲突：
                    // monitor 走周期度量，这里走部署验收事件）。
                    a.success_rate = (a.success_rate + delta).clamp(0.0, 1.0);
                });
            }
        }
    }

    // ----- 单一 create 实现（C2/C3 合一后唯一的技能创建入口） -----

    /// Execute a create_skill action using LLM, validation, and deploy.
    ///
    /// C2/C3 合一（缺陷整改批次 2026-11-14）：唯一的 create 实现（此前
    /// 与私有 `execute_create_skill` 是两份漂移的平行实现——workspace
    /// `-forge` 副本步只在私有版，公开版漂移缺步）。完整链路：slug 化 →
    /// 查重 → LLM 生成 → validate + refine 循环 → 安全闸 → forge/skills
    /// 落盘 → registry 登记 → workspace/skills `-forge` 副本 → 返回带
    /// status/artifact_id/error_msg 的结果 action（run_cycle 按此真实记账）。
    pub fn execute_create_skill_action(&self, action: &LearningAction) -> LearningAction {
        let mut result = action.clone();

        let draft_name = match &action.draft_name {
            Some(name) => name.clone(),
            None => {
                result.status = "failed".to_string();
                result.error_msg = Some("No draft name provided".to_string());
                return result;
            }
        };
        // 落盘名统一过 slug 化（防御纵深，与 store_distilled_draft 同一
        // 规则统一定义）：draft_name 源自经验聚合的工具链（generate_skill_name
        // 只 replace+lowercase，无字符白名单；error_recovery 分支直拼
        // tools.first()），下游有 3 个路径 join 点（forge/skills 落盘、
        // workspace/skills 部署、degraded 回滚 rename）。slugify 对合法
        // kebab 名是恒等变换，对穿越序列消解。
        let draft_name = crate::forge::slugify_skill_name(&draft_name);
        if draft_name.is_empty() {
            result.status = "failed".to_string();
            result.error_msg = Some("Draft name slugified to empty".to_string());
            return result;
        }

        // Check if already exists in registry (dedup by name)
        if self.find_artifact_by_fingerprint(&draft_name) {
            result.status = "skipped".to_string();
            result.error_msg = Some(format!("Artifact {} already exists", draft_name));
            return result;
        }

        // 跨注册表同名防线（BUG 批次 2026-11-14）：手动/蒸馏侧的 registry.json
        // 里若有同名活跃技能，本库照建会静默覆盖对方的 SKILL.md 磁盘文件
        // （两侧都写 forge/skills/{slug}/）。活跃态语义与 find_artifact_by_
        // fingerprint 同口径：Degraded/Archived 放行 = 替代品顶替。
        if let Some(existing) = self
            .peek_manual_registry()
            .find_by_name(&draft_name, nemesis_types::forge::ArtifactKind::Skill)
            && !matches!(
                existing.status,
                nemesis_types::forge::ArtifactStatus::Degraded
                    | nemesis_types::forge::ArtifactStatus::Archived
            )
        {
            result.status = "skipped".to_string();
            result.error_msg = Some(format!(
                "Artifact {} already exists in manual registry (cross-registry guard)",
                draft_name
            ));
            return result;
        }

        // Generate skill draft using LLM
        let provider_arc = {
            let provider = self.provider.lock();
            match provider.as_ref() {
                Some(p) => p.clone(),
                None => {
                    result.status = "failed".to_string();
                    result.error_msg = Some("No LLM provider available".to_string());
                    return result;
                }
            }
        };

        let mut current_content = match self.generate_skill_draft(&*provider_arc, action) {
            Ok(c) => c,
            Err(e) => {
                result.status = "failed".to_string();
                result.error_msg = Some(format!("LLM generation failed: {}", e));
                return result;
            }
        };

        // Iterative refinement loop
        // （refine 轮数预算统一读 `max_refine_rounds`——BUG-20261029-189
        // 修正后两路径同字段收口，C2 合一后此旋钮只剩这一处消费。）
        let max_refine = self.config.learning.max_refine_rounds;

        // Arc clone 后立即放锁（F- 同款模式）：迭代 refine 是 30s+ 墙钟
        // 的 LLM 调用，持 pipeline 锁跨 LLM 会把并发路径（observe/reflect
        // 等一切读 pipeline 的调用）整段卡死（四路评审 A- 根修）。快照
        // 语义 = 本轮创建流程锚定一个 pipeline 实例，set_pipeline 热替换
        // 下轮生效。
        let pipeline = { self.pipeline.lock().clone() };
        if let Some(ref pipeline) = pipeline {
            // C3：refine 调用失败（LLM/网络层）≠ 校验耗尽——独立跟踪，
            // 收尾文案分叉。
            let mut refine_failed: Option<String> = None;
            for attempt in 0..=max_refine {
                let validation = pipeline.validate(
                    nemesis_types::forge::ArtifactKind::Skill,
                    &draft_name,
                    &current_content,
                );
                let new_status = pipeline.determine_status(&validation);

                if new_status == nemesis_types::forge::ArtifactStatus::Active
                    || new_status == nemesis_types::forge::ArtifactStatus::Observing
                {
                    // F- 安全闸（同类扫）：与 execute_create_skill
                    // 部署点同款——refine 终态内容过 security_errors，拦下则本轮
                    // 诚实失败（不落盘不注册）。
                    let sec_errors =
                        crate::validator::StaticValidator::new().security_errors(&current_content);
                    if !sec_errors.is_empty() {
                        result.status = "failed".to_string();
                        result.error_msg = Some(format!(
                            "security gate rejected skill draft: {}",
                            sec_errors.join("; ")
                        ));
                        return result;
                    }

                    // 签名取结构化 pattern.tools（无则回落旧解析）——
                    // 与 execute_create_skill 私有实现同一规则（两条平行
                    // create 路径必须同步演进，见 同款纪律）。
                    let tool_sig = deploy_tool_signature(action);

                    // Write content to forge skills directory
                    // 横扫族 C 四轮（2026-11-06）：与私有 execute_create_skill
                    // 同口径——写盘失败 = 不能部署 = 不注册，result 诚实
                    // failed（两条平行 create 路径必须同步演进，R17 纪律）。
                    let artifact_dir = self.forge_dir.join("skills").join(&draft_name);
                    if let Err(e) = std::fs::create_dir_all(&artifact_dir).and_then(|_| {
                        std::fs::write(artifact_dir.join("SKILL.md"), &current_content)
                    }) {
                        tracing::warn!(
                            name = %draft_name,
                            error = %e,
                            "[LearningEngine] skill SKILL.md write failed, not deploying (not registered)"
                        );
                        result.status = "failed".to_string();
                        result.error_msg = Some(format!("skill SKILL.md write failed: {e}"));
                        return result;
                    }

                    // Register in registry
                    let artifact_id = format!("skill-{}", draft_name);
                    let artifact = nemesis_types::forge::Artifact {
                        id: artifact_id.clone(),
                        name: draft_name.clone(),
                        kind: nemesis_types::forge::ArtifactKind::Skill,
                        version: "1.0".to_string(),
                        status: new_status,
                        content: current_content.clone(),
                        tool_signature: tool_sig,
                        created_at: chrono::Local::now().to_rfc3339(),
                        updated_at: chrono::Local::now().to_rfc3339(),
                        usage_count: 0,
                        last_degraded_at: None,
                        success_rate: 0.0,
                        consecutive_observing_rounds: 0,
                        deployed_at: None,
                        last_efficacy: None,
                    };
                    self.registry.add(artifact);

                    // Copy to workspace/skills/ with -forge suffix
                    // （副本失败 WARN 留痕不回滚：forge 主副本已落盘已注册，
                    // 缺副本=skills 列表看不见，横扫族 C 四轮，2026-11-06。
                    // C2 合一补齐：此步此前只在私有实现存在，公开实现漂移
                    // 缺步——同一样技能两条路径可见性不一致。）
                    if let Some(workspace) = self.forge_dir.parent() {
                        let ws_skill_dir = workspace
                            .join("skills")
                            .join(format!("{}-forge", draft_name));
                        if let Err(e) = std::fs::create_dir_all(&ws_skill_dir).and_then(|_| {
                            std::fs::write(ws_skill_dir.join("SKILL.md"), &current_content)
                        }) {
                            tracing::warn!(
                                path = %ws_skill_dir.display(),
                                error = %e,
                                "[LearningEngine] workspace skill copy failed (forge primary copy intact)"
                            );
                        }
                    }

                    result.status = "executed".to_string();
                    result.artifact_id = Some(artifact_id);
                    result.executed_at = Some(chrono::Local::now().to_rfc3339());
                    return result;
                }

                // Failed - try to refine
                if attempt < max_refine {
                    let diagnosis = build_diagnosis(&validation);
                    match self.refine_skill_draft(
                        &*provider_arc,
                        action,
                        &current_content,
                        &diagnosis,
                    ) {
                        Ok(refined) => {
                            tracing::debug!(
                                attempt,
                                "[LearningEngine] Refined skill draft, re-validating"
                            );
                            current_content = refined;
                        }
                        Err(e) => {
                            tracing::warn!(attempt = attempt + 1, error = %e, "[LearningEngine] Skill refinement failed");
                            // C3：refine 调用失败（LLM/网络层）≠ 校验耗尽——
                            // 独立跟踪，收尾文案分叉，不再一律「validation
                            // failed after N rounds」说谎。
                            refine_failed = Some(e);
                            break;
                        }
                    }
                }
            }

            result.status = "failed".to_string();
            result.error_msg = Some(match refine_failed {
                Some(e) => format!("skill refinement failed: {e}"),
                None => format!(
                    "Skill validation failed after {} refinement rounds",
                    max_refine
                ),
            });
        } else {
            // No pipeline configured - register as draft
            let artifact_id = format!("skill-{}", draft_name);
            let artifact = nemesis_types::forge::Artifact {
                id: artifact_id.clone(),
                name: draft_name,
                kind: nemesis_types::forge::ArtifactKind::Skill,
                version: "1.0".to_string(),
                status: nemesis_types::forge::ArtifactStatus::Draft,
                content: current_content,
                // 与部署臂同一规则（结构化 tools 优先，回落旧解析）。
                tool_signature: deploy_tool_signature(action),
                created_at: chrono::Local::now().to_rfc3339(),
                updated_at: chrono::Local::now().to_rfc3339(),
                usage_count: 0,
                last_degraded_at: None,
                success_rate: 0.0,
                consecutive_observing_rounds: 0,
                deployed_at: None,
                last_efficacy: None,
            };
            self.registry.add(artifact);

            result.status = "executed".to_string();
            result.artifact_id = Some(artifact_id);
            result.executed_at = Some(chrono::Local::now().to_rfc3339());
        }

        result
    }

    /// Sort learning actions by priority (high first) then confidence descending.
    ///
    /// Mirrors Go's `sortActions`.
    pub fn sort_actions(actions: &mut [LearningAction]) {
        let priority_order = |p: &str| -> u8 {
            match p {
                "high" => 0,
                "medium" => 1,
                "low" => 2,
                _ => 3,
            }
        };
        actions.sort_by(|a, b| {
            let pa = priority_order(&a.priority);
            let pb = priority_order(&b.priority);
            match pa.cmp(&pb) {
                std::cmp::Ordering::Equal => b
                    .confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
                other => other,
            }
        });
    }
}

/// Generate a skill name from a tool chain description.
fn generate_skill_name(tool_chain: &str) -> String {
    let mut name = tool_chain
        .replace("->", "-")
        .replace('_', "-")
        .to_lowercase();
    if name.len() > 50 {
        let cut = nemesis_types::utils::floor_char_boundary(&name, 50);
        name.truncate(cut);
    }
    format!("{}-workflow", name)
}

/// Extract tool names from a chain description like "read->edit->exec" (mirrors Go's extractToolSignatureFromChain).
fn extract_tool_signature_from_chain(description: &str) -> Vec<String> {
    let parts: Vec<&str> = description.split("→").collect();
    let mut tools = Vec::new();
    for p in parts {
        let p = p.trim();
        // Remove leading text like "Tool chain: "
        let tool = if let Some(idx) = p.rfind(' ') {
            p[idx + 1..].to_string()
        } else {
            p.to_string()
        };
        if !tool.is_empty() {
            tools.push(tool);
        }
    }
    tools
}

/// 部署期 tool_signature 单一来源（复查）：优先取
/// `action.tools`（检测器结构化携带，generate_actions 填充），空则回落
/// description 按 `→` 解析（兼容手工构造的 action 与其既有测试语义）。
/// 此前两个部署点都直接走 description 解析，而 pattern 描述是英文散文
/// （如 "High-frequency tool usage: exec (12 times, 40%)"）不含箭头，
/// 落库的是垃圾尾词（如 `["40%)"]`）——artifact.tool_signature 从此失真。
fn deploy_tool_signature(action: &LearningAction) -> Vec<String> {
    if action.tools.is_empty() {
        extract_tool_signature_from_chain(&action.description)
    } else {
        action.tools.clone()
    }
}

// ---------------------------------------------------------------------------
// C1（缺陷整改批次 2026-11-14）：模式证据块
// ---------------------------------------------------------------------------

/// 单个模式证据块携带的最多失败样例数。
const MAX_EVIDENCE_SAMPLES: usize = 3;

/// 单条样例的字符上限（超出截尾——与蒸馏提示同族纪律，防大载荷经验
/// 把草稿生成提示词预算吃穿）。
const MAX_EVIDENCE_SAMPLE_CHARS: usize = 220;

/// Build a sanitized evidence block for a detected pattern (C1).
///
/// 草稿生成提示词的素材从「频次数字 + 一句描述」升级为结构化真实证据：
/// 链序列 / 恢复链 / 出现次数 / 成功率 / 平均耗时 / 平均轮数 + 最多
/// [`MAX_EVIDENCE_SAMPLES`] 条失败样例（Sanitizer 脱敏 + 字符边界截断）。
/// 样例选取条件 = 工具命中该模式（error_recovery 按 error_tool，其余按
/// 链上工具）&& 失败 && 输出摘要非空。返回 `None` = 无可引用素材
/// （提示词端省略证据块）。
fn build_pattern_evidence(
    cp: &crate::pattern::ConversationPattern,
    experiences: &[CollectedExperience],
) -> Option<String> {
    let want_tools: Vec<String> = match cp.pattern_type {
        crate::pattern::ConversationPatternType::ErrorRecovery => {
            vec![cp.error_tool.clone().unwrap_or_default()]
        }
        _ => cp
            .tool_chain
            .as_deref()
            .unwrap_or_default()
            .split('\u{2192}')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect(),
    };
    if want_tools.is_empty() {
        return None;
    }

    let sanitizer = crate::sanitizer::Sanitizer::new();
    let mut samples = Vec::new();
    for exp in experiences {
        if samples.len() >= MAX_EVIDENCE_SAMPLES {
            break;
        }
        let e = &exp.experience;
        if !want_tools.contains(&e.tool_name) {
            continue;
        }
        if e.success || e.output_summary.is_empty() {
            continue;
        }
        let sanitized = sanitizer.sanitize(&e.output_summary);
        let cut = nemesis_types::utils::floor_char_boundary(&sanitized, MAX_EVIDENCE_SAMPLE_CHARS);
        samples.push(format!(
            "- [{}] {} {}ms：{}",
            e.tool_name,
            if e.success { "OK" } else { "FAIL" },
            e.duration_ms,
            &sanitized[..cut]
        ));
    }

    let mut sb = String::new();
    if let Some(chain) = &cp.tool_chain {
        sb.push_str(&format!("链序列：{}\n", chain));
    }
    if cp.pattern_type == crate::pattern::ConversationPatternType::ErrorRecovery {
        if let (Some(et), Some(rt)) = (&cp.error_tool, &cp.recovery_tool) {
            sb.push_str(&format!("恢复链：{} 失败 → {} 恢复\n", et, rt));
        }
        if let Some(code) = &cp.error_code {
            sb.push_str(&format!("错误码：{}\n", code));
        }
    }
    sb.push_str(&format!("出现次数：{}\n", cp.frequency));
    if let Some(sr) = cp.success_rate {
        sb.push_str(&format!("成功率：{:.0}%\n", sr * 100.0));
    }
    if let Some(d) = cp.avg_duration_ms {
        sb.push_str(&format!("平均耗时：{}ms\n", d));
    }
    if let Some(r) = cp.avg_rounds {
        sb.push_str(&format!("平均轮数：{:.1}\n", r));
    }
    if !samples.is_empty() {
        sb.push_str("失败样例（脱敏）：\n");
        for s in &samples {
            sb.push_str(s);
            sb.push('\n');
        }
    }

    if sb.is_empty() { None } else { Some(sb) }
}

/// Build a diagnosis string from validation results (mirrors Go's buildDiagnosis).
fn build_diagnosis(validation: &ArtifactValidation) -> String {
    let mut sb = String::new();

    if let Some(ref s1) = validation.stage1_static
        && !s1.stage.passed
    {
        sb.push_str("Stage 1 (Static) FAILED:\n");
        for e in &s1.stage.errors {
            sb.push_str(&format!("  - {}\n", e));
        }
    }

    if let Some(ref s2) = validation.stage2_functional
        && !s2.stage.passed
    {
        sb.push_str("Stage 2 (Functional) FAILED:\n");
        for e in &s2.stage.errors {
            sb.push_str(&format!("  - {}\n", e));
        }
    }

    if let Some(ref s3) = validation.stage3_quality {
        sb.push_str(&format!("Stage 3 (Quality) Score: {}/100\n", s3.score));
        if !s3.notes.is_empty() {
            sb.push_str(&format!("  Notes: {}\n", s3.notes));
        }
        for (dim, score) in &s3.dimensions {
            sb.push_str(&format!("  {}: {}\n", dim, score));
        }
    }

    sb
}

/// Convert a LearningAction to a summary for cycle storage.
#[allow(dead_code)]
fn action_to_summary(a: &LearningAction) -> ActionSummary {
    ActionSummary {
        id: a.id.clone(),
        action_type: a.action_type.clone(),
        priority: a.priority.clone(),
        status: a.status.clone(),
        artifact_id: a.artifact_id.clone(),
    }
}

/// Iterative refinement loop for skill generation.
///
/// Takes an initial skill content, validates it, and attempts to refine
/// up to `max_rounds` times if validation fails.
pub struct IterativeRefiner {
    /// Maximum refinement rounds.
    pub max_rounds: u32,
}

impl IterativeRefiner {
    /// Create a new refiner with the given max rounds.
    pub fn new(max_rounds: u32) -> Self {
        let max_rounds = if max_rounds == 0 { 3 } else { max_rounds };
        Self { max_rounds }
    }

    /// Run the refinement loop on skill content.
    ///
    /// Returns the refined content if it passes validation, or the last
    /// attempt if all rounds are exhausted.
    pub fn refine<F>(&self, initial_content: &str, validate: F) -> (String, bool)
    where
        F: Fn(&str) -> bool,
    {
        let mut content = initial_content.to_string();

        for round in 0..=self.max_rounds {
            if validate(&content) {
                return (content, true);
            }

            if round < self.max_rounds {
                // Apply simple refinement heuristics:
                // - If content is too short, add structure
                // - If content has no headers, add them
                // - If content has no steps, add step markers
                content = self.apply_refinement_heuristics(&content, round);
            }
        }

        (content, false)
    }

    /// Apply simple refinement heuristics to improve skill content.
    fn apply_refinement_heuristics(&self, content: &str, round: u32) -> String {
        let mut refined = content.to_string();

        // Round 0: Add YAML frontmatter if missing
        if !content.contains("---") {
            refined = format!(
                "---\nname: generated-skill\nversion: \"1.0\"\n---\n\n{}",
                refined
            );
        }

        // Round 1: Add structure if content is flat
        if round >= 1 && !content.contains("## ") {
            refined = format!(
                "{}\n\n## Steps\n\n1. Execute the identified pattern\n2. Validate results\n3. Report outcome",
                refined
            );
        }

        // Round 2: Add error handling section
        if round >= 2 && !content.contains("Error") && !content.contains("error") {
            refined = format!(
                "{}\n\n## Error Handling\n\nIf any step fails, retry once. If still failing, report the error and continue.",
                refined
            );
        }

        refined
    }
}

#[cfg(test)]
mod tests;
