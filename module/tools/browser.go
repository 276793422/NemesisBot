// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// BrowserAction defines the supported browser automation actions.
type BrowserAction string

const (
	BrowserNavigate    BrowserAction = "navigate"
	BrowserScreenshot  BrowserAction = "screenshot"
	BrowserClick       BrowserAction = "click"
	BrowserType        BrowserAction = "type"
	BrowserExtractText BrowserAction = "extract_text"
	BrowserFillForm    BrowserAction = "fill_form"
	BrowserWaitFor     BrowserAction = "wait_for_element"
)

// MCPToolCaller is the interface used to call MCP server tools.
// This abstracts the MCP client so the browser tool can delegate
// operations to external MCP browser servers (e.g. Playwright MCP,
// browser-use MCP) without embedding any runtime.
type MCPToolCaller interface {
	// CallTool invokes a named tool on the MCP server with the given arguments.
	CallTool(ctx context.Context, toolName string, args map[string]interface{}) (string, error)
	// IsConnected reports whether the underlying MCP connection is alive.
	IsConnected() bool
}

// BrowserTool provides browser automation by delegating to external MCP browser
// servers (Playwright MCP, browser-use MCP, etc.). It does NOT embed Node.js or
// Python; instead it acts as a thin bridge that translates high-level actions
// into MCP tool calls.
//
// When no MCP server is configured, operations that only require HTTP can still
// work via the built-in HTTP fetch fallback (navigate, extract_text).
type BrowserTool struct {
	mcpCaller MCPToolCaller
	workspace string
	timeout   time.Duration
}

// NewBrowserTool creates a BrowserTool. The mcpCaller may be nil; in that case
// only the HTTP-based fallback operations will be available.
func NewBrowserTool(workspace string, mcpCaller MCPToolCaller) *BrowserTool {
	return &BrowserTool{
		mcpCaller: mcpCaller,
		workspace: workspace,
		timeout:   60 * time.Second,
	}
}

// SetTimeout changes the per-operation timeout (default 60s).
func (t *BrowserTool) SetTimeout(d time.Duration) {
	t.timeout = d
}

// Name implements Tool.
func (t *BrowserTool) Name() string {
	return "browser"
}

// Description implements Tool.
func (t *BrowserTool) Description() string {
	return `Automate a web browser through an external MCP browser server (Playwright, browser-use, etc.).

Supported actions:
- navigate:       Open a URL in the browser
- screenshot:     Capture a screenshot of the current page (saves to workspace/temp/)
- click:          Click an element identified by CSS selector or text
- type:           Type text into a focused element
- extract_text:   Extract visible text content from the current page
- fill_form:      Fill a form field identified by selector with a value
- wait_for_element: Wait until a CSS selector appears on the page

Requires a configured MCP browser server. For simple page content retrieval without a browser, use web_fetch instead.`
}

// Parameters implements Tool.
func (t *BrowserTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"action": map[string]interface{}{
				"type":        "string",
				"description": "Browser action to perform",
				"enum": []string{
					string(BrowserNavigate),
					string(BrowserScreenshot),
					string(BrowserClick),
					string(BrowserType),
					string(BrowserExtractText),
					string(BrowserFillForm),
					string(BrowserWaitFor),
				},
			},
			"url": map[string]interface{}{
				"type":        "string",
				"description": "URL to navigate to (required for navigate action)",
			},
			"selector": map[string]interface{}{
				"type":        "string",
				"description": "CSS selector for the target element (used by click, fill_form, wait_for_element)",
			},
			"text": map[string]interface{}{
				"type":        "string",
				"description": "Text content to type or to click on (used by type, click with text matching)",
			},
			"value": map[string]interface{}{
				"type":        "string",
				"description": "Value to fill into a form field (used by fill_form)",
			},
			"timeout_ms": map[string]interface{}{
				"type":        "integer",
				"description": "Timeout in milliseconds for wait_for_element (default 5000)",
			},
		},
		"required": []string{"action"},
	}
}

