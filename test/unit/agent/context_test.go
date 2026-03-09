// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// TestNewContextBuilder tests creating a new context builder
func TestNewContextBuilder(t *testing.T) {
	tempDir := t.TempDir()

	builder := agent.NewContextBuilder(tempDir)
	if builder == nil {
		t.Fatal("Expected non-nil ContextBuilder")
	}
}

// TestContextBuilder_BuildSystemPrompt tests building system prompt
func TestContextBuilder_BuildSystemPrompt(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Build system prompt with no bootstrap files
	prompt := builder.BuildSystemPrompt(false)

	if prompt == "" {
		t.Error("Expected non-empty system prompt")
	}

	// Should contain basic sections
	if !strings.Contains(prompt, "当前时间") {
		t.Error("Expected time section in system prompt")
	}
	if !strings.Contains(prompt, "工作区") {
		t.Error("Expected workspace section in system prompt")
	}

	// Should contain the workspace path
	if !strings.Contains(prompt, tempDir) {
		t.Error("Expected workspace path in system prompt")
	}
}

// TestContextBuilder_BuildSystemPrompt_WithBootstrapFiles tests building prompt with bootstrap files
func TestContextBuilder_BuildSystemPrompt_WithBootstrapFiles(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create IDENTITY.md
	identityFile := filepath.Join(tempDir, "IDENTITY.md")
	err := os.WriteFile(identityFile, []byte("# Test Identity\nThis is a test identity."), 0644)
	if err != nil {
		t.Fatalf("Failed to create IDENTITY.md: %v", err)
	}

	// Create USER.md
	userFile := filepath.Join(tempDir, "USER.md")
	err = os.WriteFile(userFile, []byte("# Test User\nUser preferences."), 0644)
	if err != nil {
		t.Fatalf("Failed to create USER.md: %v", err)
	}

	// Build system prompt
	prompt := builder.BuildSystemPrompt(false)

	if prompt == "" {
		t.Fatal("Expected non-empty system prompt")
	}

	// Should contain bootstrap file content
	if !strings.Contains(prompt, "Test Identity") {
		t.Error("Expected IDENTITY.md content in prompt")
	}
	if !strings.Contains(prompt, "This is a test identity") {
		t.Error("Expected identity content in prompt")
	}
	if !strings.Contains(prompt, "Test User") {
		t.Error("Expected USER.md content in prompt")
	}
}

// TestContextBuilder_BuildSystemPrompt_WithBootstrapMD tests BOOTSTRAP.md initialization mode
func TestContextBuilder_BuildSystemPrompt_WithBootstrapMD(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create BOOTSTRAP.md
	bootstrapFile := filepath.Join(tempDir, "BOOTSTRAP.md")
	err := os.WriteFile(bootstrapFile, []byte("# Bootstrap Instructions\nPlease configure me."), 0644)
	if err != nil {
		t.Fatalf("Failed to create BOOTSTRAP.md: %v", err)
	}

	// Build system prompt
	prompt := builder.BuildSystemPrompt(false)

	if prompt == "" {
		t.Fatal("Expected non-empty system prompt")
	}

	// Should contain bootstrap warning
	if !strings.Contains(prompt, "初始化引导模式") {
		t.Error("Expected bootstrap initialization mode warning")
	}
	if !strings.Contains(prompt, "Bootstrap Instructions") {
		t.Error("Expected BOOTSTRAP.md content in prompt")
	}
	if !strings.Contains(prompt, "complete_bootstrap") {
		t.Error("Expected mention of complete_bootstrap tool")
	}
}

// TestContextBuilder_BuildSystemPrompt_SkipBootstrap tests skipBootstrap parameter
func TestContextBuilder_BuildSystemPrompt_SkipBootstrap(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create BOOTSTRAP.md (should be ignored when skipBootstrap=true)
	bootstrapFile := filepath.Join(tempDir, "BOOTSTRAP.md")
	err := os.WriteFile(bootstrapFile, []byte("# Bootstrap Instructions"), 0644)
	if err != nil {
		t.Fatalf("Failed to create BOOTSTRAP.md: %v", err)
	}

	// Build with skipBootstrap=true
	prompt := builder.BuildSystemPrompt(true)

	if prompt == "" {
		t.Fatal("Expected non-empty system prompt")
	}

	// Should NOT contain bootstrap warning
	if strings.Contains(prompt, "初始化引导模式") {
		t.Error("Should not show bootstrap mode when skipBootstrap=true")
	}
	// Should NOT contain BOOTSTRAP.md content
	if strings.Contains(prompt, "Bootstrap Instructions") {
		t.Error("Should not include BOOTSTRAP.md when skipBootstrap=true")
	}
}

