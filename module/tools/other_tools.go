// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"fmt"
	"time"
)

// SleepTool suspends execution for a specified duration.
// This is a simple utility tool for testing and debugging purposes.
type SleepTool struct{}

func NewSleepTool() *SleepTool {
	return &SleepTool{}
}

func (t *SleepTool) Name() string {
	return "sleep"
}

func (t *SleepTool) Description() string {
	return "Suspend execution for a specified duration in seconds. Use this for testing delays, timeouts, and long-running operations."
}

func (t *SleepTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"duration": map[string]interface{}{
				"type":        "integer",
				"description": "Duration to sleep in seconds (e.g., 30 for 30 seconds, 300 for 5 minutes)",
				"minimum":     1,
				"maximum":     3600, // Maximum 1 hour
			},
		},
		"required": []string{"duration"},
	}
}

func (t *SleepTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	// Extract duration parameter
	durationSeconds, ok := args["duration"].(int)
	if !ok {
		// Try float64 (JSON numbers are often decoded as float64)
		if f, ok := args["duration"].(float64); ok {
			durationSeconds = int(f)
		} else {
			return ErrorResult("parameter 'duration' must be an integer (seconds)")
		}
	}

	// Validate duration
	if durationSeconds < 1 {
		return ErrorResult("duration must be at least 1 second")
	}
	if durationSeconds > 3600 {
		return ErrorResult("duration cannot exceed 3600 seconds (1 hour)")
	}

	// Sleep with context cancellation support
	select {
	case <-time.After(time.Duration(durationSeconds) * time.Second):
		return SilentResult(fmt.Sprintf("Slept for %d seconds", durationSeconds))
	case <-ctx.Done():
		return ErrorResult("sleep interrupted: " + ctx.Err().Error())
	}
}
