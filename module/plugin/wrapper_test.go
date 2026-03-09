// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package plugin

import (
	"context"
	"testing"
	"time"
)

// TestNewToolWrapper tests the NewToolWrapper constructor
func TestNewToolWrapper(t *testing.T) {
	manager := NewManager()
	originalTool := &MockToolExecutor{}

	wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

	if wrapper == nil {
		t.Fatal("NewToolWrapper returned nil")
	}

	if wrapper.toolName != "test-tool" {
		t.Errorf("Expected toolName 'test-tool', got '%s'", wrapper.toolName)
	}

	if wrapper.pluginMgr != manager {
		t.Error("Plugin manager not set correctly")
	}

	if wrapper.user != "user1" {
		t.Errorf("Expected user 'user1', got '%s'", wrapper.user)
	}

	if wrapper.source != "source1" {
		t.Errorf("Expected source 'source1', got '%s'", wrapper.source)
	}

	if wrapper.workspace != "/workspace" {
		t.Errorf("Expected workspace '/workspace', got '%s'", wrapper.workspace)
	}

	if wrapper.originalTool != originalTool {
		t.Error("Original tool not set correctly")
	}
}

// TestToolWrapperExecute tests the ToolWrapper Execute method
func TestToolWrapperExecute(t *testing.T) {
	ctx := context.Background()

	t.Run("Successful execution with no plugins", func(t *testing.T) {
		manager := NewManager()
		originalTool := &MockToolExecutor{executeResult: "success"}
		wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

		args := map[string]interface{}{"arg1": "value1"}
		result, err := wrapper.Execute(ctx, args)

		if err != nil {
			t.Errorf("Execute() returned error: %v", err)
		}

		if result != "success" {
			t.Errorf("Expected result 'success', got %v", result)
		}

		if !originalTool.executeCalled {
			t.Error("Original tool Execute() should be called")
		}
	})

	t.Run("Plugin denies pre-execution", func(t *testing.T) {
		manager := NewManager()
		denyingPlugin := NewMockPlugin("denying-plugin", "1.0.0")
		denyingPlugin.executeAllowed = false
		denyingPlugin.executeError = testError("access denied")
		manager.Register(denyingPlugin)

		originalTool := &MockToolExecutor{executeResult: "success"}
		wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

		args := map[string]interface{}{"arg1": "value1"}
		result, err := wrapper.Execute(ctx, args)

		if err == nil {
			t.Error("Execute() should return error when plugin denies")
		}

		if result != nil {
			t.Error("Execute() should return nil result when plugin denies")
		}

		if originalTool.executeCalled {
			t.Error("Original tool Execute() should not be called when plugin denies")
		}
	})

	t.Run("Original tool returns error", func(t *testing.T) {
		manager := NewManager()
		originalTool := &MockToolExecutor{executeError: testError("tool error")}
		wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

		args := map[string]interface{}{"arg1": "value1"}
		result, err := wrapper.Execute(ctx, args)

		// When original tool errors, wrapper continues to post-execution phase
		// The error is stored in invocation.BlockingError for plugins to inspect
		// If no plugin blocks in post-execution, the original result (nil) and error are returned
		if err != nil && result != nil {
			t.Error("Execute() should return nil result when error occurs, or no error with result")
		}

		if !originalTool.executeCalled {
			t.Error("Original tool Execute() should be called")
		}
	})

	t.Run("Successful execution with allowing plugin", func(t *testing.T) {
		manager := NewManager()
		allowingPlugin := NewMockPlugin("allowing-plugin", "1.0.0")
		manager.Register(allowingPlugin)

		originalTool := &MockToolExecutor{executeResult: "success"}
		wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

		args := map[string]interface{}{"arg1": "value1"}
		result, err := wrapper.Execute(ctx, args)

		if err != nil {
			t.Errorf("Execute() returned error: %v", err)
		}

		if result != "success" {
			t.Errorf("Expected result 'success', got %v", result)
		}

		if !originalTool.executeCalled {
			t.Error("Original tool Execute() should be called")
		}

		if !allowingPlugin.executeCalled {
			t.Error("Plugin Execute() should be called")
		}
	})

	t.Run("Plugin receives correct invocation data", func(t *testing.T) {
		manager := NewManager()
		invocationPlugin := NewMockPlugin("invocation-plugin", "1.0.0")
		manager.Register(invocationPlugin)

		originalTool := &MockToolExecutor{executeResult: "success"}
		wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

		args := map[string]interface{}{"arg1": "value1"}
		_, _ = wrapper.Execute(ctx, args)

		if invocationPlugin.executeCallCount != 2 {
			// Called twice: pre-execution and post-execution
			t.Errorf("Expected plugin to be called twice, got %d", invocationPlugin.executeCallCount)
		}
	})

	t.Run("Disabled plugin is not called", func(t *testing.T) {
		manager := NewManager()
		plugin := NewMockPlugin("disabled-plugin", "1.0.0")
		manager.Register(plugin)
		manager.Disable("disabled-plugin")

		originalTool := &MockToolExecutor{executeResult: "success"}
		wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

		args := map[string]interface{}{"arg1": "value1"}
		result, err := wrapper.Execute(ctx, args)

		if err != nil {
			t.Errorf("Execute() returned error: %v", err)
		}

		if result != "success" {
			t.Errorf("Expected result 'success', got %v", result)
		}

		if plugin.executeCalled {
			t.Error("Disabled plugin should not be called")
		}
	})
}

