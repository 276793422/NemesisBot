package forge

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/observer"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

func TestForgeTools_Parameters(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		params := tool.Parameters()
		if params == nil {
			t.Errorf("Tool %s: Parameters() should not return nil", tool.Name())
		}
		// All tools should have a "type" key
		if _, ok := params["type"]; !ok {
			t.Errorf("Tool %s: Parameters() should contain 'type' key", tool.Name())
		}
	}
}

// --- forge_update Execute tests ---

func TestForgeUpdateTool_Execute_MissingID(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_update" {
			result := tool.Execute(context.Background(), map[string]interface{}{})
			if !result.IsError {
				t.Error("Should error without id")
			}
			return
		}
	}
}

func TestForgeUpdateTool_Execute_MissingContent(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Create an artifact with a file
	skillDir := filepath.Join(workspace, "forge", "skills", "up-test")
	os.MkdirAll(skillDir, 0755)
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte("---\nname: up-test\n---\nContent"), 0644)

	f.GetRegistry().Add(Artifact{
		ID:   "skill-up-test",
		Type: ArtifactSkill,
		Name: "up-test",
		Path: skillPath,
	})

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_update" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id": "skill-up-test",
			})
			if !result.IsError {
				t.Error("Should error without content or rollback_version")
			}
			return
		}
	}
}

func TestForgeUpdateTool_Execute_Success(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	skillDir := filepath.Join(workspace, "forge", "skills", "upd-skill")
	os.MkdirAll(skillDir, 0755)
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte("---\nname: upd-skill\n---\nOriginal"), 0644)

	f.GetRegistry().Add(Artifact{
		ID:      "skill-upd-skill",
		Type:    ArtifactSkill,
		Name:    "upd-skill",
		Version: "1.0",
		Path:    skillPath,
	})

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_update" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":                 "skill-upd-skill",
				"content":            "---\nname: upd-skill\n---\nUpdated content",
				"change_description": "test update",
			})
			if result.IsError {
				t.Errorf("forge_update should succeed: %s", result.ForLLM)
			}
			return
		}
	}
}

// --- forge_create script type test ---

func TestForgeCreateTool_Execute_Script(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_create" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"type":        "script",
				"name":        "test-script",
				"content":     "print('hello')",
				"description": "A test script",
				"test_cases":  []interface{}{map[string]interface{}{"input": "test"}},
			})
			if result.IsError {
				t.Errorf("forge_create script should succeed: %s", result.ForLLM)
			}

			_, found := f.GetRegistry().Get("script-test-script")
			if !found {
				t.Error("Script artifact should be registered")
			}
			return
		}
	}
}

func TestForgeCreateTool_Execute_MCP(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_create" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"type":        "mcp",
				"name":        "test-mcp",
				"content":     "from mcp import Server",
				"description": "A test MCP",
				"language":    "python",
				"test_cases":  []interface{}{map[string]interface{}{"test": "case"}},
			})
			if result.IsError {
				t.Errorf("forge_create mcp should succeed: %s", result.ForLLM)
			}
			return
		}
	}
}

func TestForgeCreateTool_Execute_MCP_Go(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_create" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"type":        "mcp",
				"name":        "test-mcp-go",
				"content":     "package main",
				"description": "A Go MCP",
				"language":    "go",
				"test_cases":  []interface{}{map[string]interface{}{"test": "go"}},
			})
			if result.IsError {
				t.Errorf("forge_create mcp go should succeed: %s", result.ForLLM)
			}

			// Check go.mod was created
			mcpDir := filepath.Join(workspace, "forge", "mcp", "test-mcp-go")
			if _, err := os.Stat(filepath.Join(mcpDir, "go.mod")); os.IsNotExist(err) {
				t.Error("go.mod should be created for Go MCP")
			}
			return
		}
	}
}

func TestForgeCreateTool_Execute_ScriptNoTests(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_create" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"type":    "script",
				"name":    "no-tests",
				"content": "code",
			})
			if !result.IsError {
				t.Error("Should error when script has no test_cases")
			}
			return
		}
	}
}

func TestForgeCreateTool_Execute_InvalidType(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_create" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"type":    "invalid",
				"name":    "test",
				"content": "content",
			})
			if !result.IsError {
				t.Error("Should error for invalid type")
			}
			return
		}
	}
}

// --- forge_build_mcp tests ---

