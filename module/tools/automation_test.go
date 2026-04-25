// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// mockMCPToolCaller implements MCPToolCaller for testing with configurable callFunc.
// This mock supports dynamic behavior (different responses per tool name), unlike
// the simpler mockMCPCaller in browser_test.go which returns fixed results.
type mockMCPToolCaller struct {
	callFunc     func(ctx context.Context, toolName string, args map[string]interface{}) (string, error)
	connected    bool
	callCount    int
	lastToolName string
	lastArgs     map[string]interface{}
}

func (m *mockMCPToolCaller) CallTool(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
	m.callCount++
	m.lastToolName = toolName
	m.lastArgs = args
	if m.callFunc != nil {
		return m.callFunc(ctx, toolName, args)
	}
	return "mock result", nil
}

func (m *mockMCPToolCaller) IsConnected() bool {
	return m.connected
}

// ==================== DesktopTool Tests ====================

func TestNewDesktopTool(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewDesktopTool("/tmp/workspace", mock)

	if tool == nil {
		t.Fatal("NewDesktopTool returned nil")
	}
	if tool.workspace != "/tmp/workspace" {
		t.Errorf("workspace = %q, want %q", tool.workspace, "/tmp/workspace")
	}
	if tool.timeout != 30*time.Second {
		t.Errorf("timeout = %v, want %v", tool.timeout, 30*time.Second)
	}
}

func TestNewDesktopTool_NilCaller(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	if tool == nil {
		t.Fatal("NewDesktopTool with nil caller returned nil")
	}
	if tool.mcpCaller != nil {
		t.Error("mcpCaller should be nil")
	}
}

func TestDesktopTool_Name(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	if tool.Name() != "desktop" {
		t.Errorf("Name() = %q, want %q", tool.Name(), "desktop")
	}
}

func TestDesktopTool_Description(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	desc := tool.Description()
	if desc == "" {
		t.Error("Description() returned empty string")
	}
	if !strings.Contains(desc, "find_window") {
		t.Error("Description should mention find_window")
	}
	if !strings.Contains(desc, "list_windows") {
		t.Error("Description should mention list_windows")
	}
}

func TestDesktopTool_Parameters(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	params := tool.Parameters()

	if params["type"] != "object" {
		t.Errorf("parameters type = %v, want object", params["type"])
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("properties is not a map")
	}

	expectedProps := []string{"action", "title", "hwnd", "x", "y", "width", "height", "text", "button"}
	for _, key := range expectedProps {
		if _, exists := props[key]; !exists {
			t.Errorf("missing property %q in parameters", key)
		}
	}

	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("required is not a []string")
	}
	if len(required) != 1 || required[0] != "action" {
		t.Errorf("required = %v, want [action]", required)
	}
}

func TestDesktopTool_SetTimeout(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	newTimeout := 60 * time.Second
	tool.SetTimeout(newTimeout)
	if tool.timeout != newTimeout {
		t.Errorf("timeout = %v, want %v", tool.timeout, newTimeout)
	}
}

// --- Desktop Execute dispatch tests ---

func TestDesktopTool_Execute_MissingAction(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{})
	if !result.IsError {
		t.Error("expected error result")
	}
}

func TestDesktopTool_Execute_ActionNotString(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": 42,
	})
	if !result.IsError {
		t.Error("expected error when action is not a string")
	}
}

func TestDesktopTool_Execute_UnknownAction(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "nonexistent",
	})
	if !result.IsError {
		t.Error("expected error for unknown action")
	}
	if !strings.Contains(result.ForLLM, "unknown desktop action") {
		t.Errorf("error should mention 'unknown desktop action', got: %s", result.ForLLM)
	}
}

// --- Desktop find_window tests ---

func TestDesktopTool_Execute_FindWindow_MCP(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return `{"hwnd":"HWND(0x12345)","title":"Notepad"}`, nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "find_window",
		"title":  "Notepad",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "find_window_by_title" {
		t.Errorf("tool name = %q, want %q", mock.lastToolName, "find_window_by_title")
	}
	if mock.lastArgs["title_contains"] != "Notepad" {
		t.Errorf("title_contains = %v, want Notepad", mock.lastArgs["title_contains"])
	}
}

func TestDesktopTool_Execute_FindWindow_MissingTitle(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "find_window",
	})
	if !result.IsError {
		t.Error("expected error for missing title")
	}
}

func TestDesktopTool_Execute_FindWindow_MCPError(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("MCP timeout")
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "find_window",
		"title":  "Test",
	})
	if !result.IsError {
		t.Error("expected error on MCP failure")
	}
}

// --- Desktop list_windows tests ---

func TestDesktopTool_Execute_ListWindows_MCP(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return `[{"hwnd":"HWND(0x1)","title":"Window1"}]`, nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "list_windows",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "enumerate_windows" {
		t.Errorf("tool name = %q, want %q", mock.lastToolName, "enumerate_windows")
	}
}

func TestDesktopTool_Execute_ListWindows_WithTitleFilter(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "filtered results", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "list_windows",
		"title":  "Chrome",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastArgs["title_contains"] != "Chrome" {
		t.Errorf("title_contains = %v, want Chrome", mock.lastArgs["title_contains"])
	}
}

func TestDesktopTool_Execute_ListWindows_MCPError(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("connection lost")
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "list_windows",
	})
	if !result.IsError {
		t.Error("expected error on MCP failure")
	}
}

// --- Desktop click_at tests ---

func TestDesktopTool_Execute_ClickAt_MCP(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      100,
		"y":      200,
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "click_window" {
		t.Errorf("tool name = %q, want %q", mock.lastToolName, "click_window")
	}
	if mock.lastArgs["x"] != 100 {
		t.Errorf("x = %v, want 100", mock.lastArgs["x"])
	}
	if mock.lastArgs["y"] != 200 {
		t.Errorf("y = %v, want 200", mock.lastArgs["y"])
	}
}

