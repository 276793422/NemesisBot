// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package agent

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/providers"
)

const (
	// DetailLevelFull means log everything
	DetailLevelFull = "full"
	// DetailLevelTruncated means truncate long content
	DetailLevelTruncated = "truncated"

	// Truncate limits for truncated mode
	truncateMessageLimit  = 200
	truncateResponseLimit = 500
	truncateArgsLimit     = 200
)

// RequestLogger handles logging of LLM requests and responses
type RequestLogger struct {
	cfg        *config.LoggingConfig
	baseDir    string
	sessionDir string
	fileIndex  int
	enabled    bool
	startTime  time.Time
	mu         *fileWriteMutex
}

// fileWriteMutex ensures thread-safe file writing
type fileWriteMutex struct {
	mu chan struct{}
}

func newFileWriteMutex() *fileWriteMutex {
	return &fileWriteMutex{mu: make(chan struct{}, 1)}
}

func (m *fileWriteMutex) lock() {
	m.mu <- struct{}{}
}

func (m *fileWriteMutex) unlock() {
	<-m.mu
}

// UserRequestInfo holds information about the user's request
type UserRequestInfo struct {
	Timestamp time.Time
	Channel   string
	SenderID  string
	ChatID    string
	Content   string
}

// ProviderMetadata holds provider configuration metadata for logging
type ProviderMetadata struct {
	Name    string // Provider name (e.g., "zhipu", "openai")
	APIKey  string // Masked API key
	APIBase string // API base URL
}

// FallbackAttemptInfo holds information about a single fallback attempt
type FallbackAttemptInfo struct {
	ProviderName string
	ModelName    string
	APIKey       string // Masked
	APIBase      string
	ErrorMessage string
	Duration     time.Duration
}

// LLMRequestInfo holds information about an LLM request
type LLMRequestInfo struct {
	Round     int
	Timestamp time.Time
	Model     string

	// New fields for comprehensive logging
	ProviderName     string                 // Provider name (e.g., "zhipu", "openai")
	APIKey           string                 // Masked API key
	APIBase          string                 // API base URL
	HTTPHeaders      map[string]string      // HTTP headers (excluding Authorization)
	FullConfig       map[string]interface{} // Complete request configuration
	FallbackAttempts []FallbackAttemptInfo  // All attempted providers in fallback chain

	// Legacy fields kept for backward compatibility
	Messages []providers.Message
	Tools    []providers.ToolDefinition
}

// LLMResponseInfo holds information about an LLM response
type LLMResponseInfo struct {
	Round        int
	Timestamp    time.Time
	Duration     time.Duration
	Content      string
	ToolCalls    []providers.ToolCall
	Usage        *providers.UsageInfo
	FinishReason string
}

// LocalOperationInfo holds information about local operations
type LocalOperationInfo struct {
	Round      int
	Timestamp  time.Time
	Operations []Operation
}

// Operation represents a single local operation
type Operation struct {
	Type      string // "tool_call", "file_write", "file_read", etc.
	Name      string
	Arguments map[string]interface{}
	Result    interface{}
	Status    string // "Success" or "Failed"
	Error     string
	Duration  time.Duration
}

// FinalResponseInfo holds information about the final response
type FinalResponseInfo struct {
	Timestamp     time.Time
	TotalDuration time.Duration
	LLMRounds     int
	Content       string
	Channel       string
	ChatID        string
}

// NewRequestLogger creates a new request logger
func NewRequestLogger(cfg *config.LoggingConfig, workspace string) *RequestLogger {
	if cfg == nil || !cfg.LLMRequests {
		return &RequestLogger{enabled: false}
	}

	// Resolve log directory relative to workspace
	logDir := resolveLogPath(cfg.LogDir, workspace)

	return &RequestLogger{
		cfg:       cfg,
		baseDir:   logDir,
		fileIndex: 0,
		enabled:   true,
		startTime: time.Now(),
		mu:        newFileWriteMutex(),
	}
}

