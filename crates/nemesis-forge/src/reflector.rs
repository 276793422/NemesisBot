//! Reflector - analyzes collected experiences and produces reflections.
//!
//! Performs simple statistical analysis (success rates, average durations,
//! error patterns) and generates a `Reflection` with insights, patterns
//! and recommendations. Supports:
//! - Stage 1: Statistical analysis (pure code, zero tokens)
//! - Stage 1.7: Closed-loop learning integration
//! - Stage 4: Cluster sharing integration
//!
//! Also provides disk operations for reflection reports: writing markdown
//! reports, cleaning old reports, and finding the latest report.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use uuid::Uuid;

use nemesis_types::forge::Reflection;

use crate::types::{CollectedExperience, ExperienceStats, ToolStats};

// ---------------------------------------------------------------------------
// Trace stats (Stage 1.5 / 1.7)
// ---------------------------------------------------------------------------

/// Conversation-level statistical analysis results.
#[derive(Debug, Clone, Default)]
pub struct TraceStats {
    /// Total number of traces analyzed.
    pub total_traces: usize,
    /// Average number of rounds per conversation.
    pub avg_rounds: f64,
    /// Average duration in milliseconds.
    pub avg_duration_ms: i64,
    /// Efficiency score [0, 1].
    pub efficiency_score: f64,
    /// Tool chain patterns detected.
    pub tool_chain_patterns: Vec<ToolChainPattern>,
    /// Retry patterns detected.
    pub retry_patterns: Vec<RetryPattern>,
    /// Summary of signals detected.
    pub signal_summary: HashMap<String, i32>,
}

/// A frequently occurring tool call sequence.
#[derive(Debug, Clone)]
pub struct ToolChainPattern {
    /// Chain description (e.g. "read_file->edit_file->exec").
    pub chain: String,
    /// Number of occurrences.
    pub count: i32,
    /// Average rounds for this chain.
    pub avg_rounds: f64,
    /// Success rate [0, 1].
    pub success_rate: f64,
}

/// A tool that was retried after failure.
#[derive(Debug, Clone)]
pub struct RetryPattern {
    /// Tool name.
    pub tool_name: String,
    /// Number of retry attempts.
    pub retry_count: i32,
    /// Success rate after retry [0, 1].
    pub success_rate: f64,
}

/// Reflection report data.
#[derive(Debug, Clone)]
pub struct ReflectionReport {
    /// Report date (YYYY-MM-DD).
    pub date: String,
    /// Analysis period.
    pub period: String,
    /// Focus area.
    pub focus: String,
    /// Statistical analysis results.
    pub stats: ReflectionStats,
    /// LLM-generated insights (if available).
    pub llm_insights: Option<String>,
    /// Conversation-level trace stats.
    pub trace_stats: Option<TraceStats>,
    /// Learning cycle result (Phase 6).
    pub learning_cycle: Option<nemesis_types::forge::LearningCycle>,
}

/// Statistical analysis results from experience data.
#[derive(Debug, Clone, Default)]
pub struct ReflectionStats {
    /// Total number of records.
    pub total_records: usize,
    /// Number of unique patterns.
    pub unique_patterns: usize,
    /// Average success rate across all patterns.
    pub avg_success_rate: f64,
    /// Top patterns by frequency.
    pub top_patterns: Vec<PatternInsight>,
    /// Patterns with low success rate.
    pub low_success: Vec<PatternInsight>,
    /// Tool frequency map.
    pub tool_frequency: HashMap<String, i32>,
}

/// Insight data for a tool usage pattern.
#[derive(Debug, Clone)]
pub struct PatternInsight {
    /// Tool name.
    pub tool_name: String,
    /// Frequency count.
    pub count: i32,
    /// Average duration in milliseconds.
    pub avg_duration_ms: i64,
    /// Success rate [0, 1].
    pub success_rate: f64,
    /// Improvement suggestion.
    pub suggestion: String,
}

/// Result of merging local and remote reflection reports.
#[derive(Debug, Clone)]
pub struct MergedInsights {
    /// Patterns from local analysis.
    pub local_patterns: Vec<PatternInsight>,
    /// Patterns from remote reports.
    pub remote_patterns: Vec<PatternInsight>,
    /// Merged (combined) patterns.
    pub merged_patterns: Vec<PatternInsight>,
    /// Tools present in both local and remote data, with remote count.
    pub common_tools: HashMap<String, i32>,
    /// Tools unique to remote reports.
    pub unique_remote_tools: Vec<String>,
}