func TestDesktopTool_Execute_ClickAt_Float64Coords(t *testing.T) {
	// JSON numbers decode as float64
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      float64(100),
		"y":      float64(200),
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
}

func TestDesktopTool_Execute_ClickAt_CustomButton(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      100,
		"y":      200,
		"button": "right",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastArgs["button"] != "right" {
		t.Errorf("button = %v, want right", mock.lastArgs["button"])
	}
}

func TestDesktopTool_Execute_ClickAt_DefaultButton(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      50,
		"y":      50,
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastArgs["button"] != "left" {
		t.Errorf("button = %v, want left (default)", mock.lastArgs["button"])
	}
}

func TestDesktopTool_Execute_ClickAt_WithHwnd(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      50,
		"y":      60,
		"hwnd":   "HWND(0x999)",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastArgs["hwnd"] != "HWND(0x999)" {
		t.Errorf("hwnd = %v, want HWND(0x999)", mock.lastArgs["hwnd"])
	}
}

func TestDesktopTool_Execute_ClickAt_MissingCoords(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
	})
	if !result.IsError {
		t.Error("expected error for missing coords")
	}
}

func TestDesktopTool_Execute_ClickAt_MCPError(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("access denied")
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      100,
		"y":      200,
	})
	if !result.IsError {
		t.Error("expected error on MCP failure")
	}
}

func TestDesktopTool_Execute_ClickAt_SilentResult(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      100,
		"y":      200,
	})
	if !result.Silent {
		t.Error("click_at result should be silent")
	}
}

// --- Desktop type_text tests ---

func TestDesktopTool_Execute_TypeText_MCP(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "sent", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type_text",
		"text":   "hello world",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "send_key_to_window" {
		t.Errorf("tool name = %q, want %q", mock.lastToolName, "send_key_to_window")
	}
	if mock.lastArgs["key"] != "hello world" {
		t.Errorf("key = %v, want 'hello world'", mock.lastArgs["key"])
	}
}

func TestDesktopTool_Execute_TypeText_WithHwnd(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "sent", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type_text",
		"text":   "test",
		"hwnd":   "HWND(0x100)",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastArgs["hwnd"] != "HWND(0x100)" {
		t.Errorf("hwnd = %v, want HWND(0x100)", mock.lastArgs["hwnd"])
	}
}

func TestDesktopTool_Execute_TypeText_MissingText(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type_text",
	})
	if !result.IsError {
		t.Error("expected error for missing text")
	}
}

func TestDesktopTool_Execute_TypeText_MCPError(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("send failed")
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type_text",
		"text":   "test",
	})
	if !result.IsError {
		t.Error("expected error on MCP failure")
	}
}

func TestDesktopTool_Execute_TypeText_SilentResult(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "sent", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type_text",
		"text":   "hello",
	})
	if !result.Silent {
		t.Error("type_text result should be silent")
	}
}

// --- Desktop screenshot tests ---

func TestDesktopTool_Execute_Screenshot_MCP_Full(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "captured", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "take_screenshot",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "capture_screenshot_to_file" {
		t.Errorf("tool name = %q, want %q", mock.lastToolName, "capture_screenshot_to_file")
	}
}

func TestDesktopTool_Execute_Screenshot_MCP_WithRegion(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "captured", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "take_screenshot",
		"x":      10,
		"y":      20,
		"width":  100,
		"height": 200,
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastArgs["x"] != 10 {
		t.Errorf("x = %v, want 10", mock.lastArgs["x"])
	}
	if mock.lastArgs["width"] != 100 {
		t.Errorf("width = %v, want 100", mock.lastArgs["width"])
	}
}

func TestDesktopTool_Execute_Screenshot_MCP_WithHwnd(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "captured", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "take_screenshot",
		"hwnd":   "HWND(0xABC)",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastArgs["hwnd"] != "HWND(0xABC)" {
		t.Errorf("hwnd = %v, want HWND(0xABC)", mock.lastArgs["hwnd"])
	}
}

func TestDesktopTool_Execute_Screenshot_MCPError(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("capture failed")
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "take_screenshot",
	})
	if !result.IsError {
		t.Error("expected error on MCP failure")
	}
}

func TestDesktopTool_Execute_Screenshot_SilentResult(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "captured", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "take_screenshot",
	})
	if !result.Silent {
		t.Error("take_screenshot result should be silent")
	}
}

// --- Desktop get_window_text tests ---

func TestDesktopTool_Execute_GetWindowText_ByHwnd(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			if toolName == "get_window_text" {
				return "Window text content", nil
			}
			return "", fmt.Errorf("unexpected tool: %s", toolName)
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "get_window_text",
		"hwnd":   "HWND(0x12345)",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "get_window_text" {
		t.Errorf("tool name = %q, want %q", mock.lastToolName, "get_window_text")
	}
}

func TestDesktopTool_Execute_GetWindowText_ByTitle(t *testing.T) {
	callCount := 0
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			callCount++
			if toolName == "find_window_by_title" {
				return `{"hwnd":"HWND(0xABCD)"}`, nil
			}
			if toolName == "get_window_text" {
				return "Resolved text", nil
			}
			return "", fmt.Errorf("unexpected tool: %s", toolName)
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "get_window_text",
		"title":  "Notepad",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if callCount != 2 {
		t.Errorf("callCount = %d, want 2 (find + get_text)", callCount)
	}
}

func TestDesktopTool_Execute_GetWindowText_ByTitle_FindFails(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("not found")
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "get_window_text",
		"title":  "Missing",
	})
	if !result.IsError {
		t.Error("expected error when find_window fails")
	}
}

