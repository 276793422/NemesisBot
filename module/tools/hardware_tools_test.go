// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"testing"
)

// ==================== WebSearchTool Tests ====================

func TestNewWebSearchTool(t *testing.T) {
	opts := WebSearchToolOptions{
		DuckDuckGoEnabled: true,
	}
	tool := NewWebSearchTool(opts)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestWebSearchTool_Name(t *testing.T) {
	tool := &WebSearchTool{}
	if tool.Name() != "web_search" {
		t.Errorf("Expected name 'web_search', got '%s'", tool.Name())
	}
}

func TestWebSearchTool_Description(t *testing.T) {
	tool := &WebSearchTool{}
	desc := tool.Description()

	if desc == "" {
		t.Error("Description should not be empty")
	}

	if !contains(desc, "search") && !contains(desc, "web") {
		t.Error("Description should mention search or web")
	}
}

func TestWebSearchTool_Parameters(t *testing.T) {
	tool := &WebSearchTool{}
	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check for expected properties
	expectedProps := []string{"query", "count"}
	for _, prop := range expectedProps {
		if _, ok := props[prop]; !ok {
			t.Errorf("Missing property: %s", prop)
		}
	}

	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 1 || required[0] != "query" {
		t.Errorf("Expected required ['query'], got %v", required)
	}
}

func TestWebSearchTool_Execute_MissingQuery(t *testing.T) {
	opts := WebSearchToolOptions{
		DuckDuckGoEnabled: true,
	}
	tool := NewWebSearchTool(opts)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})

	if !result.IsError {
		t.Error("Expected error for missing query")
	}
}

func TestWebSearchTool_Execute_EmptyQuery(t *testing.T) {
	opts := WebSearchToolOptions{
		DuckDuckGoEnabled: true,
	}
	tool := NewWebSearchTool(opts)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"query": "",
	})

	if !result.IsError {
		t.Error("Expected error for empty query")
	}
}

func TestWebSearchTool_Execute_InvalidCount(t *testing.T) {
	opts := WebSearchToolOptions{
		DuckDuckGoEnabled: true,
	}
	tool := NewWebSearchTool(opts)
	ctx := context.Background()

	// Test with negative count
	result := tool.Execute(ctx, map[string]interface{}{
		"query": "test",
		"count": -1,
	})

	// Should handle gracefully
	_ = result

	// Test with excessive count
	result = tool.Execute(ctx, map[string]interface{}{
		"query": "test",
		"count": 1000,
	})

	// Should handle gracefully
	_ = result
}

// ==================== WebFetchTool Tests ====================

func TestNewWebFetchTool(t *testing.T) {
	tool := NewWebFetchTool(50000)

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestWebFetchTool_Name(t *testing.T) {
	tool := &WebFetchTool{}
	if tool.Name() != "web_fetch" {
		t.Errorf("Expected name 'web_fetch', got '%s'", tool.Name())
	}
}

func TestWebFetchTool_Description(t *testing.T) {
	tool := &WebFetchTool{}
	desc := tool.Description()

	if desc == "" {
		t.Error("Description should not be empty")
	}

	if !contains(desc, "fetch") && !contains(desc, "URL") {
		t.Error("Description should mention fetch or URL")
	}
}

func TestWebFetchTool_Parameters(t *testing.T) {
	tool := &WebFetchTool{}
	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check for expected properties (WebFetchTool uses "maxChars" not "max_length")
	expectedProps := []string{"url", "maxChars"}
	for _, prop := range expectedProps {
		if _, ok := props[prop]; !ok {
			t.Errorf("Missing property: %s", prop)
		}
	}

	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}

	if len(required) != 1 || required[0] != "url" {
		t.Errorf("Expected required ['url'], got %v", required)
	}
}

func TestWebFetchTool_Execute_MissingURL(t *testing.T) {
	tool := NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})

	if !result.IsError {
		t.Error("Expected error for missing URL")
	}
}