func TestForgeBuildMCPTool_Execute_NotFound(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":     "nonexistent",
				"action": "build",
			})
			if !result.IsError {
				t.Error("Should error for nonexistent artifact")
			}
			return
		}
	}
}

func TestForgeBuildMCPTool_Execute_NotMCPType(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	f.GetRegistry().Add(Artifact{
		ID:   "skill-1",
		Type: ArtifactSkill,
		Name: "not-mcp",
	})

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":     "skill-1",
				"action": "build",
			})
			if !result.IsError {
				t.Error("Should error for non-MCP artifact")
			}
			return
		}
	}
}

func TestForgeBuildMCPTool_Execute_MissingID(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			result := tool.Execute(context.Background(), map[string]interface{}{})
			if !result.IsError {
				t.Error("Should error without id")
			}
			return
		}
	}
}

func TestForgeBuildMCPTool_Execute_UnknownAction(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Create MCP artifact
	mcpDir := filepath.Join(workspace, "forge", "mcp", "test-mcp")
	os.MkdirAll(mcpDir, 0755)
	mcpPath := filepath.Join(mcpDir, "server.py")
	os.WriteFile(mcpPath, []byte("# mcp"), 0644)

	f.GetRegistry().Add(Artifact{
		ID:   "mcp-test-mcp",
		Type: ArtifactMCP,
		Name: "test-mcp",
		Path: mcpPath,
	})

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":     "mcp-test-mcp",
				"action": "unknown_action",
			})
			if !result.IsError {
				t.Error("Should error for unknown action")
			}
			return
		}
	}
}

// --- forge_evaluate Execute with real artifact ---

func TestForgeEvaluateTool_Execute_Success(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	skillDir := filepath.Join(workspace, "forge", "skills", "eval-test")
	os.MkdirAll(skillDir, 0755)
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte("---\nname: eval-test\ndescription: test\n---\nSome skill content that is valid"), 0644)

	f.GetRegistry().Add(Artifact{
		ID:      "skill-eval-test",
		Type:    ArtifactSkill,
		Name:    "eval-test",
		Version: "1.0",
		Status:  StatusDraft,
		Path:    skillPath,
	})

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_evaluate" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id": "skill-eval-test",
			})
			if result.IsError {
				t.Errorf("forge_evaluate should succeed: %s", result.ForLLM)
			}
			return
		}
	}
}

func TestForgeEvaluateTool_Execute_MissingID(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)
	tools := NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_evaluate" {
			result := tool.Execute(context.Background(), map[string]interface{}{})
			if !result.IsError {
				t.Error("Should error without id")
			}
			return
		}
	}
}

// --- forge_share Execute with report_path validation ---

func TestForgeShareTool_Execute_InvalidReportPath(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := NewForge(workspace, nil)

	// Manually enable syncer with a bridge - just set bridge directly
	mockBridge := &mockEnabledBridge{}
	f.GetSyncer().SetBridge(mockBridge)

	tools := NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_share" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"report_path": "/etc/passwd",
			})
			if !result.IsError {
				t.Error("Should error for report_path outside reflections dir")
			}
			return
		}
	}
}

// mockEnabledBridge is a simple mock that returns enabled
type mockEnabledBridge struct{}

func (m *mockEnabledBridge) ShareToPeer(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	return nil, nil
}
func (m *mockEnabledBridge) GetOnlinePeers() []PeerInfo { return nil }
func (m *mockEnabledBridge) IsClusterEnabled() bool     { return true }

// --- ParseLLMInsights test ---

func TestParseLLMInsights(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected int
	}{
		{"dash bullets", "- insight 1\n- insight 2\n- insight 3", 3},
		{"asterisk bullets", "* insight 1\n* insight 2", 2},
		{"unicode bullets", "bullet 1\nbullet 2", 0},
		{"mixed", "- dash\n* star\nbullet\nnormal text", 2},
		{"empty", "", 0},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ParseLLMInsights(tt.input)
			if len(result) != tt.expected {
				t.Errorf("ParseLLMInsights: expected %d insights, got %d", tt.expected, len(result))
			}
		})
	}
}

// --- ExtractJSONFromResponse test ---