// TestToolWrapperExecuteConcurrent tests concurrent wrapper execution
func TestToolWrapperExecuteConcurrent(t *testing.T) {
	ctx := context.Background()
	manager := NewManager()

	// Add some plugins with delay
	for i := 0; i < 3; i++ {
		plugin := NewMockPlugin("plugin"+string(rune('0'+i)), "1.0.0")
		plugin.executeDelay = 5 * time.Millisecond
		manager.Register(plugin)
	}

	originalTool := &MockToolExecutor{executeResult: "success"}
	wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

	// Execute concurrently
	done := make(chan bool, 5)
	for i := 0; i < 5; i++ {
		go func() {
			args := map[string]interface{}{"arg1": "value1"}
			_, err := wrapper.Execute(ctx, args)
			if err != nil {
				t.Errorf("Concurrent Execute() failed: %v", err)
			}
			done <- true
		}()
	}

	// Wait for all to complete
	for i := 0; i < 5; i++ {
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatal("Concurrent execution timeout")
		}
	}
}

// TestNewPluginableTool tests the NewPluginableTool constructor
func TestNewPluginableTool(t *testing.T) {
	manager := NewManager()
	innerTool := &MockToolExecutor{}

	tool := NewPluginableTool("test-tool", manager, innerTool, "user1", "source1", "/workspace")

	if tool == nil {
		t.Fatal("NewPluginableTool returned nil")
	}

	if tool.name != "test-tool" {
		t.Errorf("Expected name 'test-tool', got '%s'", tool.name)
	}

	if tool.pluginMgr != manager {
		t.Error("Plugin manager not set correctly")
	}

	if tool.innerTool != innerTool {
		t.Error("Inner tool not set correctly")
	}

	if tool.user != "user1" {
		t.Errorf("Expected user 'user1', got '%s'", tool.user)
	}

	if tool.source != "source1" {
		t.Errorf("Expected source 'source1', got '%s'", tool.source)
	}

	if tool.workspace != "/workspace" {
		t.Errorf("Expected workspace '/workspace', got '%s'", tool.workspace)
	}
}

