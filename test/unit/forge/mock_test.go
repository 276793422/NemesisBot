package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/plugin"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/tools"
)

// === Mock LLM Provider for Forge semantic analysis ===

type mockLLMProvider struct {
	defaultModel string
	chatFunc     func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error)
}

func (m *mockLLMProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	if m.chatFunc != nil {
		return m.chatFunc(ctx, messages, tools, model, options)
	}
	return &providers.LLMResponse{
		Content:      "Mock LLM analysis: Consider creating a Skill for the read_file → edit_file pattern.",
		FinishReason: "stop",
	}, nil
}

func (m *mockLLMProvider) GetDefaultModel() string {
	if m.defaultModel != "" {
		return m.defaultModel
	}
	return "mock-llm-v1"
}

// === Mock Tests: LLM Semantic Reflection ===

func TestSemanticReflectionWithMockLLM(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1
	cfg.Reflection.UseLLM = true
	cfg.Reflection.LLMBudgetTokens = 4000

	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	// Inject mock LLM provider
	mockProvider := &mockLLMProvider{
		defaultModel: "test-model-v1",
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			// Verify the prompt contains our data
			if len(messages) < 2 {
				t.Error("Expected at least 2 messages (system + user)")
			}
			if messages[0].Role != "system" {
				t.Error("First message should be system prompt")
			}
			if messages[1].Role != "user" {
				t.Error("Second message should be user prompt")
			}
			if !strings.Contains(messages[1].Content, "read_file") {
				t.Error("User prompt should contain tool usage data")
			}
			if !strings.Contains(messages[1].Content, "High-Frequency Patterns") {
				t.Error("User prompt should ask for pattern analysis (High-Frequency Patterns)")
			}

			return &providers.LLMResponse{
				Content:      "分析结果: read_file → edit_file 模式出现频率高，建议创建 Skill: config-editor。exec 工具成功率偏低，建议改进错误处理。",
				FinishReason: "stop",
			}, nil
		},
	}
	reflector.SetProvider(mockProvider)

	// Seed experience data
	now := time.Now().UTC()
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:llm1",
		ToolName:    "read_file",
		Count:       20,
		LastSeen:    now,
	})

	// Run reflection
	reportPath, err := reflector.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Reflect failed: %v", err)
	}

	content, _ := os.ReadFile(reportPath)
	reportStr := string(content)

	if !contains(reportStr, "LLM 深度分析") {
		t.Error("Report should contain LLM insights section")
	}
	if !contains(reportStr, "config-editor") {
		t.Error("Report should contain LLM suggestion: config-editor")
	}
}

func TestSemanticReflectionLLMError(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1
	cfg.Reflection.UseLLM = true

	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	// Inject failing mock
	mockProvider := &mockLLMProvider{
		chatFunc: func(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
			return nil, context.DeadlineExceeded
		},
	}
	reflector.SetProvider(mockProvider)

	// Seed data
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:err1",
		ToolName:    "test",
		Count:       5,
		LastSeen:    time.Now().UTC(),
	})

	// Should not fail - LLM error is swallowed, statistical report still generated
	reportPath, err := reflector.Reflect(context.Background(), "today", "all")
	if err != nil {
		t.Fatalf("Reflect should not fail when LLM errors: %v", err)
	}

	content, _ := os.ReadFile(reportPath)
	if !contains(string(content), "统计概要") {
		t.Error("Statistical report should still be generated even if LLM fails")
	}
}

func TestSemanticReflectionDisabled(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1
	cfg.Reflection.UseLLM = false // Disabled

	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)
	reflector.SetProvider(&mockLLMProvider{})

	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:nollm",
		ToolName:    "test",
		Count:       5,
		LastSeen:    time.Now().UTC(),
	})

	reportPath, _ := reflector.Reflect(context.Background(), "today", "all")
	content, _ := os.ReadFile(reportPath)

	if contains(string(content), "LLM 深度分析") {
		t.Error("Report should not contain LLM insights when disabled")
	}
}

// === Forge End-to-End Integration Test (Mock) ===