func TestExtractJSONFromResponse(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		wantErr bool
	}{
		{"valid json", `Here is the result: {"key": "value", "num": 42}`, false},
		{"no json", "Just plain text without any JSON", true},
		{"embedded json", `Some text {"correctness": 85, "quality": 70} more text`, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := ExtractJSONFromResponse(tt.input)
			if tt.wantErr {
				if err == nil {
					t.Error("Expected error, got nil")
				}
			} else {
				if err != nil {
					t.Errorf("Unexpected error: %v", err)
				}
				if result == nil {
					t.Error("Result should not be nil")
				}
			}
		})
	}
}

// --- Reflector AnalyzeTracesForTest test ---

func TestReflector_AnalyzeTracesForTest_NilStore(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	result := r.AnalyzeTracesForTest(time.Now().UTC())
	if result != nil {
		t.Error("analyzeTraces should return nil when traceStore is nil")
	}
}

func TestReflector_AnalyzeTracesForTest_WithData(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewExperienceStore(tmpDir, cfg)
	registry := NewRegistry(tmpDir + "/registry.json")
	r := NewReflector(tmpDir, store, registry, cfg)

	traceStore := NewTraceStore(tmpDir, cfg)
	r.SetTraceStore(traceStore)

	// Add some traces
	now := time.Now().UTC()
	for i := 0; i < 3; i++ {
		trace := &ConversationTrace{
			TraceID:     "trace-" + string(rune('A'+i)),
			StartTime:   now,
			EndTime:     now.Add(time.Duration(i+1) * time.Minute),
			DurationMs:  int64((i + 1) * 60000),
			TotalRounds: i + 2,
			ToolSteps: []ToolStep{
				{ToolName: "read_file", Success: true, DurationMs: 100, LLMRound: 1, ChainPos: 0},
				{ToolName: "edit_file", Success: true, DurationMs: 200, LLMRound: 2, ChainPos: 1},
			},
		}
		traceStore.Append(trace)
	}

	result := r.AnalyzeTracesForTest(time.Time{})
	if result == nil {
		t.Fatal("analyzeTraces should return non-nil with data")
	}
	if result.TotalTraces != 3 {
		t.Errorf("Expected 3 traces, got %d", result.TotalTraces)
	}
	if result.AvgRounds <= 0 {
		t.Errorf("AvgRounds should be > 0, got %f", result.AvgRounds)
	}
}

// --- LearningEngine additional tests ---

func TestLearningEngine_SetProvider(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	le := NewLearningEngine(tmpDir, registry, nil, nil, nil, nil, cfg)

	le.SetProvider(nil)
	// Should not panic
}

func TestLearningEngine_SetForge(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	le := NewLearningEngine(tmpDir, registry, nil, nil, nil, nil, cfg)

	f, _ := NewForge(workspace, nil)
	le.SetForge(f)
	// Should not panic
}

func TestLearningEngine_GetLatestCycle_Empty(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	le := NewLearningEngine(tmpDir, registry, nil, nil, nil, nil, cfg)

	result := le.GetLatestCycle()
	if result != nil {
		t.Error("GetLatestCycle should return nil when no cycles exist")
	}
}

func TestLearningEngine_GetLatestCycle_NilStore(t *testing.T) {
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(t.TempDir(), "registry.json"))
	le := NewLearningEngine(t.TempDir(), registry, nil, nil, nil, nil, cfg)

	// cycleStore is nil
	result := le.GetLatestCycle()
	if result != nil {
		t.Error("GetLatestCycle should return nil when cycleStore is nil")
	}
}

func TestLearningEngine_ExecuteCreateSkill_NoProvider(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))
	le := NewLearningEngine(tmpDir, registry, nil, nil, nil, nil, cfg)

	action := &LearningAction{
		DraftName:   "test-skill",
		Description: "Test skill",
	}
	le.ExecuteCreateSkillForTest(context.Background(), action)

	if action.Status != "failed" {
		t.Errorf("Expected status 'failed', got '%s'", action.Status)
	}
	if !strings.Contains(action.ErrorMsg, "No LLM provider") {
		t.Errorf("Expected 'No LLM provider' error, got '%s'", action.ErrorMsg)
	}
}

func TestLearningEngine_ExecuteCreateSkill_AlreadyExists(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(filepath.Join(tmpDir, "registry.json"))

	// Pre-register the artifact
	registry.Add(Artifact{
		ID:     "skill-test-skill",
		Type:   ArtifactSkill,
		Name:   "test-skill",
		Status: StatusActive,
	})

	le := NewLearningEngine(tmpDir, registry, nil, nil, nil, nil, cfg)

	action := &LearningAction{
		DraftName:   "test-skill",
		Description: "Test skill",
	}
	le.ExecuteCreateSkillForTest(context.Background(), action)

	if action.Status != "skipped" {
		t.Errorf("Expected status 'skipped' for existing artifact, got '%s'", action.Status)
	}
}

