//! Deployment monitor - evaluates deployed artifact effectiveness.
//!
//! Tracks usage statistics and determines whether an artifact should be
//! promoted, kept in observation, or degraded.
//!
//! Enhanced with:
//! - Tool signature subsequence matching (matching Go's `matchesToolSignature`)
//! - Before/after comparison using conversation traces
//! - Effectiveness scoring: 0.4*rounds + 0.4*success + 0.2*duration
//! - 7-day cooldown check before degradation (respects `last_degraded_at`)
//! - Auto-upgrade: 3 consecutive "observing" rounds triggers "negative"
//!   verdict and automatic degradation
//! - `classify_verdict()` with configurable threshold from config
//! - `handle_verdict()` / `track_observing()` / `try_deprecate()` flow

use std::sync::Arc;

use crate::config::ForgeConfig;
use crate::registry::Registry;
use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};

/// Evaluation result for a deployed artifact.
#[derive(Debug, Clone)]
pub struct EvaluationResult {
    /// Artifact ID.
    pub artifact_id: String,
    /// Verdict: positive, neutral, observing, negative, insufficient_data.
    pub verdict: String,
    /// Improvement score (positive = better, negative = worse).
    pub improvement_score: f64,
    /// Sample size used for evaluation.
    pub sample_size: usize,
}

/// Outcome of evaluating an artifact against conversation traces.
///
/// Matches Go's `ActionOutcome` struct.
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    /// Artifact ID.
    pub artifact_id: String,
    /// When the measurement was taken.
    pub measured_at: String,
    /// Number of traces used for evaluation.
    pub sample_size: usize,
    /// Traces before deployment (F 线实效快照 samples_before 数据源——
    /// `sample_size` 是 after 侧样本数，两者不同源不可混填）。
    pub sample_size_before: usize,
    /// Average rounds before deployment.
    pub rounds_before_avg: f64,
    /// Average rounds after deployment.
    pub rounds_after_avg: f64,
    /// Success rate before deployment (0.0-1.0).
    pub success_before: f64,
    /// Success rate after deployment (0.0-1.0).
    pub success_after: f64,
    /// Average duration before deployment (ms).
    pub duration_before_ms: i64,
    /// Average duration after deployment (ms).
    pub duration_after_ms: i64,
    /// Normalized improvement score.
    pub improvement_score: f64,
    /// Verdict: positive, neutral, observing, negative, insufficient_data.
    pub verdict: String,
}

/// Simplified conversation trace for evaluation purposes.
///
/// In Go, this maps to `ConversationTrace`. The monitor uses these
/// to compute before/after metrics for artifact evaluation.
#[derive(Debug, Clone)]
pub struct ConversationTrace {
    /// Trace start time (RFC 3339).
    pub start_time: String,
    /// Total rounds of interaction = llm_turn 事件数（缺陷整改批次
    /// D2：旧实现按事件总数代理——tool_call 密集的会话轮数被灌水）。
    /// 旧轨迹（无 llm_turn 事件，agent 未发）诚实为 0。
    pub total_rounds: u32,
    /// Duration in milliseconds (first→last event span).
    pub duration_ms: i64,
    /// Tool steps in the conversation.
    pub tool_steps: Vec<ToolStep>,
    /// llm_turn 事件时间戳序列（D3 事件级 before/after 切分的轮数分母）。
    pub round_ts: Vec<String>,
    /// Detected signals (empty = success).
    pub signals: Vec<String>,
}

/// A single tool step in a conversation trace.
#[derive(Debug, Clone, Default)]
pub struct ToolStep {
    /// Tool name used.
    pub tool_name: String,
    /// Whether the step succeeded（D2 事件粒度 success_rate 的分子来源；
    /// 旧事件缺 ok 字段时诚实回 true——无从判定就不假造失败）。
    pub ok: bool,
    /// Step duration in milliseconds（D2 步级 avg_duration 的样本）。
    pub duration_ms: i64,
    /// Step timestamp (RFC 3339, D3 事件级切分的锚点)。
    pub ts: String,
}

/// Monitors deployed artifacts and evaluates their effectiveness.
pub struct DeploymentMonitor {
    config: ForgeConfig,
    registry: Arc<Registry>,
}

