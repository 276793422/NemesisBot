package forge

import (
	"strings"
	"testing"
	"time"
)

// --- FormatReport tests ---

func TestFormatReport_Basic(t *testing.T) {
	report := &ReflectionReport{
		Date: "2026-04-25",
		Stats: &ReflectionStats{
			TotalRecords:   100,
			UniquePatterns: 15,
			AvgSuccessRate: 0.85,
		},
	}

	result := FormatReport(report)

	if !strings.Contains(result, "# Forge 反思报告 - 2026-04-25") {
		t.Error("Should contain report header")
	}
	if !strings.Contains(result, "100") {
		t.Error("Should contain total records count")
	}
	if !strings.Contains(result, "15") {
		t.Error("Should contain unique patterns count")
	}
	if !strings.Contains(result, "85.0%") {
		t.Error("Should contain average success rate")
	}
}

func TestFormatReport_WithToolFrequency(t *testing.T) {
	report := &ReflectionReport{
		Date: "2026-04-25",
		Stats: &ReflectionStats{
			ToolFrequency: map[string]int{
				"read_file": 50,
				"edit_file": 30,
			},
		},
	}

	result := FormatReport(report)

	if !strings.Contains(result, "工具使用频率") {
		t.Error("Should contain tool frequency section")
	}
	if !strings.Contains(result, "read_file") {
		t.Error("Should contain tool name")
	}
}

func TestFormatReport_WithTopPatterns(t *testing.T) {
	report := &ReflectionReport{
		Date: "2026-04-25",
		Stats: &ReflectionStats{
			TopPatterns: []*PatternInsight{
				{ToolName: "read_file", Count: 50, SuccessRate: 0.9, AvgDurationMs: 100, Suggestion: "Create a Skill"},
			},
		},
	}

	result := FormatReport(report)

	if !strings.Contains(result, "高频模式") {
		t.Error("Should contain top patterns section")
	}
	if !strings.Contains(result, "Create a Skill") {
		t.Error("Should contain suggestion")
	}
}

func TestFormatReport_WithLowSuccess(t *testing.T) {
	report := &ReflectionReport{
		Date: "2026-04-25",
		Stats: &ReflectionStats{
			LowSuccess: []*PatternInsight{
				{ToolName: "exec", Count: 10, SuccessRate: 0.4, Suggestion: "Improve error handling"},
			},
		},
	}

	result := FormatReport(report)

	if !strings.Contains(result, "低成功率模式") {
		t.Error("Should contain low success section")
	}
}

func TestFormatReport_WithArtifacts(t *testing.T) {
	report := &ReflectionReport{
		Date:  "2026-04-25",
		Stats: &ReflectionStats{TotalRecords: 1},
		Artifacts: []Artifact{
			{Type: "skill", Name: "test-skill", Version: "1.0", Status: "active", UsageCount: 10, SuccessRate: 0.9},
		},
	}

	result := FormatReport(report)

	if !strings.Contains(result, "现有自学习产物") {
		t.Error("Should contain artifacts section")
	}
	if !strings.Contains(result, "test-skill") {
		t.Error("Should contain artifact name")
	}
}

func TestFormatReport_WithLLMInsights(t *testing.T) {
	report := &ReflectionReport{
		Date:        "2026-04-25",
		Stats:       &ReflectionStats{TotalRecords: 1},
		LLMInsights: "LLM analysis result",
	}

	result := FormatReport(report)

	if !strings.Contains(result, "LLM 深度分析") {
		t.Error("Should contain LLM insights section")
	}
	if !strings.Contains(result, "LLM analysis result") {
		t.Error("Should contain LLM insights content")
	}
}

func TestFormatReport_NilStats(t *testing.T) {
	report := &ReflectionReport{
		Date:  "2026-04-25",
		Stats: nil,
	}

	result := FormatReport(report)

	if !strings.Contains(result, "统计概要") {
		t.Error("Should still contain stats section header")
	}
	if strings.Contains(result, "工具使用频率") {
		t.Error("Should not contain tool frequency when stats is nil")
	}
}

// --- formatTraceInsights tests ---

func TestFormatTraceInsights(t *testing.T) {
	ts := &TraceStats{
		TotalTraces:     100,
		AvgRounds:       3.5,
		AvgDurationMs:   500,
		EfficiencyScore: 0.85,
		ToolChainPatterns: []*ToolChainPattern{
			{Chain: "read_file→edit_file", Count: 50, AvgRounds: 3.0, SuccessRate: 0.9},
		},
		RetryPatterns: []*RetryPattern{
			{ToolName: "exec", RetryCount: 10, SuccessRate: 0.6},
		},
		SignalSummary: map[string]int{
			"retry":    5,
			"backtrack": 3,
		},
	}

	result := formatTraceInsights(ts)

	if !strings.Contains(result, "对话级洞察") {
		t.Error("Should contain trace insights header")
	}
	if !strings.Contains(result, "100") {
		t.Error("Should contain total traces")
	}
	if !strings.Contains(result, "高频工具链路") {
		t.Error("Should contain tool chain section")
	}
	if !strings.Contains(result, "重试模式") {
		t.Error("Should contain retry patterns section")
	}
	if !strings.Contains(result, "retry") {
		t.Error("Should contain signal types")
	}
}