// ---------------------------------------------------------------------------
// Reflector
// ---------------------------------------------------------------------------

/// The reflector analyzes a batch of collected experiences and produces
/// reflection reports.
pub struct Reflector {
    /// Whether cluster sharing is enabled (Stage 4).
    cluster_enabled: bool,
    /// Directory for storing reflection report files.
    reflections_dir: PathBuf,
    /// LLM caller for semantic analysis (optional).
    /// Uses RwLock for interior mutability so set_provider can take &self.
    /// Uses Arc for shared ownership with other subsystems.
    llm_caller: parking_lot::RwLock<Option<Arc<dyn crate::reflector_llm::LLMCaller>>>,
}

impl Reflector {
    /// Create a new reflector with a default (empty) reflections directory.
    pub fn new() -> Self {
        Self {
            cluster_enabled: false,
            reflections_dir: PathBuf::new(),
            llm_caller: parking_lot::RwLock::new(None),
        }
    }

    /// Create a new reflector with cluster sharing enabled.
    pub fn with_cluster() -> Self {
        Self {
            cluster_enabled: true,
            reflections_dir: PathBuf::new(),
            llm_caller: parking_lot::RwLock::new(None),
        }
    }

    /// Create a new reflector with an explicit reflections directory.
    pub fn with_reflections_dir(reflections_dir: PathBuf) -> Self {
        Self {
            cluster_enabled: false,
            reflections_dir,
            llm_caller: parking_lot::RwLock::new(None),
        }
    }

    /// Create a new reflector with both cluster sharing and reflections dir.
    pub fn with_cluster_and_dir(reflections_dir: PathBuf) -> Self {
        Self {
            cluster_enabled: true,
            reflections_dir,
            llm_caller: parking_lot::RwLock::new(None),
        }
    }

    /// Set cluster sharing enabled/disabled.
    pub fn set_cluster_enabled(&mut self, enabled: bool) {
        self.cluster_enabled = enabled;
    }

    /// Set the reflections directory for disk operations.
    pub fn set_reflections_dir(&mut self, dir: PathBuf) {
        self.reflections_dir = dir;
    }

    /// Set the LLM caller for semantic analysis.
    pub fn set_provider(&self, caller: Arc<dyn crate::reflector_llm::LLMCaller>) {
        tracing::info!("[Forge/Reflector] LLM provider set for semantic analysis");
        *self.llm_caller.write() = Some(caller);
    }