func TestWebFetchTool_Execute_InvalidURL(t *testing.T) {
	tool := NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": "not-a-valid-url",
	})

	// Should handle gracefully (may error or return empty)
	_ = result
}

func TestWebFetchTool_Execute_EmptyURL(t *testing.T) {
	tool := NewWebFetchTool(50000)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"url": "",
	})

	if !result.IsError {
		t.Error("Expected error for empty URL")
	}
}

func TestWebFetchTool_Execute_InvalidMaxLength(t *testing.T) {
	tool := NewWebFetchTool(50000)
	ctx := context.Background()

	// Test with negative max_length
	result := tool.Execute(ctx, map[string]interface{}{
		"url":        "https://example.com",
		"max_length": -1,
	})

	// Should handle gracefully
	_ = result
}

// ==================== I2CTool Tests ====================

func TestNewI2CTool(t *testing.T) {
	tool := NewI2CTool()

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestI2CTool_Name(t *testing.T) {
	tool := &I2CTool{}
	if tool.Name() != "i2c" {
		t.Errorf("Expected name 'i2c', got '%s'", tool.Name())
	}
}

func TestI2CTool_Description(t *testing.T) {
	tool := &I2CTool{}
	desc := tool.Description()

	if desc == "" {
		t.Error("Description should not be empty")
	}

	if !contains(desc, "I2C") {
		t.Error("Description should mention I2C")
	}
}

func TestI2CTool_Parameters(t *testing.T) {
	tool := &I2CTool{}
	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check for expected properties (I2CTool uses "length" not "count")
	expectedProps := []string{"action", "bus", "address", "register", "data", "length"}
	for _, prop := range expectedProps {
		if _, ok := props[prop]; !ok {
			t.Errorf("Missing property: %s", prop)
		}
	}

	// Check action enum
	actionProp, ok := props["action"].(map[string]interface{})
	if !ok {
		t.Fatal("Action should be a map")
	}

	enum, ok := actionProp["enum"].([]string)
	if !ok {
		t.Fatal("Action enum should be a string slice")
	}

	expectedActions := []string{"detect", "scan", "read", "write"}
	if len(enum) != len(expectedActions) {
		t.Errorf("Expected %d actions, got %d", len(expectedActions), len(enum))
	}
}

func TestI2CTool_Execute_DetectAction(t *testing.T) {
	tool := NewI2CTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "detect",
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	// Detect should not error (may return empty list on non-Linux)
	_ = result
}

func TestI2CTool_Execute_ScanAction_MissingBus(t *testing.T) {
	tool := NewI2CTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "scan",
	})

	// Scan should handle missing bus gracefully
	_ = result
}

func TestI2CTool_Execute_ReadAction_MissingParameters(t *testing.T) {
	tool := NewI2CTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "read",
	})

	// Read should handle missing parameters gracefully
	_ = result
}

func TestI2CTool_Execute_WriteAction_MissingParameters(t *testing.T) {
	tool := NewI2CTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "write",
	})

	// Write should handle missing parameters gracefully
	_ = result
}

func TestI2CTool_Execute_InvalidAction(t *testing.T) {
	tool := NewI2CTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "invalid_action",
	})

	// Should handle invalid action gracefully
	_ = result
}

// ==================== SPITool Tests ====================