func TestDesktopTool_Execute_GetWindowText_ByTitle_BadJSON(t *testing.T) {
	callCount := 0
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			callCount++
			if toolName == "find_window_by_title" {
				return "not valid json for hwnd", nil
			}
			return "should not reach", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "get_window_text",
		"title":  "Test",
	})
	if !result.IsError {
		t.Error("expected error when hwnd cannot be resolved")
	}
	if !strings.Contains(result.ForLLM, "could not resolve window handle") {
		t.Errorf("error = %q, should mention resolve failure", result.ForLLM)
	}
}

func TestDesktopTool_Execute_GetWindowText_ByTitle_EmptyHwndInJSON(t *testing.T) {
	callCount := 0
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			callCount++
			if toolName == "find_window_by_title" {
				return `{"hwnd":""}`, nil
			}
			return "should not reach", nil
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "get_window_text",
		"title":  "Test",
	})
	if !result.IsError {
		t.Error("expected error when hwnd is empty in JSON")
	}
	if !strings.Contains(result.ForLLM, "could not resolve window handle") {
		t.Errorf("error = %q, should mention resolve failure", result.ForLLM)
	}
}

func TestDesktopTool_Execute_GetWindowText_MissingParams(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "get_window_text",
	})
	if !result.IsError {
		t.Error("expected error for missing hwnd and title")
	}
}

func TestDesktopTool_Execute_GetWindowText_GetTextFails(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("access denied")
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "get_window_text",
		"hwnd":   "HWND(0x100)",
	})
	if !result.IsError {
		t.Error("expected error when get_window_text fails")
	}
}

func TestDesktopTool_Execute_GetWindowText_NoMCP(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "get_window_text",
		"hwnd":   "HWND(0x100)",
	})
	if !result.IsError {
		t.Error("expected error when no MCP and no fallback")
	}
	if !strings.Contains(result.ForLLM, "no standalone fallback") {
		t.Errorf("error should mention no fallback, got: %s", result.ForLLM)
	}
}

// --- Desktop hasMCP helper ---

func TestDesktopTool_HasMCP_NilCaller(t *testing.T) {
	tool := NewDesktopTool("/tmp", nil)
	if tool.hasMCP() {
		t.Error("hasMCP should be false with nil caller")
	}
}

func TestDesktopTool_HasMCP_DisconnectedCaller(t *testing.T) {
	mock := &mockMCPToolCaller{connected: false}
	tool := NewDesktopTool("/tmp", mock)
	if tool.hasMCP() {
		t.Error("hasMCP should be false when caller is disconnected")
	}
}

func TestDesktopTool_HasMCP_ConnectedCaller(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewDesktopTool("/tmp", mock)
	if !tool.hasMCP() {
		t.Error("hasMCP should be true when caller is connected")
	}
}

// --- Desktop getIntArg helper ---

func TestGetIntArg_Int(t *testing.T) {
	args := map[string]interface{}{"x": 42}
	val, ok := getIntArg(args, "x")
	if !ok {
		t.Error("expected ok=true for int arg")
	}
	if val != 42 {
		t.Errorf("val = %d, want 42", val)
	}
}

func TestGetIntArg_Float64(t *testing.T) {
	args := map[string]interface{}{"x": float64(42)}
	val, ok := getIntArg(args, "x")
	if !ok {
		t.Error("expected ok=true for float64 arg")
	}
	if val != 42 {
		t.Errorf("val = %d, want 42", val)
	}
}

func TestGetIntArg_String(t *testing.T) {
	args := map[string]interface{}{"x": "not a number"}
	_, ok := getIntArg(args, "x")
	if ok {
		t.Error("expected ok=false for string arg")
	}
}

func TestGetIntArg_Missing(t *testing.T) {
	args := map[string]interface{}{}
	_, ok := getIntArg(args, "x")
	if ok {
		t.Error("expected ok=false for missing arg")
	}
}

// --- Desktop standalone (PowerShell) fallback tests ---