// resolveLogPath resolves log directory path
// - If logDir is absolute (including ~), use it as-is
// - If logDir is relative, join it with workspace
// - Expands ~ in the final path
func resolveLogPath(logDir, workspace string) string {
	var basePath string

	// Check if logDir is an absolute path (including ~)
	// Note: On Windows, filepath.IsAbs returns false for Unix-style paths like "/var/log"
	// So we also explicitly check for paths starting with / or \
	isUnixStyleAbs := len(logDir) > 0 && (logDir[0] == '/' || logDir[0] == '\\')
	if filepath.IsAbs(logDir) || strings.HasPrefix(logDir, "~") || isUnixStyleAbs {
		// Absolute path - use directly
		basePath = logDir
	} else {
		// Relative path - join with workspace
		basePath = filepath.Join(workspace, logDir)
	}

	// Expand ~
	if strings.HasPrefix(basePath, "~") {
		home, _ := os.UserHomeDir()
		if len(basePath) > 1 && (basePath[1] == '/' || basePath[1] == '\\') {
			basePath = filepath.Join(home, basePath[2:])
		} else {
			basePath = home
		}
	}

	return basePath
}

// IsEnabled returns whether the logger is enabled
func (rl *RequestLogger) IsEnabled() bool {
	return rl != nil && rl.enabled
}

// CreateSession creates a new logging session directory
func (rl *RequestLogger) CreateSession() error {
	if !rl.enabled {
		return nil
	}

	// Create base directory
	if err := os.MkdirAll(rl.baseDir, 0700); err != nil {
		logger.WarnC("request_logger", fmt.Sprintf("Failed to create log directory: %v", err))
		rl.enabled = false
		return nil // Silent failure
	}

	// Create timestamp directory with random suffix to avoid conflicts
	timestamp := time.Now().Format("2006-01-02_15-04-05")
	suffix := fmt.Sprintf("_%x", time.Now().UnixNano()%0xffffff)
	rl.sessionDir = filepath.Join(rl.baseDir, timestamp+suffix)

	if err := os.MkdirAll(rl.sessionDir, 0700); err != nil {
		logger.WarnC("request_logger", fmt.Sprintf("Failed to create session directory: %v", err))
		rl.enabled = false
		return nil // Silent failure
	}

	return nil
}

// NextIndex returns the next file index
func (rl *RequestLogger) NextIndex() string {
	if !rl.enabled {
		return ""
	}
	rl.fileIndex++
	return fmt.Sprintf("%02d", rl.fileIndex)
}

// LogUserRequest logs the initial user request
func (rl *RequestLogger) LogUserRequest(info UserRequestInfo) error {
	if !rl.enabled {
		return nil
	}

	index := rl.NextIndex()
	filename := fmt.Sprintf("%s.request.md", index)
	content := fmt.Sprintf(`# User Request

**Timestamp**: %s
**Channel**: %s
**Sender ID**: %s
**Chat ID**: %s

## Message

%s
`,
		info.Timestamp.Format(time.RFC3339),
		info.Channel,
		info.SenderID,
		info.ChatID,
		info.Content,
	)

	return rl.writeFile(filename, content)
}