func TestForgeEndToEndWithMocks(t *testing.T) {
	tmpDir := t.TempDir()

	// Create workspace structure
	workspace := filepath.Join(tmpDir, ".nemesisbot", "workspace")
	os.MkdirAll(workspace, 0755)

	// Create a mock plugin manager
	pluginMgr := plugin.NewManager()

	// Create Forge (without workspace-level forge dir setup - NewForge does that)
	forgeInstance, err := forge.NewForge(workspace, pluginMgr)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	if forgeInstance == nil {
		t.Fatal("Forge instance is nil")
	}

	// Verify directory structure was created
	forgeDir := forgeInstance.GetWorkspace()
	dirs := []string{"experiences", "reflections", "skills", "scripts", "mcp"}
	for _, dir := range dirs {
		path := filepath.Join(forgeDir, dir)
		if _, err := os.Stat(path); os.IsNotExist(err) {
			t.Errorf("Directory %s should exist", path)
		}
	}

	// Verify ForgePlugin was registered
	if !pluginMgr.IsEnabled("forge") {
		t.Error("Forge plugin should be registered and enabled")
	}

	// Simulate tool invocations through the plugin
	for i := 0; i < 3; i++ {
		invocation := &plugin.ToolInvocation{
			ToolName: "read_file",
			Args:     map[string]interface{}{"path": "/tmp/config.json"},
			Metadata: map[string]interface{}{"session_id": "sess-test"},
		}
		allowed, err, modified := pluginMgr.ListPlugins()[0].Execute(context.Background(), invocation)
		if !allowed {
			t.Error("Plugin should allow all operations")
		}
		if err != nil {
			t.Errorf("Plugin should not error: %v", err)
		}
		if modified {
			t.Error("Plugin should not modify operations")
		}
	}
}

// === Forge Tools Tests (Mock) ===

func TestForgeToolsCreation(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	forgeInstance, _ := forge.NewForge(workspace, nil)
	tools := forge.NewForgeTools(forgeInstance)

	if len(tools) != 8 {
		t.Fatalf("Expected 8 tools, got %d", len(tools))
	}

	expectedNames := []string{"forge_reflect", "forge_create", "forge_update", "forge_list", "forge_evaluate", "forge_build_mcp", "forge_share", "forge_learning_status"}
	for i, expected := range expectedNames {
		if tools[i].Name() != expected {
			t.Errorf("Tool %d: expected name '%s', got '%s'", i, expected, tools[i].Name())
		}
	}
}

func TestForgeListToolEmpty(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	forgeInstance, _ := forge.NewForge(workspace, nil)
	tools := forge.NewForgeTools(forgeInstance)

	for _, tool := range tools {
		if tool.Name() == "forge_list" {
			result := tool.Execute(context.Background(), map[string]interface{}{})
			if result.IsError {
				t.Errorf("forge_list on empty registry should not error: %s", result.ForLLM)
			}
			if !contains(result.ForLLM, "暂无") {
				t.Errorf("forge_list should say '暂无 Forge 产物', got: %s", result.ForLLM)
			}
			return
		}
	}
	t.Fatal("forge_list tool not found")
}

// === forge_build_mcp Tool Tests ===

func newTestForgeWithMCP(t *testing.T) (*forge.Forge, string) {
	t.Helper()
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := forge.NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}
	return f, workspace
}

func TestForgeBuildMCPToolNotFound(t *testing.T) {
	f, _ := newTestForgeWithMCP(t)
	tools := forge.NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":     "mcp-nonexistent",
				"action": "build",
			})
			if !result.IsError {
				t.Error("Should error on non-existent artifact")
			}
			return
		}
	}
	t.Fatal("forge_build_mcp tool not found")
}

func TestForgeBuildMCPToolWrongType(t *testing.T) {
	f, workspace := newTestForgeWithMCP(t)

	// Create a skill artifact (not MCP)
	skillDir := filepath.Join(workspace, "forge", "skills", "test-skill")
	os.MkdirAll(skillDir, 0755)
	skillPath := filepath.Join(skillDir, "SKILL.md")
	os.WriteFile(skillPath, []byte("---\nname: test-skill\n---\nContent"), 0644)

	f.GetRegistry().Add(forge.Artifact{
		ID:     "skill-test-skill",
		Type:   forge.ArtifactSkill,
		Name:   "test-skill",
		Status: forge.StatusActive,
		Path:   skillPath,
	})

	tools := forge.NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":     "skill-test-skill",
				"action": "build",
			})
			if !result.IsError {
				t.Error("Should error when artifact is not MCP type")
			}
			return
		}
	}
	t.Fatal("forge_build_mcp tool not found")
}