func TestDesktopTool_Execute_FindWindow_NoMCP(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "find_window",
		"title":  "NonexistentWindow12345",
	})
	// Should return a result (likely "No windows found"), not panic
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_ListWindows_NoMCP(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "list_windows",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_Screenshot_NoMCP(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "take_screenshot",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_ClickAt_NoMCP(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      100,
		"y":      200,
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_ClickAt_NoMCP_RightButton(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      100,
		"y":      200,
		"button": "right",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_ClickAt_NoMCP_MiddleButton(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click_at",
		"x":      100,
		"y":      200,
		"button": "middle",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_TypeText_NoMCP(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type_text",
		"text":   "hello",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_TypeText_NoMCP_SpecialChars(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type_text",
		"text":   "it's a test", // contains single quote for PS escaping
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_Screenshot_NoMCP_PartialRegion(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewDesktopTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "take_screenshot",
		"x":      0,
		"y":      0,
		"width":  100,
		"height": 100,
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestDesktopTool_Execute_FindWindow_CancelledContext(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", ctx.Err()
		},
	}
	tool := NewDesktopTool("/tmp", mock)

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "find_window",
		"title":  "Test",
	})
	if !result.IsError {
		t.Error("expected error on cancelled context")
	}
}

// --- Desktop windowInfo JSON parsing ---

func TestDesktopTool_WindowInfoStruct(t *testing.T) {
	data := `{"hwnd":"HWND(0x1234)","title":"Test Window","class_name":"TestClass","left":10,"top":20,"width":800,"height":600}`
	var info windowInfo
	if err := json.Unmarshal([]byte(data), &info); err != nil {
		t.Fatalf("failed to parse windowInfo: %v", err)
	}
	if info.Hwnd != "HWND(0x1234)" {
		t.Errorf("Hwnd = %q, want HWND(0x1234)", info.Hwnd)
	}
	if info.Title != "Test Window" {
		t.Errorf("Title = %q, want 'Test Window'", info.Title)
	}
	if info.Width != 800 || info.Height != 600 {
		t.Errorf("size = %dx%d, want 800x600", info.Width, info.Height)
	}
}

// --- DesktopAction constants ---

func TestDesktopActionConstants(t *testing.T) {
	actions := map[DesktopAction]string{
		DesktopFindWindow:  "find_window",
		DesktopListWindows: "list_windows",
		DesktopClickAt:     "click_at",
		DesktopTypeText:    "type_text",
		DesktopScreenshot:  "take_screenshot",
		DesktopGetText:     "get_window_text",
	}

	for action, expected := range actions {
		if string(action) != expected {
			t.Errorf("DesktopAction constant mismatch: got %q, want %q", string(action), expected)
		}
	}
}

// ==================== ScreenCaptureTool Tests ====================

func TestNewScreenCaptureTool(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewScreenCaptureTool("/tmp/workspace", mock)

	if tool == nil {
		t.Fatal("NewScreenCaptureTool returned nil")
	}
	if tool.workspace != "/tmp/workspace" {
		t.Errorf("workspace = %q, want %q", tool.workspace, "/tmp/workspace")
	}
	if tool.timeout != 30*time.Second {
		t.Errorf("timeout = %v, want %v", tool.timeout, 30*time.Second)
	}
}

func TestNewScreenCaptureTool_NilCaller(t *testing.T) {
	tool := NewScreenCaptureTool("/tmp", nil)
	if tool == nil {
		t.Fatal("NewScreenCaptureTool with nil caller returned nil")
	}
	if tool.mcpCaller != nil {
		t.Error("mcpCaller should be nil")
	}
}

func TestScreenCaptureTool_Name(t *testing.T) {
	tool := NewScreenCaptureTool("/tmp", nil)
	if tool.Name() != "screen_capture" {
		t.Errorf("Name() = %q, want %q", tool.Name(), "screen_capture")
	}
}

func TestScreenCaptureTool_Description(t *testing.T) {
	tool := NewScreenCaptureTool("/tmp", nil)
	desc := tool.Description()
	if desc == "" {
		t.Error("Description() returned empty string")
	}
	if !strings.Contains(desc, "full_screen") {
		t.Error("Description should mention full_screen")
	}
	if !strings.Contains(desc, "region") {
		t.Error("Description should mention region")
	}
	if !strings.Contains(desc, "window") {
		t.Error("Description should mention window")
	}
}

func TestScreenCaptureTool_Parameters(t *testing.T) {
	tool := NewScreenCaptureTool("/tmp", nil)
	params := tool.Parameters()

	if params["type"] != "object" {
		t.Errorf("parameters type = %v, want object", params["type"])
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("properties is not a map")
	}

	expectedProps := []string{"mode", "x", "y", "width", "height", "window_title", "hwnd", "format"}
	for _, key := range expectedProps {
		if _, exists := props[key]; !exists {
			t.Errorf("missing property %q in parameters", key)
		}
	}

	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("required is not a []string")
	}
	if len(required) != 1 || required[0] != "mode" {
		t.Errorf("required = %v, want [mode]", required)
	}
}

func TestScreenCaptureTool_SetTimeout(t *testing.T) {
	tool := NewScreenCaptureTool("/tmp", nil)
	newTimeout := 60 * time.Second
	tool.SetTimeout(newTimeout)
	if tool.timeout != newTimeout {
		t.Errorf("timeout = %v, want %v", tool.timeout, newTimeout)
	}
}

// --- Screen Execute dispatch tests ---

func TestScreenCaptureTool_Execute_MissingMode(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)
	result := tool.Execute(context.Background(), map[string]interface{}{})
	if !result.IsError {
		t.Error("expected error result")
	}
	if !strings.Contains(result.ForLLM, "mode") {
		t.Errorf("error should mention 'mode', got: %s", result.ForLLM)
	}
}

func TestScreenCaptureTool_Execute_ModeNotString(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": 123,
	})
	if !result.IsError {
		t.Error("expected error when mode is not a string")
	}
}

func TestScreenCaptureTool_Execute_UnknownMode(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "nonexistent",
	})
	if !result.IsError {
		t.Error("expected error for unknown mode")
	}
	if !strings.Contains(result.ForLLM, "unknown capture mode") {
		t.Errorf("error should mention 'unknown capture mode', got: %s", result.ForLLM)
	}
}

// --- Screen full_screen tests ---

func TestScreenCaptureTool_Execute_FullScreen_MCP(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "captured via MCP", nil
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "full_screen",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "capture_screenshot_to_file" {
		t.Errorf("tool name = %q, want %q", mock.lastToolName, "capture_screenshot_to_file")
	}
	outputPath := mock.lastArgs["file_path"].(string)
	if !strings.Contains(outputPath, "screenshot_") {
		t.Errorf("file_path = %q, should contain 'screenshot_'", outputPath)
	}
}

func TestScreenCaptureTool_Execute_FullScreen_MCPFallsBackToPS(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("MCP error")
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "full_screen",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestScreenCaptureTool_Execute_FullScreen_NoMCP(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "full_screen",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
	if !result.IsError {
		if !strings.Contains(result.ForLLM, "Screenshot saved") {
			t.Errorf("result should mention saved file, got: %s", result.ForLLM)
		}
	}
}

// --- Screen region tests ---

func TestScreenCaptureTool_Execute_Region_MCP(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "region captured", nil
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":   "region",
		"x":      10,
		"y":      20,
		"width":  100,
		"height": 200,
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "100x200") {
		t.Errorf("result should contain region dimensions, got: %s", result.ForLLM)
	}
	if mock.lastArgs["x"] != 10 || mock.lastArgs["y"] != 20 {
		t.Errorf("coords = %v,%v, want 10,20", mock.lastArgs["x"], mock.lastArgs["y"])
	}
}