// LogLLMRequest logs an LLM request
func (rl *RequestLogger) LogLLMRequest(info LLMRequestInfo) error {
	if !rl.enabled {
		return nil
	}

	index := rl.NextIndex()
	filename := fmt.Sprintf("%s.AI.Request.md", index)

	var builder strings.Builder

	builder.WriteString(fmt.Sprintf("# LLM Request\n\n"))
	builder.WriteString(fmt.Sprintf("**Timestamp**: %s\n", info.Timestamp.Format(time.RFC3339)))
	builder.WriteString(fmt.Sprintf("**Round**: %d\n\n", info.Round))

	// Provider Information Section
	builder.WriteString("## Provider\n\n")
	if info.ProviderName != "" {
		builder.WriteString(fmt.Sprintf("- **Provider**: %s\n", info.ProviderName))
	}
	builder.WriteString(fmt.Sprintf("- **Model**: %s\n", info.Model))
	if info.APIBase != "" {
		builder.WriteString(fmt.Sprintf("- **API Base**: %s\n", info.APIBase))
	}
	if info.APIKey != "" {
		builder.WriteString(fmt.Sprintf("- **API Key**: %s\n", info.APIKey))
	}
	builder.WriteString("\n")

	// HTTP Headers Section
	if len(info.HTTPHeaders) > 0 {
		builder.WriteString("## HTTP Headers\n\n")
		builder.WriteString(formatHeaders(info.HTTPHeaders))
		builder.WriteString("\n\n")
	}

	// Full Configuration Section
	builder.WriteString("## Configuration\n\n")
	if info.FullConfig != nil {
		for key, value := range info.FullConfig {
			builder.WriteString(fmt.Sprintf("- **%s**: %v\n", key, value))
		}
	}
	builder.WriteString("\n")

	// Fallback Attempts Section (if any)
	if len(info.FallbackAttempts) > 0 {
		builder.WriteString(fmt.Sprintf("## Fallback Attempts (%d attempts)\n\n", len(info.FallbackAttempts)))
		for i, attempt := range info.FallbackAttempts {
			builder.WriteString(fmt.Sprintf("### Attempt %d: %s/%s\n", i+1, attempt.ProviderName, attempt.ModelName))
			builder.WriteString(fmt.Sprintf("- **API Base**: %s\n", attempt.APIBase))
			builder.WriteString(fmt.Sprintf("- **API Key**: %s\n", attempt.APIKey))
			builder.WriteString(fmt.Sprintf("- **Duration**: %.1fs\n", attempt.Duration.Seconds()))
			if attempt.ErrorMessage != "" {
				builder.WriteString(fmt.Sprintf("- **Error**: %s\n", attempt.ErrorMessage))
			}
			builder.WriteString("\n")
		}
	}

	// Messages Section
	builder.WriteString(fmt.Sprintf("## Messages (%d messages)\n\n", len(info.Messages)))
	for i, msg := range info.Messages {
		builder.WriteString(fmt.Sprintf("### Message %d: %s\n", i, msg.Role))

		if len(msg.ToolCalls) > 0 {
			builder.WriteString("Tool Calls:\n")
			for _, tc := range msg.ToolCalls {
				arguments := formatArguments(tc.Arguments, rl.cfg.DetailLevel)
				builder.WriteString(fmt.Sprintf("- ID: %s, Type: %s, Name: %s\n", tc.ID, tc.Type, tc.Name))
				builder.WriteString(fmt.Sprintf("  Arguments: %s\n", arguments))
			}
		}

		if msg.Content != "" {
			content := msg.Content
			if rl.cfg.DetailLevel == DetailLevelTruncated && len(content) > truncateMessageLimit {
				content = content[:truncateMessageLimit] + "\n\n... [truncated]"
			}
			builder.WriteString(fmt.Sprintf("\n%s\n\n", content))
		}
	}

	// Tools Section
	if len(info.Tools) > 0 {
		builder.WriteString(fmt.Sprintf("## Tools Available (%d tools)\n\n", len(info.Tools)))
		for _, tool := range info.Tools {
			if tool.Type != "function" {
				continue
			}
			builder.WriteString(fmt.Sprintf("### %s\n", tool.Function.Name))
			if tool.Function.Description != "" {
				builder.WriteString(fmt.Sprintf("**Description**: %s\n", tool.Function.Description))
			}
			if len(tool.Function.Parameters) > 0 {
				paramsJSON, _ := json.Marshal(tool.Function.Parameters)
				paramsStr := string(paramsJSON)
				if rl.cfg.DetailLevel == DetailLevelTruncated && len(paramsStr) > truncateArgsLimit {
					paramsStr = paramsStr[:truncateArgsLimit] + "... [truncated]"
				}
				builder.WriteString(fmt.Sprintf("**Parameters**:\n```json\n%s\n```\n", paramsStr))
			}
			builder.WriteString("\n")
		}
	}

	return rl.writeFile(filename, builder.String())
}

