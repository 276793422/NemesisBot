// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package mcp provides tool adapters for integrating MCP tools with NemesisBot.
package mcp

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/tools"
)

// Adapter converts an MCP tool to a NemesisBot Tool.
type Adapter struct {
	client Client
	mcpTool Tool
}

// NewAdapter creates a new NemesisBot tool from an MCP tool definition.
func NewAdapter(client Client, mcpTool Tool) *Adapter {
	return &Adapter{
		client:  client,
		mcpTool: mcpTool,
	}
}

// Name returns the tool name with server prefix to avoid conflicts.
// Format: mcp_<server_name>_<tool_name>
func (a *Adapter) Name() string {
	// Sanitize server and tool names for use as identifiers
	serverName := sanitizeName(a.client.ServerInfo().Name)
	toolName := sanitizeName(a.mcpTool.Name)
	return fmt.Sprintf("mcp_%s_%s", serverName, toolName)
}

// Description returns the tool description with server prefix.
func (a *Adapter) Description() string {
	prefix := fmt.Sprintf("[MCP:%s] ", a.client.ServerInfo().Name)
	return prefix + a.mcpTool.Description
}

// Parameters returns the tool input schema in NemesisBot format.
// MCP uses JSON Schema which is compatible with NemesisBot's format.
func (a *Adapter) Parameters() map[string]interface{} {
	// Return the input schema wrapped in a standard JSON Schema object
	return map[string]interface{}{
		"type":                 "object",
		"properties":           a.mcpTool.InputSchema,
		"additionalProperties": false,
	}
}

// Execute calls the MCP tool with the given arguments.
func (a *Adapter) Execute(ctx context.Context, args map[string]interface{}) *tools.ToolResult {
	// Get timeout from server config
	timeout := 30 * time.Second // default timeout

	// Create timeout context
	execCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	logger.InfoCF("mcp.adapter", "Executing MCP tool",
		map[string]interface{}{
			"tool": a.mcpTool.Name,
			"args": args,
		})

	// Call the MCP tool
	result, err := a.client.CallTool(execCtx, a.mcpTool.Name, args)
	if err != nil {
		// Check for timeout
		if execCtx.Err() == context.DeadlineExceeded {
			logger.ErrorCF("mcp.adapter", "MCP tool execution timed out",
				map[string]interface{}{
					"tool":    a.mcpTool.Name,
					"timeout": timeout,
				})
			return tools.ErrorResult(fmt.Sprintf("MCP tool '%s' timed out after %v", a.mcpTool.Name, timeout))
		}

		logger.ErrorCF("mcp.adapter", "MCP tool execution failed",
			map[string]interface{}{
				"tool":  a.mcpTool.Name,
				"error": err.Error(),
			})
		return tools.ErrorResult(fmt.Sprintf("MCP tool error: %v", err))
	}

	// Check if tool returned an error
	if result.IsError {
		logger.WarnCF("mcp.adapter", "MCP tool returned error status",
			map[string]interface{}{
				"tool": a.mcpTool.Name,
			})

		// Extract error message from content
		var errMsg string
		for _, content := range result.Content {
			if content.Type == "text" {
				if errMsg != "" {
					errMsg += "; "
				}
				errMsg += content.Text
			}
		}

		return tools.ErrorResult(fmt.Sprintf("MCP tool '%s' returned error: %s", a.mcpTool.Name, errMsg))
	}

	// Extract text content from result
	var textParts []string
	for _, content := range result.Content {
		if content.Type == "text" {
			textParts = append(textParts, content.Text)
		} else if content.Type == "image" {
			textParts = append(textParts, fmt.Sprintf("[Image: %s, mime-type: %s]", content.Data, content.MimeType))
		} else if content.Type == "resource" {
			textParts = append(textParts, fmt.Sprintf("[Resource: %s]", content.Data))
		}
	}

	// Join text parts with newlines
	resultText := strings.Join(textParts, "\n")

	logger.DebugCF("mcp.adapter", "MCP tool execution completed",
		map[string]interface{}{
			"tool":          a.mcpTool.Name,
			"result_length": len(resultText),
		})

	// Return successful result
	return tools.NewToolResult(resultText)
}

// CreateToolsFromClient creates NemesisBot tools from all available MCP server tools.
func CreateToolsFromClient(client Client) ([]tools.Tool, error) {
	ctx := context.Background()

	// List all tools from the MCP server
	mcpTools, err := client.ListTools(ctx)
	if err != nil {
		return nil, fmt.Errorf("failed to list MCP tools: %w", err)
	}

	// Create an adapter for each MCP tool
	var nemesisTools []tools.Tool
	for _, mcpTool := range mcpTools {
		adapter := NewAdapter(client, mcpTool)
		nemesisTools = append(nemesisTools, adapter)

		logger.DebugCF("mcp.adapter", "Created tool adapter",
			map[string]interface{}{
				"mcp_tool":      mcpTool.Name,
				"nemesis_tool":  adapter.Name(),
				"description":   adapter.Description(),
			})
	}

	return nemesisTools, nil
}

// sanitizeName converts a string to a valid identifier by replacing invalid characters with underscores.
func sanitizeName(name string) string {
	// Replace any character that's not alphanumeric or hyphen/underscore with underscore
	var result strings.Builder
	for _, r := range name {
		if (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') || r == '-' || r == '_' {
			result.WriteRune(r)
		} else {
			result.WriteRune('_')
		}
	}
	return result.String()
}