    /// Get a reference to the LLM caller (if set).
    pub fn llm_caller(&self) -> parking_lot::RwLockReadGuard<'_, Option<Arc<dyn crate::reflector_llm::LLMCaller>>> {
        self.llm_caller.read()
    }

    /// Compute aggregate statistics over a slice of collected experiences.
    pub fn analyze(&self, experiences: &[CollectedExperience]) -> ExperienceStats {
        let mut tool_data: HashMap<String, Vec<(bool, u64)>> = HashMap::new();
        let mut total_duration: u64 = 0;
        let mut success_count: usize = 0;

        for ce in experiences {
            let e = &ce.experience;
            total_duration += e.duration_ms;
            if e.success {
                success_count += 1;
            }
            tool_data
                .entry(e.tool_name.clone())
                .or_default()
                .push((e.success, e.duration_ms));
        }

        let total_count = experiences.len();
        let mut tool_counts: HashMap<String, ToolStats> = HashMap::new();
        for (name, records) in &tool_data {
            let count = records.len();
            let sc = records.iter().filter(|(s, _)| *s).count();
            let avg_dur = records.iter().map(|(_, d)| *d as f64).sum::<f64>() / count as f64;
            tool_counts.insert(
                name.clone(),
                ToolStats {
                    count,
                    success_count: sc,
                    avg_duration_ms: avg_dur,
                },
            );
        }

        ExperienceStats {
            total_count,
            success_count,
            failure_count: total_count - success_count,
            avg_duration_ms: if total_count > 0 {
                total_duration as f64 / total_count as f64
            } else {
                0.0
            },
            tool_counts,
        }
    }

    /// Generate a reflection report from the given experiences.
    ///
    /// This is a purely statistical / rule-based analysis. In a full
    /// implementation an LLM call would be made here for deeper semantic
    /// analysis.
    pub fn generate_reflection(&self, experiences: &[CollectedExperience]) -> Reflection {
        tracing::debug!(
            experience_count = experiences.len(),
            "[Forge/Reflector] Generating reflection from experiences"
        );
        let stats = self.analyze(experiences);

        let mut insights: Vec<String> = Vec::new();
        let mut patterns: Vec<String> = Vec::new();
        let mut recommendations: Vec<String> = Vec::new();

        if stats.total_count == 0 {
            insights.push("No experiences collected yet.".into());
            return self.build_reflection(insights, patterns, recommendations, &stats);
        }

        // Overall success rate insight
        let success_rate =
            stats.success_count as f64 / stats.total_count as f64 * 100.0;
        insights.push(format!(
            "Overall success rate: {:.1}% ({}/{})",
            success_rate, stats.success_count, stats.total_count
        ));

        // Average duration insight
        insights.push(format!(
            "Average tool call duration: {:.1}ms",
            stats.avg_duration_ms
        ));

        // Per-tool analysis
        for (tool_name, ts) in &stats.tool_counts {
            let tool_sr = if ts.count > 0 {
                ts.success_count as f64 / ts.count as f64 * 100.0
            } else {
                0.0
            };

            // Pattern: tools with low success rate
            if tool_sr < 50.0 && ts.count >= 3 {
                patterns.push(format!(
                    "Tool '{}' has low success rate ({:.1}%) over {} calls",
                    tool_name, tool_sr, ts.count
                ));
                recommendations.push(format!(
                    "Investigate failures in tool '{}' and consider adding error handling or fallbacks.",
                    tool_name
                ));
            }

            // Pattern: tools that are slow
            if ts.avg_duration_ms > 5000.0 && ts.count >= 2 {
                patterns.push(format!(
                    "Tool '{}' is consistently slow (avg {:.0}ms)",
                    tool_name, ts.avg_duration_ms
                ));
                recommendations.push(format!(
                    "Consider optimizing or caching results for tool '{}'.",
                    tool_name
                ));
            }

            // Pattern: frequently used tools
            if ts.count >= 5 {
                patterns.push(format!(
                    "Tool '{}' is frequently used ({}) times",
                    tool_name, ts.count
                ));
            }
        }

        // General recommendations
        if stats.failure_count > 0 && success_rate < 80.0 {
            recommendations.push(
                "Overall success rate is below 80%. Review error logs for common failure modes."
                    .into(),
            );
        }

        if insights.len() == 1 {
            // Only the generic success-rate insight; nothing more to say.
            insights.push("All tools performing within normal parameters.".into());
        }

        self.build_reflection(insights, patterns, recommendations, &stats)
    }

    /// Perform Stage 1.5/1.7 trace analysis.
    ///
    /// This analyzes conversation-level patterns from collected experiences.
    /// When learning engine results are available, they are integrated into
    /// the trace stats.
    pub fn analyze_traces(
        &self,
        experiences: &[CollectedExperience],
        learning_cycle: Option<&nemesis_types::forge::LearningCycle>,
    ) -> TraceStats {
        if experiences.is_empty() {
            return TraceStats::default();
        }

        let mut stats = TraceStats::default();

        // Count unique sessions as "traces" (conversations)
        let mut sessions: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut tool_counts: HashMap<String, (i32, i32)> = HashMap::new(); // (total, errors)
        let mut total_duration: i64 = 0;
        let mut total_steps: usize = 0;

        for ce in experiences {
            let e = &ce.experience;
            sessions.insert(&e.session_key);
            total_duration += e.duration_ms as i64;
            total_steps += 1;

            let entry = tool_counts
                .entry(e.tool_name.clone())
                .or_insert((0, 0));
            entry.0 += 1;
            if !e.success {
                entry.1 += 1;
            }
        }

        stats.total_traces = sessions.len();
        let total_rounds = experiences.len() as f64; // total experiences as round proxy

        stats.avg_duration_ms = total_duration / experiences.len() as i64;

        // Build tool chain patterns from sequential tool usage
        let mut chain_counts: HashMap<String, (i32, i32, f64)> = HashMap::new(); // (count, successes, total_rounds)
        let mut current_chain = String::new();
        let mut chain_tool_count = 0;

        for ce in experiences {
            let tool = &ce.experience.tool_name;
            if current_chain.is_empty() {
                current_chain = tool.clone();
            } else {
                current_chain = format!("{}->{}", current_chain, tool);
            }
            chain_tool_count += 1;

            // Commit chain every 3 tools or at the end
            if chain_tool_count >= 3 {
                let success = ce.experience.success;
                let entry = chain_counts.entry(current_chain.clone()).or_insert((0, 0, 0.0));
                entry.0 += 1;
                if success {
                    entry.1 += 1;
                }
                current_chain = String::new();
                chain_tool_count = 0;
            }
        }

        // Build tool chain patterns
        for (chain, (count, successes, _)) in &chain_counts {
            let success_rate = if *count > 0 {
                *successes as f64 / *count as f64
            } else {
                0.0
            };
            stats.tool_chain_patterns.push(ToolChainPattern {
                chain: chain.clone(),
                count: *count,
                avg_rounds: *count as f64,
                success_rate,
            });
        }

        // Sort by count descending, keep top 5
        stats
            .tool_chain_patterns
            .sort_by(|a, b| b.count.cmp(&a.count));
        stats.tool_chain_patterns.truncate(5);

        // Build retry patterns
        for (tool, (total, errors)) in &tool_counts {
            if *total >= 2 && *errors > 0 {
                stats.retry_patterns.push(RetryPattern {
                    tool_name: tool.clone(),
                    retry_count: *errors,
                    success_rate: (*total - *errors) as f64 / *total as f64,
                });
            }
        }
        stats
            .retry_patterns
            .sort_by(|a, b| b.retry_count.cmp(&a.retry_count));
        stats.retry_patterns.truncate(5);

        // Efficiency score: totalSteps / totalRounds (matching Go's formula)
        // More steps per round = higher efficiency, capped at 1.0.
        if total_steps > 0 && total_rounds > 0.0 {
            stats.efficiency_score = total_steps as f64 / total_rounds;
            if stats.efficiency_score > 1.0 {
                stats.efficiency_score = 1.0;
            }
        }

        // Stage 1.7: Integrate learning cycle data
        if let Some(cycle) = learning_cycle {
            stats.signal_summary.insert(
                "learning_patterns_found".to_string(),
                cycle.patterns_found as i32,
            );
            stats.signal_summary.insert(
                "learning_actions_taken".to_string(),
                cycle.actions_taken as i32,
            );
        }

        stats
    }

    /// Perform full reflection cycle (Stages 1-4).
    ///
    /// Stage 1: Statistical analysis
    /// Stage 1.5: Trace analysis
    /// Stage 1.7: Learning engine integration
    /// Stage 4: Cluster sharing (if enabled)
    pub fn reflect(
        &self,
        experiences: &[CollectedExperience],
        learning_cycle: Option<&nemesis_types::forge::LearningCycle>,
        period: &str,
        focus: &str,
    ) -> ReflectionReport {
        // Stage 1: Statistical analysis
        let stats = self.statistical_analysis(experiences);

        // Stage 1.5: Trace analysis
        let trace_stats = if !experiences.is_empty() {
            Some(self.analyze_traces(experiences, learning_cycle))
        } else {
            None
        };

        // Stage 4: Cluster sharing integration
        if self.cluster_enabled {
            tracing::info!("[Reflector] Stage 4: Cluster sharing enabled, report will be shared after generation");
        }

        ReflectionReport {
            date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            period: period.to_string(),
            focus: focus.to_string(),
            stats,
            llm_insights: None,
            trace_stats,
            learning_cycle: learning_cycle.cloned(),
        }
    }

    /// Perform full reflection cycle with LLM semantic analysis (Stage 4 LLM).
    ///
    /// This mirrors Go's `Reflector.Reflect()` which calls `semanticAnalysis()`
    /// when a provider is available. The synchronous stages (1-3) run first,
    /// then LLM is invoked asynchronously for semantic insights.
    pub async fn reflect_with_llm(
        &self,
        experiences: &[CollectedExperience],
        artifacts: &[crate::types::Artifact],
        learning_cycle: Option<&nemesis_types::forge::LearningCycle>,
        period: &str,
        focus: &str,
    ) -> ReflectionReport {
        let mut report = self.reflect(experiences, learning_cycle, period, focus);

        // Stage 4 LLM: Semantic analysis (if provider available)
        let caller_guard = self.llm_caller.read();
        if let Some(ref caller) = *caller_guard {
            match crate::reflector_llm::semantic_analysis(
                caller.as_ref(),
                &report.stats,
                artifacts,
                report.trace_stats.as_ref(),
                report.learning_cycle.as_ref(),
                None,
            )
            .await
            {
                Ok(insights) => {
                    tracing::info!(len = insights.len(), "[Reflector] LLM semantic analysis completed");
                    report.llm_insights = Some(insights);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Reflector] LLM semantic analysis failed, skipping");
                }
            }
        }

        report
    }

    /// Perform statistical analysis on collected experiences and return
    /// structured reflection stats.
    pub fn statistical_analysis(&self, experiences: &[CollectedExperience]) -> ReflectionStats {
        let mut stats = ReflectionStats {
            tool_frequency: HashMap::new(),
            ..Default::default()
        };

        let mut tool_data: HashMap<String, (i32, i64, i32, i32)> = HashMap::new(); // (count, total_duration, successes, failures)
        let mut total_rate = 0.0f64;
        let mut total_weighted = 0i32;

        for ce in experiences {
            let e = &ce.experience;
            stats.total_records += 1;
            *stats.tool_frequency.entry(e.tool_name.clone()).or_insert(0) += 1;

            let entry = tool_data
                .entry(e.tool_name.clone())
                .or_insert((0, 0, 0, 0));
            entry.0 += 1;
            entry.1 += e.duration_ms as i64;
            if e.success {
                entry.2 += 1;
                total_rate += 1.0;
            } else {
                entry.3 += 1;
            }
            total_weighted += 1;
        }

        stats.unique_patterns = tool_data.len();

        if stats.total_records > 0 {
            stats.avg_success_rate = total_rate / total_weighted as f64;
        }

        // Build top patterns (sorted by count)
        let mut sorted_tools: Vec<_> = tool_data.iter().collect();
        sorted_tools.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));

        for (tool, (count, total_dur, successes, _)) in sorted_tools.iter().take(10) {
            let sr = if *count > 0 {
                *successes as f64 / *count as f64
            } else {
                0.0
            };
            stats.top_patterns.push(PatternInsight {
                tool_name: (*tool).clone(),
                count: *count,
                avg_duration_ms: *total_dur / *count as i64,
                success_rate: sr,
                suggestion: self.generate_suggestion(*count, sr),
            });
        }

        // Low success patterns (success rate < 0.7)
        for (tool, (count, total_dur, successes, _)) in &tool_data {
            let sr = if *count > 0 {
                *successes as f64 / *count as f64
            } else {
                0.0
            };
            if sr < 0.7 && *count >= 3 {
                stats.low_success.push(PatternInsight {
                    tool_name: tool.clone(),
                    count: *count,
                    avg_duration_ms: total_dur / *count as i64,
                    success_rate: sr,
                    suggestion: "Low success rate - investigate failure causes and improve error handling".to_string(),
                });
            }
        }

        stats
    }

    /// Resolve a period string to a cutoff timestamp.
    ///
    /// Supported values: "today", "week", "all" (default: "today").
    /// Returns `None` for "all" (meaning no time filter).
    pub fn resolve_period(period: &str) -> Option<String> {
        match period {
            "today" => {
                let now = chrono::Local::now();
                let start_of_day = now.date_naive().and_hms_opt(0, 0, 0)
                    .unwrap_or_default();
                Some(start_of_day.and_utc().to_rfc3339())
            }
            "week" => {
                let cutoff = chrono::Local::now() - chrono::Duration::days(7);
                Some(cutoff.to_rfc3339())
            }
            "all" => None,
            _ => {
                // Default: today
                let now = chrono::Local::now();
                let start_of_day = now.date_naive().and_hms_opt(0, 0, 0)
                    .unwrap_or_default();
                Some(start_of_day.and_utc().to_rfc3339())
            }
        }
    }

    /// Filter experiences by period string.
    ///
    /// Returns only experiences whose timestamp is after the resolved cutoff.
    pub fn filter_by_period<'a>(
        &self,
        experiences: &'a [CollectedExperience],
        period: &str,
    ) -> Vec<&'a CollectedExperience> {
        let cutoff = Self::resolve_period(period);
        match cutoff {
            None => experiences.iter().collect(),
            Some(cutoff_str) => {
                experiences.iter().filter(|ce| {
                    ce.experience.timestamp >= cutoff_str
                }).collect()
            }
        }
    }

    /// Filter experiences by focus string.
    ///
    /// If focus is "all" or empty, returns all. Otherwise only experiences
    /// whose tool_name matches the focus.
    pub fn filter_by_focus<'a>(
        &self,
        experiences: &'a [CollectedExperience],
        focus: &str,
    ) -> Vec<&'a CollectedExperience> {
        if focus.is_empty() || focus == "all" {
            experiences.iter().collect()
        } else {
            experiences.iter().filter(|ce| {
                ce.experience.tool_name == focus
            }).collect()
        }
    }

    /// Generate a basic improvement suggestion.
    fn generate_suggestion(&self, count: i32, success_rate: f64) -> String {
        if success_rate >= 0.9 && count >= 5 {
            format!(
                "High frequency ({} uses), consider creating a Skill for this pattern",
                count
            )
        } else if success_rate >= 0.7 {
            "Stable pattern, monitor for potential Skill creation".to_string()
        } else if success_rate < 0.7 {
            "Review failure modes and consider adding error handling".to_string()
        } else {
            "Normal usage pattern".to_string()
        }
    }

    // ----- Remote reflection merging (Stage 4) -----

    /// Merge remote reflection reports with local patterns.
    ///
    /// Reads tool usage patterns from remote report files and combines them
    /// with local statistical data for richer cross-node analysis.
    /// Mirrors Go's `Reflector.MergeRemoteReflections`.
    pub fn merge_remote_reflections(
        &self,
        remote_reports: &[PathBuf],
        experiences: &[CollectedExperience],
    ) -> MergedInsights {
        // Collect local patterns for comparison
        let local_stats = self.get_local_patterns(experiences);

        let mut result = MergedInsights {
            local_patterns: local_stats.top_patterns.clone(),
            remote_patterns: Vec::new(),
            merged_patterns: Vec::new(),
            common_tools: HashMap::new(),
            unique_remote_tools: Vec::new(),
        };

        // Track local tool names
        let local_tools: HashMap<String, bool> = local_stats
            .top_patterns
            .iter()
            .map(|p| (p.tool_name.clone(), true))
            .collect();

        // Extract patterns from remote reports
        let mut remote_tool_freq: HashMap<String, i32> = HashMap::new();
        for report_path in remote_reports {
            let tool_freq = self.extract_tool_patterns_from_report(report_path);
            for (tool, count) in tool_freq {
                *remote_tool_freq.entry(tool).or_insert(0) += count;
            }
        }

        // Build remote patterns
        for (tool, count) in &remote_tool_freq {
            result.remote_patterns.push(PatternInsight {
                tool_name: tool.clone(),
                count: *count,
                avg_duration_ms: 0,
                success_rate: 0.0,
                suggestion: String::new(),
            });
            if local_tools.contains_key(tool) {
                result.common_tools.insert(tool.clone(), *count);
            } else {
                result.unique_remote_tools.push(tool.clone());
            }
        }

        // Merge: start with local patterns, add unique remote patterns
        let mut merged = local_stats.top_patterns.clone();
        for rp in &result.remote_patterns {
            let mut found = false;
            for mp in &mut merged {
                if mp.tool_name == rp.tool_name {
                    mp.count += rp.count;
                    found = true;
                    break;
                }
            }
            if !found {
                merged.push(rp.clone());
            }
        }
        result.merged_patterns = merged;

        result
    }

    /// Run a lightweight statistical analysis on local experiences.
    /// Mirrors Go's `Reflector.getLocalPatterns`.
    fn get_local_patterns(&self, experiences: &[CollectedExperience]) -> ReflectionStats {
        if experiences.is_empty() {
            return ReflectionStats::default();
        }
        self.statistical_analysis(experiences)
    }

    /// Read a remote report file and extract tool usage patterns by scanning
    /// for common markdown table markers.
    /// Mirrors Go's `Reflector.extractToolPatternsFromReport`.
    fn extract_tool_patterns_from_report(&self, report_path: &PathBuf) -> HashMap<String, i32> {
        let mut freq: HashMap<String, i32> = HashMap::new();

        let content = match std::fs::read_to_string(report_path) {
            Ok(c) => c,
            Err(_) => return freq,
        };

        // Look for tool names in common report sections
        // Pattern: lines like "| tool_name | count |" in markdown tables
        for line in content.lines() {
            let line = line.trim();
            // Skip separator lines
            if line.starts_with("|-") || line.starts_with("| ---") {
                continue;
            }
            // Look for table rows with tool data
            if line.starts_with('|') && line.matches('|').count() >= 3 {
                let fields: Vec<&str> = line.split('|').map(|f| f.trim()).collect();
                // Try to find tool name + count pattern
                for i in 1..fields.len().saturating_sub(1) {
                    let tool_name = fields[i];
                    if tool_name.is_empty()
                        || tool_name.contains("---")
                        || tool_name.contains("Tool")
                        || tool_name.contains("Name")
                    {
                        continue;
                    }
                    // Check if next field looks like a count
                    if let Ok(count) = fields.get(i + 1).unwrap_or(&"").parse::<i32>() {
                        if count > 0 {
                            *freq.entry(tool_name.to_string()).or_insert(0) += count;
                            break; // one tool per row
                        }
                    }
                }
            }
        }

        // Also look for known tool keywords in text
        let tool_keywords = [
            "read_file",
            "write_file",
            "edit_file",
            "exec",
            "file_read",
            "file_write",
            "file_edit",
            "process_exec",
            "network_request",
            "http_request",
            "web_search",
            "code_execute",
            "shell",
            "bash",
        ];
        for tool in &tool_keywords {
            let count = content.matches(tool).count() as i32;
            if count > 0 {
                *freq.entry(tool.to_string()).or_insert(0) += count;
            }
        }

        freq
    }

    // ----- Disk operations -----

    /// Write a reflection report as a markdown file to the reflections directory.
    ///
    /// The file is named `reflection_{date}_{timestamp}.md`. Returns the path
    /// of the written file.
    pub fn write_report(&self, report: &ReflectionReport) -> Result<PathBuf, String> {
        if self.reflections_dir.as_os_str().is_empty() {
            return Err("reflections directory not configured".to_string());
        }

        std::fs::create_dir_all(&self.reflections_dir)
            .map_err(|e| format!("failed to create reflections dir: {}", e))?;

        let now = chrono::Local::now();
        let filename = format!(
            "reflection_{}_{}.md",
            report.date,
            now.format("%H%M%S")
        );
        let path = self.reflections_dir.join(&filename);

        let md_content = self.format_report_markdown(report);

        std::fs::write(&path, md_content)
            .map_err(|e| format!("failed to write report: {}", e))?;

        tracing::info!(path = %path.display(), "[Reflector] Wrote reflection report");
        Ok(path)
    }

    /// Delete report files older than `max_age_days` days from the reflections directory.
    ///
    /// Returns the number of files deleted.
    pub fn cleanup_reports(&self, max_age_days: u64) -> usize {
        if self.reflections_dir.as_os_str().is_empty() || !self.reflections_dir.exists() {
            return 0;
        }

        let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days as i64);
        let mut deleted = 0;

        if let Ok(entries) = std::fs::read_dir(&self.reflections_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    if let Ok(metadata) = path.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            let modified_time: chrono::DateTime<chrono::Local> =
                                modified.into();
                            if modified_time < cutoff {
                                if std::fs::remove_file(&path).is_ok() {
                                    deleted += 1;
                                    tracing::debug!(
                                        path = %path.display(),
                                        "[Reflector] Deleted old reflection report"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        if deleted > 0 {
            tracing::info!(deleted, max_age_days, "[Reflector] Cleaned up old reflection reports");
        }
        deleted
    }

    /// Find the most recent report file in the reflections directory.
    ///
    /// Returns `None` if no report files exist or the directory is not configured.
    pub fn get_latest_report(&self) -> Option<PathBuf> {
        if self.reflections_dir.as_os_str().is_empty() || !self.reflections_dir.exists() {
            return None;
        }

        let mut latest: Option<(PathBuf, std::time::SystemTime)> = None;

        if let Ok(entries) = std::fs::read_dir(&self.reflections_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    if let Ok(metadata) = path.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            if latest.as_ref().map_or(true, |(_, t)| modified > *t) {
                                latest = Some((path, modified));
                            }
                        }
                    }
                }
            }
        }

        latest.map(|(p, _)| p)
    }

    /// Read the content of the latest report file.
    ///
    /// Returns `Err` if no reports exist or the file cannot be read.
    pub fn get_latest_report_content(&self) -> Result<String, String> {
        match self.get_latest_report() {
            Some(path) => std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read report: {}", e)),
            None => Err("no reflection reports found".to_string()),
        }
    }

    /// Delete report files older than `max_age_days` days.
    ///
    /// Returns `Ok(())` on success. Errors during individual file deletion
    /// are logged but do not cause the overall operation to fail.
    pub fn cleanup_reports_result(&self, max_age_days: i64) -> Result<(), String> {
        if self.reflections_dir.as_os_str().is_empty() {
            return Err("reflections directory not configured".to_string());
        }
        if !self.reflections_dir.exists() {
            return Ok(());
        }

        let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days);
        let mut errors = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&self.reflections_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    if let Ok(metadata) = path.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            let modified_time: chrono::DateTime<chrono::Local> =
                                modified.into();
                            if modified_time < cutoff {
                                if let Err(e) = std::fs::remove_file(&path) {
                                    errors.push(format!(
                                        "failed to delete {}: {}",
                                        path.display(),
                                        e
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("{} errors during cleanup: {}", errors.len(), errors.join("; ")))
        }
    }

    /// Format a reflection report as a markdown string.
    fn format_report_markdown(&self, report: &ReflectionReport) -> String {
        let mut md = String::new();
        md.push_str(&format!("# Reflection Report: {}\n\n", report.date));
        md.push_str(&format!("**Period:** {}\n\n", report.period));
        md.push_str(&format!("**Focus:** {}\n\n", report.focus));

        md.push_str("## Statistics\n\n");
        md.push_str(&format!("- Total records: {}\n", report.stats.total_records));
        md.push_str(&format!("- Unique patterns: {}\n", report.stats.unique_patterns));
        md.push_str(&format!(
            "- Average success rate: {:.1}%\n",
            report.stats.avg_success_rate * 100.0
        ));

        if !report.stats.top_patterns.is_empty() {
            md.push_str("\n### Top Patterns\n\n");
            for p in &report.stats.top_patterns {
                md.push_str(&format!(
                    "- **{}**: {} uses, {:.0}% success, {:.0}ms avg\n",
                    p.tool_name, p.count, p.success_rate * 100.0, p.avg_duration_ms
                ));
            }
        }

        if !report.stats.low_success.is_empty() {
            md.push_str("\n### Low Success Patterns\n\n");
            for p in &report.stats.low_success {
                md.push_str(&format!(
                    "- **{}**: {:.0}% success over {} calls\n",
                    p.tool_name, p.success_rate * 100.0, p.count
                ));
            }
        }

        if let Some(ref trace_stats) = report.trace_stats {
            md.push_str("\n## Trace Analysis\n\n");
            md.push_str(&format!("- Total traces: {}\n", trace_stats.total_traces));
            md.push_str(&format!("- Avg duration: {}ms\n", trace_stats.avg_duration_ms));
            md.push_str(&format!(
                "- Efficiency score: {:.2}\n",
                trace_stats.efficiency_score
            ));
        }

        if let Some(ref insights) = report.llm_insights {
            md.push_str("\n## LLM Insights\n\n");
            md.push_str(insights);
            md.push('\n');
        }

        md
    }

    fn build_reflection(
        &self,
        insights: Vec<String>,
        patterns: Vec<String>,
        recommendations: Vec<String>,
        stats: &ExperienceStats,
    ) -> Reflection {
        let mut statistics = serde_json::to_value(stats).unwrap_or(serde_json::Value::Null);
        // Embed detected patterns into the statistics JSON so they are not lost.
        if let Some(obj) = statistics.as_object_mut() {
            obj.insert(
                "patterns".into(),
                serde_json::Value::Array(
                    patterns
                        .into_iter()
                        .map(|p| serde_json::Value::String(p))
                        .collect(),
                ),
            );
        }
        Reflection {
            id: Uuid::new_v4().to_string(),
            period_start: String::new(),
            period_end: String::new(),
            insights,
            recommendations,
            statistics,
            is_remote: false,
        }
    }
}

impl Default for Reflector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