// LogLLMResponse logs an LLM response
func (rl *RequestLogger) LogLLMResponse(info LLMResponseInfo) error {
	if !rl.enabled {
		return nil
	}

	index := rl.NextIndex()
	filename := fmt.Sprintf("%s.AI.Response.md", index)

	var builder strings.Builder

	builder.WriteString(fmt.Sprintf("# LLM Response\n\n"))
	builder.WriteString(fmt.Sprintf("**Timestamp**: %s\n", info.Timestamp.Format(time.RFC3339)))
	builder.WriteString(fmt.Sprintf("**Round**: %d\n", info.Round))
	builder.WriteString(fmt.Sprintf("**Duration**: %.1fs\n\n", info.Duration.Seconds()))

	// Response Content
	if info.Content != "" {
		content := info.Content
		if rl.cfg.DetailLevel == DetailLevelTruncated && len(content) > truncateResponseLimit {
			content = content[:truncateResponseLimit] + "\n\n... [truncated]"
		}
		builder.WriteString(fmt.Sprintf("## Response Content\n\n%s\n\n", content))
	}

	// Tool Calls
	if len(info.ToolCalls) > 0 {
		builder.WriteString(fmt.Sprintf("## Tool Calls (%d tool call)\n\n", len(info.ToolCalls)))
		for i, tc := range info.ToolCalls {
			arguments := formatArguments(tc.Arguments, rl.cfg.DetailLevel)
			builder.WriteString(fmt.Sprintf("### Tool Call %d: %s\n", i+1, tc.Name))
			builder.WriteString(fmt.Sprintf("**ID**: %s\n", tc.ID))
			builder.WriteString(fmt.Sprintf("**Arguments**:\n```json\n%s\n```\n\n", arguments))
		}
	}

	// Usage
	if info.Usage != nil {
		builder.WriteString("## Usage\n\n")
		builder.WriteString(fmt.Sprintf("- **Prompt Tokens**: %d\n", info.Usage.PromptTokens))
		builder.WriteString(fmt.Sprintf("- **Completion Tokens**: %d\n", info.Usage.CompletionTokens))
		builder.WriteString(fmt.Sprintf("- **Total Tokens**: %d\n\n", info.Usage.TotalTokens))
	}

	// Finish Reason
	builder.WriteString("## Finish Reason\n\n")
	builder.WriteString(fmt.Sprintf("%s\n", info.FinishReason))

	return rl.writeFile(filename, builder.String())
}

// LogLocalOperations logs local operations for a round
func (rl *RequestLogger) LogLocalOperations(info LocalOperationInfo) error {
	if !rl.enabled {
		return nil
	}

	if len(info.Operations) == 0 {
		return nil
	}

	index := rl.NextIndex()
	filename := fmt.Sprintf("%s.Local.md", index)

	var builder strings.Builder

	builder.WriteString(fmt.Sprintf("# Local Operations\n\n"))
	builder.WriteString(fmt.Sprintf("**Timestamp**: %s\n", info.Timestamp.Format(time.RFC3339)))
	builder.WriteString(fmt.Sprintf("**Round**: %d\n", info.Round))
	builder.WriteString(fmt.Sprintf("**Operations Count**: %d\n\n", len(info.Operations)))

	for i, op := range info.Operations {
		builder.WriteString(fmt.Sprintf("## Operation %d: %s\n\n", i+1, formatOperationType(op.Type)))

		if op.Name != "" {
			builder.WriteString(fmt.Sprintf("**%s**: %s\n", getTitleForType(op.Type), op.Name))
		}

		builder.WriteString(fmt.Sprintf("**Status**: %s\n\n", op.Status))

		if op.Arguments != nil {
			argsJSON, _ := json.Marshal(op.Arguments)
			argsStr := string(argsJSON)
			if rl.cfg.DetailLevel == DetailLevelTruncated && len(argsStr) > truncateArgsLimit {
				argsStr = argsStr[:truncateArgsLimit] + "... [truncated]"
			}
			builder.WriteString(fmt.Sprintf("### Arguments\n```json\n%s\n```\n\n", argsStr))
		}

		if op.Result != nil && op.Status == "Success" {
			resultJSON, _ := json.MarshalIndent(op.Result, "", "  ")
			resultStr := string(resultJSON)
			if rl.cfg.DetailLevel == DetailLevelTruncated && len(resultStr) > truncateArgsLimit {
				resultStr = resultStr[:truncateArgsLimit] + "... [truncated]"
			}
			builder.WriteString(fmt.Sprintf("### Result\n```json\n%s\n```\n\n", resultStr))
		}

		if op.Error != "" {
			builder.WriteString(fmt.Sprintf("### Error\n%s\n\n", op.Error))
		}

		if op.Duration > 0 {
			builder.WriteString(fmt.Sprintf("### Duration\n%.3fs\n\n", op.Duration.Seconds()))
		}

		builder.WriteString("---\n\n")
	}

	return rl.writeFile(filename, builder.String())
}