func TestForgeBuildMCPToolInstallUninstall(t *testing.T) {
	f, workspace := newTestForgeWithMCP(t)

	// Create MCP artifact
	mcpDir := filepath.Join(workspace, "forge", "mcp", "test-mcp")
	os.MkdirAll(mcpDir, 0755)
	mcpPath := filepath.Join(mcpDir, "server.py")
	os.WriteFile(mcpPath, []byte("from mcp.server import Server\nimport mcp\n\ndef main():\n    pass\n"), 0644)

	f.GetRegistry().Add(forge.Artifact{
		ID:      "mcp-test-mcp",
		Type:    forge.ArtifactMCP,
		Name:    "test-mcp",
		Version: "1.0",
		Status:  forge.StatusActive,
		Path:    mcpPath,
	})

	// Create MCP config dir
	configDir := filepath.Join(workspace, "config")
	os.MkdirAll(configDir, 0755)

	tools := forge.NewForgeTools(f)
	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			// Install
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":     "mcp-test-mcp",
				"action": "install",
			})
			if result.IsError {
				t.Errorf("Install should succeed: %s", result.ForLLM)
			}

			// Verify installed
			if !f.GetMCPInstaller().IsInstalled("test-mcp") {
				t.Error("Should be installed after install action")
			}

			// Uninstall
			result = tool.Execute(context.Background(), map[string]interface{}{
				"id":     "mcp-test-mcp",
				"action": "uninstall",
			})
			if result.IsError {
				t.Errorf("Uninstall should succeed: %s", result.ForLLM)
			}

			if f.GetMCPInstaller().IsInstalled("test-mcp") {
				t.Error("Should not be installed after uninstall action")
			}
			return
		}
	}
	t.Fatal("forge_build_mcp tool not found")
}

func TestForgeBuildMCPToolInvalidAction(t *testing.T) {
	f, _ := newTestForgeWithMCP(t)
	tools := forge.NewForgeTools(f)

	for _, tool := range tools {
		if tool.Name() == "forge_build_mcp" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"id":     "mcp-nonexistent",
				"action": "invalid",
			})
			if !result.IsError {
				t.Error("Should error on invalid action")
			}
			return
		}
	}
	t.Fatal("forge_build_mcp tool not found")
}

// === forge_create MCP End-to-End Tests ===

func TestForgeCreateMCPDefaultPython(t *testing.T) {
	f, workspace := newTestForgeWithMCP(t)
	ftools := forge.NewForgeTools(f)

	var createTool interface {
		Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
	}
	for _, t2 := range ftools {
		if t2.Name() == "forge_create" {
			createTool = t2.(interface {
				Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
			})
			break
		}
	}
	if createTool == nil {
		t.Fatal("forge_create tool not found")
	}

	result := createTool.Execute(context.Background(), map[string]interface{}{
		"type":        "mcp",
		"name":        "json-validator",
		"content":     "from mcp.server import Server\nimport mcp\n\ndef main():\n    pass\n",
		"description": "A JSON validator MCP server",
		"test_cases":  []interface{}{map[string]interface{}{"input": "test"}},
	})

	if result.IsError {
		t.Fatalf("forge_create MCP should succeed: %s", result.ForLLM)
	}

	// Verify server.py was created (not main.go)
	serverPy := filepath.Join(workspace, "forge", "mcp", "json-validator", "server.py")
	if _, err := os.Stat(serverPy); os.IsNotExist(err) {
		t.Error("server.py should be created for default Python MCP")
	}

	// Verify requirements.txt was generated
	reqFile := filepath.Join(workspace, "forge", "mcp", "json-validator", "requirements.txt")
	data, err := os.ReadFile(reqFile)
	if err != nil {
		t.Errorf("requirements.txt should exist: %v", err)
	} else if !strings.Contains(string(data), "mcp") {
		t.Errorf("requirements.txt should contain 'mcp', got: %s", string(data))
	}

	// Verify README.md was generated
	readmeFile := filepath.Join(workspace, "forge", "mcp", "json-validator", "README.md")
	if _, err := os.Stat(readmeFile); os.IsNotExist(err) {
		t.Error("README.md should be generated")
	}

	// Verify test_cases.json was written
	testFile := filepath.Join(workspace, "forge", "mcp", "json-validator", "tests", "test_cases.json")
	if _, err := os.Stat(testFile); os.IsNotExist(err) {
		t.Error("tests/test_cases.json should be created when test_cases provided")
	}

	// Verify registry entry
	artifact, found := f.GetRegistry().Get("mcp-json-validator")
	if !found {
		t.Fatal("Artifact should be registered in registry")
	}
	if artifact.Type != forge.ArtifactMCP {
		t.Errorf("Expected type 'mcp', got '%s'", artifact.Type)
	}
}