impl DeploymentMonitor {
    /// Create a new deployment monitor.
    pub fn new(config: ForgeConfig, registry: Arc<Registry>) -> Self {
        Self { config, registry }
    }

    /// Evaluate a single artifact's deployment effectiveness (simple mode).
    ///
    /// Uses `success_rate` and `usage_count` as proxies when traces are not available.
    /// Matches Go's scoring logic using the same formula:
    /// `improvement = success_rate - baseline`, where baseline is 0.5.
    pub fn evaluate(&self, artifact: &Artifact) -> EvaluationResult {
        let sample_size = artifact.usage_count as usize;
        let min_samples = self.config.learning.min_outcome_samples as usize;
        let min_samples = if min_samples == 0 { 5 } else { min_samples };

        if sample_size < min_samples {
            return EvaluationResult {
                artifact_id: artifact.id.clone(),
                verdict: "observing".into(),
                improvement_score: 0.0,
                sample_size,
            };
        }

        // Calculate effectiveness score using success_rate field
        // Matching Go's pattern: success rate above 0.5 baseline is positive
        let improvement = artifact.success_rate - 0.5;

        let verdict = self.classify_verdict(improvement, artifact);

        EvaluationResult {
            artifact_id: artifact.id.clone(),
            verdict: verdict.to_string(),
            improvement_score: improvement,
            sample_size,
        }
    }

    /// Classify verdict based on improvement score and consecutive observing rounds.
    ///
    /// Matches Go's `classifyVerdict()`:
    /// - improvement > 0.1 => "positive"
    /// - improvement >= -0.1 => "neutral"
    /// - improvement >= threshold => "observing"
    /// - otherwise => "negative"
    pub fn classify_verdict(&self, improvement_score: f64, _artifact: &Artifact) -> String {
        let threshold = self.config.learning.degrade_threshold;
        // BUG-20261113-161：旧守卫在此把 `threshold == 0.0` 当「未配置默认值」
        // 回落 -0.2——前提自 INIT 起即为假：serde default 与出厂模板都是
        // -0.2，`== 0.0` 唯一入口是用户显式配置。显式 0.0 是合法最严档
        // （observing 缓冲带 [-0.1, 0) 为空、低于 -0.1 一律 negative），静默
        // 改回 -0.2 恰是旧注释声称要消灭的「silently overriding
        // user-configured threshold」。删除回落臂：未配置走 serde default
        // -0.2，显式配置值（含 0.0）一律尊重。

        if improvement_score > 0.1 {
            "positive"
        } else if improvement_score >= -0.1 {
            "neutral"
        } else if improvement_score >= threshold {
            "observing"
        } else {
            "negative"
        }
        .to_string()
    }

