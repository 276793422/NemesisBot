package forge_test

import (
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
)

func TestExtractPatternsEmpty(t *testing.T) {
	patterns := forge.ExtractPatternsForTest(nil, 5)
	if len(patterns) != 0 {
		t.Errorf("Expected 0 patterns from nil traces, got %d", len(patterns))
	}

	patterns = forge.ExtractPatternsForTest([]*forge.ConversationTrace{}, 5)
	if len(patterns) != 0 {
		t.Errorf("Expected 0 patterns from empty traces, got %d", len(patterns))
	}
}

func TestExtractPatternsZeroMinFreq(t *testing.T) {
	traces := makeToolChainTraces("read→edit→exec", 10)
	patterns := forge.ExtractPatternsForTest(traces, 0)
	if len(patterns) != 0 {
		t.Errorf("Expected 0 patterns with minFreq=0, got %d", len(patterns))
	}
}

func TestToolChainDetection(t *testing.T) {
	traces := makeToolChainTraces("read→edit→exec", 10)
	patterns := forge.ExtractPatternsForTest(traces, 5)

	found := false
	for _, p := range patterns {
		if p.Type == forge.PatternToolChain && p.ToolChain == "read→edit→exec" {
			found = true
			if p.Frequency != 10 {
				t.Errorf("Expected frequency 10, got %d", p.Frequency)
			}
			if p.Confidence <= 0 || p.Confidence > 1.0 {
				t.Errorf("Confidence should be in (0,1], got %f", p.Confidence)
			}
		}
	}
	if !found {
		t.Error("Expected tool_chain pattern for 'read→edit→exec'")
	}
}

func TestToolChainBelowMinFreq(t *testing.T) {
	traces := makeToolChainTraces("read→edit→exec", 3)
	patterns := forge.ExtractPatternsForTest(traces, 5)

	for _, p := range patterns {
		if p.Type == forge.PatternToolChain && p.ToolChain == "read→edit→exec" {
			t.Error("Should not detect tool_chain below min frequency")
		}
	}
}

func TestToolChainOrderSensitive(t *testing.T) {
	traces1 := makeToolChainTraces("read→edit→exec", 6)
	traces2 := makeToolChainTraces("exec→edit→read", 6)
	all := append(traces1, traces2...)

	patterns := forge.ExtractPatternsForTest(all, 5)

	chainCount := 0
	for _, p := range patterns {
		if p.Type == forge.PatternToolChain {
			chainCount++
		}
	}
	if chainCount != 2 {
		t.Errorf("Expected 2 different tool chain patterns, got %d", chainCount)
	}
}

func TestToolChainConfidenceFormula(t *testing.T) {
	traces := makeToolChainTracesWithSuccess("read→edit", 10, true)
	patterns := forge.ExtractPatternsForTest(traces, 5)

	for _, p := range patterns {
		if p.Type == forge.PatternToolChain && p.ToolChain == "read→edit" {
			if p.Confidence != 1.0 {
				t.Errorf("Expected confidence 1.0, got %f", p.Confidence)
			}
		}
	}
}

func TestErrorRecoveryDetection(t *testing.T) {
	traces := makeErrorRecoveryTraces("file_read", "file_edit", 6)
	patterns := forge.ExtractPatternsForTest(traces, 5)

	found := false
	for _, p := range patterns {
		if p.Type == forge.PatternErrorRecovery && p.ErrorTool == "file_read" && p.RecoveryTool == "file_edit" {
			found = true
			if p.Frequency != 6 {
				t.Errorf("Expected frequency 6, got %d", p.Frequency)
			}
		}
	}
	if !found {
		t.Error("Expected error_recovery pattern")
	}
}

func TestErrorRecoveryBelowMinFreq(t *testing.T) {
	traces := makeErrorRecoveryTraces("file_read", "file_edit", 3)
	patterns := forge.ExtractPatternsForTest(traces, 5)

	for _, p := range patterns {
		if p.Type == forge.PatternErrorRecovery {
			t.Error("Should not detect error_recovery below min frequency")
		}
	}
}

func TestSuccessTemplateDetection(t *testing.T) {
	var traces []*forge.ConversationTrace
	now := time.Now().UTC()

	for i := 0; i < 10; i++ {
		traces = append(traces, &forge.ConversationTrace{
			TraceID:     "slow-" + string(rune(i)),
			StartTime:   now.Add(-time.Duration(i) * time.Hour),
			DurationMs:  10000,
			TotalRounds: 10,
			ToolSteps:   []forge.ToolStep{
				{ToolName: "slow-tool", Success: true},
			},
		})
	}

	for i := 0; i < 6; i++ {
		traces = append(traces, &forge.ConversationTrace{
			TraceID:     "fast-" + string(rune(i)),
			StartTime:   now.Add(-time.Duration(i) * time.Minute),
			DurationMs:  1000,
			TotalRounds: 2,
			ToolSteps: []forge.ToolStep{
				{ToolName: "fast-tool", Success: true},
			},
		})
	}

	patterns := forge.ExtractPatternsForTest(traces, 5)
	found := false
	for _, p := range patterns {
		if p.Type == forge.PatternSuccessTemplate {
			found = true
		}
	}
	if !found {
		t.Error("Expected success_template pattern")
	}
}

