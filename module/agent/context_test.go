// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/tools"
)

// TestContextBuilder_BuildSystemPrompt tests system prompt building
func TestContextBuilder_BuildSystemPrompt(t *testing.T) {
	tempDir := t.TempDir()
	workspace := filepath.Join(tempDir, ".nemesisbot")
	err := os.MkdirAll(workspace, 0755)
	if err != nil {
		t.Fatalf("Failed to create workspace: %v", err)
	}

	identityPath := filepath.Join(workspace, "IDENTITY.md")
	err = os.WriteFile(identityPath, []byte("# Test Identity\nYou are a helpful assistant."), 0644)
	if err != nil {
		t.Fatalf("Failed to create IDENTITY.md: %v", err)
	}

	builder := NewContextBuilder(workspace)
	prompt := builder.BuildSystemPrompt(false)

	if prompt == "" {
		t.Error("Expected non-empty system prompt")
	}

	if !strings.Contains(prompt, "Test Identity") {
		t.Error("Expected system prompt to contain identity content")
	}
}

// TestMemoryStore_ReadLongTerm tests reading long-term memory
func TestMemoryStore_ReadLongTerm(t *testing.T) {
	tempDir := t.TempDir()

	store := NewMemoryStore(tempDir)

	// Write test memory first
	content := "# Important Memory\nThis is critical information."
	err := store.WriteLongTerm(content)
	if err != nil {
		t.Fatalf("Failed to write long-term memory: %v", err)
	}

	memory := store.ReadLongTerm()

	if memory == "" {
		t.Error("Expected non-empty long-term memory")
	}

	if !strings.Contains(memory, "Important Memory") {
		t.Error("Expected memory to contain content")
	}
}

// TestMemoryStore_WriteLongTerm tests writing long-term memory
func TestMemoryStore_WriteLongTerm(t *testing.T) {
	tempDir := t.TempDir()

	store := NewMemoryStore(tempDir)
	content := "# New Memory\nThis is new information."

	err := store.WriteLongTerm(content)
	if err != nil {
		t.Fatalf("Failed to write long-term memory: %v", err)
	}

	readContent := store.ReadLongTerm()
	if readContent != content {
		t.Error("Written content doesn't match")
	}
}

// TestMemoryStore_AppendToday tests appending to today's notes
func TestMemoryStore_AppendToday(t *testing.T) {
	tempDir := t.TempDir()
	memoryDir := filepath.Join(tempDir, ".nemesisbot", "memory")
	err := os.MkdirAll(memoryDir, 0755)
	if err != nil {
		t.Fatalf("Failed to create memory dir: %v", err)
	}

	store := NewMemoryStore(tempDir)
	content := "Today's note\n"

	store.AppendToday(content)

	todayContent := store.ReadToday()
	if !strings.Contains(todayContent, "Today's note") {
		t.Error("Expected today's notes to contain appended content")
	}
}

// TestMemoryStore_GetMemoryContext tests getting memory context
func TestMemoryStore_GetMemoryContext(t *testing.T) {
	tempDir := t.TempDir()

	store := NewMemoryStore(tempDir)

	// Write test memory
	content := "# Memory\nImportant info"
	err := store.WriteLongTerm(content)
	if err != nil {
		t.Fatalf("Failed to write long-term memory: %v", err)
	}

	context := store.GetMemoryContext()

	if context == "" {
		t.Error("Expected non-empty memory context")
	}

	if !strings.Contains(context, "Important info") {
		t.Error("Expected context to contain memory content")
	}
}

// TestAgentRegistry_GetAgentWithDefaults tests getting agent with defaults
func TestAgentRegistry_GetAgentWithDefaults(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
				MaxTokens: 4000,
			},
		},
	}

	provider := &mockProviderForTest{}
	registry := NewAgentRegistry(cfg, provider)

	agent, ok := registry.GetAgent("")

	if !ok || agent == nil {
		t.Fatal("Expected non-nil default agent")
	}
}

// TestAgentRegistry_ListAgentIDsEmpty tests listing agents when empty
func TestAgentRegistry_ListAgentIDsEmpty(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			List: []config.AgentConfig{},
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	provider := &mockProviderForTest{}
	registry := NewAgentRegistry(cfg, provider)

	agentIDs := registry.ListAgentIDs()

	if len(agentIDs) == 0 {
		t.Error("Expected at least default agent ID")
	}
}