func TestForgeCreateMCPGoVariant(t *testing.T) {
	f, workspace := newTestForgeWithMCP(t)
	ftools := forge.NewForgeTools(f)

	var createTool interface {
		Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
	}
	for _, t2 := range ftools {
		if t2.Name() == "forge_create" {
			createTool = t2.(interface {
				Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
			})
			break
		}
	}
	if createTool == nil {
		t.Fatal("forge_create tool not found")
	}

	result := createTool.Execute(context.Background(), map[string]interface{}{
		"type":        "mcp",
		"name":        "go-server",
		"content":     "package main\n\nfunc main() {}\n",
		"language":    "go",
		"description": "A Go MCP server",
		"test_cases":  []interface{}{map[string]interface{}{"input": "test"}},
	})

	if result.IsError {
		t.Fatalf("forge_create MCP Go should succeed: %s", result.ForLLM)
	}

	// Verify main.go was created (not server.py)
	mainGo := filepath.Join(workspace, "forge", "mcp", "go-server", "main.go")
	if _, err := os.Stat(mainGo); os.IsNotExist(err) {
		t.Error("main.go should be created for Go MCP")
	}

	// Verify server.py was NOT created
	serverPy := filepath.Join(workspace, "forge", "mcp", "go-server", "server.py")
	if _, err := os.Stat(serverPy); err == nil {
		t.Error("server.py should NOT exist for Go MCP")
	}

	// Verify go.mod was generated
	goMod := filepath.Join(workspace, "forge", "mcp", "go-server", "go.mod")
	data, err := os.ReadFile(goMod)
	if err != nil {
		t.Errorf("go.mod should exist: %v", err)
	} else if !strings.Contains(string(data), "module forge-mcp-go-server") {
		t.Errorf("go.mod should contain module name, got: %s", string(data))
	}
}

func TestForgeCreateMCPRequiresTestCases(t *testing.T) {
	f, _ := newTestForgeWithMCP(t)
	ftools := forge.NewForgeTools(f)

	var createTool interface {
		Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
	}
	for _, t2 := range ftools {
		if t2.Name() == "forge_create" {
			createTool = t2.(interface {
				Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
			})
			break
		}
	}
	if createTool == nil {
		t.Fatal("forge_create tool not found")
	}

	// No test_cases provided
	result := createTool.Execute(context.Background(), map[string]interface{}{
		"type":    "mcp",
		"name":    "no-tests",
		"content": "from mcp.server import Server\n",
	})

	if !result.IsError {
		t.Error("forge_create MCP without test_cases should error")
	}
	if !contains(result.ForLLM, "test_cases") {
		t.Errorf("Error should mention test_cases, got: %s", result.ForLLM)
	}
}

func TestForgeCreateMCPAutoRegisterWhenActive(t *testing.T) {
	f, workspace := newTestForgeWithMCP(t)

	// Create MCP config dir so installer can write
	configDir := filepath.Join(workspace, "config")
	os.MkdirAll(configDir, 0755)

	// Create MCP artifact, set to active, then verify installer works
	mcpDir := filepath.Join(workspace, "forge", "mcp", "auto-reg")
	os.MkdirAll(mcpDir, 0755)
	mcpPath := filepath.Join(mcpDir, "server.py")
	os.WriteFile(mcpPath, []byte("from mcp.server import Server\nimport mcp\n\ndef main():\n    pass\n"), 0644)

	f.GetRegistry().Add(forge.Artifact{
		ID:      "mcp-auto-reg",
		Type:    forge.ArtifactMCP,
		Name:    "auto-reg",
		Version: "1.0",
		Status:  forge.StatusActive,
		Path:    mcpPath,
	})

	inst := f.GetMCPInstaller()
	err := inst.Install(&forge.Artifact{
		ID:   "mcp-auto-reg",
		Name: "auto-reg",
		Type: forge.ArtifactMCP,
	}, mcpDir)
	if err != nil {
		t.Fatalf("Install should work: %v", err)
	}

	if !inst.IsInstalled("auto-reg") {
		t.Error("MCP should be installed after calling Install")
	}

	// Verify config.mcp.json has the server
	configData, _ := os.ReadFile(filepath.Join(configDir, "config.mcp.json"))
	if !strings.Contains(string(configData), "auto-reg") {
		t.Errorf("config.mcp.json should contain 'auto-reg', got: %s", string(configData))
	}
}