// --- TraceCollector OnEvent tests ---

func TestTraceCollector_OnEvent_Start(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	collector.OnEvent(context.Background(), observerConversationStart("trace-1", "sess-abc", "web"))

	// Verify active trace was created
	collector.activeMu.Lock()
	_, exists := collector.active["trace-1"]
	collector.activeMu.Unlock()

	if !exists {
		t.Error("Active trace should be created on conversation start")
	}
}

func TestTraceCollector_OnEvent_Start_BadData(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	// Send wrong data type - should not panic
	collector.OnEvent(context.Background(), convEvent("conversation_start", "trace-1", "bad data"))

	collector.activeMu.Lock()
	_, exists := collector.active["trace-1"]
	collector.activeMu.Unlock()

	if exists {
		t.Error("Active trace should NOT be created with bad data")
	}
}

func TestTraceCollector_OnEvent_LLMResponse(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	// First start a conversation
	collector.OnEvent(context.Background(), observerConversationStart("trace-2", "sess", "web"))

	// Then send LLM response
	collector.OnEvent(context.Background(), observerLLMResponse("trace-2", 100, 50))

	collector.activeMu.Lock()
	trace := collector.active["trace-2"]
	collector.activeMu.Unlock()

	if trace == nil {
		t.Fatal("Trace should exist")
	}
	if trace.TokensUsed != 150 {
		t.Errorf("Expected TokensUsed 150, got %d", trace.TokensUsed)
	}
}

func TestTraceCollector_OnEvent_LLMResponse_NoActiveTrace(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	// Send LLM response without start - should not panic
	collector.OnEvent(context.Background(), observerLLMResponse("nonexistent", 100, 50))
}

func TestTraceCollector_OnEvent_ToolCall(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	collector.OnEvent(context.Background(), observerConversationStart("trace-3", "sess", "web"))
	collector.OnEvent(context.Background(), observerToolCall("trace-3", "read_file", true, 100, 1, 0))

	collector.activeMu.Lock()
	trace := collector.active["trace-3"]
	collector.activeMu.Unlock()

	if trace == nil {
		t.Fatal("Trace should exist")
	}
	if len(trace.ToolSteps) != 1 {
		t.Fatalf("Expected 1 tool step, got %d", len(trace.ToolSteps))
	}
	if trace.ToolSteps[0].ToolName != "read_file" {
		t.Errorf("Expected tool 'read_file', got '%s'", trace.ToolSteps[0].ToolName)
	}
}

func TestTraceCollector_OnEvent_ToolCall_WithArgs(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	collector.OnEvent(context.Background(), observerConversationStart("trace-4", "sess", "web"))
	collector.OnEvent(context.Background(), observerToolCallWithArgs("trace-4", "edit_file", true, 200, 2, 1, map[string]interface{}{"path": "/test", "content": "hi"}))

	collector.activeMu.Lock()
	trace := collector.active["trace-4"]
	collector.activeMu.Unlock()

	if trace == nil {
		t.Fatal("Trace should exist")
	}
	if len(trace.ToolSteps) != 1 {
		t.Fatalf("Expected 1 tool step, got %d", len(trace.ToolSteps))
	}
	// ArgKeys should contain key names only
	if len(trace.ToolSteps[0].ArgKeys) != 2 {
		t.Errorf("Expected 2 arg keys, got %d", len(trace.ToolSteps[0].ArgKeys))
	}
}

func TestTraceCollector_OnEvent_ToolCall_Failure(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	collector.OnEvent(context.Background(), observerConversationStart("trace-5", "sess", "web"))
	collector.OnEvent(context.Background(), observerToolCallWithErr("trace-5", "exec", false, 500, 1, 0, "command failed: permission denied"))

	collector.activeMu.Lock()
	trace := collector.active["trace-5"]
	collector.activeMu.Unlock()

	if trace == nil {
		t.Fatal("Trace should exist")
	}
	if trace.ToolSteps[0].ErrorCode == "" {
		t.Error("ErrorCode should be set for failed tool calls")
	}
}