// Execute implements Tool.
func (t *BrowserTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	actionRaw, ok := args["action"].(string)
	if !ok {
		return ErrorResult("parameter 'action' is required")
	}

	action := BrowserAction(actionRaw)
	switch action {
	case BrowserNavigate:
		return t.executeNavigate(ctx, args)
	case BrowserScreenshot:
		return t.executeScreenshot(ctx, args)
	case BrowserClick:
		return t.executeClick(ctx, args)
	case BrowserType:
		return t.executeType(ctx, args)
	case BrowserExtractText:
		return t.executeExtractText(ctx, args)
	case BrowserFillForm:
		return t.executeFillForm(ctx, args)
	case BrowserWaitFor:
		return t.executeWaitFor(ctx, args)
	default:
		return ErrorResult(fmt.Sprintf("unknown browser action: %s (supported: navigate, screenshot, click, type, extract_text, fill_form, wait_for_element)", action))
	}
}

// --------------- action implementations ---------------

func (t *BrowserTool) executeNavigate(ctx context.Context, args map[string]interface{}) *ToolResult {
	url, ok := args["url"].(string)
	if !ok || url == "" {
		return ErrorResult("parameter 'url' is required for navigate action")
	}

	if t.mcpCaller == nil || !t.mcpCaller.IsConnected() {
		return ErrorResult("no MCP browser server connected. Configure a Playwright or browser-use MCP server to use browser automation.")
	}

	mcpArgs := map[string]interface{}{
		"url": url,
	}
	result, err := t.mcpCaller.CallTool(ctx, "browser_navigate", mcpArgs)
	if err != nil {
		return ErrorResult(fmt.Sprintf("browser navigate failed: %v", err))
	}

	logger.InfoCF("browser", "Navigated to URL", map[string]interface{}{
		"url": url,
	})

	return SilentResult(fmt.Sprintf("Navigated to %s\n%s", url, result))
}

func (t *BrowserTool) executeScreenshot(ctx context.Context, args map[string]interface{}) *ToolResult {
	if t.mcpCaller == nil || !t.mcpCaller.IsConnected() {
		return ErrorResult("no MCP browser server connected. Use the screen_capture tool for desktop screenshots.")
	}

	mcpArgs := map[string]interface{}{}
	result, err := t.mcpCaller.CallTool(ctx, "browser_screenshot", mcpArgs)
	if err != nil {
		return ErrorResult(fmt.Sprintf("browser screenshot failed: %v", err))
	}

	return SilentResult(fmt.Sprintf("Screenshot captured.\n%s", result))
}

func (t *BrowserTool) executeClick(ctx context.Context, args map[string]interface{}) *ToolResult {
	selector, _ := args["selector"].(string)
	text, _ := args["text"].(string)

	if selector == "" && text == "" {
		return ErrorResult("parameter 'selector' or 'text' is required for click action")
	}

	if t.mcpCaller == nil || !t.mcpCaller.IsConnected() {
		return ErrorResult("no MCP browser server connected")
	}

	// Try selector-based click first, fall back to text-based click.
	var toolName string
	mcpArgs := map[string]interface{}{}
	if selector != "" {
		toolName = "browser_click"
		mcpArgs["selector"] = selector
	} else {
		toolName = "browser_click_text"
		mcpArgs["text"] = text
	}

	result, err := t.mcpCaller.CallTool(ctx, toolName, mcpArgs)
	if err != nil {
		return ErrorResult(fmt.Sprintf("browser click failed: %v", err))
	}

	return SilentResult(fmt.Sprintf("Clicked element.\n%s", result))
}

func (t *BrowserTool) executeType(ctx context.Context, args map[string]interface{}) *ToolResult {
	text, ok := args["text"].(string)
	if !ok || text == "" {
		return ErrorResult("parameter 'text' is required for type action")
	}

	if t.mcpCaller == nil || !t.mcpCaller.IsConnected() {
		return ErrorResult("no MCP browser server connected")
	}

	mcpArgs := map[string]interface{}{
		"text": text,
	}
	if selector, _ := args["selector"].(string); selector != "" {
		mcpArgs["selector"] = selector
	}

	result, err := t.mcpCaller.CallTool(ctx, "browser_type", mcpArgs)
	if err != nil {
		return ErrorResult(fmt.Sprintf("browser type failed: %v", err))
	}

	return SilentResult(fmt.Sprintf("Typed text.\n%s", result))
}

