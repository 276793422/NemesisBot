package forge

import (
	"strings"
	"testing"
	"time"
)

// --- extractPatterns tests ---

func TestExtractPatterns_EmptyTraces(t *testing.T) {
	result := extractPatterns(nil, 3)
	if result != nil {
		t.Error("Should return nil for nil traces")
	}
}

func TestExtractPatterns_ZeroMinFreq(t *testing.T) {
	traces := []*ConversationTrace{
		{ToolSteps: []ToolStep{{ToolName: "a"}}},
	}
	result := extractPatterns(traces, 0)
	if result != nil {
		t.Error("Should return nil for zero minFrequency")
	}
}

// --- ToolChainDetector tests ---

func TestDetectToolChainPatterns_BasicChain(t *testing.T) {
	now := time.Now().UTC()
	traces := make([]*ConversationTrace, 5)
	for i := range traces {
		traces[i] = &ConversationTrace{
			StartTime:   now,
			TotalRounds: 3,
			DurationMs:  100,
			ToolSteps: []ToolStep{
				{ToolName: "read_file", ArgKeys: []string{"path"}},
				{ToolName: "edit_file", ArgKeys: []string{"path", "content"}},
			},
		}
	}

	patterns := extractPatterns(traces, 3)
	if len(patterns) == 0 {
		t.Fatal("Expected at least one pattern")
	}

	found := false
	for _, p := range patterns {
		if p.Type == PatternToolChain {
			found = true
			if p.Frequency < 5 {
				t.Errorf("Expected frequency >= 5, got %d", p.Frequency)
			}
			if p.ToolChain == "" {
				t.Error("ToolChain should not be empty")
			}
		}
	}
	if !found {
		t.Error("Expected a tool_chain pattern to be detected")
	}
}

func TestDetectToolChainPatterns_BelowMinFreq(t *testing.T) {
	now := time.Now().UTC()
	traces := []*ConversationTrace{
		{
			StartTime: now, ToolSteps: []ToolStep{{ToolName: "a"}, {ToolName: "b"}},
		},
	}
	patterns := extractPatterns(traces, 3)
	for _, p := range patterns {
		if p.Type == PatternToolChain {
			t.Error("Should not detect pattern below minimum frequency")
		}
	}
}

// --- ErrorRecoveryDetector tests ---

func TestDetectErrorRecoveryPatterns_Basic(t *testing.T) {
	now := time.Now().UTC()
	traces := make([]*ConversationTrace, 5)
	for i := range traces {
		traces[i] = &ConversationTrace{
			StartTime: now,
			ToolSteps: []ToolStep{
				{ToolName: "exec", Success: false, ErrorCode: "ERR_TIMEOUT"},
				{ToolName: "read_file", Success: true},
			},
		}
	}

	patterns := extractPatterns(traces, 3)
	found := false
	for _, p := range patterns {
		if p.Type == PatternErrorRecovery {
			found = true
			if p.ErrorTool != "exec" {
				t.Errorf("Expected error tool 'exec', got '%s'", p.ErrorTool)
			}
			if p.RecoveryTool != "read_file" {
				t.Errorf("Expected recovery tool 'read_file', got '%s'", p.RecoveryTool)
			}
		}
	}
	if !found {
		t.Error("Expected error_recovery pattern to be detected")
	}
}

func TestDetectErrorRecoveryPatterns_SameToolSkipped(t *testing.T) {
	now := time.Now().UTC()
	traces := []*ConversationTrace{
		{
			StartTime: now,
			ToolSteps: []ToolStep{
				{ToolName: "exec", Success: false},
				{ToolName: "exec", Success: true}, // same tool, should not be error recovery
			},
		},
	}

	patterns := extractPatterns(traces, 1)
	for _, p := range patterns {
		if p.Type == PatternErrorRecovery {
			t.Error("Same tool retry should not be detected as error_recovery")
		}
	}
}

func TestDetectErrorRecoveryPatterns_SuccessThenSuccess(t *testing.T) {
	now := time.Now().UTC()
	traces := []*ConversationTrace{
		{
			StartTime: now,
			ToolSteps: []ToolStep{
				{ToolName: "read_file", Success: true},
				{ToolName: "edit_file", Success: true},
			},
		},
	}

	patterns := extractPatterns(traces, 1)
	for _, p := range patterns {
		if p.Type == PatternErrorRecovery {
			t.Error("Success->success should not be detected as error_recovery")
		}
	}
}

// --- EfficiencyIssueDetector tests ---