func TestNewSPITool(t *testing.T) {
	tool := NewSPITool()

	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestSPITool_Name(t *testing.T) {
	tool := &SPITool{}
	if tool.Name() != "spi" {
		t.Errorf("Expected name 'spi', got '%s'", tool.Name())
	}
}

func TestSPITool_Description(t *testing.T) {
	tool := &SPITool{}
	desc := tool.Description()

	if desc == "" {
		t.Error("Description should not be empty")
	}

	if !contains(desc, "SPI") {
		t.Error("Description should mention SPI")
	}
}

func TestSPITool_Parameters(t *testing.T) {
	tool := &SPITool{}
	params := tool.Parameters()

	if params == nil {
		t.Fatal("Parameters should not be nil")
	}

	props, ok := params["properties"].(map[string]interface{})
	if !ok {
		t.Fatal("Properties should be a map")
	}

	// Check for expected properties (SPITool uses "length" not "count")
	expectedProps := []string{"action", "device", "speed", "mode", "data", "length"}
	for _, prop := range expectedProps {
		if _, ok := props[prop]; !ok {
			t.Errorf("Missing property: %s", prop)
		}
	}

	// Check action enum
	actionProp, ok := props["action"].(map[string]interface{})
	if !ok {
		t.Fatal("Action should be a map")
	}

	enum, ok := actionProp["enum"].([]string)
	if !ok {
		t.Fatal("Action enum should be a string slice")
	}

	expectedActions := []string{"list", "transfer", "read"}
	if len(enum) != len(expectedActions) {
		t.Errorf("Expected %d actions, got %d", len(expectedActions), len(enum))
	}
}

func TestSPITool_Execute_ListAction(t *testing.T) {
	tool := NewSPITool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "list",
	})

	if result == nil {
		t.Fatal("Expected non-nil result")
	}

	// List should not error (may return empty list on non-Linux)
	_ = result
}

func TestSPITool_Execute_TransferAction_MissingDevice(t *testing.T) {
	tool := NewSPITool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "transfer",
	})

	// Transfer should handle missing device gracefully
	_ = result
}

func TestSPITool_Execute_ReadAction_MissingDevice(t *testing.T) {
	tool := NewSPITool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "read",
	})

	// Read should handle missing device gracefully
	_ = result
}

func TestSPITool_Execute_InvalidAction(t *testing.T) {
	tool := NewSPITool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"action": "invalid_action",
	})

	// Should handle invalid action gracefully
	_ = result
}

// ==================== Integration-Style Tests ====================

func TestWebTools_ParameterValidation(t *testing.T) {
	t.Run("WebSearch with various query types", func(t *testing.T) {
		opts := WebSearchToolOptions{
			DuckDuckGoEnabled: true,
		}
		tool := NewWebSearchTool(opts)
		ctx := context.Background()

		testQueries := []interface{}{
			"simple query",
			"",
			123,
			nil,
		}

		for _, query := range testQueries {
			result := tool.Execute(ctx, map[string]interface{}{
				"query": query,
			})
			// Just verify no crashes
			_ = result
		}
	})

	t.Run("WebFetch with various URL types", func(t *testing.T) {
		tool := NewWebFetchTool(50000)
		ctx := context.Background()

		testURLs := []interface{}{
			"https://example.com",
			"http://example.com",
			"ftp://example.com",
			"not-a-url",
			"",
			123,
		}

		for _, url := range testURLs {
			result := tool.Execute(ctx, map[string]interface{}{
				"url": url,
			})
			// Just verify no crashes
			_ = result
		}
	})
}

func TestHardwareTools_PlatformHandling(t *testing.T) {
	t.Run("I2C tool on all platforms", func(t *testing.T) {
		tool := NewI2CTool()
		ctx := context.Background()

		// All actions should be handled without crashing
		actions := []string{"detect", "scan", "read", "write", "invalid"}
		for _, action := range actions {
			result := tool.Execute(ctx, map[string]interface{}{
				"action": action,
			})
			_ = result
		}
	})

	t.Run("SPI tool on all platforms", func(t *testing.T) {
		tool := NewSPITool()
		ctx := context.Background()

		// All actions should be handled without crashing
		actions := []string{"list", "transfer", "read", "invalid"}
		for _, action := range actions {
			result := tool.Execute(ctx, map[string]interface{}{
				"action": action,
			})
			_ = result
		}
	})
}