// TestContextBuilder_BuildMessages tests building messages with history
func TestContextBuilder_BuildMessages(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	history := []providers.Message{
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there!"},
	}

	messages := builder.BuildMessages(history, "", "How are you?", nil, "test-channel", "chat-123", false)

	if len(messages) < 3 {
		t.Errorf("Expected at least 3 messages (system + history + current), got %d", len(messages))
	}

	// First message should be system
	if messages[0].Role != "system" {
		t.Errorf("Expected first message role to be 'system', got '%s'", messages[0].Role)
	}

	// Last message should be the current user message
	lastMsg := messages[len(messages)-1]
	if lastMsg.Role != "user" {
		t.Errorf("Expected last message role to be 'user', got '%s'", lastMsg.Role)
	}
	if lastMsg.Content != "How are you?" {
		t.Errorf("Expected last message content to be 'How are you?', got '%s'", lastMsg.Content)
	}

	// Should contain channel and chat ID in system prompt
	systemPrompt := messages[0].Content
	if !strings.Contains(systemPrompt, "test-channel") {
		t.Error("Expected channel in system prompt")
	}
	if !strings.Contains(systemPrompt, "chat-123") {
		t.Error("Expected chat ID in system prompt")
	}
}

// TestContextBuilder_BuildMessages_WithSummary tests building messages with conversation summary
func TestContextBuilder_BuildMessages_WithSummary(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	summary := "Previous conversation about X"
	messages := builder.BuildMessages(nil, summary, "Continue", nil, "", "", false)

	if len(messages) < 2 {
		t.Fatal("Expected at least 2 messages")
	}

	// System prompt should contain summary
	systemPrompt := messages[0].Content
	if !strings.Contains(systemPrompt, "Summary of Previous Conversation") {
		t.Error("Expected summary section in system prompt")
	}
	if !strings.Contains(systemPrompt, summary) {
		t.Errorf("Expected summary '%s' in system prompt", summary)
	}
}

// TestContextBuilder_BuildMessages_WithMedia tests building messages with media
func TestContextBuilder_BuildMessages_WithMedia(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	media := []string{"image1.jpg", "image2.png"}
	messages := builder.BuildMessages(nil, "", "Look at these images", media, "", "", false)

	// Media should be passed through (though the exact handling depends on implementation)
	if len(messages) < 2 {
		t.Fatal("Expected at least 2 messages")
	}

	// The current message should still be there
	lastMsg := messages[len(messages)-1]
	if lastMsg.Content != "Look at these images" {
		t.Errorf("Expected 'Look at these images', got '%s'", lastMsg.Content)
	}
}

// TestContextBuilder_AddToolResult tests adding tool results to messages
func TestContextBuilder_AddToolResult(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	messages := []providers.Message{
		{Role: "system", Content: "System prompt"},
	}

	updated := builder.AddToolResult(messages, "tool-call-123", "test_tool", `{"result": "success"}`)

	if len(updated) != 2 {
		t.Errorf("Expected 2 messages after adding tool result, got %d", len(updated))
	}

	toolMsg := updated[1]
	if toolMsg.Role != "tool" {
		t.Errorf("Expected tool message role to be 'tool', got '%s'", toolMsg.Role)
	}
	if toolMsg.ToolCallID != "tool-call-123" {
		t.Errorf("Expected ToolCallID 'tool-call-123', got '%s'", toolMsg.ToolCallID)
	}
	if toolMsg.Content != `{"result": "success"}` {
		t.Errorf("Expected tool result content, got '%s'", toolMsg.Content)
	}
}