func TestEfficiencyIssueDetection(t *testing.T) {
	var traces []*forge.ConversationTrace
	now := time.Now().UTC()

	for i := 0; i < 10; i++ {
		traces = append(traces, &forge.ConversationTrace{
			TraceID:     "normal-" + string(rune(i)),
			StartTime:   now.Add(-time.Duration(i) * time.Hour),
			DurationMs:  1000,
			TotalRounds: 3,
			ToolSteps: []forge.ToolStep{
				{ToolName: "tool-a", Success: true},
			},
		})
	}

	// Inefficient traces (rounds much higher than 2x global avg)
	// Global avg = (10*3 + 6*20)/16 = (30+120)/16 = 9.375
	// 2x avg = 18.75, so rounds=20 triggers it
	for i := 0; i < 6; i++ {
		traces = append(traces, &forge.ConversationTrace{
			TraceID:     "ineff-" + string(rune(i)),
			StartTime:   now.Add(-time.Duration(i) * time.Minute),
			DurationMs:  5000,
			TotalRounds: 20, // significantly higher than 2 * globalAvg
			ToolSteps: []forge.ToolStep{
				{ToolName: "tool-b", Success: true},
			},
		})
	}

	patterns := forge.ExtractPatternsForTest(traces, 5)
	found := false
	for _, p := range patterns {
		if p.Type == forge.PatternEfficiencyIssue {
			found = true
		}
	}
	if !found {
		// Debug: print global avg
		var totalRounds float64
		for _, t := range traces {
			totalRounds += float64(t.TotalRounds)
		}
		avg := totalRounds / float64(len(traces))
		t.Errorf("Expected efficiency_issue pattern. Global avg=%.1f, 2x=%.1f. Patterns found: %d", avg, 2*avg, len(patterns))
	}
}

func TestPatternFingerprintDedup(t *testing.T) {
	fp1 := forge.PatternFingerprintForTest("tool_chain", "read→edit")
	fp2 := forge.PatternFingerprintForTest("tool_chain", "read→edit")
	if fp1 != fp2 {
		t.Error("Same data should produce same fingerprint")
	}

	fp3 := forge.PatternFingerprintForTest("tool_chain", "edit→read")
	if fp1 == fp3 {
		t.Error("Different data should produce different fingerprints")
	}
}

func TestPatternFingerprintTypePrefix(t *testing.T) {
	fp1 := forge.PatternFingerprintForTest("tool_chain", "read")
	fp2 := forge.PatternFingerprintForTest("error_recovery", "read")
	if fp1 == fp2 {
		t.Error("Different type prefixes should produce different fingerprints")
	}
}

// --- Test Helpers ---

func makeToolChainTraces(chain string, count int) []*forge.ConversationTrace {
	return makeToolChainTracesWithSuccess(chain, count, true)
}

func makeToolChainTracesWithSuccess(chain string, count int, success bool) []*forge.ConversationTrace {
	now := time.Now().UTC()
	var traces []*forge.ConversationTrace

	tools := splitChain(chain)
	for i := 0; i < count; i++ {
		steps := make([]forge.ToolStep, len(tools))
		for j, tool := range tools {
			steps[j] = forge.ToolStep{
				ToolName: tool,
				Success:  success,
				ArgKeys:  []string{"path"},
			}
		}

		var signals []forge.SessionSignal
		if !success {
			signals = []forge.SessionSignal{{Type: "retry"}}
		}

		traces = append(traces, &forge.ConversationTrace{
			TraceID:     "trace-" + chain + "-" + string(rune(i)),
			StartTime:   now.Add(-time.Duration(i) * time.Hour),
			DurationMs:  1000,
			TotalRounds: len(tools),
			ToolSteps:   steps,
			Signals:     signals,
		})
	}
	return traces
}

func makeErrorRecoveryTraces(errorTool, recoveryTool string, count int) []*forge.ConversationTrace {
	now := time.Now().UTC()
	var traces []*forge.ConversationTrace

	for i := 0; i < count; i++ {
		traces = append(traces, &forge.ConversationTrace{
			TraceID:     "err-" + string(rune(i)),
			StartTime:   now.Add(-time.Duration(i) * time.Hour),
			DurationMs:  2000,
			TotalRounds: 3,
			ToolSteps: []forge.ToolStep{
				{ToolName: errorTool, Success: false, ErrorCode: "E001"},
				{ToolName: recoveryTool, Success: true},
			},
			Signals: []forge.SessionSignal{{Type: "retry"}},
		})
	}
	return traces
}

func splitChain(chain string) []string {
	result := []string{}
	current := ""
	for _, ch := range chain {
		if ch == '→' {
			if current != "" {
				result = append(result, current)
				current = ""
			}
		} else {
			current += string(ch)
		}
	}
	if current != "" {
		result = append(result, current)
	}
	return result
}
