// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package plugin

import (
	"context"
)

// ToolWrapper wraps a tool with plugin support
type ToolWrapper struct {
	toolName     string
	pluginMgr    *Manager
	user         string
	source       string
	workspace    string
	originalTool ToolExecutor
}

// ToolExecutor is the interface for executing tools
type ToolExecutor interface {
	Execute(ctx context.Context, args map[string]interface{}) (interface{}, error)
}

// NewToolWrapper creates a new tool wrapper with plugin support
func NewToolWrapper(toolName string, pluginMgr *Manager, user, source, workspace string, originalTool ToolExecutor) *ToolWrapper {
	return &ToolWrapper{
		toolName:     toolName,
		pluginMgr:    pluginMgr,
		user:         user,
		source:       source,
		workspace:    workspace,
		originalTool: originalTool,
	}
}

// Execute executes the tool with plugin interception
func (w *ToolWrapper) Execute(ctx context.Context, args map[string]interface{}) (interface{}, error) {
	// Create tool invocation
	invocation := &ToolInvocation{
		ToolName:  w.toolName,
		Method:    "Execute",
		Args:      args,
		Context:   ctx,
		User:      w.user,
		Source:    w.source,
		Workspace: w.workspace,
		Metadata:  make(map[string]interface{}),
	}

	// Phase 1: Pre-execution - ask plugins if we should proceed
	allowed, err := w.pluginMgr.Execute(ctx, invocation)
	if !allowed {
		return nil, err
	}

	// Phase 2: Execute the original tool
	result, err := w.originalTool.Execute(ctx, args)
	if err != nil {
		// Store error for plugins to inspect
		invocation.BlockingError = err
	}

	// Phase 3: Post-execution - let plugins inspect/modify result
	invocation.Result = result
	_, postErr := w.pluginMgr.Execute(ctx, invocation)
	if postErr != nil {
		return nil, postErr
	}

	// Check if a plugin modified the result
	if invocation.Result != nil {
		return invocation.Result, nil
	}

	return result, nil
}

// PluginableTool wraps an existing tool to make it plugin-aware
type PluginableTool struct {
	name      string
	pluginMgr *Manager
	innerTool ToolExecutor
	user      string
	source    string
	workspace string
}

// NewPluginableTool creates a plugin-aware tool
func NewPluginableTool(name string, pluginMgr *Manager, innerTool ToolExecutor, user, source, workspace string) *PluginableTool {
	return &PluginableTool{
		name:      name,
		pluginMgr: pluginMgr,
		innerTool: innerTool,
		user:      user,
		source:    source,
		workspace: workspace,
	}
}

// Execute implements ToolExecutor with plugin support
func (t *PluginableTool) Execute(ctx context.Context, args map[string]interface{}) (interface{}, error) {
	wrapper := NewToolWrapper(t.name, t.pluginMgr, t.user, t.source, t.workspace, t.innerTool)
	return wrapper.Execute(ctx, args)
}