    /// Evaluate outcomes for all active artifacts using conversation traces.
    ///
    /// Matches Go's `EvaluateOutcomes()`. This is the trace-based evaluation
    /// that compares before/after metrics.
    ///
    /// Returns `ActionOutcome` for each evaluated artifact.
    pub fn evaluate_outcomes(&self, traces: &[ConversationTrace]) -> Vec<ActionOutcome> {
        let mut outcomes = Vec::new();

        let window_days = self.config.learning.monitor_window_days;
        let window_days = if window_days == 0 { 7 } else { window_days };

        // Filter traces within the observation window
        // （window_days 是 u32：as i64 恒正，4.29e9 天 < Duration::days
        // 恐慌带 ~1.07e14——无回绕/恐慌面，无需钳制。）
        let since = chrono::Local::now() - chrono::Duration::days(window_days as i64);
        let recent_traces: Vec<&ConversationTrace> = traces
            .iter()
            .filter(|t| {
                // 全量偏移感知解析（验证轮二）；缺陷整改批次 D1 收编到
                // parse_ts 单一解析锚点。不可解析仍放行（沿旧语义）。
                parse_ts(&t.start_time)
                    .map(|dt| dt >= since)
                    .unwrap_or(true)
            })
            .collect();

        if recent_traces.is_empty() {
            return outcomes;
        }

        // 样本护栏（验证轮四接线）：`min_traces_for_analysis`
        // 此前零消费者（死键）。窗口内轨迹少于 N 条不做实效分析——样本
        // 太少，结论即噪声。0 = 不设下限（默认，护栏关）；存量配置模板
        // 自带 5 的部署按其声明生效。
        let min_traces = self.config.trace.min_traces_for_analysis;
        if min_traces > 0 && recent_traces.len() < min_traces {
            return outcomes;
        }

        // Get all active skills with ToolSignatures
        let artifacts = self.registry.list(None, None);
        for artifact in &artifacts {
            if artifact.status != ArtifactStatus::Active
                || artifact.kind != ArtifactKind::Skill
                || artifact.tool_signature.is_empty()
            {
                continue;
            }

            if let Some(outcome) = self.evaluate_artifact(artifact, &recent_traces) {
                // Handle auto-deprecation
                self.handle_verdict(artifact, &outcome);
                // F 线：实效快照写回 registry（Dashboard/裁决数据源）。
                // insufficient_data（样本不足、无结论）**不写回**：快照槽位
                // 语义是「最后一次有效实效结论」，无结论测量覆盖它会周期性
                // 冲掉历史证据（滚动窗口把 after 样本滚出窗后，verdict=
                // positive 的快照会被 insufficient_data + 全 0 数值冒充的
                // 「新结论」顶掉——handle_verdict 对同一 outcome 走 `_ =>`
                // 忽略，两个消费面语义必须一致）。outcome 仍进返回值。
                if outcome.verdict != "insufficient_data" {
                    let snapshot = nemesis_types::forge::EfficacySnapshot {
                        measured_at: outcome.measured_at.clone(),
                        success_before: outcome.success_before,
                        success_after: outcome.success_after,
                        samples_before: outcome.sample_size_before,
                        samples_after: outcome.sample_size,
                        verdict: outcome.verdict.clone(),
                    };
                    self.registry.update(&artifact.id, |a| {
                        a.last_efficacy = Some(snapshot.clone());
                    });
                }
                outcomes.push(outcome);
            }
        }

        outcomes
    }