func TestScreenCaptureTool_Execute_Region_Float64Coords(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "ok", nil
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":   "region",
		"x":      float64(10),
		"y":      float64(20),
		"width":  float64(100),
		"height": float64(200),
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
}

func TestScreenCaptureTool_Execute_Region_MissingCoords(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "region",
	})
	if !result.IsError {
		t.Error("expected error for missing region coords")
	}
	if !strings.Contains(result.ForLLM, "x") || !strings.Contains(result.ForLLM, "y") {
		t.Errorf("error should mention x,y coords, got: %s", result.ForLLM)
	}
}

func TestScreenCaptureTool_Execute_Region_MCPFallsBackToPS(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("MCP error")
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":   "region",
		"x":      0,
		"y":      0,
		"width":  100,
		"height": 100,
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestScreenCaptureTool_Execute_Region_NoMCP(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":   "region",
		"x":      0,
		"y":      0,
		"width":  50,
		"height": 50,
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
	if !result.IsError {
		if !strings.Contains(result.ForLLM, "Screenshot saved") {
			t.Errorf("result should mention saved file, got: %s", result.ForLLM)
		}
		// Verify temp dir was created and has files
		tempDir := filepath.Join(tmpDir, "temp")
		entries, err := os.ReadDir(tempDir)
		if err != nil {
			t.Errorf("failed to read temp dir: %v", err)
		}
		if len(entries) == 0 {
			t.Error("expected at least one file in temp dir")
		}
	}
}

// --- Screen window tests ---

func TestScreenCaptureTool_Execute_Window_MCP_ByHwnd(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "window captured", nil
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "window",
		"hwnd": "HWND(0x12345)",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "capture_screenshot_to_file" {
		t.Errorf("tool name = %q, want %q", mock.lastToolName, "capture_screenshot_to_file")
	}
	if mock.lastArgs["hwnd"] != "HWND(0x12345)" {
		t.Errorf("hwnd = %v, want HWND(0x12345)", mock.lastArgs["hwnd"])
	}
}

func TestScreenCaptureTool_Execute_Window_MCP_ByTitle(t *testing.T) {
	callCount := 0
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			callCount++
			if toolName == "find_window_by_title" {
				return `{"hwnd":"HWND(0xABCD)"}`, nil
			}
			if toolName == "capture_screenshot_to_file" {
				return "window captured", nil
			}
			return "", fmt.Errorf("unexpected tool: %s", toolName)
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":         "window",
		"window_title": "Notepad",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if callCount != 2 {
		t.Errorf("callCount = %d, want 2 (find + capture)", callCount)
	}
}

func TestScreenCaptureTool_Execute_Window_MCP_HwndOverridesTitle(t *testing.T) {
	callCount := 0
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			callCount++
			if toolName == "capture_screenshot_to_file" {
				return "captured", nil
			}
			return "", fmt.Errorf("unexpected tool: %s", toolName)
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	// When both hwnd and window_title are provided, hwnd is used directly
	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":         "window",
		"hwnd":         "HWND(0x111)",
		"window_title": "Some Window",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	// Should only call capture_screenshot_to_file (not find_window_by_title)
	if callCount != 1 {
		t.Errorf("callCount = %d, want 1 (no find needed when hwnd given)", callCount)
	}
}

func TestScreenCaptureTool_Execute_Window_MCP_ByTitle_FindFails(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("not found")
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":         "window",
		"window_title": "Missing",
	})
	if !result.IsError {
		t.Error("expected error when find_window fails")
	}
}

func TestScreenCaptureTool_Execute_Window_MCP_ByTitle_BadJSON(t *testing.T) {
	callCount := 0
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			callCount++
			if toolName == "find_window_by_title" {
				return `{"title":"some window"}`, nil // no hwnd field
			}
			return "captured without hwnd", nil
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":         "window",
		"window_title": "Test",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if callCount != 2 {
		t.Errorf("callCount = %d, want 2", callCount)
	}
}

func TestScreenCaptureTool_Execute_Window_MCP_CaptureFails(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", fmt.Errorf("capture error")
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "window",
		"hwnd": "HWND(0x100)",
	})
	if !result.IsError {
		t.Error("expected error when capture fails")
	}
}

func TestScreenCaptureTool_Execute_Window_MissingParams(t *testing.T) {
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "window",
	})
	if !result.IsError {
		t.Error("expected error for missing hwnd and window_title")
	}
}

// --- Screen window fallback (PowerShell) tests ---

func TestScreenCaptureTool_Execute_Window_NoMCP_Hwnd(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "window",
		"hwnd": "HWND(0xFFFF)",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

func TestScreenCaptureTool_Execute_Window_NoMCP_Title(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("PowerShell fallback only on Windows")
	}
	tmpDir := t.TempDir()
	tool := NewScreenCaptureTool(tmpDir, nil)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":         "window",
		"window_title": "Some Window",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

// --- Screen format tests ---

func TestScreenCaptureTool_Execute_JPGFormat(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			filePath := args["file_path"].(string)
			if !strings.HasSuffix(filePath, ".jpg") {
				t.Errorf("file_path = %q, should end with .jpg", filePath)
			}
			return "captured", nil
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode":   "full_screen",
		"format": "jpg",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
}

func TestScreenCaptureTool_Execute_DefaultPNGFormat(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			filePath := args["file_path"].(string)
			if !strings.HasSuffix(filePath, ".png") {
				t.Errorf("file_path = %q, should end with .png (default)", filePath)
			}
			return "captured", nil
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "full_screen",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
}

// --- Screen temp dir creation ---

func TestScreenCaptureTool_Execute_CreatesTempDir(t *testing.T) {
	tmpDir := t.TempDir()
	workspaceDir := filepath.Join(tmpDir, "workspace")
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "ok", nil
		},
	}
	tool := NewScreenCaptureTool(workspaceDir, mock)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"mode": "full_screen",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}

	tempDir := filepath.Join(workspaceDir, "temp")
	info, err := os.Stat(tempDir)
	if err != nil {
		t.Errorf("temp dir should be created: %v", err)
	}
	if !info.IsDir() {
		t.Error("temp should be a directory")
	}
}

