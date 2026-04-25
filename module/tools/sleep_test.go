// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"strings"
	"testing"
	"time"
)

// ==================== SleepTool Tests ====================

func TestSleepTool_New(t *testing.T) {
	tool := NewSleepTool()
	if tool == nil {
		t.Fatal("Expected non-nil tool")
	}
}

func TestSleepTool_Name(t *testing.T) {
	tool := NewSleepTool()
	if tool.Name() != "sleep" {
		t.Errorf("Expected name 'sleep', got '%s'", tool.Name())
	}
}

func TestSleepTool_Description(t *testing.T) {
	tool := NewSleepTool()
	desc := tool.Description()
	if desc == "" {
		t.Error("Description should not be empty")
	}
	if !strings.Contains(desc, "uspend") {
		t.Errorf("Description should mention suspend/Suspend, got '%s'", desc)
	}
}

func TestSleepTool_Parameters(t *testing.T) {
	tool := NewSleepTool()
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
	if _, ok := props["duration"]; !ok {
		t.Error("Missing property: duration")
	}

	required, ok := params["required"].([]string)
	if !ok {
		t.Fatal("Required should be a string slice")
	}
	if len(required) != 1 || required[0] != "duration" {
		t.Errorf("Expected required=['duration'], got %v", required)
	}
}

func TestSleepTool_Execute_Success(t *testing.T) {
	tool := NewSleepTool()
	ctx := context.Background()

	start := time.Now()
	result := tool.Execute(ctx, map[string]interface{}{
		"duration": float64(1),
	})
	elapsed := time.Since(start)

	if result.IsError {
		t.Errorf("Expected success, got error: %s", result.ForLLM)
	}
	if !result.Silent {
		t.Error("SleepTool result should be silent")
	}
	if elapsed < 900*time.Millisecond {
		t.Errorf("Expected at least 1s sleep, got %v", elapsed)
	}
}

func TestSleepTool_Execute_Float64Duration(t *testing.T) {
	tool := NewSleepTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"duration": float64(1),
	})

	if result.IsError {
		t.Errorf("Expected success with float64 duration, got error: %s", result.ForLLM)
	}
}

func TestSleepTool_Execute_IntDuration(t *testing.T) {
	tool := NewSleepTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"duration": 1,
	})

	if result.IsError {
		t.Errorf("Expected success with int duration, got error: %s", result.ForLLM)
	}
}

func TestSleepTool_Execute_MissingDuration(t *testing.T) {
	tool := NewSleepTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{})

	if !result.IsError {
		t.Error("Expected error for missing duration")
	}
	if !strings.Contains(result.ForLLM, "must be an integer") {
		t.Errorf("Expected error about integer type, got '%s'", result.ForLLM)
	}
}

func TestSleepTool_Execute_InvalidType(t *testing.T) {
	tool := NewSleepTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"duration": "not_a_number",
	})

	if !result.IsError {
		t.Error("Expected error for invalid duration type")
	}
}

func TestSleepTool_Execute_ZeroDuration(t *testing.T) {
	tool := NewSleepTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"duration": 0,
	})

	if !result.IsError {
		t.Error("Expected error for zero duration")
	}
	if !strings.Contains(result.ForLLM, "at least 1") {
		t.Errorf("Expected error about minimum, got '%s'", result.ForLLM)
	}
}

func TestSleepTool_Execute_NegativeDuration(t *testing.T) {
	tool := NewSleepTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"duration": -5,
	})

	if !result.IsError {
		t.Error("Expected error for negative duration")
	}
}

func TestSleepTool_Execute_ExceedsMax(t *testing.T) {
	tool := NewSleepTool()
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"duration": 5000,
	})

	if !result.IsError {
		t.Error("Expected error for duration exceeding max")
	}
	if !strings.Contains(result.ForLLM, "3600") {
		t.Errorf("Expected error about max 3600, got '%s'", result.ForLLM)
	}
}

func TestSleepTool_Execute_ContextCancellation(t *testing.T) {
	tool := NewSleepTool()
	ctx, cancel := context.WithCancel(context.Background())

	// Cancel after 100ms
	go func() {
		time.Sleep(100 * time.Millisecond)
		cancel()
	}()

	start := time.Now()
	result := tool.Execute(ctx, map[string]interface{}{
		"duration": 60, // 60 seconds
	})
	elapsed := time.Since(start)

	if !result.IsError {
		t.Error("Expected error from context cancellation")
	}
	if !strings.Contains(result.ForLLM, "interrupted") {
		t.Errorf("Expected 'interrupted' in error, got '%s'", result.ForLLM)
	}
	// Should be cancelled quickly, not wait the full 60s
	if elapsed > 5*time.Second {
		t.Errorf("Sleep should have been cancelled quickly, took %v", elapsed)
	}
}