func TestForgeUpdateMCPReRegister(t *testing.T) {
	f, workspace := newTestForgeWithMCP(t)

	// Setup MCP config
	configDir := filepath.Join(workspace, "config")
	os.MkdirAll(configDir, 0755)

	// Create MCP artifact
	mcpDir := filepath.Join(workspace, "forge", "mcp", "update-test")
	os.MkdirAll(mcpDir, 0755)
	mcpPath := filepath.Join(mcpDir, "server.py")
	os.WriteFile(mcpPath, []byte("from mcp.server import Server\nimport mcp\n\ndef main():\n    pass\n"), 0644)

	f.GetRegistry().Add(forge.Artifact{
		ID:      "mcp-update-test",
		Type:    forge.ArtifactMCP,
		Name:    "update-test",
		Version: "1.0",
		Status:  forge.StatusActive,
		Path:    mcpPath,
	})

	// Pre-register
	inst := f.GetMCPInstaller()
	inst.Install(&forge.Artifact{
		ID:   "mcp-update-test",
		Name: "update-test",
		Type: forge.ArtifactMCP,
	}, mcpDir)

	if !inst.IsInstalled("update-test") {
		t.Fatal("Pre-condition: MCP should be installed before update")
	}

	// Update via forge_update tool
	ftools := forge.NewForgeTools(f)
	var updateTool interface {
		Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
	}
	for _, t2 := range ftools {
		if t2.Name() == "forge_update" {
			updateTool = t2.(interface {
				Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
			})
			break
		}
	}
	if updateTool == nil {
		t.Fatal("forge_update tool not found")
	}

	result := updateTool.Execute(context.Background(), map[string]interface{}{
		"id":                 "mcp-update-test",
		"content":            "from mcp.server import Server\nimport mcp\n\ndef main():\n    print('updated')\n",
		"change_description": "Updated server logic",
	})

	if result.IsError {
		t.Fatalf("forge_update should succeed: %s", result.ForLLM)
	}

	// MCP should still be installed after update (re-registered)
	if !inst.IsInstalled("update-test") {
		t.Error("MCP should still be installed after update (re-registered)")
	}

	// Verify version was incremented
	artifact, _ := f.GetRegistry().Get("mcp-update-test")
	if artifact.Version == "1.0" {
		t.Error("Version should be incremented after update")
	}
}

func TestForgeBuildMCPBuildAction(t *testing.T) {
	f, workspace := newTestForgeWithMCP(t)

	// Setup MCP config
	configDir := filepath.Join(workspace, "config")
	os.MkdirAll(configDir, 0755)

	// Create MCP artifact with valid Python content
	mcpDir := filepath.Join(workspace, "forge", "mcp", "build-test")
	os.MkdirAll(mcpDir, 0755)
	mcpPath := filepath.Join(mcpDir, "server.py")
	os.WriteFile(mcpPath, []byte("from mcp.server import Server\nimport mcp\n\ndef main():\n    pass\n"), 0644)

	// Create test cases
	tdir := filepath.Join(mcpDir, "tests")
	os.MkdirAll(tdir, 0755)
	os.WriteFile(filepath.Join(tdir, "test_cases.json"), []byte(`[{"name": "test1", "input": "hello", "expected": "world"}]`), 0644)

	f.GetRegistry().Add(forge.Artifact{
		ID:      "mcp-build-test",
		Type:    forge.ArtifactMCP,
		Name:    "build-test",
		Version: "1.0",
		Status:  forge.StatusDraft,
		Path:    mcpPath,
	})

	ftools := forge.NewForgeTools(f)
	var buildTool interface {
		Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
	}
	for _, t2 := range ftools {
		if t2.Name() == "forge_build_mcp" {
			buildTool = t2.(interface {
				Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult
			})
			break
		}
	}
	if buildTool == nil {
		t.Fatal("forge_build_mcp tool not found")
	}

	result := buildTool.Execute(context.Background(), map[string]interface{}{
		"id":     "mcp-build-test",
		"action": "build",
	})

	if result.IsError {
		t.Fatalf("build action should succeed: %s", result.ForLLM)
	}

	// Should contain validation results
	if !contains(result.ForLLM, "静态验证") {
		t.Errorf("Build result should mention validation, got: %s", result.ForLLM)
	}
}