func TestFormatTraceInsights_Empty(t *testing.T) {
	ts := &TraceStats{
		TotalTraces: 0,
	}

	result := formatTraceInsights(ts)

	if !strings.Contains(result, "对话级洞察") {
		t.Error("Should contain header even when empty")
	}
}

// --- formatLearningInsights tests ---

func TestFormatLearningInsights(t *testing.T) {
	cycle := &LearningCycle{
		PatternsFound:   5,
		ActionsCreated:  3,
		ActionsExecuted: 2,
		ActionsSkipped:  1,
		PatternSummary: []PatternSummary{
			{ID: "p1", Type: "tool_chain", Fingerprint: "abcdef1234567890", Frequency: 10, Confidence: 0.9},
		},
		ActionSummary: []ActionSummary{
			{ID: "a1", Type: "create_skill", Priority: "high", Status: "executed", ArtifactID: "skill-test"},
			{ID: "a2", Type: "suggest_prompt", Priority: "medium", Status: "skipped"},
		},
		PreviousOutcomes: []*ActionOutcome{
			{ArtifactID: "skill-test", Verdict: "positive", ImprovementScore: 0.3, SampleSize: 10},
		},
	}

	result := formatLearningInsights(cycle)

	if !strings.Contains(result, "闭环学习状态") {
		t.Error("Should contain learning insights header")
	}
	if !strings.Contains(result, "检测到的模式") {
		t.Error("Should contain patterns section")
	}
	if !strings.Contains(result, "学习行动") {
		t.Error("Should contain actions section")
	}
	if !strings.Contains(result, "上一轮反馈") {
		t.Error("Should contain previous outcomes section")
	}
	if !strings.Contains(result, "5 模式, 3 行动创建, 2 已执行, 1 已跳过") {
		t.Error("Should contain summary stats")
	}
}

func TestFormatLearningInsights_Empty(t *testing.T) {
	cycle := &LearningCycle{}

	result := formatLearningInsights(cycle)

	if !strings.Contains(result, "闭环学习状态") {
		t.Error("Should contain header")
	}
	if !strings.Contains(result, "0 模式, 0 行动创建, 0 已执行, 0 已跳过") {
		t.Error("Should show zero stats")
	}
}

// --- truncate tests ---

func TestTruncate_Short(t *testing.T) {
	result := truncate("hello", 10)
	if result != "hello" {
		t.Errorf("Expected 'hello', got '%s'", result)
	}
}

func TestTruncate_Long(t *testing.T) {
	input := "this is a very long string that exceeds the limit"
	result := truncate(input, 20)
	if len(result) > 20 {
		t.Errorf("Result should be <= 20 chars, got %d: %s", len(result), result)
	}
	if !strings.Contains(result, "...") {
		t.Error("Truncated result should contain '...'")
	}
}

func TestTruncate_PipeEscape(t *testing.T) {
	input := "text with | pipe"
	result := truncate(input, 100)
	if strings.Contains(result, "|") && !strings.Contains(result, "\\|") {
		t.Error("Pipe should be escaped")
	}
}

// --- FormatReport with TraceStats ---

func TestFormatReport_WithTraceStats(t *testing.T) {
	report := &ReflectionReport{
		Date:  "2026-04-25",
		Stats: &ReflectionStats{TotalRecords: 1},
		TraceStats: &TraceStats{
			TotalTraces:     50,
			AvgRounds:       4.0,
			AvgDurationMs:   300,
			EfficiencyScore: 0.75,
		},
	}

	result := FormatReport(report)

	if !strings.Contains(result, "对话级洞察") {
		t.Error("Should contain trace insights when TraceStats is set")
	}
}

// --- FormatReport with LearningCycle ---

func TestFormatReport_WithLearningCycle(t *testing.T) {
	report := &ReflectionReport{
		Date:  "2026-04-25",
		Stats: &ReflectionStats{TotalRecords: 1},
		LearningCycle: &LearningCycle{
			PatternsFound:  3,
			ActionsCreated: 2,
		},
	}

	result := FormatReport(report)

	if !strings.Contains(result, "闭环学习状态") {
		t.Error("Should contain learning insights when LearningCycle is set")
	}
}

// --- ReflectionStats integration ---

func TestReflectionStats_Fields(t *testing.T) {
	stats := &ReflectionStats{
		TotalRecords:   100,
		UniquePatterns: 20,
		AvgSuccessRate: 0.85,
		ToolFrequency: map[string]int{
			"read_file": 50,
			"edit_file": 30,
			"exec":      20,
		},
		TopPatterns: []*PatternInsight{
			{ToolName: "read_file", Count: 50, SuccessRate: 0.9, AvgDurationMs: 100, Suggestion: "Create Skill"},
		},
		LowSuccess: []*PatternInsight{
			{ToolName: "exec", Count: 20, SuccessRate: 0.5, Suggestion: "Improve error handling"},
		},
	}

	if stats.TotalRecords != 100 {
		t.Errorf("Expected TotalRecords 100, got %d", stats.TotalRecords)
	}
	if len(stats.ToolFrequency) != 3 {
		t.Errorf("Expected 3 tool frequencies, got %d", len(stats.ToolFrequency))
	}
	if len(stats.TopPatterns) != 1 {
		t.Errorf("Expected 1 top pattern, got %d", len(stats.TopPatterns))
	}
	if len(stats.LowSuccess) != 1 {
		t.Errorf("Expected 1 low success pattern, got %d", len(stats.LowSuccess))
	}
}

// Make sure time import is used
var _ = time.Time{}