    /// Evaluate a single artifact's effectiveness against traces.
    ///
    /// Matches Go's `evaluateArtifact()`. Uses tool signature subsequence
    /// matching to find relevant traces, then computes before/after metrics.
    ///
    /// 缺陷整改批次 D1+D3：
    /// - D1 分桶比较换解析序（`ts_before`：双侧可解析=时刻序，跨时区偏移
    ///   正确；任一侧不可解析回落字符串序——不劣于旧实现）。
    /// - D3 事件级切分——长命会话跨部署点不再整桶掺水。每条 matched
    ///   trace 的 tool_steps / round_ts 按 `ts_before(事件 ts, deploy_time)`
    ///   拆 ≤2 条子轨迹；子轨迹非空 = 有步或有轮（一侧无事件不计样本，
    ///   不再让「整条轨迹挂 before 名下」稀释 after 指标）。avg_rounds
    ///   在子轨迹上的真值 = 该桶 round_ts 条数（llm_turn 事件计数）。
    pub fn evaluate_artifact(
        &self,
        artifact: &Artifact,
        traces: &[&ConversationTrace],
    ) -> Option<ActionOutcome> {
        // F 线：显式 deployed_at 优先（SPO 迭代重部署独立实效窗）；
        // None = 回落 created_at（旧行为零变化）。
        let deploy_time = artifact
            .deployed_at
            .as_deref()
            .unwrap_or(artifact.created_at.as_str());

        let mut before_traces: Vec<ConversationTrace> = Vec::new();
        let mut after_traces: Vec<ConversationTrace> = Vec::new();

        for t in traces {
            if !matches_tool_signature(t, &artifact.tool_signature) {
                continue;
            }
            let before_steps: Vec<ToolStep> = t
                .tool_steps
                .iter()
                .filter(|s| ts_before(&s.ts, deploy_time))
                .cloned()
                .collect();
            let before_rounds: Vec<String> = t
                .round_ts
                .iter()
                .filter(|ts| ts_before(ts, deploy_time))
                .cloned()
                .collect();
            // 双侧构造：非空侧（有步或有轮）才成样本；空侧丢弃。
            // 子轨迹 start_time 保留母轨迹值（仅描述性字段，不参与计算）；
            // signals/duration_ms 是轨迹级旧度量残留，事件级指标不消费。
            for (steps, rounds, bucket) in [
                (
                    before_steps,
                    before_rounds,
                    &mut before_traces as &mut Vec<ConversationTrace>,
                ),
                (
                    t.tool_steps
                        .iter()
                        .filter(|s| !ts_before(&s.ts, deploy_time))
                        .cloned()
                        .collect::<Vec<ToolStep>>(),
                    t.round_ts
                        .iter()
                        .filter(|ts| !ts_before(ts, deploy_time))
                        .cloned()
                        .collect::<Vec<String>>(),
                    &mut after_traces,
                ),
            ] {
                if !steps.is_empty() || !rounds.is_empty() {
                    bucket.push(ConversationTrace {
                        start_time: t.start_time.clone(),
                        total_rounds: rounds.len() as u32,
                        duration_ms: 0,
                        tool_steps: steps,
                        round_ts: rounds,
                        signals: Vec::new(),
                    });
                }
            }
        }

        let min_samples = if self.config.learning.min_outcome_samples == 0 {
            5
        } else {
            self.config.learning.min_outcome_samples as usize
        };

        let before_refs: Vec<&ConversationTrace> = before_traces.iter().collect();
        let after_refs: Vec<&ConversationTrace> = after_traces.iter().collect();

        if after_refs.len() < min_samples {
            return Some(ActionOutcome {
                artifact_id: artifact.id.clone(),
                measured_at: chrono::Local::now().to_rfc3339(),
                sample_size: after_refs.len(),
                sample_size_before: before_refs.len(),
                rounds_before_avg: 0.0,
                rounds_after_avg: 0.0,
                success_before: 0.0,
                success_after: 0.0,
                duration_before_ms: 0,
                duration_after_ms: 0,
                improvement_score: 0.0,
                verdict: "insufficient_data".to_string(),
            });
        }

        // Calculate metrics
        let before_rounds = avg_rounds(&before_refs);
        let after_rounds = avg_rounds(&after_refs);
        let before_success = success_rate(&before_refs);
        let after_success = success_rate(&after_refs);
        let before_dur = avg_duration(&before_refs);
        let after_dur = avg_duration(&after_refs);

        // Normalized improvement score: 0.4*rounds + 0.4*success + 0.2*duration
        let norm_rounds = normalize(before_rounds, after_rounds);
        let norm_success = after_success - before_success; // already 0-1 range
        let norm_duration = normalize(before_dur as f64, after_dur as f64);

        let improvement_score = 0.4 * norm_rounds + 0.4 * norm_success + 0.2 * norm_duration;

        let verdict = self.classify_verdict(improvement_score, artifact);

        Some(ActionOutcome {
            artifact_id: artifact.id.clone(),
            measured_at: chrono::Local::now().to_rfc3339(),
            sample_size: after_refs.len(),
            sample_size_before: before_refs.len(),
            rounds_before_avg: before_rounds,
            rounds_after_avg: after_rounds,
            success_before: before_success,
            success_after: after_success,
            duration_before_ms: before_dur,
            duration_after_ms: after_dur,
            improvement_score,
            verdict,
        })
    }

    /// Handle the verdict for an artifact.
    ///
    /// Matches Go's `handleVerdict()`:
    /// - "negative" => try to deprecate
    /// - "observing" => track observing rounds
    /// - "positive" => reset observing counter
    pub fn handle_verdict(&self, artifact: &Artifact, outcome: &ActionOutcome) {
        match outcome.verdict.as_str() {
            "negative" => {
                self.try_deprecate(artifact);
            }
            "observing" => {
                self.track_observing(artifact);
            }
            "positive" => {
                // Reset observing counter on positive outcome
                self.registry.update(&artifact.id, |a| {
                    a.consecutive_observing_rounds = 0;
                });
            }
            _ => {}
        }
    }