// --- Screen context cancellation ---

func TestScreenCaptureTool_Execute_FullScreen_CancelledContext(t *testing.T) {
	tmpDir := t.TempDir()
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", ctx.Err()
		},
	}
	tool := NewScreenCaptureTool(tmpDir, mock)

	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	result := tool.Execute(ctx, map[string]interface{}{
		"mode": "full_screen",
	})
	if result == nil {
		t.Error("expected non-nil result")
	}
}

// ==================== imageFormatEnum Tests ====================

func TestImageFormatEnum(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"png", "Png"},
		{"PNG", "Png"},
		{"jpg", "Jpeg"},
		{"jpeg", "Jpeg"},
		{"JPEG", "Jpeg"},
		{"bmp", "Bmp"},
		{"BMP", "Bmp"},
		{"gif", "Png"},
		{"tiff", "Png"},
		{"", "Png"},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := imageFormatEnum(tt.input)
			if result != tt.expected {
				t.Errorf("imageFormatEnum(%q) = %q, want %q", tt.input, result, tt.expected)
			}
		})
	}
}

// ==================== buildFullScreenScript / buildRegionScript Tests ====================

func TestScreenCaptureTool_BuildFullScreenScript(t *testing.T) {
	tool := NewScreenCaptureTool("/tmp", nil)
	script := tool.buildFullScreenScript(`C:\temp\screenshot.png`, "png")

	if !strings.Contains(script, "System.Windows.Forms") {
		t.Error("script should load System.Windows.Forms")
	}
	if !strings.Contains(script, "System.Drawing") {
		t.Error("script should load System.Drawing")
	}
	if !strings.Contains(script, "C:\\temp\\screenshot.png") {
		t.Error("script should contain the output path")
	}
	if !strings.Contains(script, "Png") {
		t.Error("script should contain Png format")
	}
}

func TestScreenCaptureTool_BuildFullScreenScript_JPG(t *testing.T) {
	tool := NewScreenCaptureTool("/tmp", nil)
	script := tool.buildFullScreenScript("/tmp/out.jpg", "jpg")

	if !strings.Contains(script, "Jpeg") {
		t.Error("script should contain Jpeg format")
	}
}

func TestScreenCaptureTool_BuildRegionScript(t *testing.T) {
	tool := NewScreenCaptureTool("/tmp", nil)
	script := tool.buildRegionScript(10, 20, 100, 200, "/tmp/region.png", "png")

	if !strings.Contains(script, "10, 20, 100, 200") {
		t.Error("script should contain the region coordinates")
	}
	if !strings.Contains(script, "/tmp/region.png") {
		t.Error("script should contain the output path")
	}
}

// ==================== CaptureMode Constants Test ====================

func TestCaptureModeConstants(t *testing.T) {
	modes := map[CaptureMode]string{
		CaptureFullScreen: "full_screen",
		CaptureRegion:     "region",
		CaptureWindow:     "window",
	}

	for mode, expected := range modes {
		if string(mode) != expected {
			t.Errorf("CaptureMode constant mismatch: got %q, want %q", string(mode), expected)
		}
	}
}

// ==================== BrowserTool Tests ====================

func TestNewBrowserTool(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewBrowserTool("/workspace", mock)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
	if tool.timeout != 60*time.Second {
		t.Errorf("Expected default timeout 60s, got %v", tool.timeout)
	}
	if tool.workspace != "/workspace" {
		t.Errorf("workspace = %q, want /workspace", tool.workspace)
	}
}

func TestNewBrowserTool_NilCaller(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
	if tool.mcpCaller != nil {
		t.Error("mcpCaller should be nil")
	}
}

func TestBrowserTool_SetTimeout(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	tool.SetTimeout(30 * time.Second)
	if tool.timeout != 30*time.Second {
		t.Errorf("Expected timeout 30s, got %v", tool.timeout)
	}
}

func TestBrowserTool_Name(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	if tool.Name() != "browser" {
		t.Errorf("Expected name 'browser', got '%s'", tool.Name())
	}
}

func TestBrowserTool_Description(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	if !strings.Contains(desc, "navigate") {
		t.Error("Description should mention navigate")
	}
}

func TestBrowserTool_Parameters(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	params := tool.Parameters()
	if params == nil {
		t.Fatal("Parameters should not be nil")
	}
	if params["type"] != "object" {
		t.Errorf("Expected type 'object', got '%v'", params["type"])
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}
	expectedProps := []string{"action", "url", "selector", "text", "value", "timeout_ms"}
	for _, p := range expectedProps {
		if _, ok := props[p]; !ok {
			t.Errorf("Missing property: %s", p)
		}
	}
}

func TestBrowserTool_Execute_MissingAction(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{})
	if !result.IsError {
		t.Error("Expected error for missing action")
	}
	if !strings.Contains(result.ForLLM, "action") {
		t.Errorf("Expected error about action, got '%s'", result.ForLLM)
	}
}

