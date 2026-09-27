//! Reflection LLM integration - semantic analysis via LLM.
//!
//! Provides LLM-powered deep analysis of statistical data, generating
//! insights, suggestions, and extracting structured data from responses.

use async_trait::async_trait;

use crate::reflector::{ReflectionStats, TraceStats};
use crate::types::{Artifact, ExperienceStats};

/// Trait for making LLM calls within the forge module.
///
/// This avoids a direct dependency on nemesis-providers. The integration
/// layer (bot_service) provides the concrete implementation.
#[async_trait]
pub trait LLMCaller: Send + Sync {
    /// Send a chat message to the LLM and return the text response.
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: Option<i64>,
    ) -> Result<String, String>;
}

/// Build an LLM prompt from reflection statistics for semantic analysis.
pub fn build_analysis_prompt(stats: &ExperienceStats, total_tools: usize) -> String {
    let mut sb = String::new();
    sb.push_str("分析以下来自智能代理系统的工具使用数据：\n\n");

    sb.push_str(&format!("- 工具调用总数：{}\n", stats.total_count));
    sb.push_str(&format!("- 独立模式数：{}\n", total_tools));
    sb.push_str(&format!(
        "- 平均成功率：{:.1}%\n\n",
        if stats.total_count > 0 {
            stats.success_count as f64 / stats.total_count as f64 * 100.0
        } else {
            0.0
        }
    ));

    sb.push_str("## 工具频次\n");
    for (tool, ts) in &stats.tool_counts {
        sb.push_str(&format!("- {}: {} 次\n", tool, ts.count));
    }

    sb.push_str("\n请给出：\n");
    sb.push_str("1. 可沉淀为可复用技能的关键模式\n");
    sb.push_str("2. 有待改进之处\n");
    sb.push_str("3. 优化建议\n");

    sb
}

/// Build the full analysis prompt including all stages (matches Go semanticAnalysis).
///
/// This is the comprehensive prompt builder that includes:
/// - Statistical summary (tool frequency, success rates)
/// - High-frequency and low-success patterns
/// - Existing artifacts
/// - Phase 5: conversation-level trace insights
/// - Phase 6: closed-loop learning state
pub fn build_full_analysis_prompt(
    stats: &ReflectionStats,
    artifacts: &[Artifact],
    trace_stats: Option<&TraceStats>,
    cycle: Option<&nemesis_types::forge::LearningCycle>,
) -> String {
    let mut sb = String::new();
    sb.push_str("分析以下来自智能代理系统的工具使用数据，并给出洞察：\n\n");

    // Statistical Summary
    sb.push_str("## 统计概览\n");
    sb.push_str(&format!("- 工具调用总数：{}\n", stats.total_records));
    sb.push_str(&format!("- 独立模式数：{}\n", stats.unique_patterns));
    sb.push_str(&format!(
        "- 平均成功率：{:.1}%\n\n",
        stats.avg_success_rate * 100.0
    ));

    // Tool Frequency
    sb.push_str("## 工具频次\n");
    for (tool, count) in &stats.tool_frequency {
        sb.push_str(&format!("- {}: {} 次\n", tool, count));
    }

    // High-Frequency Patterns
    sb.push_str("\n## 高频模式\n");
    for (i, p) in stats.top_patterns.iter().enumerate() {
        if i >= 5 {
            break;
        }
        sb.push_str(&format!(
            "- {}: {} 次，成功率 {:.0}%，平均 {}ms\n",
            p.tool_name,
            p.count,
            p.success_rate * 100.0,
            p.avg_duration_ms
        ));
    }

    // Low Success Patterns
    if !stats.low_success.is_empty() {
        sb.push_str("\n## 低成功率模式\n");
        for p in &stats.low_success {
            sb.push_str(&format!(
                "- {}: {} 次，成功率 {:.0}%\n",
                p.tool_name,
                p.count,
                p.success_rate * 100.0
            ));
        }
    }

    // Existing Artifacts
    sb.push_str("\n## 现有 Forge 产物\n");
    for a in artifacts {
        sb.push_str(&format!(
            "- [{:?}] {} v{}（{:?}，{} 次使用）\n",
            a.kind, a.name, a.version, a.status, a.usage_count
        ));
    }

    // Phase 5: Conversation-level trace insights
    if let Some(ts) = trace_stats {
        sb.push_str("\n## 会话级轨迹洞察\n");
        sb.push_str(&format!("- 会话总数：{}\n", ts.total_traces));
        sb.push_str(&format!("- 平均每会话 LLM 轮数：{:.1}\n", ts.avg_rounds));
        sb.push_str(&format!(
            "- 效率得分：{:.2}（每轮工具步数）\n",
            ts.efficiency_score
        ));

        if !ts.tool_chain_patterns.is_empty() {
            sb.push_str("\n### 高频工具链\n");
            for p in &ts.tool_chain_patterns {
                sb.push_str(&format!(
                    "- {}: {} 次，平均 {:.1} 轮，成功率 {:.0}%\n",
                    p.chain,
                    p.count,
                    p.avg_rounds,
                    p.success_rate * 100.0
                ));
            }
        }

        if !ts.retry_patterns.is_empty() {
            sb.push_str("\n### 重试模式\n");
            for p in &ts.retry_patterns {
                sb.push_str(&format!(
                    "- {}: {} 次重试，成功率 {:.0}%\n",
                    p.tool_name,
                    p.retry_count,
                    p.success_rate * 100.0
                ));
            }
        }

        if !ts.signal_summary.is_empty() {
            sb.push_str("\n### 会话信号\n");
            for (sig_type, count) in &ts.signal_summary {
                sb.push_str(&format!("- {}：{} 次\n", sig_type, count));
            }
        }
    }

    // Phase 6: Closed-loop learning state
    if let Some(cycle) = cycle {
        sb.push_str("\n## 闭环学习状态（第六阶段）\n");
        sb.push_str(&format!("- 发现模式数：{}\n", cycle.patterns_found));
        sb.push_str(&format!("- 已执行动作数：{}\n", cycle.actions_taken));
    }

    sb.push_str("\n请给出：\n");
    sb.push_str("1. 可沉淀为可复用技能或脚本的关键模式\n");
    sb.push_str("2. 工具使用有待改进之处\n");
    sb.push_str("3. 高频操作的优化建议\n");

    sb
}

