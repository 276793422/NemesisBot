// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/plugin"
	"github.com/276793422/NemesisBot/module/providers"
)

type ToolRegistry struct {
	tools map[string]Tool
	mu    sync.RWMutex
}

// NewToolRegistry creates a new empty tool registry.
// The registry is thread-safe and can be used to manage tool lifecycle.
//
// Returns:
//   A new ToolRegistry instance ready to register and execute tools.
//
// Example:
//
//	registry := NewToolRegistry()
//	registry.Register(NewReadFileTool("/workspace", true))
//	result := registry.Execute(ctx, "read_file", map[string]interface{}{"path": "file.txt"})
func NewToolRegistry() *ToolRegistry {
	return &ToolRegistry{
		tools: make(map[string]Tool),
	}
}

func (r *ToolRegistry) Register(tool Tool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.tools[tool.Name()] = tool
}

func (r *ToolRegistry) Get(name string) (Tool, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	tool, ok := r.tools[name]
	return tool, ok
}

func (r *ToolRegistry) Execute(ctx context.Context, name string, args map[string]interface{}) *ToolResult {
	return r.ExecuteWithContext(ctx, name, args, "", "", nil)
}

// ExecuteWithContext executes a tool with channel/chatID context and optional async callback.
// If the tool implements AsyncTool and a non-nil callback is provided,
// the callback will be set on the tool before execution.
func (r *ToolRegistry) ExecuteWithContext(ctx context.Context, name string, args map[string]interface{}, channel, chatID string, asyncCallback AsyncCallback) *ToolResult {
	logger.InfoCF("tool", "Tool execution started",
		map[string]interface{}{
			"tool": name,
			"args": args,
		})

	tool, ok := r.Get(name)
	if !ok {
		logger.ErrorCF("tool", "Tool not found",
			map[string]interface{}{
				"tool": name,
			})
		return ErrorResult(fmt.Sprintf("tool %q not found", name)).WithError(fmt.Errorf("tool not found"))
	}

	// If tool implements ContextualTool, set context
	if contextualTool, ok := tool.(ContextualTool); ok && channel != "" && chatID != "" {
		contextualTool.SetContext(channel, chatID)
	}

	// If tool implements AsyncTool and callback is provided, set callback
	if asyncTool, ok := tool.(AsyncTool); ok && asyncCallback != nil {
		asyncTool.SetCallback(asyncCallback)
		logger.DebugCF("tool", "Async callback injected",
			map[string]interface{}{
				"tool": name,
			})
	}

	start := time.Now()
	result := tool.Execute(ctx, args)
	duration := time.Since(start)

	// Log based on result type
	if result.IsError {
		logger.ErrorCF("tool", "Tool execution failed",
			map[string]interface{}{
				"tool":     name,
				"duration": duration.Milliseconds(),
				"error":    result.ForLLM,
			})
	} else if result.Async {
		logger.InfoCF("tool", "Tool started (async)",
			map[string]interface{}{
				"tool":     name,
				"duration": duration.Milliseconds(),
			})
	} else {
		logger.InfoCF("tool", "Tool execution completed",
			map[string]interface{}{
				"tool":          name,
				"duration_ms":   duration.Milliseconds(),
				"result_length": len(result.ForLLM),
			})
	}

	return result
}

func (r *ToolRegistry) GetDefinitions() []map[string]interface{} {
	r.mu.RLock()
	defer r.mu.RUnlock()

	definitions := make([]map[string]interface{}, 0, len(r.tools))
	for _, tool := range r.tools {
		definitions = append(definitions, ToolToSchema(tool))
	}
	return definitions
}

// ToProviderDefs converts tool definitions to provider-compatible format.
// This is the format expected by LLM provider APIs.
func (r *ToolRegistry) ToProviderDefs() []providers.ToolDefinition {
	r.mu.RLock()
	defer r.mu.RUnlock()

	definitions := make([]providers.ToolDefinition, 0, len(r.tools))
	for _, tool := range r.tools {
		schema := ToolToSchema(tool)

		// Safely extract nested values with type checks
		fn, ok := schema["function"].(map[string]interface{})
		if !ok {
			continue
		}

		name, _ := fn["name"].(string)
		desc, _ := fn["description"].(string)
		params, _ := fn["parameters"].(map[string]interface{})

		definitions = append(definitions, providers.ToolDefinition{
			Type: "function",
			Function: providers.ToolFunctionDefinition{
				Name:        name,
				Description: desc,
				Parameters:  params,
			},
		})
	}
	return definitions
}

// List returns a list of all registered tool names.
func (r *ToolRegistry) List() []string {
	r.mu.RLock()
	defer r.mu.RUnlock()

	names := make([]string, 0, len(r.tools))
	for name := range r.tools {
		names = append(names, name)
	}
	return names
}

// Count returns the number of registered tools.
func (r *ToolRegistry) Count() int {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return len(r.tools)
}

// GetSummaries returns human-readable summaries of all registered tools.
// Returns a slice of "name - description" strings.
func (r *ToolRegistry) GetSummaries() []string {
	r.mu.RLock()
	defer r.mu.RUnlock()

	summaries := make([]string, 0, len(r.tools))
	for _, tool := range r.tools {
		summaries = append(summaries, fmt.Sprintf("- `%s` - %s", tool.Name(), tool.Description()))
	}
	return summaries
}

// RegisterWithPlugin registers a tool with plugin support.
// The tool will be wrapped to allow plugins to intercept its execution.
func (r *ToolRegistry) RegisterWithPlugin(tool Tool, pluginMgr *plugin.Manager, user, source, workspace string) {
	r.mu.Lock()
	defer r.mu.Unlock()

	// Wrap tool with plugin support
	pluginableTool := &PluginableTool{
		Tool:       tool,
		pluginMgr:  pluginMgr,
		user:       user,
		source:     source,
		workspace:  workspace,
	}

	r.tools[tool.Name()] = pluginableTool
}

// PluginableTool wraps a Tool to add plugin support
type PluginableTool struct {
	Tool       Tool
	pluginMgr  *plugin.Manager
	user       string
	source     string
	workspace  string
}

// Name returns the tool name
func (p *PluginableTool) Name() string {
	return p.Tool.Name()
}

// Description returns the tool description
func (p *PluginableTool) Description() string {
	return p.Tool.Description()
}

// Parameters returns the tool parameters
func (p *PluginableTool) Parameters() map[string]interface{} {
	return p.Tool.Parameters()
}

// Execute executes the tool with plugin interception
func (p *PluginableTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	// Create tool invocation
	invocation := &plugin.ToolInvocation{
		ToolName:  p.Tool.Name(),
		Method:    "Execute",
		Args:      args,
		Context:   ctx,
		User:      p.user,
		Source:    p.source,
		Workspace: p.workspace,
		Metadata:  make(map[string]interface{}),
	}

	// Phase 1: Pre-execution - ask plugins if we should proceed
	allowed, err := p.pluginMgr.Execute(ctx, invocation)
	if !allowed {
		return ErrorResult(err.Error())
	}

	// Phase 2: Execute the original tool
	result := p.Tool.Execute(ctx, args)

	// Phase 3: Post-execution - let plugins inspect/modify result
	invocation.Result = result
	_, postErr := p.pluginMgr.Execute(ctx, invocation)
	if postErr != nil {
		return ErrorResult(postErr.Error())
	}

	return result
}