func TestBrowserTool_Execute_ActionNotString(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": 123,
	})
	if !result.IsError {
		t.Error("expected error when action is not a string")
	}
}

func TestBrowserTool_Execute_UnknownAction(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "unknown_action",
	})
	if !result.IsError {
		t.Error("Expected error for unknown action")
	}
	if !strings.Contains(result.ForLLM, "unknown browser action") {
		t.Errorf("Expected 'unknown browser action', got '%s'", result.ForLLM)
	}
}

func TestBrowserTool_Execute_Navigate_NoURL(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "navigate",
	})
	if !result.IsError {
		t.Error("Expected error for missing URL")
	}
}

func TestBrowserTool_Execute_Navigate_NoMCP(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "navigate",
		"url":    "https://example.com",
	})
	if !result.IsError {
		t.Error("Expected error when no MCP connected")
	}
	if !strings.Contains(result.ForLLM, "no MCP") {
		t.Errorf("Expected 'no MCP' error, got '%s'", result.ForLLM)
	}
}

func TestBrowserTool_Execute_Navigate_MCPDisconnected(t *testing.T) {
	mock := &mockMCPToolCaller{connected: false}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "navigate",
		"url":    "https://example.com",
	})
	if !result.IsError {
		t.Error("Expected error when MCP disconnected")
	}
}

func TestBrowserTool_Execute_Navigate_Success(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "page loaded", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "navigate",
		"url":    "https://example.com",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if mock.lastToolName != "browser_navigate" {
		t.Errorf("Expected tool 'browser_navigate', got '%s'", mock.lastToolName)
	}
	if !result.Silent {
		t.Error("navigate result should be silent")
	}
}

func TestBrowserTool_Execute_Navigate_MCPError(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", context.DeadlineExceeded
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "navigate",
		"url":    "https://example.com",
	})
	if !result.IsError {
		t.Error("Expected error when MCP call fails")
	}
}

func TestBrowserTool_Execute_Screenshot_NoMCP(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "screenshot",
	})
	if !result.IsError {
		t.Error("Expected error when no MCP connected")
	}
}

func TestBrowserTool_Execute_Screenshot_Success(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "screenshot.png", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "screenshot",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if !result.Silent {
		t.Error("Screenshot result should be silent")
	}
}

func TestBrowserTool_Execute_Click_NoSelectorOrText(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click",
	})
	if !result.IsError {
		t.Error("Expected error when no selector or text provided")
	}
}

func TestBrowserTool_Execute_Click_WithSelector(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "click",
		"selector": "#button",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if mock.lastToolName != "browser_click" {
		t.Errorf("Expected tool 'browser_click', got '%s'", mock.lastToolName)
	}
}

func TestBrowserTool_Execute_Click_WithText(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "click",
		"text":   "Submit",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if mock.lastToolName != "browser_click_text" {
		t.Errorf("Expected tool 'browser_click_text', got '%s'", mock.lastToolName)
	}
}

func TestBrowserTool_Execute_Click_SelectorPriorityOverText(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "clicked", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "click",
		"selector": "#btn",
		"text":     "Submit",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	if mock.lastToolName != "browser_click" {
		t.Errorf("tool name = %q, want %q (selector takes priority)", mock.lastToolName, "browser_click")
	}
}

func TestBrowserTool_Execute_Click_NoMCP(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "click",
		"selector": "#btn",
	})
	if !result.IsError {
		t.Error("Expected error when no MCP connected")
	}
}

func TestBrowserTool_Execute_Type_NoText(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type",
	})
	if !result.IsError {
		t.Error("Expected error when no text provided")
	}
}

func TestBrowserTool_Execute_Type_Success(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "typed", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "type",
		"text":     "hello",
		"selector": "#input",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if mock.lastArgs["selector"] != "#input" {
		t.Errorf("Expected selector to be passed, got %v", mock.lastArgs["selector"])
	}
}

func TestBrowserTool_Execute_Type_NoMCP(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "type",
		"text":   "hello",
	})
	if !result.IsError {
		t.Error("Expected error when no MCP connected")
	}
}

func TestBrowserTool_Execute_ExtractText_Success(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "page text content", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "extract_text",
		"selector": "#content",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if result.ForLLM != "page text content" {
		t.Errorf("Expected 'page text content', got '%s'", result.ForLLM)
	}
	if result.Silent {
		t.Error("extract_text should return non-silent result")
	}
}

func TestBrowserTool_Execute_ExtractText_NoSelector(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "full page text", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "extract_text",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
}

func TestBrowserTool_Execute_ExtractText_NoMCP(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "extract_text",
	})
	if !result.IsError {
		t.Error("Expected error when no MCP connected")
	}
}

func TestBrowserTool_Execute_FillForm_NoSelector(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "fill_form",
		"value":  "test",
	})
	if !result.IsError {
		t.Error("Expected error when no selector provided")
	}
}

func TestBrowserTool_Execute_FillForm_NoValue(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "fill_form",
		"selector": "#field",
	})
	if !result.IsError {
		t.Error("Expected error when no value provided")
	}
}

func TestBrowserTool_Execute_FillForm_Success(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "filled", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "fill_form",
		"selector": "#field",
		"value":    "test value",
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if mock.lastToolName != "browser_fill" {
		t.Errorf("Expected tool 'browser_fill', got '%s'", mock.lastToolName)
	}
	if !result.Silent {
		t.Error("fill_form result should be silent")
	}
}

func TestBrowserTool_Execute_WaitFor_NoSelector(t *testing.T) {
	mock := &mockMCPToolCaller{connected: true}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "wait_for_element",
	})
	if !result.IsError {
		t.Error("Expected error when no selector provided")
	}
}