/// Perform semantic analysis by calling the LLM.
///
/// This is the core function that was missing in Rust. It builds the full
/// analysis prompt (including all phases) and calls the LLM provider.
pub async fn semantic_analysis(
    caller: &dyn LLMCaller,
    stats: &ReflectionStats,
    artifacts: &[Artifact],
    trace_stats: Option<&TraceStats>,
    cycle: Option<&nemesis_types::forge::LearningCycle>,
    max_tokens: Option<i64>,
) -> Result<String, String> {
    let user_prompt = build_full_analysis_prompt(stats, artifacts, trace_stats, cycle);

    // 提示词单一真相源在 nemesis-prompts（M7 集中化）。
    let system_prompt = nemesis_prompts::forge::SEMANTIC_ANALYSIS_SYSTEM_PROMPT;

    caller.chat(system_prompt, &user_prompt, max_tokens).await
}

/// Forge 产物质量评审 user prompt 构造（LLM-as-Judge 单一真相源）：
/// evaluator / validator / pipeline 三个调用点共用同一份中文计分卡与 JSON
/// schema（`version` 传 `None` 时省略版本行，兼容不带版本号的调用面）。
/// JSON 键与权重是解析契约（消费端按 `correctness`/`quality`/`security`/
/// `reusability` 取数加权），任何一侧都不得单方面改动。
pub fn quality_review_prompt(
    kind: &str,
    name: &str,
    version: Option<&str>,
    content: &str,
) -> String {
    // 文本单一真相源在 nemesis-prompts（M7 集中化）；此处保留原签名作
    // forge 内便捷入口（evaluator/validator/pipeline 调用点不动）。
    nemesis_prompts::forge::quality_review_prompt(kind, name, version, content)
}

/// 产物质量评审员 system prompt（三个评审调用点共用）。
pub use nemesis_prompts::forge::QUALITY_REVIEWER_SYSTEM_PROMPT;

/// Parse bullet-point insights from an LLM response.
pub fn parse_insights(response: &str) -> Vec<String> {
    let mut insights = Vec::new();
    for line in response.lines() {
        let line = line.trim();
        if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("• ") {
            let insight = line
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim_start_matches("• ")
                .to_string();
            if !insight.is_empty() {
                insights.push(insight);
            }
        }
    }
    insights
}

/// Attempt to extract JSON from an LLM response.
pub fn extract_json(response: &str) -> Option<serde_json::Value> {
    let start = response.find('{')?;
    let end = response.rfind('}')?;
    if end > start {
        serde_json::from_str(&response[start..=end]).ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// ProviderBridge: wraps LlmProvider into the LLMCaller interface
// ---------------------------------------------------------------------------

// Provider bridge note:
// The concrete ProviderBridge adapter is defined in `nemesisbot/src/commands/gateway.rs`
// because it needs access to `nemesis_providers::router::LLMProvider` which is only
// available at the binary crate level. The adapter wraps the provider and implements
// this `LLMCaller` trait.
//
// Mirrors Go's `forgeInstance.SetProvider(s.provider)`.

#[cfg(test)]
mod tests;