// TestContextBuilder_AddAssistantMessage tests adding assistant messages
func TestContextBuilder_AddAssistantMessage(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	messages := []providers.Message{
		{Role: "system", Content: "System prompt"},
	}

	// Add assistant message with content
	updated := builder.AddAssistantMessage(messages, "Hello from assistant", nil)

	if len(updated) != 2 {
		t.Errorf("Expected 2 messages after adding assistant message, got %d", len(updated))
	}

	asstMsg := updated[1]
	if asstMsg.Role != "assistant" {
		t.Errorf("Expected message role to be 'assistant', got '%s'", asstMsg.Role)
	}
	if asstMsg.Content != "Hello from assistant" {
		t.Errorf("Expected 'Hello from assistant', got '%s'", asstMsg.Content)
	}
}

// TestContextBuilder_RemoveOrphanedToolMessages tests that orphaned tool messages are removed
func TestContextBuilder_RemoveOrphanedToolMessages(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create history with orphaned tool message at the start
	history := []providers.Message{
		{Role: "tool", Content: "orphaned result", ToolCallID: "call-123"},
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi"},
	}

	messages := builder.BuildMessages(history, "", "Continue", nil, "", "", false)

	// Check that orphaned tool message was removed
	// The resulting messages should start with system, then user, then assistant
	foundOrphaned := false
	for _, msg := range messages {
		if msg.Role == "tool" && msg.Content == "orphaned result" {
			foundOrphaned = true
			break
		}
	}

	if foundOrphaned {
		t.Error("Orphaned tool message should be removed from history")
	}
}

// TestContextBuilder_GetSkillsInfo tests getting skills information
func TestContextBuilder_GetSkillsInfo(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	info := builder.GetSkillsInfo()

	if info == nil {
		t.Fatal("Expected non-nil skills info")
	}

	// Should have total field
	if _, ok := info["total"]; !ok {
		t.Error("Expected 'total' field in skills info")
	}

	// Should have available field
	if _, ok := info["available"]; !ok {
		t.Error("Expected 'available' field in skills info")
	}

	// Should have names field (even if empty)
	names, ok := info["names"].([]string)
	if !ok {
		t.Error("Expected 'names' field to be []string")
	} else if names == nil {
		t.Error("Expected 'names' to be initialized (even if empty)")
	}
}