// TestAgentLoop_ProcessDirect_WithSystemPrompt tests processing with custom system prompt
func TestAgentLoop_ProcessDirect_WithSystemPrompt(t *testing.T) {
	tempDir := t.TempDir()
	workspace := filepath.Join(tempDir, ".nemesisbot")
	err := os.MkdirAll(workspace, 0755)
	if err != nil {
		t.Fatalf("Failed to create workspace: %v", err)
	}

	identityPath := filepath.Join(workspace, "IDENTITY.md")
	err = os.WriteFile(identityPath, []byte("# Custom Bot\nYou are custom."), 0644)
	if err != nil {
		t.Fatalf("Failed to create IDENTITY.md: %v", err)
	}

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{
		responses: []string{"Custom response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()
	response, err := loop.ProcessDirect(ctx, "Who are you?", "test-session")

	if err != nil {
		t.Fatalf("Expected no error, got: %v", err)
	}

	if response == "" {
		t.Error("Expected non-empty response")
	}
}

// TestAgentLoop_ProcessDirect_EmptyMessage tests empty message handling
func TestAgentLoop_ProcessDirect_EmptyMessage(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{
		responses: []string{"Response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()
	_, _ = loop.ProcessDirect(ctx, "", "test-session")
	// Empty message should be handled gracefully
}

// TestAgentLoop_ProcessDirect_VeryLongMessage tests very long message handling
func TestAgentLoop_ProcessDirect_VeryLongMessage(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
				MaxTokens: 4000,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{
		responses: []string{"Response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()
	longMessage := strings.Repeat("This is a long message. ", 1000)
	_, _ = loop.ProcessDirect(ctx, longMessage, "test-session")
	// Long message should be handled gracefully
}

// TestAgentLoop_ProcessDirect_SpecialCharacters tests special characters
func TestAgentLoop_ProcessDirect_SpecialCharacters(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{
		responses: []string{"Response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()

	specialMessages := []string{
		"Message with \n newlines",
		"Message with \t tabs",
		"Message with emoji 🎉",
		"Message with 中文",
		"Message with \"quotes\"",
	}

	for _, msg := range specialMessages {
		_, err := loop.ProcessDirect(ctx, msg, "test-session")
		if err != nil {
			t.Errorf("Failed to process special message: %v", err)
		}
	}
}

// TestAgentLoop_ProcessDirectWithChannel_DifferentChannels tests different channels
func TestAgentLoop_ProcessDirectWithChannel_DifferentChannels(t *testing.T) {
	tempDir := t.TempDir()

	cfg := &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:       "test/model",
				Workspace: tempDir,
			},
		},
	}

	msgBus := bus.NewMessageBus()
	provider := &mockProviderForTest{
		responses: []string{"Response"},
	}

	loop := NewAgentLoop(cfg, msgBus, provider)

	ctx := context.Background()

	channels := []string{"discord", "telegram", "rpc", "websocket"}
	for _, channel := range channels {
		_, err := loop.ProcessDirectWithChannel(ctx, "Test", "session", channel, "chat-123")
		if err != nil {
			t.Errorf("Failed for channel %s: %v", channel, err)
		}
	}
}

// TestContextBuilder_AddToolResult tests adding tool results to messages
func TestContextBuilder_AddToolResult(t *testing.T) {
	tempDir := t.TempDir()
	builder := NewContextBuilder(tempDir)

	messages := []providers.Message{
		{Role: "system", Content: "System prompt"},
	}

	toolCallID := "call_123"
	toolName := "test_tool"
	result := "Tool executed successfully"

	messages = builder.AddToolResult(messages, toolCallID, toolName, result)

	if len(messages) != 2 {
		t.Fatalf("Expected 2 messages, got %d", len(messages))
	}

	lastMsg := messages[len(messages)-1]
	if lastMsg.Role != "tool" {
		t.Errorf("Expected role 'tool', got %s", lastMsg.Role)
	}

	if lastMsg.ToolCallID != toolCallID {
		t.Errorf("Expected ToolCallID %s, got %s", toolCallID, lastMsg.ToolCallID)
	}

	if lastMsg.Content != result {
		t.Errorf("Expected content %q, got %q", result, lastMsg.Content)
	}
}

// TestContextBuilder_AddAssistantMessage tests adding assistant messages
func TestContextBuilder_AddAssistantMessage(t *testing.T) {
	tempDir := t.TempDir()
	builder := NewContextBuilder(tempDir)

	messages := []providers.Message{
		{Role: "system", Content: "System prompt"},
	}

	content := "Assistant response"
	toolCalls := []map[string]interface{}{
		{"id": "call_123", "type": "function"},
	}

	messages = builder.AddAssistantMessage(messages, content, toolCalls)

	if len(messages) != 2 {
		t.Fatalf("Expected 2 messages, got %d", len(messages))
	}

	lastMsg := messages[len(messages)-1]
	if lastMsg.Role != "assistant" {
		t.Errorf("Expected role 'assistant', got %s", lastMsg.Role)
	}

	if lastMsg.Content != content {
		t.Errorf("Expected content %q, got %q", content, lastMsg.Content)
	}
}

// TestContextBuilder_AddAssistantMessage_NoContent tests adding assistant message without content
func TestContextBuilder_AddAssistantMessage_NoContent(t *testing.T) {
	tempDir := t.TempDir()
	builder := NewContextBuilder(tempDir)

	messages := []providers.Message{}
	messages = builder.AddAssistantMessage(messages, "", nil)

	if len(messages) != 1 {
		t.Fatalf("Expected 1 message, got %d", len(messages))
	}

	if messages[0].Role != "assistant" {
		t.Errorf("Expected role 'assistant', got %s", messages[0].Role)
	}
}

// TestContextBuilder_GetSkillsInfo tests getting skills information
func TestContextBuilder_GetSkillsInfo(t *testing.T) {
	tempDir := t.TempDir()
	builder := NewContextBuilder(tempDir)

	info := builder.GetSkillsInfo()

	if info == nil {
		t.Fatal("Expected non-nil skills info")
	}

	// Check required fields
	if _, ok := info["total"]; !ok {
		t.Error("Expected 'total' field in skills info")
	}

	if _, ok := info["available"]; !ok {
		t.Error("Expected 'available' field in skills info")
	}

	if _, ok := info["names"]; !ok {
		t.Error("Expected 'names' field in skills info")
	}
}

// TestContextBuilder_BuildMessages_WithSummary tests building messages with summary
func TestContextBuilder_BuildMessages_WithSummary(t *testing.T) {
	tempDir := t.TempDir()
	builder := NewContextBuilder(tempDir)

	history := []providers.Message{
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there!"},
	}

	summary := "Summary of previous conversation"
	currentMessage := "Continue testing"
	messages := builder.BuildMessages(history, summary, currentMessage, nil, "test-channel", "chat-123", false)

	systemMsg := messages[0]
	if !strings.Contains(systemMsg.Content, summary) {
		t.Error("Expected system prompt to contain summary")
	}

	if !strings.Contains(systemMsg.Content, "Summary of Previous Conversation") {
		t.Error("Expected summary section header")
	}
}

// TestContextBuilder_LoadBootstrapFiles tests loading bootstrap files
func TestContextBuilder_LoadBootstrapFiles(t *testing.T) {
	tempDir := t.TempDir()

	// Create bootstrap files
	files := map[string]string{
		"AGENT.md":    "# Agent\nAgent info",
		"IDENTITY.md": "# Identity\nI am a bot.",
		"SOUL.md":     "# Soul\nBe helpful.",
		"USER.md":     "# User\nPreferences here.",
		"MCP.md":      "# MCP\nMCP config",
	}

	for filename, fileContent := range files {
		path := filepath.Join(tempDir, filename)
		err := os.WriteFile(path, []byte(fileContent), 0644)
		if err != nil {
			t.Fatalf("Failed to create %s: %v", filename, err)
		}
	}

	builder := NewContextBuilder(tempDir)
	content := builder.LoadBootstrapFiles(true)

	// Should contain all files
	for filename := range files {
		if !strings.Contains(content, filename) {
			t.Errorf("Expected content to contain %s", filename)
		}
	}
}

// TestContextBuilder_LoadBootstrapFiles_WithBootstrapMode tests with BOOTSTRAP.md
func TestContextBuilder_LoadBootstrapFiles_WithBootstrapMode(t *testing.T) {
	tempDir := t.TempDir()

	// Create BOOTSTRAP.md
	bootstrapPath := filepath.Join(tempDir, "BOOTSTRAP.md")
	bootstrapContent := "# Bootstrap Instructions\nPlease configure me."
	err := os.WriteFile(bootstrapPath, []byte(bootstrapContent), 0644)
	if err != nil {
		t.Fatalf("Failed to create bootstrap file: %v", err)
	}

	builder := NewContextBuilder(tempDir)
	content := builder.LoadBootstrapFiles(false)

	if !strings.Contains(content, "BOOTSTRAP.md") {
		t.Error("Expected content to contain BOOTSTRAP.md")
	}

	if !strings.Contains(content, "初始化引导模式") {
		t.Error("Expected bootstrap mode indicator")
	}

	if !strings.Contains(content, "complete_bootstrap") {
		t.Error("Expected complete_bootstrap tool instruction")
	}
}

// TestContextBuilder_SetToolsRegistry tests setting tools registry
func TestContextBuilder_SetToolsRegistry(t *testing.T) {
	tempDir := t.TempDir()
	builder := NewContextBuilder(tempDir)

	// Create a real tool registry
	registry := tools.NewToolRegistry()
	builder.SetToolsRegistry(registry)

	// Should not panic
}