func TestWebTools_EdgeCases(t *testing.T) {
	t.Run("WebSearch with special characters", func(t *testing.T) {
		opts := WebSearchToolOptions{
			DuckDuckGoEnabled: true,
		}
		tool := NewWebSearchTool(opts)
		ctx := context.Background()

		specialQueries := []string{
			"query with spaces",
			"query-with-dashes",
			"query_with_underscores",
			"query@with$special#chars",
			"查询中文",           // Chinese characters
			"🔍 emoji search", // Emoji
		}

		for _, query := range specialQueries {
			result := tool.Execute(ctx, map[string]interface{}{
				"query": query,
			})
			// Just verify no crashes
			_ = result
		}
	})

	t.Run("WebFetch with various URLs", func(t *testing.T) {
		tool := NewWebFetchTool(50000)
		ctx := context.Background()

		testURLs := []string{
			"https://example.com/path?query=value",
			"https://example.com:8080/path",
			"https://user:pass@example.com/path",
			"https://example.com/path#fragment",
			"https://example.com/path/with/multiple/segments",
		}

		for _, url := range testURLs {
			result := tool.Execute(ctx, map[string]interface{}{
				"url": url,
			})
			// Just verify no crashes
			_ = result
		}
	})
}

func TestHardwareTools_ParameterValidation(t *testing.T) {
	t.Run("I2C tool with various parameter types", func(t *testing.T) {
		tool := NewI2CTool()
		ctx := context.Background()

		testCases := []map[string]interface{}{
			{"action": "scan", "bus": "1"},
			{"action": "scan", "bus": 1},
			{"action": "read", "bus": "1", "address": 0x48},
			{"action": "read", "bus": "1", "address": 72},
			{"action": "write", "bus": "1", "address": 0x48, "data": "0x01"},
			{"action": "write", "bus": "1", "address": 72, "data": "1"},
		}

		for _, params := range testCases {
			result := tool.Execute(ctx, params)
			_ = result
		}
	})

	t.Run("SPI tool with various parameter types", func(t *testing.T) {
		tool := NewSPITool()
		ctx := context.Background()

		testCases := []map[string]interface{}{
			{"action": "transfer", "device": "0.0", "data": "0x01"},
			{"action": "transfer", "device": "0.0", "data": "1"},
			{"action": "read", "device": "0.0", "count": 10},
			{"action": "read", "device": "0.0", "count": 10.5},
			{"action": "transfer", "device": "0.0", "speed": 1000000},
			{"action": "transfer", "device": "0.0", "mode": 0},
		}

		for _, params := range testCases {
			result := tool.Execute(ctx, params)
			_ = result
		}
	})
}

// Benchmark tests
func BenchmarkWebSearchTool_Execute(b *testing.B) {
	opts := WebSearchToolOptions{
		DuckDuckGoEnabled: true,
	}
	tool := NewWebSearchTool(opts)
	ctx := context.Background()

	for i := 0; i < b.N; i++ {
		result := tool.Execute(ctx, map[string]interface{}{
			"query": "test query",
		})
		_ = result
	}
}

func BenchmarkWebFetchTool_Execute(b *testing.B) {
	tool := NewWebFetchTool(50000)
	ctx := context.Background()

	for i := 0; i < b.N; i++ {
		result := tool.Execute(ctx, map[string]interface{}{
			"url": "https://example.com",
		})
		_ = result
	}
}

func BenchmarkI2CTool_Execute(b *testing.B) {
	tool := NewI2CTool()
	ctx := context.Background()

	for i := 0; i < b.N; i++ {
		result := tool.Execute(ctx, map[string]interface{}{
			"action": "detect",
		})
		_ = result
	}
}

func BenchmarkSPITool_Execute(b *testing.B) {
	tool := NewSPITool()
	ctx := context.Background()

	for i := 0; i < b.N; i++ {
		result := tool.Execute(ctx, map[string]interface{}{
			"action": "list",
		})
		_ = result
	}
}