    /// Try to deprecate an artifact with cooldown check.
    ///
    /// Matches Go's `tryDeprecate()`. Checks cooldown period and
    /// consecutive observing rounds before deprecating.
    pub fn try_deprecate(&self, artifact: &Artifact) {
        let cooldown_days = if self.config.learning.degradation_cooldown_days == 0 {
            7
        } else {
            self.config.learning.degradation_cooldown_days
        };

        // Check cooldown
        if let Some(ref last_degraded) = artifact.last_degraded_at
            && let Ok(degraded_at) = chrono::DateTime::parse_from_rfc3339(last_degraded)
        {
            let now = chrono::Local::now();
            let days_since = (now - degraded_at.with_timezone(&chrono::Local)).num_days();
            if days_since < cooldown_days as i64 {
                return; // still in cooldown
            }
        }

        // try_deprecate always degrades when reached — the caller gates on the
        // verdict (and a cooldown). 历史上的 `>= 3 || < 3` 恒真包裹条件已移除：
        // 无条件体与恒真 if 行为完全一致，而 clippy correctness 组 deny 恒真
        // 布尔式（overly_complex_bool_expr）。测试语义不变。
        self.registry.update(&artifact.id, |a| {
            a.status = ArtifactStatus::Degraded;
            a.last_degraded_at = Some(chrono::Local::now().to_rfc3339());
            a.consecutive_observing_rounds = 0;
        });

        tracing::info!(
            artifact_id = %artifact.id,
            name = %artifact.name,
            "[Monitor] Artifact deprecated due to negative outcome"
        );
    }

    /// Track observing rounds and auto-upgrade to deprecation if needed.
    ///
    /// Matches Go's `trackObserving()`. Increments the consecutive observing
    /// counter and triggers deprecation if it reaches 3.
    pub fn track_observing(&self, artifact: &Artifact) {
        self.registry.update(&artifact.id, |a| {
            a.consecutive_observing_rounds += 1;
            if a.consecutive_observing_rounds >= 3 {
                a.consecutive_observing_rounds = 3; // cap at 3
            }
        });

        // Re-read to check if we should deprecate
        if let Some(updated) = self.registry.get(&artifact.id)
            && updated.consecutive_observing_rounds >= 3
        {
            self.try_deprecate(&updated);
        }
    }

    /// Evaluate all active/observing artifacts (simple mode).
    ///
    /// Also performs auto-upgrade: if an artifact has 3 or more consecutive
    /// "observing" rounds, it is automatically upgraded to "negative" verdict
    /// and degradation is triggered.
    pub fn evaluate_all(&self) -> Vec<EvaluationResult> {
        let artifacts = self.registry.list(None, None);
        let mut results = Vec::new();
        for artifact in artifacts
            .iter()
            .filter(|a| a.status == ArtifactStatus::Active || a.status == ArtifactStatus::Observing)
        {
            let result = self.evaluate(artifact);
            results.push(result);
        }

        // Post-process: auto-upgrade consecutive observing rounds to negative
        for result in results.iter_mut() {
            if let Some(artifact) = self.registry.get(&result.artifact_id)
                && artifact.consecutive_observing_rounds >= 3
            {
                tracing::warn!(
                    artifact_id = %artifact.id,
                    consecutive_rounds = artifact.consecutive_observing_rounds,
                    "[Monitor] Auto-upgrading artifact to negative (3+ consecutive observing rounds)"
                );
                result.verdict = "negative".to_string();
                result.improvement_score = -0.3;
            }
        }

        results
    }

    /// Check if an artifact should be degraded based on consecutive observing rounds.
    pub fn should_degrade(&self, artifact: &Artifact) -> bool {
        artifact.consecutive_observing_rounds >= 3
    }

    /// Check if the cooldown period has elapsed since the last degradation.
    ///
    /// Returns `true` if degradation is allowed (no recent degradation or
    /// the cooldown period has passed). Returns `false` if the artifact was
    /// degraded recently and is still within the cooldown period.
    pub fn is_degradation_cooldown_elapsed(&self, artifact: &Artifact) -> bool {
        let cooldown_days = self.config.learning.degradation_cooldown_days;
        match &artifact.last_degraded_at {
            None => true, // Never degraded before
            Some(last_degraded) => {
                match chrono::DateTime::parse_from_rfc3339(last_degraded) {
                    Ok(degraded_at) => {
                        let now = chrono::Local::now();
                        let days_since =
                            (now - degraded_at.with_timezone(&chrono::Local)).num_days();
                        days_since >= cooldown_days as i64
                    }
                    Err(_) => true, // Unparseable timestamp, allow degradation
                }
            }
        }
    }