func TestTraceCollector_OnEvent_End(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	collector.OnEvent(context.Background(), observerConversationStart("trace-6", "sess", "web"))
	collector.OnEvent(context.Background(), observerConversationEnd("trace-6", 5, 10*time.Second))

	// Active trace should be removed
	collector.activeMu.Lock()
	_, exists := collector.active["trace-6"]
	collector.activeMu.Unlock()

	if exists {
		t.Error("Active trace should be removed after end")
	}

	// Trace should be persisted
	traces, err := store.ReadTraces(time.Time{})
	if err != nil {
		t.Fatalf("ReadTraces failed: %v", err)
	}
	if len(traces) != 1 {
		t.Errorf("Expected 1 persisted trace, got %d", len(traces))
	}
	if traces[0].TraceID != "trace-6" {
		t.Errorf("Expected TraceID 'trace-6', got '%s'", traces[0].TraceID)
	}
	if traces[0].TotalRounds != 5 {
		t.Errorf("Expected TotalRounds 5, got %d", traces[0].TotalRounds)
	}
}

func TestTraceCollector_OnEvent_End_NoActiveTrace(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	store := NewTraceStore(tmpDir, cfg)
	collector := NewTraceCollector(store, cfg)

	// End without start - should not panic
	collector.OnEvent(context.Background(), observerConversationEnd("nonexistent", 1, time.Second))
}

// --- observer event helpers ---

func observerConversationStart(traceID, sessionKey, channel string) observer.ConversationEvent {
	return observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   traceID,
		Timestamp: time.Now().UTC(),
		Data: &observer.ConversationStartData{
			SessionKey: sessionKey,
			Channel:    channel,
		},
	}
}

func observerLLMResponse(traceID string, promptTokens, completionTokens int) observer.ConversationEvent {
	return observer.ConversationEvent{
		Type:      observer.EventLLMResponse,
		TraceID:   traceID,
		Timestamp: time.Now().UTC(),
		Data: &observer.LLMResponseData{
			Usage: &protocoltypes.UsageInfo{
				PromptTokens:     promptTokens,
				CompletionTokens: completionTokens,
				TotalTokens:      promptTokens + completionTokens,
			},
		},
	}
}

func observerToolCall(traceID string, toolName string, success bool, durationMs int64, round int, chainPos int) observer.ConversationEvent {
	return observer.ConversationEvent{
		Type:      observer.EventToolCall,
		TraceID:   traceID,
		Timestamp: time.Now().UTC(),
		Data: &observer.ToolCallData{
			ToolName:  toolName,
			Success:   success,
			Duration:  time.Duration(durationMs) * time.Millisecond,
			LLMRound:  round,
			ChainPos:  chainPos,
		},
	}
}

func observerToolCallWithArgs(traceID string, toolName string, success bool, durationMs int64, round int, chainPos int, args map[string]interface{}) observer.ConversationEvent {
	return observer.ConversationEvent{
		Type:      observer.EventToolCall,
		TraceID:   traceID,
		Timestamp: time.Now().UTC(),
		Data: &observer.ToolCallData{
			ToolName:  toolName,
			Arguments: args,
			Success:   success,
			Duration:  time.Duration(durationMs) * time.Millisecond,
			LLMRound:  round,
			ChainPos:  chainPos,
		},
	}
}

func observerToolCallWithErr(traceID string, toolName string, success bool, durationMs int64, round int, chainPos int, errMsg string) observer.ConversationEvent {
	return observer.ConversationEvent{
		Type:      observer.EventToolCall,
		TraceID:   traceID,
		Timestamp: time.Now().UTC(),
		Data: &observer.ToolCallData{
			ToolName:  toolName,
			Success:   success,
			Duration:  time.Duration(durationMs) * time.Millisecond,
			LLMRound:  round,
			ChainPos:  chainPos,
			Error:     errMsg,
		},
	}
}

func observerConversationEnd(traceID string, totalRounds int, totalDuration time.Duration) observer.ConversationEvent {
	return observer.ConversationEvent{
		Type:      observer.EventConversationEnd,
		TraceID:   traceID,
		Timestamp: time.Now().UTC(),
		Data: &observer.ConversationEndData{
			TotalRounds:   totalRounds,
			TotalDuration: totalDuration,
		},
	}
}

func convEvent(eventType, traceID string, data interface{}) observer.ConversationEvent {
	return observer.ConversationEvent{
		Type:      observer.EventType(eventType),
		TraceID:   traceID,
		Timestamp: time.Now().UTC(),
		Data:      data,
	}
}