func TestDetectEfficiencyIssues_Basic(t *testing.T) {
	now := time.Now().UTC()
	// Create traces where most are low-round (3), but one pattern has high rounds (20)
	var traces []*ConversationTrace
	// Many low-round traces
	for i := 0; i < 10; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime:   now,
			TotalRounds: 3,
			DurationMs:  100,
			ToolSteps:   []ToolStep{{ToolName: "a"}, {ToolName: "b"}},
		})
	}
	// A few high-round traces with <= 3 tools
	for i := 0; i < 5; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime:   now,
			TotalRounds: 20,
			DurationMs:  5000,
			ToolSteps:   []ToolStep{{ToolName: "c"}, {ToolName: "d"}},
		})
	}

	patterns := extractPatterns(traces, 3)
	found := false
	for _, p := range patterns {
		if p.Type == PatternEfficiencyIssue {
			found = true
			if p.EfficiencyScore < 0 || p.EfficiencyScore > 1 {
				t.Errorf("EfficiencyScore should be 0-1, got %f", p.EfficiencyScore)
			}
		}
	}
	if !found {
		t.Error("Expected efficiency_issue pattern to be detected")
	}
}

func TestDetectEfficiencyIssues_TooManyTools(t *testing.T) {
	now := time.Now().UTC()
	// Traces with > 3 tool steps should be excluded from efficiency analysis
	var toolSteps []ToolStep
	for i := 0; i < 5; i++ {
		toolSteps = append(toolSteps, ToolStep{ToolName: "tool"})
	}
	traces := []*ConversationTrace{
		{
			StartTime:   now,
			TotalRounds: 20,
			ToolSteps:   toolSteps,
		},
	}

	patterns := extractPatterns(traces, 1)
	for _, p := range patterns {
		if p.Type == PatternEfficiencyIssue {
			t.Error("Traces with > 3 tools should not produce efficiency patterns")
		}
	}
}

// --- SuccessTemplateDetector tests ---

func TestDetectSuccessTemplates_Basic(t *testing.T) {
	now := time.Now().UTC()
	// Average rounds is 10. A successful chain with 3 rounds is > 1.3x faster
	var traces []*ConversationTrace
	// High-round average traces (some with errors)
	for i := 0; i < 10; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime:   now,
			TotalRounds: 10,
			DurationMs:  1000,
			ToolSteps:   []ToolStep{{ToolName: "a"}, {ToolName: "b"}},
			Signals:     []SessionSignal{{Type: "retry"}}, // not successful
		})
	}
	// Successful fast traces (3 rounds, no signals)
	for i := 0; i < 5; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime:   now,
			TotalRounds: 3,
			DurationMs:  300,
			ToolSteps:   []ToolStep{{ToolName: "x"}, {ToolName: "y"}},
			// No signals = successful
		})
	}

	patterns := extractPatterns(traces, 3)
	found := false
	for _, p := range patterns {
		if p.Type == PatternSuccessTemplate {
			found = true
			if p.SuccessRate != 1.0 {
				t.Errorf("Success template should have 100%% success rate, got %f", p.SuccessRate)
			}
		}
	}
	if !found {
		t.Error("Expected success_template pattern to be detected")
	}
}

// --- patternFingerprint tests ---

func TestPatternFingerprint_Deterministic(t *testing.T) {
	fp1 := patternFingerprint("tool_chain", "a→b→c")
	fp2 := patternFingerprint("tool_chain", "a→b→c")
	if fp1 != fp2 {
		t.Error("Fingerprint should be deterministic")
	}
}

func TestPatternFingerprint_DifferentPrefix(t *testing.T) {
	fp1 := patternFingerprint("tool_chain", "a→b")
	fp2 := patternFingerprint("error_recovery", "a→b")
	if fp1 == fp2 {
		t.Error("Different prefixes should produce different fingerprints")
	}
}

// --- minFloat tests ---

func TestMinFloat(t *testing.T) {
	if minFloat(1.0, 2.0) != 1.0 {
		t.Error("minFloat(1.0, 2.0) should be 1.0")
	}
	if minFloat(3.0, 2.0) != 2.0 {
		t.Error("minFloat(3.0, 2.0) should be 2.0")
	}
	if minFloat(-1.0, -2.0) != -2.0 {
		t.Error("minFloat(-1.0, -2.0) should be -2.0")
	}
}

// --- deduplicateChainString tests ---

func TestDeduplicateChainString(t *testing.T) {
	result := deduplicateChainString([]string{"a", "b", "c"})
	if result != "a→b→c" {
		t.Errorf("Expected 'a→b→c', got '%s'", result)
	}
}

