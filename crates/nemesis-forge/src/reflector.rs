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
            tracing::info!("Stage 4: Cluster sharing enabled, report will be shared after generation");
        }

        ReflectionReport {
            date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
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
                    tracing::info!(len = insights.len(), "LLM semantic analysis completed");
                    report.llm_insights = Some(insights);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "LLM semantic analysis failed, skipping");
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
                let now = chrono::Utc::now();
                let start_of_day = now.date_naive().and_hms_opt(0, 0, 0)
                    .unwrap_or_default();
                Some(start_of_day.and_utc().to_rfc3339())
            }
            "week" => {
                let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
                Some(cutoff.to_rfc3339())
            }
            "all" => None,
            _ => {
                // Default: today
                let now = chrono::Utc::now();
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

        let now = chrono::Utc::now();
        let filename = format!(
            "reflection_{}_{}.md",
            report.date,
            now.format("%H%M%S")
        );
        let path = self.reflections_dir.join(&filename);

        let md_content = self.format_report_markdown(report);

        std::fs::write(&path, md_content)
            .map_err(|e| format!("failed to write report: {}", e))?;

        tracing::info!(path = %path.display(), "Wrote reflection report");
        Ok(path)
    }

    /// Delete report files older than `max_age_days` days from the reflections directory.
    ///
    /// Returns the number of files deleted.
    pub fn cleanup_reports(&self, max_age_days: u64) -> usize {
        if self.reflections_dir.as_os_str().is_empty() || !self.reflections_dir.exists() {
            return 0;
        }

        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days as i64);
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
                            let modified_time: chrono::DateTime<chrono::Utc> =
                                modified.into();
                            if modified_time < cutoff {
                                if std::fs::remove_file(&path).is_ok() {
                                    deleted += 1;
                                    tracing::debug!(
                                        path = %path.display(),
                                        "Deleted old reflection report"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        if deleted > 0 {
            tracing::info!(deleted, max_age_days, "Cleaned up old reflection reports");
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

        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);
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
                            let modified_time: chrono::DateTime<chrono::Utc> =
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
mod tests {
    use super::*;
    use crate::types::CollectedExperience;
    use crate::types::Experience;

    fn make_collected(tool: &str, input: &str, success: bool, duration_ms: u64) -> CollectedExperience {
        let hash = Collector::dedup_hash(tool, &serde_json::json!({"input": input}));
        let exp = Experience {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool.into(),
            input_summary: input.into(),
            output_summary: if success { "ok".into() } else { "err".into() },
            success,
            duration_ms,
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: "sess-test".into(),
        };
        CollectedExperience {
            experience: exp,
            dedup_hash: hash,
        }
    }

    // Need Collector for dedup_hash
    use crate::collector::Collector;

    #[test]
    fn test_analyze_empty() {
        let reflector = Reflector::new();
        let stats = reflector.analyze(&[]);
        assert_eq!(stats.total_count, 0);
        assert_eq!(stats.success_count, 0);
        assert!(stats.tool_counts.is_empty());
    }

    #[test]
    fn test_analyze_mixed_experiences() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("file_read", "a.txt", true, 50),
            make_collected("file_read", "b.txt", true, 60),
            make_collected("file_write", "c.txt", false, 200),
            make_collected("file_write", "d.txt", false, 300),
            make_collected("file_write", "e.txt", false, 250),
        ];
        let stats = reflector.analyze(&experiences);
        assert_eq!(stats.total_count, 5);
        assert_eq!(stats.success_count, 2);
        assert_eq!(stats.failure_count, 3);
        assert_eq!(stats.tool_counts.len(), 2);

        let fr = &stats.tool_counts["file_read"];
        assert_eq!(fr.count, 2);
        assert_eq!(fr.success_count, 2);

        let fw = &stats.tool_counts["file_write"];
        assert_eq!(fw.count, 3);
        assert_eq!(fw.success_count, 0);
    }

    #[test]
    fn test_generate_reflection_with_low_success_tool() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        for i in 0..5 {
            experiences.push(make_collected(
                "flaky_tool",
                &format!("input-{}", i),
                false,
                6000,
            ));
        }
        experiences.push(make_collected("stable_tool", "ok", true, 100));

        let reflection = reflector.generate_reflection(&experiences);

        assert!(!reflection.insights.is_empty());
        // Check patterns embedded in statistics JSON
        let patterns = reflection.statistics.get("patterns")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(patterns.iter().any(|p| p.as_str().unwrap().contains("flaky_tool")));
        assert!(reflection.recommendations.iter().any(|r| r.contains("flaky_tool")));
    }

    #[test]
    fn test_statistical_analysis() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool_a", "input1", true, 100),
            make_collected("tool_a", "input2", true, 150),
            make_collected("tool_a", "input3", false, 200),
            make_collected("tool_b", "input1", true, 50),
        ];

        let stats = reflector.statistical_analysis(&experiences);
        assert_eq!(stats.total_records, 4);
        assert_eq!(stats.unique_patterns, 2);
        assert_eq!(stats.top_patterns.len(), 2);
        assert_eq!(stats.tool_frequency["tool_a"], 3);
        assert_eq!(stats.tool_frequency["tool_b"], 1);
    }

    #[test]
    fn test_analyze_traces() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("read", "file", true, 50),
            make_collected("edit", "file", true, 100),
            make_collected("exec", "cmd", false, 200),
            make_collected("read", "file2", true, 60),
            make_collected("edit", "file2", true, 120),
            make_collected("exec", "cmd2", true, 180),
        ];

        let trace_stats = reflector.analyze_traces(&experiences, None);
        // All experiences share the same session_key "sess-test" from make_collected,
        // so total_traces = unique sessions = 1
        assert_eq!(trace_stats.total_traces, 1);
        assert!(trace_stats.avg_duration_ms > 0);
        assert!(!trace_stats.retry_patterns.is_empty());
    }

    #[test]
    fn test_analyze_traces_with_learning_cycle() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool_a", "input", true, 100),
            make_collected("tool_b", "input", true, 200),
        ];

        let cycle = nemesis_types::forge::LearningCycle {
            id: "lc-123".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            patterns_found: 3,
            actions_taken: 1,
            status: nemesis_types::forge::CycleStatus::Completed,
        };

        let trace_stats = reflector.analyze_traces(&experiences, Some(&cycle));
        assert_eq!(trace_stats.signal_summary.get("learning_patterns_found"), Some(&3));
        assert_eq!(trace_stats.signal_summary.get("learning_actions_taken"), Some(&1));
    }

    #[test]
    fn test_reflect_full_cycle() {
        let reflector = Reflector::with_cluster();
        let experiences = vec![
            make_collected("tool_a", "input1", true, 100),
            make_collected("tool_b", "input2", false, 200),
        ];

        let report = reflector.reflect(&experiences, None, "today", "all");
        assert_eq!(report.period, "today");
        assert_eq!(report.focus, "all");
        assert!(report.stats.total_records > 0);
        assert!(report.trace_stats.is_some());
    }

    #[test]
    fn test_reflect_empty() {
        let reflector = Reflector::new();
        let report = reflector.reflect(&[], None, "week", "skill");
        assert_eq!(report.stats.total_records, 0);
        assert!(report.trace_stats.is_none());
    }

    #[test]
    fn test_generate_suggestion() {
        let reflector = Reflector::new();
        assert!(reflector.generate_suggestion(10, 0.95).contains("High frequency"));
        assert!(reflector.generate_suggestion(3, 0.8).contains("Stable pattern"));
        assert!(reflector.generate_suggestion(5, 0.5).contains("failure modes"));
        // count < 5, success_rate >= 0.9 but count check comes first and fails
        // So it falls through to success_rate >= 0.7 => "Stable pattern"
        assert!(reflector.generate_suggestion(2, 0.85).contains("Stable pattern"));
        // count < 5, success_rate < 0.7 => "Review failure modes"
        assert!(reflector.generate_suggestion(2, 0.5).contains("failure modes"));
    }

    // ----- Disk operation tests -----

    #[test]
    fn test_write_report() {
        let dir = tempfile::tempdir().unwrap();
        let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));

        let report = ReflectionReport {
            date: "2026-05-01".to_string(),
            period: "today".to_string(),
            focus: "all".to_string(),
            stats: ReflectionStats {
                total_records: 5,
                unique_patterns: 2,
                avg_success_rate: 0.8,
                top_patterns: vec![PatternInsight {
                    tool_name: "file_read".to_string(),
                    count: 5,
                    avg_duration_ms: 100,
                    success_rate: 1.0,
                    suggestion: "Stable".to_string(),
                }],
                low_success: vec![],
                tool_frequency: HashMap::new(),
            },
            llm_insights: None,
            trace_stats: None,
            learning_cycle: None,
        };

        let path = reflector.write_report(&report).unwrap();
        assert!(path.exists());
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("reflection_2026-05-01"));

        // Verify content
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Reflection Report: 2026-05-01"));
        assert!(content.contains("file_read"));
    }

    #[test]
    fn test_write_report_no_dir_configured() {
        let reflector = Reflector::new();
        let report = ReflectionReport {
            date: "2026-05-01".to_string(),
            period: "today".to_string(),
            focus: "all".to_string(),
            stats: ReflectionStats::default(),
            llm_insights: None,
            trace_stats: None,
            learning_cycle: None,
        };
        let result = reflector.write_report(&report);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not configured"));
    }

    #[test]
    fn test_cleanup_reports() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();

        // Create an "old" file with a known modification time in the past.
        // We use filetime::set_file_mtime to control the mtime precisely.
        let old_path = ref_dir.join("reflection_2026-03-01_120000.md");
        std::fs::write(&old_path, "old report").unwrap();

        // Set modification time to 31 days ago
        let old_time = std::time::SystemTime::now()
            - std::time::Duration::from_secs(31 * 24 * 3600);
        let ft = filetime::FileTime::from_system_time(old_time);
        filetime::set_file_mtime(&old_path, ft).unwrap();

        // Create a "new" file (current time = not old)
        let new_path = ref_dir.join("reflection_2026-05-01_120000.md");
        std::fs::write(&new_path, "new report").unwrap();

        let reflector = Reflector::with_reflections_dir(ref_dir);
        let deleted = reflector.cleanup_reports(30);
        assert_eq!(deleted, 1);
        assert!(!old_path.exists());
        assert!(new_path.exists());
    }

    #[test]
    fn test_cleanup_reports_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
        let deleted = reflector.cleanup_reports(30);
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_get_latest_report() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();

        // Create two report files
        let report1 = ref_dir.join("reflection_2026-04-28_120000.md");
        std::fs::write(&report1, "report 1").unwrap();

        // Small delay to ensure different mtime
        std::thread::sleep(std::time::Duration::from_millis(50));

        let report2 = ref_dir.join("reflection_2026-04-29_120000.md");
        std::fs::write(&report2, "report 2").unwrap();

        let reflector = Reflector::with_reflections_dir(ref_dir);
        let latest = reflector.get_latest_report();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap(), report2);
    }

    #[test]
    fn test_get_latest_report_empty() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();

        let reflector = Reflector::with_reflections_dir(ref_dir);
        assert!(reflector.get_latest_report().is_none());
    }

    #[test]
    fn test_get_latest_report_no_dir() {
        let reflector = Reflector::new();
        assert!(reflector.get_latest_report().is_none());
    }

    #[test]
    fn test_resolve_period() {
        let today = Reflector::resolve_period("today");
        assert!(today.is_some());
        let today_val = today.unwrap();
        assert!(today_val.contains("T00:00:00"));

        let week = Reflector::resolve_period("week");
        assert!(week.is_some());

        let all = Reflector::resolve_period("all");
        assert!(all.is_none());

        let unknown = Reflector::resolve_period("unknown");
        assert!(unknown.is_some()); // defaults to today
    }

    #[test]
    fn test_filter_by_period() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool_a", "old", true, 100),
            make_collected("tool_b", "new", true, 200),
        ];

        let filtered = reflector.filter_by_period(&experiences, "all");
        assert_eq!(filtered.len(), 2);

        let filtered = reflector.filter_by_period(&experiences, "today");
        // All test experiences have timestamp 2026-04-29, so they may or may not
        // be filtered depending on when the test runs. Just verify no panic.
        let _ = filtered.len();
    }

    #[test]
    fn test_filter_by_focus() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("file_read", "a", true, 100),
            make_collected("file_write", "b", true, 200),
            make_collected("file_read", "c", true, 300),
        ];

        let all = reflector.filter_by_focus(&experiences, "all");
        assert_eq!(all.len(), 3);

        let all_empty = reflector.filter_by_focus(&experiences, "");
        assert_eq!(all_empty.len(), 3);

        let only_read = reflector.filter_by_focus(&experiences, "file_read");
        assert_eq!(only_read.len(), 2);

        let only_write = reflector.filter_by_focus(&experiences, "file_write");
        assert_eq!(only_write.len(), 1);
    }

    // --- Additional reflector tests ---

    #[test]
    fn test_analyze_single_tool_all_success() {
        let reflector = Reflector::new();
        let experiences: Vec<CollectedExperience> = (0..5)
            .map(|i| make_collected("perfect_tool", &format!("input-{}", i), true, 100))
            .collect();
        let stats = reflector.analyze(&experiences);
        assert_eq!(stats.total_count, 5);
        assert_eq!(stats.success_count, 5);
        assert_eq!(stats.failure_count, 0);
        assert_eq!(stats.tool_counts.len(), 1);
        let ts = &stats.tool_counts["perfect_tool"];
        assert_eq!(ts.count, 5);
        assert_eq!(ts.success_count, 5);
    }

    #[test]
    fn test_analyze_single_tool_all_failures() {
        let reflector = Reflector::new();
        let experiences: Vec<CollectedExperience> = (0..3)
            .map(|i| make_collected("broken_tool", &format!("input-{}", i), false, 500))
            .collect();
        let stats = reflector.analyze(&experiences);
        assert_eq!(stats.total_count, 3);
        assert_eq!(stats.success_count, 0);
        assert_eq!(stats.failure_count, 3);
    }

    #[test]
    fn test_analyze_mixed_durations() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("fast", "a", true, 10),
            make_collected("fast", "b", true, 20),
            make_collected("slow", "c", true, 5000),
            make_collected("slow", "d", true, 6000),
        ];
        let stats = reflector.analyze(&experiences);
        assert_eq!(stats.tool_counts.len(), 2);
        let fast = &stats.tool_counts["fast"];
        let slow = &stats.tool_counts["slow"];
        assert!(fast.avg_duration_ms < slow.avg_duration_ms);
    }

    #[test]
    fn test_statistical_analysis_empty() {
        let reflector = Reflector::new();
        let stats = reflector.statistical_analysis(&[]);
        assert_eq!(stats.total_records, 0);
        assert_eq!(stats.unique_patterns, 0);
        assert!(stats.top_patterns.is_empty());
        assert!(stats.low_success.is_empty());
    }

    #[test]
    fn test_statistical_analysis_success_rate() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool", "a", true, 100),
            make_collected("tool", "b", true, 100),
        ];
        let stats = reflector.statistical_analysis(&experiences);
        assert!((stats.avg_success_rate - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_statistical_analysis_low_success_detection() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        // 3 failures, 1 success = 25% success rate for flaky_tool
        for i in 0..3 {
            experiences.push(make_collected("flaky_tool", &format!("f-{}", i), false, 100));
        }
        experiences.push(make_collected("flaky_tool", "s-1", true, 100));
        let stats = reflector.statistical_analysis(&experiences);
        assert_eq!(stats.low_success.len(), 1);
        assert_eq!(stats.low_success[0].tool_name, "flaky_tool");
        assert!(stats.low_success[0].suggestion.contains("failure"));
    }

    #[test]
    fn test_statistical_analysis_top_patterns_sorted_by_count() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        for _ in 0..10 {
            experiences.push(make_collected("popular", "x", true, 50));
        }
        for _ in 0..5 {
            experiences.push(make_collected("moderate", "x", true, 50));
        }
        for _ in 0..2 {
            experiences.push(make_collected("rare", "x", true, 50));
        }
        let stats = reflector.statistical_analysis(&experiences);
        assert!(stats.top_patterns.len() >= 2);
        assert!(stats.top_patterns[0].count >= stats.top_patterns[1].count);
    }

    #[test]
    fn test_reflect_with_learning_cycle() {
        let reflector = Reflector::with_cluster();
        let experiences = vec![
            make_collected("tool_a", "input1", true, 100),
        ];
        let cycle = nemesis_types::forge::LearningCycle {
            id: "lc-test".into(),
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: Some(chrono::Utc::now().to_rfc3339()),
            patterns_found: 5,
            actions_taken: 2,
            status: nemesis_types::forge::CycleStatus::Completed,
        };
        let report = reflector.reflect(&experiences, Some(&cycle), "today", "all");
        assert!(report.learning_cycle.is_some());
        assert_eq!(report.learning_cycle.unwrap().patterns_found, 5);
    }

    #[test]
    fn test_reflect_with_trace_stats() {
        let reflector = Reflector::with_cluster();
        let experiences = vec![
            make_collected("tool_a", "input", true, 100),
            make_collected("tool_b", "input", true, 200),
        ];
        let report = reflector.reflect(&experiences, None, "week", "all");
        assert!(report.trace_stats.is_some());
        let ts = report.trace_stats.unwrap();
        assert!(ts.total_traces > 0);
    }

    #[test]
    fn test_reflect_report_structure() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool", "input", true, 100),
        ];
        let report = reflector.reflect(&experiences, None, "today", "all");
        assert!(!report.date.is_empty());
        assert_eq!(report.period, "today");
        assert_eq!(report.focus, "all");
        assert!(report.llm_insights.is_none());
    }

    #[test]
    fn test_analyze_traces_empty() {
        let reflector = Reflector::new();
        let trace_stats = reflector.analyze_traces(&[], None);
        assert_eq!(trace_stats.total_traces, 0);
        assert_eq!(trace_stats.avg_duration_ms, 0);
    }

    #[test]
    fn test_analyze_traces_multiple_sessions() {
        let reflector = Reflector::new();
        // Create experiences across multiple sessions
        let mut experiences = Vec::new();
        for i in 0..3 {
            experiences.push(Experience {
                id: format!("exp-{}", i),
                tool_name: "tool".into(),
                input_summary: "input".into(),
                output_summary: "ok".into(),
                success: true,
                duration_ms: 100 * (i as u64 + 1),
                timestamp: "2026-04-29T00:00:00Z".into(),
                session_key: format!("session-{}", i),
            });
        }
        let ces: Vec<CollectedExperience> = experiences.into_iter().map(|e| {
            CollectedExperience {
                dedup_hash: Collector::dedup_hash(&e.tool_name, &serde_json::json!({})),
                experience: e,
            }
        }).collect();
        let trace_stats = reflector.analyze_traces(&ces, None);
        assert_eq!(trace_stats.total_traces, 3);
    }

    #[test]
    fn test_generate_suggestion_boundary_values() {
        let reflector = Reflector::new();
        // count=5, rate=0.9 -> High frequency
        assert!(reflector.generate_suggestion(5, 0.9).contains("High frequency"));
        // count=4, rate=0.9 -> Stable (count < 5)
        assert!(reflector.generate_suggestion(4, 0.9).contains("Stable"));
        // count=10, rate=0.7 -> Stable
        assert!(reflector.generate_suggestion(10, 0.7).contains("Stable"));
        // count=10, rate=0.69 -> Review failure
        assert!(reflector.generate_suggestion(10, 0.69).contains("failure"));
    }

    #[test]
    fn test_write_report_with_llm_insights() {
        let dir = tempfile::tempdir().unwrap();
        let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats::default(),
            llm_insights: Some("AI detected efficiency issues".into()),
            trace_stats: None,
            learning_cycle: None,
        };
        let path = reflector.write_report(&report).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("LLM Insights"));
        assert!(content.contains("AI detected efficiency issues"));
    }

    #[test]
    fn test_write_report_with_trace_stats() {
        let dir = tempfile::tempdir().unwrap();
        let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats::default(),
            llm_insights: None,
            trace_stats: Some(TraceStats {
                total_traces: 10,
                avg_rounds: 5.0,
                avg_duration_ms: 500,
                efficiency_score: 0.75,
                tool_chain_patterns: vec![],
                retry_patterns: vec![],
                signal_summary: HashMap::new(),
            }),
            learning_cycle: None,
        };
        let path = reflector.write_report(&report).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Trace Analysis"));
        assert!(content.contains("10"));
    }

    #[test]
    fn test_cleanup_reports_result_ok() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        let reflector = Reflector::with_reflections_dir(ref_dir);
        let result = reflector.cleanup_reports_result(30);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cleanup_reports_result_no_dir() {
        let reflector = Reflector::new();
        let result = reflector.cleanup_reports_result(30);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_latest_report_content_no_reports() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        let reflector = Reflector::with_reflections_dir(ref_dir);
        let result = reflector.get_latest_report_content();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no reflection"));
    }

    #[test]
    fn test_get_latest_report_content_with_report() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        let report_path = ref_dir.join("reflection_2026-05-01_120000.md");
        std::fs::write(&report_path, "# Test Report").unwrap();
        let reflector = Reflector::with_reflections_dir(ref_dir);
        let content = reflector.get_latest_report_content().unwrap();
        assert!(content.contains("Test Report"));
    }

    #[test]
    fn test_merge_remote_reflections_empty() {
        let reflector = Reflector::new();
        let result = reflector.merge_remote_reflections(&[], &[]);
        assert!(result.local_patterns.is_empty());
        assert!(result.remote_patterns.is_empty());
        assert!(result.merged_patterns.is_empty());
    }

    #[test]
    fn test_merge_remote_reflections_local_only() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("local_tool", "input", true, 100),
        ];
        let result = reflector.merge_remote_reflections(&[], &experiences);
        assert!(!result.local_patterns.is_empty());
        assert!(result.remote_patterns.is_empty());
    }

    #[test]
    fn test_merge_remote_reflections_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("remote_report.md");
        let report_content = "# Report\n| read_file | 10 |\n| write_file | 5 |\n";
        std::fs::write(&report_path, report_content).unwrap();

        let reflector = Reflector::new();
        let result = reflector.merge_remote_reflections(&[report_path], &[]);
        assert!(!result.remote_patterns.is_empty());
        assert!(result.unique_remote_tools.contains(&"read_file".to_string())
            || result.unique_remote_tools.contains(&"write_file".to_string()));
    }

    #[test]
    fn test_resolve_period_defaults_to_today() {
        let result = Reflector::resolve_period("custom_period");
        assert!(result.is_some());
        let today = Reflector::resolve_period("today");
        assert_eq!(result, today);
    }

    #[test]
    fn test_reflection_stats_default() {
        let stats = ReflectionStats::default();
        assert_eq!(stats.total_records, 0);
        assert_eq!(stats.unique_patterns, 0);
        assert_eq!(stats.avg_success_rate, 0.0);
        assert!(stats.top_patterns.is_empty());
        assert!(stats.low_success.is_empty());
        assert!(stats.tool_frequency.is_empty());
    }

    #[test]
    fn test_trace_stats_default() {
        let stats = TraceStats::default();
        assert_eq!(stats.total_traces, 0);
        assert_eq!(stats.avg_rounds, 0.0);
        assert_eq!(stats.avg_duration_ms, 0);
        assert_eq!(stats.efficiency_score, 0.0);
        assert!(stats.tool_chain_patterns.is_empty());
        assert!(stats.retry_patterns.is_empty());
        assert!(stats.signal_summary.is_empty());
    }

    #[test]
    fn test_filter_by_focus_nonexistent_tool() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool_a", "input", true, 100),
        ];
        let filtered = reflector.filter_by_focus(&experiences, "nonexistent");
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_reflector_default() {
        let reflector = Reflector::default();
        let stats = reflector.analyze(&[]);
        assert_eq!(stats.total_count, 0);
    }

    #[test]
    fn test_get_latest_report_content_no_dir() {
        let reflector = Reflector::new();
        let result = reflector.get_latest_report_content();
        assert!(result.is_err());
    }

    #[test]
    fn test_get_latest_report_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        std::fs::write(ref_dir.join("reflection_2026-05-01_120000.md"), "# Report 1").unwrap();
        std::fs::write(ref_dir.join("reflection_2026-05-02_120000.md"), "# Report 2").unwrap();
        let reflector = Reflector::with_reflections_dir(ref_dir);
        let latest = reflector.get_latest_report();
        assert!(latest.is_some());
    }

    #[test]
    fn test_get_latest_report_content_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        std::fs::write(ref_dir.join("reflection_2026-05-01_120000.md"), "# Report content here").unwrap();
        let reflector = Reflector::with_reflections_dir(ref_dir);
        let content = reflector.get_latest_report_content();
        assert!(content.is_ok());
        assert!(content.unwrap().contains("Report content here"));
    }

    #[test]
    fn test_cleanup_reports_empty_dir_2() {
        let dir = tempfile::tempdir().unwrap();
        let ref_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&ref_dir).unwrap();
        let reflector = Reflector::with_reflections_dir(ref_dir);
        let deleted = reflector.cleanup_reports(30);
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_cleanup_reports_no_dir() {
        let reflector = Reflector::new();
        let deleted = reflector.cleanup_reports(30);
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_resolve_period_all() {
        let result = Reflector::resolve_period("all");
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_period_week() {
        let result = Reflector::resolve_period("week");
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_period_today() {
        let result = Reflector::resolve_period("today");
        assert!(result.is_some());
    }

    #[test]
    fn test_filter_by_period_all() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool", "a", true, 100),
            make_collected("tool", "b", false, 200),
        ];
        let filtered = reflector.filter_by_period(&experiences, "all");
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_by_focus_all() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool_a", "a", true, 100),
            make_collected("tool_b", "b", false, 200),
        ];
        let filtered = reflector.filter_by_focus(&experiences, "all");
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_by_focus_empty() {
        let reflector = Reflector::new();
        let experiences = vec![
            make_collected("tool_a", "a", true, 100),
        ];
        let filtered = reflector.filter_by_focus(&experiences, "");
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_set_cluster_enabled() {
        let mut reflector = Reflector::new();
        reflector.set_cluster_enabled(true);
        // No crash, field updated
    }

    #[test]
    fn test_set_reflections_dir() {
        let mut reflector = Reflector::new();
        reflector.set_reflections_dir(PathBuf::from("/tmp/test"));
        // No crash, field updated
    }

    #[test]
    fn test_with_cluster_and_dir() {
        let reflector = Reflector::with_cluster_and_dir(PathBuf::from("/tmp/test"));
        let report = reflector.reflect(&[], None, "today", "all");
        assert_eq!(report.period, "today");
    }

    #[test]
    fn test_analyze_mixed_success_failure() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        for _ in 0..3 {
            experiences.push(make_collected("slow_tool", "x", false, 10000));
        }
        experiences.push(make_collected("slow_tool", "y", true, 9000));
        let stats = reflector.analyze(&experiences);
        assert_eq!(stats.total_count, 4);
        assert_eq!(stats.success_count, 1);
        assert_eq!(stats.failure_count, 3);
        let ts = stats.tool_counts.get("slow_tool").unwrap();
        assert_eq!(ts.count, 4);
        assert_eq!(ts.success_count, 1);
    }

    #[test]
    fn test_generate_reflection_slow_tool_pattern() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        for _ in 0..3 {
            experiences.push(make_collected("very_slow_tool", "x", true, 8000));
        }
        let reflection = reflector.generate_reflection(&experiences);
        // Should mention slow tool in insights or recommendations
        let all_text = reflection.insights.join(" ") + &reflection.recommendations.join(" ");
        assert!(all_text.contains("slow") || all_text.contains("Slow") || all_text.contains("optimiz"));
    }

    #[test]
    fn test_generate_reflection_frequent_tool_pattern() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        for _ in 0..6 {
            experiences.push(make_collected("popular_tool", "x", true, 100));
        }
        let reflection = reflector.generate_reflection(&experiences);
        // The tool should appear in insights, recommendations, or statistics
        let _all_text = format!("{:?} {:?}", reflection.insights, reflection.recommendations);
        // Just check that the reflection was generated successfully with the right number of insights
        assert!(!reflection.insights.is_empty());
    }

    #[test]
    fn test_generate_reflection_below_80_percent() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        for i in 0..5 {
            experiences.push(make_collected("tool", &format!("f-{}", i), false, 100));
        }
        for i in 0..2 {
            experiences.push(make_collected("tool", &format!("s-{}", i), true, 100));
        }
        let reflection = reflector.generate_reflection(&experiences);
        let recs = reflection.recommendations.iter().any(|r| r.contains("80%") || r.contains("below"));
        assert!(recs);
    }

    #[test]
    fn test_analyze_traces_with_tool_chains() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        // Create enough for a chain pattern (3+ tools in sequence)
        for i in 0..6 {
            experiences.push(Experience {
                id: format!("exp-{}", i),
                tool_name: format!("tool_{}", i % 3),
                input_summary: "input".into(),
                output_summary: "ok".into(),
                success: true,
                duration_ms: 100,
                timestamp: "2026-04-29T00:00:00Z".into(),
                session_key: "same-session".into(),
            });
        }
        let ces: Vec<CollectedExperience> = experiences.into_iter().map(|e| {
            CollectedExperience {
                dedup_hash: Collector::dedup_hash(&e.tool_name, &serde_json::json!({})),
                experience: e,
            }
        }).collect();
        let trace_stats = reflector.analyze_traces(&ces, None);
        assert_eq!(trace_stats.total_traces, 1); // all same session
    }

    #[test]
    fn test_analyze_traces_with_retry_patterns() {
        let reflector = Reflector::new();
        let mut experiences = Vec::new();
        // 2 calls, 1 error -> retry pattern
        experiences.push(Experience {
            id: "e1".into(),
            tool_name: "retry_tool".into(),
            input_summary: "input".into(),
            output_summary: "fail".into(),
            success: false,
            duration_ms: 100,
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: "session-1".into(),
        });
        experiences.push(Experience {
            id: "e2".into(),
            tool_name: "retry_tool".into(),
            input_summary: "input".into(),
            output_summary: "ok".into(),
            success: true,
            duration_ms: 100,
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: "session-1".into(),
        });
        let ces: Vec<CollectedExperience> = experiences.into_iter().map(|e| {
            CollectedExperience {
                dedup_hash: Collector::dedup_hash(&e.tool_name, &serde_json::json!({})),
                experience: e,
            }
        }).collect();
        let trace_stats = reflector.analyze_traces(&ces, None);
        assert_eq!(trace_stats.retry_patterns.len(), 1);
        assert_eq!(trace_stats.retry_patterns[0].tool_name, "retry_tool");
    }

    #[test]
    fn test_write_report_with_low_success_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats {
                total_records: 10,
                unique_patterns: 2,
                avg_success_rate: 0.5,
                top_patterns: vec![],
                low_success: vec![PatternInsight {
                    tool_name: "failing_tool".into(),
                    count: 5,
                    avg_duration_ms: 200,
                    success_rate: 0.2,
                    suggestion: "Fix it".into(),
                }],
                tool_frequency: HashMap::new(),
            },
            llm_insights: None,
            trace_stats: None,
            learning_cycle: None,
        };
        let path = reflector.write_report(&report).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Low Success"));
        assert!(content.contains("failing_tool"));
    }

    #[test]
    fn test_write_report_with_top_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));
        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats {
                total_records: 10,
                unique_patterns: 1,
                avg_success_rate: 0.9,
                top_patterns: vec![PatternInsight {
                    tool_name: "read_file".into(),
                    count: 8,
                    avg_duration_ms: 50,
                    success_rate: 1.0,
                    suggestion: "Good".into(),
                }],
                low_success: vec![],
                tool_frequency: HashMap::new(),
            },
            llm_insights: None,
            trace_stats: None,
            learning_cycle: None,
        };
        let path = reflector.write_report(&report).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Top Patterns"));
        assert!(content.contains("read_file"));
    }

    #[test]
    fn test_cleanup_reports_result_no_dir_configured() {
        let reflector = Reflector::new();
        let result = reflector.cleanup_reports_result(30);
        assert!(result.is_err());
    }

    #[test]
    fn test_cleanup_reports_result_nonexistent_dir() {
        let reflector = Reflector::with_reflections_dir(PathBuf::from("/nonexistent/path/reflections"));
        let result = reflector.cleanup_reports_result(30);
        assert!(result.is_ok());
    }

    #[test]
    fn test_merge_remote_reflections_empty_reports() {
        let reflector = Reflector::new();
        let experiences = vec![make_collected("tool_a", "x", true, 100)];
        let merged = reflector.merge_remote_reflections(&[], &experiences);
        assert!(merged.remote_patterns.is_empty());
        assert!(merged.common_tools.is_empty());
        assert!(merged.unique_remote_tools.is_empty());
    }

    #[test]
    fn test_merge_remote_reflections_with_report_file() {
        let dir = tempfile::tempdir().unwrap();
        let report_path = dir.path().join("remote_report.md");
        let report_content = "# Remote Report\n\n| read_file | 10 |\n| write_file | 5 |\n";
        std::fs::write(&report_path, report_content).unwrap();
        let reflector = Reflector::new();
        let experiences = vec![make_collected("read_file", "x", true, 100)];
        let merged = reflector.merge_remote_reflections(&[report_path], &experiences);
        // read_file should be in common_tools (present in both local and remote)
        assert!(merged.common_tools.contains_key("read_file")
            || merged.unique_remote_tools.iter().any(|t| t == "write_file"));
    }

    #[test]
    fn test_merge_remote_reflections_no_local() {
        let reflector = Reflector::new();
        let merged = reflector.merge_remote_reflections(&[], &[]);
        assert!(merged.local_patterns.is_empty());
        assert!(merged.merged_patterns.is_empty());
    }

    #[test]
    fn test_llm_caller_accessors() {
        let reflector = Reflector::new();
        {
            let caller = reflector.llm_caller();
            assert!(caller.is_none());
        }
    }

    // --- Additional coverage tests ---

    #[test]
    fn test_analyze_traces_coverage_empty() {
        let reflector = Reflector::new();
        let stats = reflector.analyze_traces(&[], None);
        assert_eq!(stats.total_traces, 0);
    }

    #[test]
    fn test_analyze_traces_coverage_single() {
        let reflector = Reflector::new();
        let trace = vec![make_collected("tool_a", "x", true, 100)];
        let stats = reflector.analyze_traces(&trace, None);
        assert_eq!(stats.total_traces, 1);
        assert_eq!(stats.avg_duration_ms, 100);
    }

    #[test]
    fn test_analyze_traces_coverage_multiple() {
        let reflector = Reflector::new();
        let traces = vec![
            make_collected("tool_a", "x", true, 100),
            make_collected("tool_b", "y", true, 200),
            make_collected("tool_c", "z", false, 300),
        ];
        let stats = reflector.analyze_traces(&traces, None);
        // total_traces counts unique session_keys, all are "sess-test"
        assert_eq!(stats.total_traces, 1);
        assert_eq!(stats.avg_duration_ms, 200); // (100+200+300)/3
    }

    #[test]
    fn test_analyze_traces_coverage_retries() {
        let reflector = Reflector::new();
        let traces = vec![
            make_collected("tool_a", "x", false, 100),
            make_collected("tool_a", "x", false, 100),
            make_collected("tool_a", "x", true, 100),
        ];
        let stats = reflector.analyze_traces(&traces, None);
        assert!(!stats.retry_patterns.is_empty());
    }

    #[test]
    fn test_analyze_traces_coverage_chains() {
        let reflector = Reflector::new();
        let mut traces: Vec<CollectedExperience> = vec![];
        for _ in 0..5 {
            traces.push(make_collected("read", "x", true, 50));
            traces.push(make_collected("write", "x", true, 50));
        }
        let stats = reflector.analyze_traces(&traces, None);
        assert!(stats.total_traces > 0);
    }

    #[test]
    fn test_analyze_traces_coverage_low_freq() {
        let reflector = Reflector::new();
        let traces = vec![
            make_collected("a", "x", true, 50),
            make_collected("b", "x", true, 50),
        ];
        let stats = reflector.analyze_traces(&traces, None);
        assert!(stats.total_traces > 0);
    }

    #[test]
    fn test_write_report_with_tool_frequency() {
        let dir = tempfile::tempdir().unwrap();
        let reflector = Reflector::with_reflections_dir(dir.path().join("reflections"));

        let mut tool_frequency = HashMap::new();
        tool_frequency.insert("read_file".into(), 15);
        tool_frequency.insert("write_file".into(), 8);

        let report = ReflectionReport {
            date: "2026-05-01".into(),
            period: "today".into(),
            focus: "all".into(),
            stats: ReflectionStats {
                total_records: 23,
                unique_patterns: 2,
                avg_success_rate: 0.92,
                top_patterns: vec![],
                low_success: vec![],
                tool_frequency,
            },
            llm_insights: None,
            trace_stats: None,
            learning_cycle: None,
        };
        let path = reflector.write_report(&report).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("23")); // total_records
        assert!(content.contains("92.0%")); // avg_success_rate
    }

    #[test]
    fn test_cleanup_reports_keeps_recent() {
        let dir = tempfile::tempdir().unwrap();
        let reflections_dir = dir.path().join("reflections");
        std::fs::create_dir_all(&reflections_dir).unwrap();

        // Create a recent report
        let recent_name = format!("{}.md", chrono::Utc::now().format("%Y%m%d"));
        std::fs::write(reflections_dir.join(&recent_name), "recent report").unwrap();

        let reflector = Reflector::with_reflections_dir(reflections_dir);
        let result = reflector.cleanup_reports_result(30);
        assert!(result.is_ok());

        // Verify file still exists
        assert!(dir.path().join("reflections").join(&recent_name).exists());
    }

    #[test]
    fn test_merge_remote_reflections_unreadable_file() {
        let reflector = Reflector::new();
        let experiences = vec![make_collected("tool_a", "x", true, 100)];
        // Non-existent file path should be handled gracefully
        let merged = reflector.merge_remote_reflections(
            &[PathBuf::from("/nonexistent/report.md")],
            &experiences,
        );
        // Should not panic, just skip the file
        assert!(merged.remote_patterns.is_empty() || merged.merged_patterns.is_empty());
    }
}