    /// Apply degradation to an artifact if warranted and not in cooldown.
    ///
    /// Returns `true` if degradation was actually applied, `false` if
    /// skipped due to cooldown or artifact not found.
    pub fn apply_degradation(&self, artifact_id: &str) -> bool {
        // Check cooldown before updating
        if let Some(artifact) = self.registry.get(artifact_id)
            && !self.is_degradation_cooldown_elapsed(&artifact)
        {
            return false;
        }

        self.registry.update(artifact_id, |a| {
            a.status = ArtifactStatus::Degraded;
            a.last_degraded_at = Some(chrono::Local::now().to_rfc3339());
            a.consecutive_observing_rounds = 0;
        })
    }

    /// Run a full evaluation and auto-degradation cycle.
    ///
    /// Evaluates all active/observing artifacts, auto-upgrades consecutive
    /// observing rounds, and degrades any with "negative" verdict.
    ///
    /// Returns the list of evaluation results.
    pub fn run_evaluation_cycle(&self) -> Vec<EvaluationResult> {
        let results = self.evaluate_all();

        for result in &results {
            if result.verdict == "negative"
                && let Some(artifact) = self.registry.get(&result.artifact_id)
            {
                if self.is_degradation_cooldown_elapsed(&artifact) {
                    tracing::info!(
                        artifact_id = %result.artifact_id,
                        improvement_score = result.improvement_score,
                        "[Monitor] Degrading artifact due to negative evaluation"
                    );
                    self.apply_degradation(&result.artifact_id);
                } else {
                    tracing::info!(
                        artifact_id = %result.artifact_id,
                        "[Monitor] Skipping degradation: cooldown period active"
                    );
                }
            }
        }

        results
    }
}

// ----- Helper functions (matching Go's helper functions) -----

/// Build conversation traces from raw trace events (audit -,).
///
/// Groups events by `session_key` (insertion order preserved), maps
/// `tool_call` events to tool steps (携带 ok/duration_ms/ts——D2 事件粒度
/// 度量的原始样本), and emits a `tool_error` signal for failed tool calls
/// (`data.ok == false`). `total_rounds` counts **llm_turn** events only
/// （缺陷整改批次 D2：旧实现按事件总数代理——tool_call 密集的会话轮数被
/// 灌水；llm_turn 事件由 agent loop 每次 LLM 响应落定时发射，旧轨迹无此
/// 事件则诚实为 0）; `duration_ms` is last-minus-first event timestamp
/// (unparseable timestamps degrade to 0). Pure function — no I/O.
pub fn build_conversation_traces(events: &[crate::trace::TraceEvent]) -> Vec<ConversationTrace> {
    use std::collections::HashMap;

    let mut order: Vec<String> = Vec::new();
    let mut grouped: HashMap<String, ConversationTrace> = HashMap::new();

    for event in events {
        let trace = grouped.entry(event.session_key.clone()).or_insert_with(|| {
            order.push(event.session_key.clone());
            ConversationTrace {
                start_time: event.timestamp.clone(),
                total_rounds: 0,
                duration_ms: 0,
                tool_steps: Vec::new(),
                round_ts: Vec::new(),
                signals: Vec::new(),
            }
        });
        if event.event_type == "llm_turn" {
            trace.total_rounds += 1;
            trace.round_ts.push(event.timestamp.clone());
        } else if event.event_type == "tool_call" {
            let tool = event
                .data
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let ok = event
                .data
                .get("ok")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let duration_ms = event
                .data
                .get("duration_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if !ok {
                trace.signals.push("tool_error".into());
            }
            if !tool.is_empty() {
                trace.tool_steps.push(ToolStep {
                    tool_name: tool,
                    ok,
                    duration_ms,
                    ts: event.timestamp.clone(),
                });
            }
        }
    }

    // duration_ms = last - first timestamp per session (0 when unparseable).
    for (key, trace) in grouped.iter_mut() {
        let stamps: Vec<&crate::trace::TraceEvent> =
            events.iter().filter(|e| &e.session_key == key).collect();
        if stamps.len() >= 2 {
            let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
            if let (Some(first), Some(last)) = (
                parse(&stamps[0].timestamp),
                parse(&stamps[stamps.len() - 1].timestamp),
            ) {
                let delta = (last - first).num_milliseconds();
                if delta > 0 {
                    trace.duration_ms = delta;
                }
            }
        }
    }

    order
        .into_iter()
        .filter_map(|key| grouped.remove(&key))
        .collect()
}

