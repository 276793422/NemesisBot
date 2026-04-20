package forge

import (
	"fmt"
	"strings"
)

// FormatReport generates a Markdown reflection report.
func FormatReport(report *ReflectionReport) string {
	var sb strings.Builder

	sb.WriteString(fmt.Sprintf("# Forge 反思报告 - %s\n\n", report.Date))

	// Statistical summary
	sb.WriteString("## 统计概要\n\n")
	sb.WriteString("| 指标 | 值 |\n")
	sb.WriteString("|------|-----|\n")
	if report.Stats != nil {
		sb.WriteString(fmt.Sprintf("| 分析经验数 | %d |\n", report.Stats.TotalRecords))
		sb.WriteString(fmt.Sprintf("| 去重模式数 | %d |\n", report.Stats.UniquePatterns))
		sb.WriteString(fmt.Sprintf("| 平均成功率 | %.1f%% |\n", report.Stats.AvgSuccessRate*100))
	}
	sb.WriteString("\n")

	if report.Stats == nil {
		return sb.String()
	}

	// Tool frequency
	if len(report.Stats.ToolFrequency) > 0 {
		sb.WriteString("## 工具使用频率\n\n")
		sb.WriteString("| 工具 | 使用次数 |\n")
		sb.WriteString("|------|----------|\n")
		for tool, count := range report.Stats.ToolFrequency {
			sb.WriteString(fmt.Sprintf("| %s | %d |\n", tool, count))
		}
		sb.WriteString("\n")
	}

	// High frequency patterns
	if len(report.Stats.TopPatterns) > 0 {
		sb.WriteString("## 高频模式\n\n")
		sb.WriteString("| 工具 | 次数 | 成功率 | 平均耗时 | 建议 |\n")
		sb.WriteString("|------|------|--------|----------|------|\n")
		for _, p := range report.Stats.TopPatterns {
			sb.WriteString(fmt.Sprintf("| %s | %d | %.0f%% | %dms | %s |\n",
				p.ToolName, p.Count, p.SuccessRate*100, p.AvgDurationMs, truncate(p.Suggestion, 40)))
		}
		sb.WriteString("\n")
	}

	// Low success patterns
	if len(report.Stats.LowSuccess) > 0 {
		sb.WriteString("## 低成功率模式\n\n")
		sb.WriteString("| 工具 | 次数 | 成功率 | 建议 |\n")
		sb.WriteString("|------|------|--------|------|\n")
		for _, p := range report.Stats.LowSuccess {
			sb.WriteString(fmt.Sprintf("| %s | %d | %.0f%% | %s |\n",
				p.ToolName, p.Count, p.SuccessRate*100, truncate(p.Suggestion, 50)))
		}
		sb.WriteString("\n")
	}

	// Existing artifacts
	if len(report.Artifacts) > 0 {
		sb.WriteString("## 现有自学习产物\n\n")
		sb.WriteString("| 类型 | 名称 | 版本 | 状态 | 使用次数 | 成功率 |\n")
		sb.WriteString("|------|------|------|------|----------|--------|\n")
		for _, a := range report.Artifacts {
			sb.WriteString(fmt.Sprintf("| %s | %s | %s | %s | %d | %.0f%% |\n",
				a.Type, a.Name, a.Version, a.Status, a.UsageCount, a.SuccessRate*100))
		}
		sb.WriteString("\n")
	}

	// LLM insights
	if report.LLMInsights != "" {
		sb.WriteString("## LLM 深度分析\n\n")
		sb.WriteString(report.LLMInsights)
		sb.WriteString("\n")
	}

	return sb.String()
}

// truncate shortens a string to maxLen, adding "..." if truncated.
func truncate(s string, maxLen int) string {
	s = strings.ReplaceAll(s, "|", "\\|")
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen-3] + "..."
}