// LogFinalResponse logs the final response to the user
func (rl *RequestLogger) LogFinalResponse(info FinalResponseInfo) error {
	if !rl.enabled {
		return nil
	}

	index := rl.NextIndex()
	filename := fmt.Sprintf("%s.response.md", index)

	content := fmt.Sprintf(`# Agent Response

**Timestamp**: %s
**Total Duration**: %.1fs
**LLM Rounds**: %d

## Response Content

%s

---

**Channel**: %s
**Chat ID**: %s
**Sent At**: %s
`,
		info.Timestamp.Format(time.RFC3339),
		info.TotalDuration.Seconds(),
		info.LLMRounds,
		info.Content,
		info.Channel,
		info.ChatID,
		info.Timestamp.Format(time.RFC3339),
	)

	return rl.writeFile(filename, content)
}

// writeFile writes content to a file in the session directory
func (rl *RequestLogger) writeFile(filename, content string) error {
	if !rl.enabled {
		return nil
	}

	rl.mu.lock()
	defer rl.mu.unlock()

	filePath := filepath.Join(rl.sessionDir, filename)
	if err := os.WriteFile(filePath, []byte(content), 0600); err != nil {
		logger.WarnC("request_logger", fmt.Sprintf("Failed to write log file %s: %v", filename, err))
		// Don't disable logger on single file write failure
		return nil // Silent failure
	}

	return nil
}

// maskAPIKey masks the API key for logging (shows first 3 and last 3 characters)
func maskAPIKey(key string) string {
	if key == "" {
		return "<empty>"
	}

	// Remove leading/trailing whitespace
	key = strings.TrimSpace(key)

	// If key is too short, just show asterisks
	if len(key) <= 6 {
		return "***"
	}

	// Show first 3 and last 3 characters
	return key[:3] + "***" + key[len(key)-3:]
}

// formatHeaders formats HTTP headers for logging
func formatHeaders(headers map[string]string) string {
	if len(headers) == 0 {
		return "<none>"
	}

	var builder strings.Builder
	builder.WriteString("```\n")
	for key, value := range headers {
		builder.WriteString(fmt.Sprintf("%s: %s\n", key, value))
	}
	builder.WriteString("```")
	return builder.String()
}

// formatArguments formats tool arguments for logging
func formatArguments(args map[string]interface{}, detailLevel string) string {
	if args == nil {
		return "{}"
	}

	argsJSON, err := json.Marshal(args)
	if err != nil {
		return fmt.Sprintf("%v", args)
	}

	argsStr := string(argsJSON)
	if detailLevel == DetailLevelTruncated && len(argsStr) > truncateArgsLimit {
		argsStr = argsStr[:truncateArgsLimit] + "... [truncated]"
	}

	return argsStr
}

// formatOperationType formats operation type for display
func formatOperationType(opType string) string {
	switch opType {
	case "tool_call":
		return "Tool Execution"
	case "file_write":
		return "File Write"
	case "file_read":
		return "File Read"
	case "command_exec":
		return "Command Execution"
	default:
		return strings.Title(strings.ReplaceAll(opType, "_", " "))
	}
}

// getTitleForType returns the appropriate title for an operation type
func getTitleForType(opType string) string {
	switch opType {
	case "tool_call":
		return "Tool"
	case "file_write", "file_read":
		return "File"
	case "command_exec":
		return "Command"
	default:
		return "Name"
	}
}

// Close closes the request logger
func (rl *RequestLogger) Close() {
	// Nothing to clean up currently
	// Session directory is already created and files are written
}
