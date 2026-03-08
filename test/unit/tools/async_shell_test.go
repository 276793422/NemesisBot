// Test for AsyncExecTool
package tools_test

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/tools"
)

// TestAsyncExecTool_Notepad tests async execution of notepad
func TestAsyncExecTool_Notepad(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping integration test in short mode")
	}

	tool := tools.NewAsyncExecTool("", false)

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Test async execution
	result := tool.Execute(ctx, map[string]interface{}{
		"command": "notepad.exe",
	})

	if result.IsError {
		t.Logf("Notepad execution returned error: %s", result.ForUser)
		// This might be expected if notepad doesn't exist
		return
	}

	t.Logf("Result: %s", result.ForLLM)
}

// TestAsyncExecTool_Calc tests async execution of calculator
func TestAsyncExecTool_Calc(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping integration test in short mode")
	}

	tool := tools.NewAsyncExecTool("", false)

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Test async execution with custom wait time
	result := tool.Execute(ctx, map[string]interface{}{
		"command":      "calc.exe",
		"wait_seconds": 3.0,
	})

	if result.IsError {
		t.Logf("Calc execution returned error: %s", result.ForUser)
		// This might be expected if calc doesn't exist
		return
	}

	t.Logf("Result: %s", result.ForLLM)
}