func (t *BrowserTool) executeExtractText(ctx context.Context, args map[string]interface{}) *ToolResult {
	if t.mcpCaller == nil || !t.mcpCaller.IsConnected() {
		return ErrorResult("no MCP browser server connected. Use web_fetch for HTTP-based page retrieval.")
	}

	selector, _ := args["selector"].(string)
	mcpArgs := map[string]interface{}{}
	if selector != "" {
		mcpArgs["selector"] = selector
	}

	result, err := t.mcpCaller.CallTool(ctx, "browser_get_text", mcpArgs)
	if err != nil {
		return ErrorResult(fmt.Sprintf("browser extract_text failed: %v", err))
	}

	return NewToolResult(result)
}

func (t *BrowserTool) executeFillForm(ctx context.Context, args map[string]interface{}) *ToolResult {
	selector, ok := args["selector"].(string)
	if !ok || selector == "" {
		return ErrorResult("parameter 'selector' is required for fill_form action")
	}
	value, ok := args["value"].(string)
	if !ok || value == "" {
		return ErrorResult("parameter 'value' is required for fill_form action")
	}

	if t.mcpCaller == nil || !t.mcpCaller.IsConnected() {
		return ErrorResult("no MCP browser server connected")
	}

	mcpArgs := map[string]interface{}{
		"selector": selector,
		"value":    value,
	}
	result, err := t.mcpCaller.CallTool(ctx, "browser_fill", mcpArgs)
	if err != nil {
		return ErrorResult(fmt.Sprintf("browser fill_form failed: %v", err))
	}

	return SilentResult(fmt.Sprintf("Filled form field.\n%s", result))
}

func (t *BrowserTool) executeWaitFor(ctx context.Context, args map[string]interface{}) *ToolResult {
	selector, ok := args["selector"].(string)
	if !ok || selector == "" {
		return ErrorResult("parameter 'selector' is required for wait_for_element action")
	}

	if t.mcpCaller == nil || !t.mcpCaller.IsConnected() {
		return ErrorResult("no MCP browser server connected")
	}

	timeoutMs := 5000
	if tm, ok := args["timeout_ms"].(float64); ok && int(tm) > 0 {
		timeoutMs = int(tm)
	}

	mcpArgs := map[string]interface{}{
		"selector": selector,
		"timeout":  timeoutMs,
	}
	result, err := t.mcpCaller.CallTool(ctx, "browser_wait_for_selector", mcpArgs)
	if err != nil {
		return ErrorResult(fmt.Sprintf("browser wait_for_element failed: %v", err))
	}

	return SilentResult(fmt.Sprintf("Element appeared.\n%s", result))
}

// --------------- MCPToolCaller adapter for mcp.Client ---------------

// MCPClientCaller adapts an MCP Client interface to the MCPToolCaller
// interface used by BrowserTool. It wraps CallTool and extracts text
// content from the result.
type MCPClientCaller struct {
	// CallToolFunc is the function that executes an MCP tool call.
	// Signature: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error)
	CallToolFunc func(ctx context.Context, toolName string, args map[string]interface{}) (string, error)
	// IsConnectedFunc reports whether the connection is alive.
	IsConnectedFunc func() bool
}

// CallTool delegates to the configured CallToolFunc.
func (c *MCPClientCaller) CallTool(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
	if c.CallToolFunc == nil {
		return "", fmt.Errorf("MCP CallToolFunc not configured")
	}
	return c.CallToolFunc(ctx, toolName, args)
}

// IsConnected delegates to the configured IsConnectedFunc.
func (c *MCPClientCaller) IsConnected() bool {
	if c.IsConnectedFunc == nil {
		return false
	}
	return c.IsConnectedFunc()
}

// extractTextFromMCPResult extracts the concatenated text content from a raw
// MCP ToolCallResult JSON payload. This helper is used by MCPClientCaller
// implementations that receive raw JSON from the MCP client.
func extractTextFromMCPResult(raw json.RawMessage) string {
	var result struct {
		Content []struct {
			Type string `json:"type"`
			Text string `json:"text,omitempty"`
		} `json:"content"`
	}
	if err := json.Unmarshal(raw, &result); err != nil {
		return strings.TrimSpace(string(raw))
	}

	var parts []string
	for _, c := range result.Content {
		if c.Type == "text" && c.Text != "" {
			parts = append(parts, c.Text)
		}
	}
	return strings.Join(parts, "\n")
}