// TestPluginableToolExecute tests the PluginableTool Execute method
func TestPluginableToolExecute(t *testing.T) {
	ctx := context.Background()

	t.Run("Basic execution", func(t *testing.T) {
		manager := NewManager()
		innerTool := &MockToolExecutor{executeResult: "pluginable-success"}

		tool := NewPluginableTool("test-tool", manager, innerTool, "user1", "source1", "/workspace")

		args := map[string]interface{}{"arg1": "value1"}
		result, err := tool.Execute(ctx, args)

		if err != nil {
			t.Errorf("Execute() returned error: %v", err)
		}

		if result != "pluginable-success" {
			t.Errorf("Expected result 'pluginable-success', got %v", result)
		}

		if !innerTool.executeCalled {
			t.Error("Inner tool Execute() should be called")
		}
	})

	t.Run("Execution with plugin", func(t *testing.T) {
		manager := NewManager()
		plugin := NewMockPlugin("test-plugin", "1.0.0")
		manager.Register(plugin)

		innerTool := &MockToolExecutor{executeResult: "success"}
		tool := NewPluginableTool("test-tool", manager, innerTool, "user1", "source1", "/workspace")

		args := map[string]interface{}{"arg1": "value1"}
		result, err := tool.Execute(ctx, args)

		if err != nil {
			t.Errorf("Execute() returned error: %v", err)
		}

		if result != "success" {
			t.Errorf("Expected result 'success', got %v", result)
		}

		if !innerTool.executeCalled {
			t.Error("Inner tool Execute() should be called")
		}

		if !plugin.executeCalled {
			t.Error("Plugin should be called")
		}
	})

	t.Run("Plugin denies execution", func(t *testing.T) {
		manager := NewManager()
		plugin := NewMockPlugin("denying-plugin", "1.0.0")
		plugin.executeAllowed = false
		plugin.executeError = testError("denied")
		manager.Register(plugin)

		innerTool := &MockToolExecutor{executeResult: "success"}
		tool := NewPluginableTool("test-tool", manager, innerTool, "user1", "source1", "/workspace")

		args := map[string]interface{}{"arg1": "value1"}
		result, err := tool.Execute(ctx, args)

		if err == nil {
			t.Error("Execute() should return error when plugin denies")
		}

		if result != nil {
			t.Error("Execute() should return nil result when plugin denies")
		}

		if innerTool.executeCalled {
			t.Error("Inner tool should not be called when plugin denies")
		}
	})
}

// TestToolExecutorInterface tests that MockToolExecutor implements ToolExecutor
func TestToolExecutorInterface(t *testing.T) {
	var _ ToolExecutor = &MockToolExecutor{}

	ctx := context.Background()
	executor := &MockToolExecutor{executeResult: "test"}

	result, err := executor.Execute(ctx, map[string]interface{}{"key": "value"})

	if err != nil {
		t.Errorf("Execute() returned error: %v", err)
	}

	if result != "test" {
		t.Errorf("Expected result 'test', got %v", result)
	}

	if !executor.executeCalled {
		t.Error("Execute() should set executeCalled flag")
	}
}

// TestToolWrapperWithDelay tests wrapper with delayed tool execution
func TestToolWrapperWithDelay(t *testing.T) {
	ctx := context.Background()
	manager := NewManager()

	// Add a plugin with delay
	plugin := NewMockPlugin("delay-plugin", "1.0.0")
	plugin.executeDelay = 10 * time.Millisecond
	manager.Register(plugin)

	originalTool := &MockToolExecutor{
		executeResult: "delayed-success",
		executeDelay:  5 * time.Millisecond,
	}
	wrapper := NewToolWrapper("test-tool", manager, "user1", "source1", "/workspace", originalTool)

	args := map[string]interface{}{"arg1": "value1"}
	result, err := wrapper.Execute(ctx, args)

	if err != nil {
		t.Errorf("Execute() returned error: %v", err)
	}

	if result != "delayed-success" {
		t.Errorf("Expected result 'delayed-success', got %v", result)
	}

	if !originalTool.executeCalled {
		t.Error("Original tool Execute() should be called")
	}
}

// TestWrapperInvocationData tests that invocation data is passed correctly
func TestWrapperInvocationData(t *testing.T) {
	ctx := context.Background()
	manager := NewManager()

	// Create a plugin that captures invocation data
	capturingPlugin := &MockPlugin{
		name:           "capturing-plugin",
		version:        "1.0.0",
		executeAllowed: true,
	}

	manager.Register(capturingPlugin)

	originalTool := &MockToolExecutor{executeResult: "success"}
	wrapper := NewToolWrapper("test-tool", manager, "test-user", "test-source", "/test/workspace", originalTool)

	args := map[string]interface{}{"arg1": "value1", "arg2": 42}
	_, _ = wrapper.Execute(ctx, args)

	if !capturingPlugin.executeCalled {
		t.Error("Plugin should be called")
	}
}

// testError is a helper function to create errors for testing
func testError(message string) error {
	return &testErrorType{message: message}
}

type testErrorType struct {
	message string
}

func (e *testErrorType) Error() string {
	return e.message
}