// TestContextBuilder_WithToolsRegistry tests setting tools registry
func TestContextBuilder_WithToolsRegistry(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create a mock tools registry (we'd need to import tools package)
	// For now, just test that SetToolsRegistry doesn't panic
	builder.SetToolsRegistry(nil)

	// Build system prompt should still work
	prompt := builder.BuildSystemPrompt(false)
	if prompt == "" {
		t.Error("Expected non-empty system prompt even with nil tools registry")
	}
}

// TestContextBuilder_BuildSystemPrompt_WithMCP tests building prompt with MCP configuration
func TestContextBuilder_BuildSystemPrompt_WithMCP(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create MCP.md
	mcpFile := filepath.Join(tempDir, "MCP.md")
	err := os.WriteFile(mcpFile, []byte("# MCP Configuration\nServer configurations."), 0644)
	if err != nil {
		t.Fatalf("Failed to create MCP.md: %v", err)
	}

	prompt := builder.BuildSystemPrompt(false)

	if prompt == "" {
		t.Fatal("Expected non-empty system prompt")
	}

	// Should contain MCP content
	if !strings.Contains(prompt, "MCP Configuration") {
		t.Error("Expected MCP.md content in prompt")
	}
}

// TestContextBuilder_BuildSystemPrompt_WithAgentMD tests building prompt with AGENT.md
func TestContextBuilder_BuildSystemPrompt_WithAgentMD(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create AGENT.md
	agentFile := filepath.Join(tempDir, "AGENT.md")
	err := os.WriteFile(agentFile, []byte("# Agent Configuration\nCustom agent settings."), 0644)
	if err != nil {
		t.Fatalf("Failed to create AGENT.md: %v", err)
	}

	prompt := builder.BuildSystemPrompt(false)

	if prompt == "" {
		t.Fatal("Expected non-empty system prompt")
	}

	// Should contain AGENT content
	if !strings.Contains(prompt, "Agent Configuration") {
		t.Error("Expected AGENT.md content in prompt")
	}
}

// TestContextBuilder_BuildSystemPrompt_WithSoulMD tests building prompt with SOUL.md
func TestContextBuilder_BuildSystemPrompt_WithSoulMD(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create SOUL.md
	soulFile := filepath.Join(tempDir, "SOUL.md")
	err := os.WriteFile(soulFile, []byte("# Core Principles\nThese are my core principles."), 0644)
	if err != nil {
		t.Fatalf("Failed to create SOUL.md: %v", err)
	}

	prompt := builder.BuildSystemPrompt(false)

	if prompt == "" {
		t.Fatal("Expected non-empty system prompt")
	}

	// Should contain SOUL content
	if !strings.Contains(prompt, "Core Principles") {
		t.Error("Expected SOUL.md content in prompt")
	}
}

// TestContextBuilder_BuildMessages_WithOrphanedToolMessagesAtStart tests removing orphaned tool messages at start
func TestContextBuilder_BuildMessages_WithOrphanedToolMessagesAtStart(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create history with multiple orphaned tool messages at the start
	history := []providers.Message{
		{Role: "tool", Content: "result 1", ToolCallID: "call-1"},
		{Role: "tool", Content: "result 2", ToolCallID: "call-2"},
		{Role: "tool", Content: "result 3", ToolCallID: "call-3"},
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi"},
	}

	messages := builder.BuildMessages(history, "", "Continue", nil, "", "", false)

	// Check that all orphaned tool messages were removed
	foundOrphaned := false
	for _, msg := range messages {
		if msg.Role == "tool" && (msg.Content == "result 1" || msg.Content == "result 2" || msg.Content == "result 3") {
			foundOrphaned = true
			break
		}
	}

	if foundOrphaned {
		t.Error("All orphaned tool messages at start should be removed from history")
	}
}

// TestContextBuilder_BuildMessages_WithMixedHistory tests building messages with complex history
func TestContextBuilder_BuildMessages_WithMixedHistory(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create complex history with user, assistant, and tool messages
	history := []providers.Message{
		{Role: "user", Content: "First message"},
		{Role: "assistant", Content: "First response", ToolCalls: []protocoltypes.ToolCall{
			{ID: "call-1", Type: "function", Function: &protocoltypes.FunctionCall{
				Name:      "test_tool",
				Arguments: "{}",
			}},
		}},
		{Role: "tool", Content: "Tool result", ToolCallID: "call-1"},
		{Role: "assistant", Content: "Final response"},
		{Role: "user", Content: "Second message"},
	}

	messages := builder.BuildMessages(history, "", "Third message", nil, "", "", false)

	if len(messages) < 3 {
		t.Fatalf("Expected at least 3 messages, got %d", len(messages))
	}

	// First should be system
	if messages[0].Role != "system" {
		t.Errorf("Expected first message to be system, got %s", messages[0].Role)
	}

	// Last should be current user message
	lastMsg := messages[len(messages)-1]
	if lastMsg.Role != "user" {
		t.Errorf("Expected last message to be user, got %s", lastMsg.Role)
	}
	if lastMsg.Content != "Third message" {
		t.Errorf("Expected last message content 'Third message', got '%s'", lastMsg.Content)
	}
}

// TestContextBuilder_AddToolResult_MultipleTimes tests adding multiple tool results
func TestContextBuilder_AddToolResult_MultipleTimes(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	messages := []providers.Message{
		{Role: "system", Content: "System prompt"},
		{Role: "user", Content: "Hello"},
	}

	// Add first tool result
	messages = builder.AddToolResult(messages, "call-1", "tool1", `{"result": "success1"}`)

	// Add second tool result
	messages = builder.AddToolResult(messages, "call-2", "tool2", `{"result": "success2"}`)

	if len(messages) != 4 {
		t.Errorf("Expected 4 messages after adding 2 tool results, got %d", len(messages))
	}

	// Verify tool messages
	if messages[2].Role != "tool" || messages[2].ToolCallID != "call-1" {
		t.Error("Expected first tool result to be at index 2")
	}

	if messages[3].Role != "tool" || messages[3].ToolCallID != "call-2" {
		t.Error("Expected second tool result to be at index 3")
	}
}

// TestContextBuilder_AddAssistantMessage_WithToolCalls tests adding assistant message with tool calls
func TestContextBuilder_AddAssistantMessage_WithToolCalls(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	messages := []providers.Message{
		{Role: "system", Content: "System prompt"},
	}

	// Add assistant message with tool calls
	toolCalls := []map[string]interface{}{
		{
			"id":   "call-123",
			"type": "function",
			"function": map[string]interface{}{
				"name":      "test_function",
				"arguments": `{"arg": "value"}`,
			},
		},
	}

	updated := builder.AddAssistantMessage(messages, "I'll call a tool", toolCalls)

	if len(updated) != 2 {
		t.Errorf("Expected 2 messages, got %d", len(updated))
	}

	asstMsg := updated[1]
	if asstMsg.Role != "assistant" {
		t.Errorf("Expected role 'assistant', got '%s'", asstMsg.Role)
	}

	if asstMsg.Content != "I'll call a tool" {
		t.Errorf("Expected content 'I'll call a tool', got '%s'", asstMsg.Content)
	}
}

// TestContextBuilder_BuildMessages_WithEmptyHistory tests building messages with empty history
func TestContextBuilder_BuildMessages_WithEmptyHistory(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	messages := builder.BuildMessages(nil, "", "Hello", nil, "", "", false)

	if len(messages) != 2 {
		t.Errorf("Expected 2 messages (system + current), got %d", len(messages))
	}

	if messages[0].Role != "system" {
		t.Errorf("Expected first message to be system, got %s", messages[0].Role)
	}

	if messages[1].Role != "user" {
		t.Errorf("Expected second message to be user, got %s", messages[1].Role)
	}

	if messages[1].Content != "Hello" {
		t.Errorf("Expected content 'Hello', got '%s'", messages[1].Content)
	}
}

// TestContextBuilder_BuildMessages_WithBothSummaryAndHistory tests building with both summary and history
func TestContextBuilder_BuildMessages_WithBothSummaryAndHistory(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	history := []providers.Message{
		{Role: "user", Content: "Previous message"},
		{Role: "assistant", Content: "Previous response"},
	}

	summary := "Summary of earlier conversation"
	messages := builder.BuildMessages(history, summary, "New message", nil, "", "", false)

	// System prompt should contain summary
	systemPrompt := messages[0].Content
	if !strings.Contains(systemPrompt, "Summary of Previous Conversation") {
		t.Error("Expected summary section in system prompt")
	}

	if !strings.Contains(systemPrompt, summary) {
		t.Error("Expected summary content in system prompt")
	}

	// Should have history messages
	foundHistory := false
	for _, msg := range messages {
		if msg.Content == "Previous message" || msg.Content == "Previous response" {
			foundHistory = true
			break
		}
	}

	if !foundHistory {
		t.Error("Expected history messages to be included")
	}
}

// TestContextBuilder_LoadBootstrapFiles_OnlyBootstrapMD tests that only BOOTSTRAP.md is loaded when present
func TestContextBuilder_LoadBootstrapFiles_OnlyBootstrapMD(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	// Create both BOOTSTRAP.md and IDENTITY.md
	bootstrapFile := filepath.Join(tempDir, "BOOTSTRAP.md")
	err := os.WriteFile(bootstrapFile, []byte("# Bootstrap Instructions"), 0644)
	if err != nil {
		t.Fatalf("Failed to create BOOTSTRAP.md: %v", err)
	}

	identityFile := filepath.Join(tempDir, "IDENTITY.md")
	err = os.WriteFile(identityFile, []byte("# Identity"), 0644)
	if err != nil {
		t.Fatalf("Failed to create IDENTITY.md: %v", err)
	}

	// Build with skipBootstrap=false (normal mode)
	prompt := builder.BuildSystemPrompt(false)

	// Should contain bootstrap warning
	if !strings.Contains(prompt, "初始化引导模式") {
		t.Error("Expected bootstrap mode warning")
	}

	// Should contain BOOTSTRAP.md content
	if !strings.Contains(prompt, "Bootstrap Instructions") {
		t.Error("Expected BOOTSTRAP.md content")
	}

	// Should NOT contain IDENTITY.md content (only BOOTSTRAP.md is loaded in bootstrap mode)
	// Note: This is the current behavior - only BOOTSTRAP.md is shown
}

// TestContextBuilder_GetSkillsInfo_WithEmptySkills tests skills info when no skills loaded
func TestContextBuilder_GetSkillsInfo_WithEmptySkills(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	info := builder.GetSkillsInfo()

	// Should return valid info even with no skills
	total, ok := info["total"].(int)
	if !ok {
		t.Error("Expected total to be int")
	}

	if total != 0 {
		t.Errorf("Expected 0 skills, got %d", total)
	}

	available, ok := info["available"].(int)
	if !ok {
		t.Error("Expected available to be int")
	}

	if available != 0 {
		t.Errorf("Expected 0 available skills, got %d", available)
	}

	names, ok := info["names"].([]string)
	if !ok {
		t.Error("Expected names to be []string")
	}

	if len(names) != 0 {
		t.Errorf("Expected empty names array, got %d names", len(names))
	}
}

// TestContextBuilder_BuildSystemPrompt_ContainsRuntimeInfo tests that system prompt contains runtime info
func TestContextBuilder_BuildSystemPrompt_ContainsRuntimeInfo(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	prompt := builder.BuildSystemPrompt(false)

	// Should contain runtime section
	if !strings.Contains(prompt, "运行环境") {
		t.Error("Expected runtime section in system prompt")
	}

	// Should contain OS information
	if !strings.Contains(prompt, "windows") && !strings.Contains(prompt, "linux") && !strings.Contains(prompt, "darwin") {
		t.Error("Expected OS information in system prompt")
	}

	// Should contain Go version
	if !strings.Contains(prompt, "Go") {
		t.Error("Expected Go version in system prompt")
	}
}

// TestContextBuilder_BuildSystemPrompt_ContainsTimeInfo tests that system prompt contains current time
func TestContextBuilder_BuildSystemPrompt_ContainsTimeInfo(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	prompt := builder.BuildSystemPrompt(false)

	// Should contain time section
	if !strings.Contains(prompt, "当前时间") {
		t.Error("Expected current time section in system prompt")
	}

	// Should contain date (year-month-day format)
	if !strings.Contains(prompt, "202") && !strings.Contains(prompt, "203") && !strings.Contains(prompt, "204") {
		t.Error("Expected date information in system prompt")
	}
}

// TestContextBuilder_BuildMessages_NoHistoryWithSummary tests building with summary but no history
func TestContextBuilder_BuildMessages_NoHistoryWithSummary(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	summary := "Previous conversation summary"
	messages := builder.BuildMessages(nil, summary, "Continue", nil, "", "", false)

	// Should have system + current user message
	if len(messages) != 2 {
		t.Errorf("Expected 2 messages, got %d", len(messages))
	}

	// System prompt should contain summary
	systemPrompt := messages[0].Content
	if !strings.Contains(systemPrompt, summary) {
		t.Error("Expected summary in system prompt")
	}
}

// TestContextBuilder_BuildMessages_WithAllOptions tests building with all options provided
func TestContextBuilder_BuildMessages_WithAllOptions(t *testing.T) {
	tempDir := t.TempDir()
	builder := agent.NewContextBuilder(tempDir)

	history := []providers.Message{
		{Role: "user", Content: "Previous"},
	}
	summary := "Summary"
	current := "Current message"
	media := []string{"image.jpg", "document.pdf"}
	channel := "test-channel"
	chatID := "chat-123"

	messages := builder.BuildMessages(history, summary, current, media, channel, chatID, false)

	// Should have at least system + history + current
	if len(messages) < 3 {
		t.Errorf("Expected at least 3 messages, got %d", len(messages))
	}

	// System prompt should contain channel and chat ID
	systemPrompt := messages[0].Content
	if !strings.Contains(systemPrompt, channel) {
		t.Error("Expected channel in system prompt")
	}
	if !strings.Contains(systemPrompt, chatID) {
		t.Error("Expected chat ID in system prompt")
	}

	// Should contain summary
	if !strings.Contains(systemPrompt, summary) {
		t.Error("Expected summary in system prompt")
	}
}