/// Parse an RFC 3339 timestamp（缺陷整改批次 D1 单一解析锚点）。
///
/// Returns `None` when unparseable — callers decide the honest fallback
/// （窗口过滤放行 / 分桶回落字符串序），不再各自手写 parse。
fn parse_ts(s: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local))
}

/// 事件/步时间戳与分界点的比较（D1/D3 共用）。
///
/// 双侧可解析 = 时刻序（跨时区偏移正确）；任一侧解析失败 = 回落字符串序
/// （同偏移写入的时间戳字符串序 == 时刻序，正确性不劣于旧实现）。
fn ts_before(stamp: &str, boundary: &str) -> bool {
    match (parse_ts(stamp), parse_ts(boundary)) {
        (Some(a), Some(b)) => a < b,
        _ => stamp < boundary,
    }
}

/// Check if a trace's tool steps contain the given signature as a subsequence.
///
/// Matches Go's `matchesToolSignature()`.
pub fn matches_tool_signature(trace: &ConversationTrace, signature: &[String]) -> bool {
    if signature.is_empty() {
        return false;
    }

    let mut sig_idx = 0;
    for step in &trace.tool_steps {
        if step.tool_name == signature[sig_idx] {
            sig_idx += 1;
            if sig_idx == signature.len() {
                return true;
            }
        }
    }
    false
}

/// Calculate (before - after) / max(before, 1).
///
/// Matches Go's `normalize()`.
pub fn normalize(before: f64, after: f64) -> f64 {
    if before <= 0.0 {
        return 0.0;
    }
    (before - after) / before
}

/// Calculate average total rounds across traces.
///
/// Matches Go's `avgRounds()`. total_rounds = llm_turn 计数（D2）——
/// 旧轨迹无 llm_turn 事件时贡献 0（诚实无轮数，不拿 tool_call 数灌水）。
pub fn avg_rounds(traces: &[&ConversationTrace]) -> f64 {
    if traces.is_empty() {
        return 0.0;
    }
    let total: u32 = traces.iter().map(|t| t.total_rounds).sum();
    total as f64 / traces.len() as f64
}

/// Calculate success rate at **event granularity**（缺陷整改批次 D2）.
///
/// 旧实现按轨迹级「无 signals = 成功」——一条轨迹 20 步成功 + 1 步失败
/// 会被整轨判负。现按步池化：sum(ok steps) / sum(all steps)；零步轨迹
/// （纯 LLM 对话）不进分母——无工具步 = 无失败证据（全空输入返回 1.0）。
pub fn success_rate(traces: &[&ConversationTrace]) -> f64 {
    let total_steps: usize = traces.iter().map(|t| t.tool_steps.len()).sum();
    if total_steps == 0 {
        return 1.0;
    }
    let ok_steps: usize = traces
        .iter()
        .flat_map(|t| t.tool_steps.iter())
        .filter(|s| s.ok)
        .count();
    ok_steps as f64 / total_steps as f64
}

/// Calculate average duration at **step granularity**（缺陷整改批次 D2）.
///
/// 旧实现取轨迹级 first→last span 均值——长会话单次工具失败把整轨时长
/// 拉爆。现为步级 duration_ms 池化均值；零步轨迹不进分母。
pub fn avg_duration(traces: &[&ConversationTrace]) -> i64 {
    let mut total_ms: i64 = 0;
    let mut total_steps: usize = 0;
    for t in traces {
        for s in &t.tool_steps {
            total_ms += s.duration_ms;
            total_steps += 1;
        }
    }
    if total_steps == 0 {
        return 0;
    }
    total_ms / total_steps as i64
}

#[cfg(test)]
mod tests;
