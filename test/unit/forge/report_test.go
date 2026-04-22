package forge_test

import (
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
)

func TestFormatReport_BasicStats(t *testing.T) {
	report := &forge.ReflectionReport{
		Date: "2026-04-21",
		Stats: &forge.ReflectionStats{
			TotalRecords:   100,
			UniquePatterns: 15,
			AvgSuccessRate: 0.85,
			ToolFrequency: map[string]int{
				"read_file": 50,
				"exec":      30,
			},
		},
	}

	result := forge.FormatReport(report)

	if !contains(result, "2026-04-21") {
		t.Error("Report should contain date")
	}
	if !contains(result, "100") {
		t.Error("Report should contain total records")
	}
	if !contains(result, "85.0%") {
		t.Error("Report should contain success rate")
	}
	if !contains(result, "read_file") {
		t.Error("Report should contain tool name in frequency")
	}
	if !contains(result, "统计概要") {
		t.Error("Report should contain statistical summary section")
	}
}

func TestFormatReport_WithArtifacts(t *testing.T) {
	report := &forge.ReflectionReport{
		Date: "2026-04-21",
		Stats: &forge.ReflectionStats{
			TotalRecords:   50,
			UniquePatterns: 5,
			AvgSuccessRate: 0.7,
		},
		Artifacts: []forge.Artifact{
			{
				ID:          "skill-test",
				Type:        forge.ArtifactSkill,
				Name:        "test-skill",
				Version:     "1.0",
				Status:      forge.StatusActive,
				UsageCount:  10,
				SuccessRate: 0.9,
			},
		},
	}

	result := forge.FormatReport(report)

	if !contains(result, "现有自学习产物") {
		t.Error("Report should contain artifacts section")
	}
	if !contains(result, "test-skill") {
		t.Error("Report should contain artifact name")
	}
	if !contains(result, "skill") {
		t.Error("Report should contain artifact type")
	}
}

func TestFormatReport_WithLLMInsights(t *testing.T) {
	report := &forge.ReflectionReport{
		Date: "2026-04-21",
		Stats: &forge.ReflectionStats{
			TotalRecords:   20,
			UniquePatterns: 3,
			AvgSuccessRate: 0.5,
		},
		LLMInsights: "Key pattern: read_file→edit_file is highly efficient. Consider creating a Skill.",
	}

	result := forge.FormatReport(report)

	if !contains(result, "LLM 深度分析") {
		t.Error("Report should contain LLM insights section")
	}
	if !contains(result, "read_file→edit_file") {
		t.Error("Report should contain LLM insight content")
	}
}

func TestFormatReport_WithTraceInsights(t *testing.T) {
	report := &forge.ReflectionReport{
		Date: "2026-04-21",
		Stats: &forge.ReflectionStats{
			TotalRecords:   10,
			UniquePatterns: 2,
			AvgSuccessRate: 0.6,
		},
		TraceStats: &forge.TraceStats{
			TotalTraces:    50,
			AvgRounds:      3.5,
			AvgDurationMs:  1200,
			EfficiencyScore: 0.72,
			ToolChainPatterns: []*forge.ToolChainPattern{
				{Chain: "read_file→edit_file", Count: 15, AvgRounds: 2.5, SuccessRate: 0.9},
			},
			RetryPatterns: []*forge.RetryPattern{
				{ToolName: "exec", RetryCount: 5, SuccessRate: 0.6},
			},
			SignalSummary: map[string]int{
				"retry":    10,
				"backtrack": 3,
			},
		},
	}

	result := forge.FormatReport(report)

	if !contains(result, "对话级洞察") {
		t.Error("Report should contain trace insights section")
	}
	if !contains(result, "50") {
		t.Error("Report should contain total traces count")
	}
	if !contains(result, "read_file→edit_file") {
		t.Error("Report should contain tool chain pattern")
	}
	if !contains(result, "retry") {
		t.Error("Report should contain signal summary")
	}
}

func TestFormatReport_WithLearningInsights(t *testing.T) {
	report := &forge.ReflectionReport{
		Date: "2026-04-21",
		Stats: &forge.ReflectionStats{
			TotalRecords:   30,
			UniquePatterns: 8,
			AvgSuccessRate: 0.75,
		},
		LearningCycle: &forge.LearningCycle{
			ID:              "cycle-001",
			PatternsFound:   3,
			ActionsCreated:  2,
			ActionsExecuted: 1,
			ActionsSkipped:  1,
			PatternSummary: []forge.PatternSummary{
				{Type: "tool_chain", Fingerprint: "abc123def456", Frequency: 10, Confidence: 0.85},
			},
			ActionSummary: []forge.ActionSummary{
				{Type: "create_skill", Priority: "high", Status: "executed", ArtifactID: "skill-gen-1"},
			},
		},
	}

	result := forge.FormatReport(report)

	if !contains(result, "闭环学习状态") {
		t.Error("Report should contain learning insights section")
	}
	if !contains(result, "tool_chain") {
		t.Error("Report should contain pattern type")
	}
	if !contains(result, "create_skill") {
		t.Error("Report should contain action type")
	}
	if !contains(result, "3 模式") {
		t.Error("Report should contain pattern count summary")
	}
}

func TestFormatReport_NilStats(t *testing.T) {
	report := &forge.ReflectionReport{
		Date:  "2026-04-21",
		Stats: nil,
	}

	result := forge.FormatReport(report)

	if !contains(result, "2026-04-21") {
		t.Error("Report should contain date even with nil stats")
	}
	if !contains(result, "统计概要") {
		t.Error("Report should still have stats section header")
	}
	// Should not panic
}

func TestFormatTraceInsights_WithData(t *testing.T) {
	ts := &forge.TraceStats{
		TotalTraces:    25,
		AvgRounds:      4.2,
		AvgDurationMs:  800,
		EfficiencyScore: 0.65,
		ToolChainPatterns: []*forge.ToolChainPattern{
			{Chain: "exec→read_file", Count: 8, AvgRounds: 3.0, SuccessRate: 0.75},
		},
		SignalSummary: map[string]int{
			"backtrack": 5,
		},
	}

	result := forge.FormatTraceInsightsForTest(ts)

	if !contains(result, "对话级洞察") {
		t.Error("Should contain trace insights header")
	}
	if !contains(result, "25") {
		t.Error("Should contain total traces count")
	}
	if !contains(result, "exec→read_file") {
		t.Error("Should contain tool chain")
	}
}

func TestFormatTraceInsights_Empty(t *testing.T) {
	ts := &forge.TraceStats{
		TotalTraces:    0,
		AvgRounds:      0,
		AvgDurationMs:  0,
		EfficiencyScore: 0,
	}

	result := forge.FormatTraceInsightsForTest(ts)

	// Should not panic and should have basic structure
	if !contains(result, "对话级洞察") {
		t.Error("Should contain trace insights header even when empty")
	}
	if strings.Contains(result, "高频工具链路") {
		t.Error("Should not have tool chain section when empty")
	}
}
