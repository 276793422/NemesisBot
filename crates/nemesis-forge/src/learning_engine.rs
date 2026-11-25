//! Learning engine - closed-loop monitoring (自学习铺路批次三收窄).
//!
//! Evaluates deployment outcomes, adjusts artifact confidence, and extracts
//! conversation patterns（模式只供证据包消费；suggest_prompt 一族已随
//! 自学习铺路批次四退役）.
//!
//! Supports four pattern detectors:
//! - tool_chain: High-frequency tool sequences
//! - error_recovery: Tools that fail then succeed
//! - efficiency_issue: Slow or wasteful tool usage
//! - success_template: Consistently successful tool patterns
//!
//! 线 A（B2 决策表 + LLM 草稿生成/校验/部署）已随自学习铺路批次三整体
//! 退役——学习动作改由多轮学习对话引擎（`learn_dialogue`）承担，本模块
//! 只保留监控与实效闭环（run_cycle Steps 1-4）。

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::ForgeConfig;
use crate::cycle_store::CycleStore;
use crate::monitor::DeploymentMonitor;
use crate::registry::Registry;
use crate::types::CollectedExperience;
use nemesis_types::forge::LearningCycle;

// 自学习铺路批次三：线 A（generate_actions 决策表 + execute_create_skill
// LLM 草稿生成/校验/部署 + LLM provider/pipeline 注入面）整体退役——
// 学习动作由多轮学习对话引擎（`learn_dialogue`）承担。本模块收窄为
// 监控与实效闭环（run_cycle Steps 1-4 + suggest 家族批次四保留件）。
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

/// The learning engine drives the closed-loop monitoring cycle.
pub struct LearningEngine {
    config: ForgeConfig,
    forge_dir: PathBuf,
    registry: Arc<Registry>,
    cycle_store: CycleStore,
    monitor: Mutex<Option<Arc<DeploymentMonitor>>>,
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
            monitor: Mutex::new(None),
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
            monitor: Mutex::new(None),
            latest_cycle: Mutex::new(None),
        }
    }

    /// Set the deployment monitor for outcome evaluation.
    pub fn set_monitor(&self, monitor: Arc<DeploymentMonitor>) {
        tracing::info!("[Forge/LearningEngine] Deployment monitor set");
        *self.monitor.lock() = Some(monitor);
    }

    /// Run a single learning cycle.
    ///
    /// 自学习铺路批次三收窄为监控与实效闭环（原 Steps 5-6 的 B2 决策表
    /// action 生成/执行随线 A 整体退役，学习动作改由 `learn_dialogue`
    /// 多轮对话引擎承担）：
    /// 1. Evaluate previous deployment outcomes
    /// 2. Adjust confidence based on feedback
    /// 3. Extract patterns from experiences
    /// 4. Save cycle record
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

        // Steps 5-6（generate_actions 决策表 + action 执行）已随自学习铺路
        // 批次三线 A 退役：学习动作改由 `learn_dialogue` 多轮对话引擎承担
        // （Forge 层 run_reflection_cycle 编排），本周期只产出模式供证据
        // 包消费。actions_taken 恒 0（保留字段兼容 LearningCycle 形态）。

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
            Ok(mut cycles) => {
                // BUG-20261118-167 横向排查（同族 BUG-20261118-140）：started_at
                // 是带本地 offset 的 RFC3339 字符串，read_cycles 只保证月目录/
                // 文件名序 + JSONL 行 append 序，不保证 started_at 时刻序（跨
                // DST 切换/系统时区变更后同批混存不同 offset 形态，文件序≠时刻
                // 序）。doc 承诺 "most recent"，必须显式按 UTC 时刻排序后再取
                // 尾——对齐同 crate load_latest_cycle 的 BUG-140 修复形态，
                // 单一真相源 rfc3339_order。
                cycles.sort_by(|a, b| crate::pattern::rfc3339_order(&a.started_at, &b.started_at));
                cycles.into_iter().last()
            }
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

#[cfg(test)]
mod tests;