func TestBrowserTool_Execute_WaitFor_Success(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "element found", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":     "wait_for_element",
		"selector":   "#content",
		"timeout_ms": float64(3000),
	})
	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if mock.lastToolName != "browser_wait_for_selector" {
		t.Errorf("Expected tool 'browser_wait_for_selector', got '%s'", mock.lastToolName)
	}
	if mock.lastArgs["timeout"] != 3000 {
		t.Errorf("Expected timeout 3000, got %v", mock.lastArgs["timeout"])
	}
}

func TestBrowserTool_Execute_WaitFor_DefaultTimeout(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "found", nil
		},
	}
	tool := NewBrowserTool("/workspace", mock)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "wait_for_element",
		"selector": "#el",
	})
	if result.IsError {
		t.Errorf("unexpected error: %s", result.ForLLM)
	}
	// Default timeout should be 5000
	if mock.lastArgs["timeout"] != 5000 {
		t.Errorf("default timeout = %v, want 5000", mock.lastArgs["timeout"])
	}
}

func TestBrowserTool_Execute_WaitFor_NoMCP(t *testing.T) {
	tool := NewBrowserTool("/workspace", nil)
	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":   "wait_for_element",
		"selector": "#content",
	})
	if !result.IsError {
		t.Error("Expected error when no MCP connected")
	}
}

func TestBrowserTool_Execute_Navigate_CancelledContext(t *testing.T) {
	mock := &mockMCPToolCaller{
		connected: true,
		callFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "", ctx.Err()
		},
	}
	tool := NewBrowserTool("/workspace", mock)

	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	result := tool.Execute(ctx, map[string]interface{}{
		"action": "navigate",
		"url":    "https://example.com",
	})
	if !result.IsError {
		t.Error("Expected error on cancelled context")
	}
}

// ==================== MCPClientCaller Tests ====================

func TestMCPClientCaller_CallTool_NilFunc(t *testing.T) {
	caller := &MCPClientCaller{}
	_, err := caller.CallTool(context.Background(), "test_tool", nil)
	if err == nil {
		t.Error("Expected error when CallToolFunc is nil")
	}
	if !strings.Contains(err.Error(), "not configured") {
		t.Errorf("Expected 'not configured' error, got '%v'", err)
	}
}

func TestMCPClientCaller_CallTool_Success(t *testing.T) {
	caller := &MCPClientCaller{
		CallToolFunc: func(ctx context.Context, toolName string, args map[string]interface{}) (string, error) {
			return "result: " + toolName, nil
		},
	}
	result, err := caller.CallTool(context.Background(), "test_tool", map[string]interface{}{"key": "val"})
	if err != nil {
		t.Errorf("Expected no error, got %v", err)
	}
	if result != "result: test_tool" {
		t.Errorf("Expected 'result: test_tool', got '%s'", result)
	}
}

func TestMCPClientCaller_IsConnected_NilFunc(t *testing.T) {
	caller := &MCPClientCaller{}
	if caller.IsConnected() {
		t.Error("Expected false when IsConnectedFunc is nil")
	}
}

func TestMCPClientCaller_IsConnected_True(t *testing.T) {
	caller := &MCPClientCaller{
		IsConnectedFunc: func() bool { return true },
	}
	if !caller.IsConnected() {
		t.Error("Expected true")
	}
}

func TestMCPClientCaller_IsConnected_False(t *testing.T) {
	caller := &MCPClientCaller{
		IsConnectedFunc: func() bool { return false },
	}
	if caller.IsConnected() {
		t.Error("Expected false")
	}
}

// ==================== extractTextFromMCPResult Tests ====================

func TestExtractTextFromMCPResult(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "single text content",
			input:    `{"content":[{"type":"text","text":"Hello World"}]}`,
			expected: "Hello World",
		},
		{
			name:     "multiple text contents",
			input:    `{"content":[{"type":"text","text":"Line 1"},{"type":"text","text":"Line 2"}]}`,
			expected: "Line 1\nLine 2",
		},
		{
			name:     "mixed content types",
			input:    `{"content":[{"type":"image","text":"ignored"},{"type":"text","text":"actual text"}]}`,
			expected: "actual text",
		},
		{
			name:     "empty text filtered",
			input:    `{"content":[{"type":"text","text":""},{"type":"text","text":"real"}]}`,
			expected: "real",
		},
		{
			name:     "invalid JSON returns raw",
			input:    "not json at all",
			expected: "not json at all",
		},
		{
			name:     "empty content array",
			input:    `{"content":[]}`,
			expected: "",
		},
		{
			name:     "no text entries",
			input:    `{"content":[{"type":"image","data":"abc"}]}`,
			expected: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := extractTextFromMCPResult(json.RawMessage(tt.input))
			if result != tt.expected {
				t.Errorf("Expected '%s', got '%s'", tt.expected, result)
			}
		})
	}
}

func TestExtractTextFromMCPResult_WhitespaceTrim(t *testing.T) {
	input := `  some plain text  `
	result := extractTextFromMCPResult(json.RawMessage(input))
	if result != "some plain text" {
		t.Errorf("Expected trimmed text, got '%s'", result)
	}
}

// ==================== BrowserAction Constants Test ====================

func TestBrowserActionConstants(t *testing.T) {
	actions := map[BrowserAction]string{
		BrowserNavigate:    "navigate",
		BrowserScreenshot:  "screenshot",
		BrowserClick:       "click",
		BrowserType:        "type",
		BrowserExtractText: "extract_text",
		BrowserFillForm:    "fill_form",
		BrowserWaitFor:     "wait_for_element",
	}

	for action, expected := range actions {
		if string(action) != expected {
			t.Errorf("BrowserAction constant mismatch: got %q, want %q", string(action), expected)
		}
	}
}