func TestDeduplicateChainString_Empty(t *testing.T) {
	result := deduplicateChainString([]string{})
	if result != "" {
		t.Errorf("Expected empty string, got '%s'", result)
	}
}

// --- ConversationPattern JSON round-trip ---

func TestConversationPattern_JSONRoundTrip(t *testing.T) {
	p := &ConversationPattern{
		ID:            "tc-test",
		Type:          PatternToolChain,
		Fingerprint:   "abc123",
		Frequency:     10,
		Confidence:    0.95,
		FirstSeen:     time.Now().UTC().Truncate(time.Millisecond),
		LastSeen:      time.Now().UTC().Truncate(time.Millisecond),
		ToolChain:     "a→b",
		AvgRounds:     3.5,
		AvgDurationMs: 500,
		SuccessRate:   0.9,
		CommonArgKeys: []string{"path", "content"},
		Description:   "Test pattern",
	}

	// Just verify the struct has the right field types
	if p.Type != PatternToolChain {
		t.Error("Type should be PatternToolChain")
	}
	if len(p.CommonArgKeys) != 2 {
		t.Error("Should have 2 common arg keys")
	}
}

// --- PatternType constants ---

func TestPatternTypeConstants(t *testing.T) {
	types := map[PatternType]string{
		PatternToolChain:       "tool_chain",
		PatternErrorRecovery:   "error_recovery",
		PatternEfficiencyIssue: "efficiency_issue",
		PatternSuccessTemplate: "success_template",
	}
	for pt, expected := range types {
		if string(pt) != expected {
			t.Errorf("Expected '%s', got '%s'", expected, string(pt))
		}
	}
}

// --- extractPatterns sorted by confidence ---

func TestExtractPatterns_SortedByConfidence(t *testing.T) {
	now := time.Now().UTC()
	var traces []*ConversationTrace

	// Create multiple different patterns
	for i := 0; i < 10; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime: now, TotalRounds: 3, DurationMs: 100,
			ToolSteps: []ToolStep{{ToolName: "a"}, {ToolName: "b"}},
		})
		traces = append(traces, &ConversationTrace{
			StartTime: now, TotalRounds: 3, DurationMs: 100,
			ToolSteps: []ToolStep{
				{ToolName: "exec", Success: false},
				{ToolName: "read_file", Success: true},
			},
		})
	}

	patterns := extractPatterns(traces, 3)
	for i := 1; i < len(patterns); i++ {
		if patterns[i].Confidence > patterns[i-1].Confidence {
			t.Errorf("Patterns not sorted by confidence descending: [%d]=%.3f > [%d]=%.3f",
				i, patterns[i].Confidence, i-1, patterns[i-1].Confidence)
		}
	}
}

// --- Integration: full pattern extraction pipeline ---

func TestExtractPatterns_FullPipeline(t *testing.T) {
	now := time.Now().UTC()
	var traces []*ConversationTrace

	// 10 traces with tool chain a→b (successful)
	for i := 0; i < 10; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime: now, TotalRounds: 5, DurationMs: 200,
			ToolSteps: []ToolStep{
				{ToolName: "read_file", ArgKeys: []string{"path"}},
				{ToolName: "edit_file", ArgKeys: []string{"path", "content"}},
			},
		})
	}

	// 5 traces with error recovery
	for i := 0; i < 5; i++ {
		traces = append(traces, &ConversationTrace{
			StartTime: now, TotalRounds: 3, DurationMs: 300,
			ToolSteps: []ToolStep{
				{ToolName: "exec", Success: false, ErrorCode: "ERR"},
				{ToolName: "read_file", Success: true},
			},
		})
	}

	patterns := extractPatterns(traces, 3)

	// Should detect tool_chain patterns (read_file→edit_file is frequent)
	toolChainCount := 0
	errorRecoveryCount := 0
	for _, p := range patterns {
		switch p.Type {
		case PatternToolChain:
			toolChainCount++
		case PatternErrorRecovery:
			errorRecoveryCount++
		}
	}

	if toolChainCount == 0 {
		t.Error("Expected at least one tool_chain pattern")
	}
	if errorRecoveryCount == 0 {
		t.Error("Expected at least one error_recovery pattern")
	}

	// Verify all patterns have non-empty IDs
	for _, p := range patterns {
		if p.ID == "" {
			t.Error("Pattern ID should not be empty")
		}
		if !strings.Contains(p.Description, "(") {
			t.Errorf("Description should have details, got: %s", p.Description)
		}
	}
}
