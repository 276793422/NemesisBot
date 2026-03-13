// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package plugin

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"
)

// MockPlugin is a test implementation of the Plugin interface
type MockPlugin struct {
	name             string
	version          string
	initCalled       bool
	initError        error
	executeCalled    bool
	executeAllowed   bool
	executeError     error
	executeModified  bool
	cleanupCalled    bool
	cleanupError     error
	executeDelay     time.Duration
	executeCallCount int
}

func NewMockPlugin(name, version string) *MockPlugin {
	return &MockPlugin{
		name:            name,
		version:         version,
		executeAllowed:  true,
		executeModified: false,
	}
}

func (m *MockPlugin) Name() string {
	return m.name
}

func (m *MockPlugin) Version() string {
	return m.version
}

func (m *MockPlugin) Init(config map[string]interface{}) error {
	m.initCalled = true
	return m.initError
}

func (m *MockPlugin) Execute(ctx context.Context, invocation *ToolInvocation) (bool, error, bool) {
	m.executeCalled = true
	m.executeCallCount++
	if m.executeDelay > 0 {
		time.Sleep(m.executeDelay)
	}
	return m.executeAllowed, m.executeError, m.executeModified
}

func (m *MockPlugin) Cleanup() error {
	m.cleanupCalled = true
	return m.cleanupError
}

// MockToolExecutor is a test implementation of ToolExecutor
type MockToolExecutor struct {
	executeCalled bool
	executeResult interface{}
	executeError  error
	executeDelay  time.Duration
}

func (m *MockToolExecutor) Execute(ctx context.Context, args map[string]interface{}) (interface{}, error) {
	m.executeCalled = true
	if m.executeDelay > 0 {
		time.Sleep(m.executeDelay)
	}
	return m.executeResult, m.executeError
}

// Test NewBasePlugin
func TestNewBasePlugin(t *testing.T) {
	base := NewBasePlugin("test-plugin", "1.0.0")

	if base == nil {
		t.Fatal("NewBasePlugin returned nil")
	}

	if base.name != "test-plugin" {
		t.Errorf("Expected name 'test-plugin', got '%s'", base.name)
	}

	if base.version != "1.0.0" {
		t.Errorf("Expected version '1.0.0', got '%s'", base.version)
	}

	if base.config == nil {
		t.Error("Config map should be initialized")
	}
}

// Test BasePlugin methods
func TestBasePlugin_Methods(t *testing.T) {
	base := NewBasePlugin("test", "2.0.0")

	// Test Name
	if base.Name() != "test" {
		t.Errorf("Name() returned wrong value: %s", base.Name())
	}

	// Test Version
	if base.Version() != "2.0.0" {
		t.Errorf("Version() returned wrong value: %s", base.Version())
	}

	// Test Init
	config := map[string]interface{}{"key": "value"}
	err := base.Init(config)
	if err != nil {
		t.Errorf("Init() returned error: %v", err)
	}

	if base.config["key"] != "value" {
		t.Error("Init() did not store config")
	}

	// Test Cleanup
	err = base.Cleanup()
	if err != nil {
		t.Errorf("Cleanup() returned error: %v", err)
	}
}

// Test NewManager
func TestNewManager(t *testing.T) {
	manager := NewManager()

	if manager == nil {
		t.Fatal("NewManager returned nil")
	}

	if manager.plugins == nil {
		t.Error("Plugins slice should be initialized")
	}

	if manager.enabled == nil {
		t.Error("Enabled map should be initialized")
	}

	if len(manager.plugins) != 0 {
		t.Errorf("Expected empty plugins slice, got length %d", len(manager.plugins))
	}
}

// Test Manager Register
func TestManager_Register(t *testing.T) {
	manager := NewManager()
	plugin := NewMockPlugin("test-plugin", "1.0.0")

	// Test successful registration
	err := manager.Register(plugin)
	if err != nil {
		t.Errorf("Register() returned error: %v", err)
	}

	if len(manager.plugins) != 1 {
		t.Errorf("Expected 1 plugin, got %d", len(manager.plugins))
	}

	if !manager.enabled["test-plugin"] {
		t.Error("Plugin should be enabled after registration")
	}

	// Test duplicate registration
	err = manager.Register(plugin)
	if err == nil {
		t.Error("Register() should return error for duplicate plugin")
	}

	// Test nil plugin
	err = manager.Register(nil)
	if err == nil {
		t.Error("Register() should return error for nil plugin")
	}
}

// Test Manager Unregister
func TestManager_Unregister(t *testing.T) {
	manager := NewManager()
	plugin := NewMockPlugin("test-plugin", "1.0.0")

	manager.Register(plugin)

	// Test successful unregistration
	err := manager.Unregister("test-plugin")
	if err != nil {
		t.Errorf("Unregister() returned error: %v", err)
	}

	if len(manager.plugins) != 0 {
		t.Errorf("Expected 0 plugins after unregister, got %d", len(manager.plugins))
	}

	if _, exists := manager.enabled["test-plugin"]; exists {
		t.Error("Plugin should be removed from enabled map")
	}

	if !plugin.cleanupCalled {
		t.Error("Cleanup() should be called on unregistration")
	}

	// Test unregister non-existent plugin
	err = manager.Unregister("non-existent")
	if err == nil {
		t.Error("Unregister() should return error for non-existent plugin")
	}
}

// Test Manager Enable/Disable/IsEnabled
func TestManager_EnableDisableIsEnabled(t *testing.T) {
	manager := NewManager()
	plugin := NewMockPlugin("test-plugin", "1.0.0")
	manager.Register(plugin)

	// Test IsEnabled after registration
	if !manager.IsEnabled("test-plugin") {
		t.Error("Plugin should be enabled after registration")
	}

	// Test Disable
	err := manager.Disable("test-plugin")
	if err != nil {
		t.Errorf("Disable() returned error: %v", err)
	}

	if manager.IsEnabled("test-plugin") {
		t.Error("Plugin should be disabled after Disable()")
	}

	// Test Enable
	err = manager.Enable("test-plugin")
	if err != nil {
		t.Errorf("Enable() returned error: %v", err)
	}

	if !manager.IsEnabled("test-plugin") {
		t.Error("Plugin should be enabled after Enable()")
	}

	// Test Enable non-existent plugin (should not error)
	err = manager.Enable("non-existent")
	if err != nil {
		t.Errorf("Enable() should not error for non-existent plugin: %v", err)
	}

	// Test Disable non-existent plugin (should not error)
	err = manager.Disable("non-existent")
	if err != nil {
		t.Errorf("Disable() should not error for non-existent plugin: %v", err)
	}

	// Test double disable
	err = manager.Disable("test-plugin")
	if err != nil {
		t.Errorf("Disable() should not error when already disabled: %v", err)
	}

	// Test double enable
	err = manager.Enable("test-plugin")
	if err != nil {
		t.Errorf("Enable() should not error when already enabled: %v", err)
	}
}

// Test Manager GetPlugin
func TestManager_GetPlugin(t *testing.T) {
	manager := NewManager()
	plugin := NewMockPlugin("test-plugin", "1.0.0")
	manager.Register(plugin)

	// Test getting existing plugin
	p, ok := manager.GetPlugin("test-plugin")
	if !ok {
		t.Error("GetPlugin() should find existing plugin")
	}

	if p.Name() != "test-plugin" {
		t.Errorf("Got wrong plugin: %s", p.Name())
	}

	// Test getting disabled plugin
	manager.Disable("test-plugin")
	_, ok = manager.GetPlugin("test-plugin")
	if ok {
		t.Error("GetPlugin() should not return disabled plugin")
	}

	// Test getting non-existent plugin
	_, ok = manager.GetPlugin("non-existent")
	if ok {
		t.Error("GetPlugin() should not find non-existent plugin")
	}
}

// Test Manager ListPlugins
func TestManager_ListPlugins(t *testing.T) {
	manager := NewManager()

	plugin1 := NewMockPlugin("plugin1", "1.0.0")
	plugin2 := NewMockPlugin("plugin2", "2.0.0")
	plugin3 := NewMockPlugin("plugin3", "3.0.0")

	manager.Register(plugin1)
	manager.Register(plugin2)
	manager.Register(plugin3)

	// Test listing all plugins
	plugins := manager.ListPlugins()
	if len(plugins) != 3 {
		t.Errorf("Expected 3 plugins, got %d", len(plugins))
	}

	// Test listing with disabled plugin
	manager.Disable("plugin2")
	plugins = manager.ListPlugins()
	if len(plugins) != 2 {
		t.Errorf("Expected 2 plugins after disabling one, got %d", len(plugins))
	}

	for _, p := range plugins {
		if p.Name() == "plugin2" {
			t.Error("ListPlugins() should not include disabled plugins")
		}
	}
}

// Test Manager Execute
func TestManager_Execute(t *testing.T) {
	ctx := context.Background()
	invocation := &ToolInvocation{
		ToolName:  "test-tool",
		Method:    "Execute",
		Args:      map[string]interface{}{"arg1": "value1"},
		Context:   ctx,
		User:      "test-user",
		Source:    "test-source",
		Workspace: "/test/workspace",
		Metadata:  make(map[string]interface{}),
	}

	t.Run("All plugins allow", func(t *testing.T) {
		manager := NewManager()
		plugin1 := NewMockPlugin("plugin1", "1.0.0")
		plugin2 := NewMockPlugin("plugin2", "1.0.0")

		manager.Register(plugin1)
		manager.Register(plugin2)

		allowed, err := manager.Execute(ctx, invocation)
		if !allowed {
			t.Error("Execute() should return true when all plugins allow")
		}

		if err != nil {
			t.Errorf("Execute() should not return error when all plugins allow: %v", err)
		}

		if !plugin1.executeCalled {
			t.Error("Plugin1 Execute() should be called")
		}

		if !plugin2.executeCalled {
			t.Error("Plugin2 Execute() should be called")
		}
	})

	t.Run("Plugin denies operation", func(t *testing.T) {
		manager := NewManager()
		plugin1 := NewMockPlugin("plugin1", "1.0.0")
		plugin2 := NewMockPlugin("plugin2", "1.0.0")
		plugin2.executeAllowed = false
		plugin2.executeError = errors.New("access denied")

		manager.Register(plugin1)
		manager.Register(plugin2)

		allowed, err := manager.Execute(ctx, invocation)
		if allowed {
			t.Error("Execute() should return false when plugin denies")
		}

		if err == nil {
			t.Error("Execute() should return error when plugin denies")
		}

		if !plugin1.executeCalled {
			t.Error("Plugin1 Execute() should be called before denial")
		}

		// Second plugin should also be called even though first one allowed
		if !plugin2.executeCalled {
			t.Error("Plugin2 Execute() should be called")
		}
	})

	t.Run("Disabled plugin not called", func(t *testing.T) {
		manager := NewManager()
		plugin1 := NewMockPlugin("plugin1", "1.0.0")
		plugin2 := NewMockPlugin("plugin2", "1.0.0")
		plugin2.executeAllowed = false

		manager.Register(plugin1)
		manager.Register(plugin2)
		manager.Disable("plugin2")

		allowed, err := manager.Execute(ctx, invocation)
		if !allowed {
			t.Error("Execute() should succeed when denying plugin is disabled")
		}

		if err != nil {
			t.Errorf("Execute() should not error with disabled plugin: %v", err)
		}

		if plugin2.executeCalled {
			t.Error("Disabled plugin Execute() should not be called")
		}
	})

	t.Run("Blocking error stops execution", func(t *testing.T) {
		manager := NewManager()
		plugin1 := NewMockPlugin("plugin1", "1.0.0")
		plugin2 := NewMockPlugin("plugin2", "1.0.0")

		manager.Register(plugin1)
		manager.Register(plugin2)

		blockingErr := errors.New("blocking error")
		invocation.BlockingError = blockingErr

		allowed, err := manager.Execute(ctx, invocation)
		if allowed {
			t.Error("Execute() should return false when blocking error is set")
		}

		if err != blockingErr {
			t.Errorf("Execute() should return blocking error: %v", err)
		}
	})

	t.Run("Plugin denies without error", func(t *testing.T) {
		manager := NewManager()
		plugin := NewMockPlugin("plugin1", "1.0.0")
		plugin.executeAllowed = false

		manager.Register(plugin)

		allowed, err := manager.Execute(ctx, invocation)
		if allowed {
			t.Error("Execute() should return false when plugin denies")
		}

		if err == nil {
			t.Error("Execute() should return error when plugin denies, even if plugin doesn't provide one")
		}
	})
}

// Test Manager Execute Concurrent
func TestManager_Execute_Concurrent(t *testing.T) {
	ctx := context.Background()
	manager := NewManager()

	// Register multiple plugins
	for i := 0; i < 5; i++ {
		plugin := NewMockPlugin("plugin"+string(rune('0'+i)), "1.0.0")
		plugin.executeDelay = 10 * time.Millisecond
		manager.Register(plugin)
	}

	// Execute concurrent requests
	var wg sync.WaitGroup
	errors := make(chan error, 10)

	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(iteration int) {
			defer wg.Done()
			invocation := &ToolInvocation{
				ToolName: "test-tool",
				Method:   "Execute",
				Args:     map[string]interface{}{"iter": iteration},
				Context:  ctx,
				Metadata: make(map[string]interface{}),
			}

			allowed, err := manager.Execute(ctx, invocation)
			if !allowed || err != nil {
				errors <- err
			}
		}(i)
	}

	wg.Wait()
	close(errors)

	for err := range errors {
		t.Errorf("Concurrent execution failed: %v", err)
	}
}

// Test Manager Cleanup
func TestManager_Cleanup(t *testing.T) {
	t.Run("Cleanup all plugins", func(t *testing.T) {
		manager := NewManager()
		plugin1 := NewMockPlugin("plugin1", "1.0.0")
		plugin2 := NewMockPlugin("plugin2", "1.0.0")

		manager.Register(plugin1)
		manager.Register(plugin2)

		err := manager.Cleanup()
		if err != nil {
			t.Errorf("Cleanup() returned error: %v", err)
		}

		if !plugin1.cleanupCalled {
			t.Error("Plugin1 Cleanup() should be called")
		}

		if !plugin2.cleanupCalled {
			t.Error("Plugin2 Cleanup() should be called")
		}

		if manager.plugins != nil {
			t.Error("Plugins should be nil after cleanup")
		}

		if manager.enabled != nil {
			t.Error("Enabled should be nil after cleanup")
		}
	})

	t.Run("Cleanup with plugin error", func(t *testing.T) {
		manager := NewManager()
		plugin1 := NewMockPlugin("plugin1", "1.0.0")
		plugin2 := NewMockPlugin("plugin2", "1.0.0")
		plugin2.cleanupError = errors.New("cleanup error")

		manager.Register(plugin1)
		manager.Register(plugin2)

		err := manager.Cleanup()
		// Cleanup should not return error even if individual plugins fail
		if err != nil {
			t.Errorf("Cleanup() should not return error: %v", err)
		}

		if !plugin1.cleanupCalled {
			t.Error("Plugin1 Cleanup() should be called even if plugin2 fails")
		}

		if !plugin2.cleanupCalled {
			t.Error("Plugin2 Cleanup() should be called even if it returns error")
		}
	})
}

// Test ToolInvocation
func TestToolInvocation(t *testing.T) {
	ctx := context.Background()
	invocation := &ToolInvocation{
		ToolName:  "test-tool",
		Method:    "Execute",
		Args:      map[string]interface{}{"arg1": "value1"},
		Context:   ctx,
		User:      "user1",
		Source:    "source1",
		Workspace: "/workspace",
		Result:    "result",
		Metadata:  make(map[string]interface{}),
	}

	if invocation.ToolName != "test-tool" {
		t.Errorf("ToolName not set correctly: %s", invocation.ToolName)
	}

	if invocation.Method != "Execute" {
		t.Errorf("Method not set correctly: %s", invocation.Method)
	}

	if invocation.Result != "result" {
		t.Errorf("Result not set correctly: %v", invocation.Result)
	}

	invocation.Metadata["key"] = "value"
	if invocation.Metadata["key"] != "value" {
		t.Error("Metadata not working correctly")
	}
}

// Test Edge Cases
func TestEdgeCases(t *testing.T) {
	t.Run("Manager with no plugins", func(t *testing.T) {
		manager := NewManager()
		ctx := context.Background()
		invocation := &ToolInvocation{
			ToolName: "test-tool",
			Metadata: make(map[string]interface{}),
		}

		allowed, err := manager.Execute(ctx, invocation)
		if !allowed {
			t.Error("Execute() should return true with no plugins")
		}

		if err != nil {
			t.Errorf("Execute() should not error with no plugins: %v", err)
		}
	})

	t.Run("ListPlugins with no plugins", func(t *testing.T) {
		manager := NewManager()
		plugins := manager.ListPlugins()

		if len(plugins) != 0 {
			t.Errorf("Expected empty list, got %d plugins", len(plugins))
		}
	})

	t.Run("Cleanup empty manager", func(t *testing.T) {
		manager := NewManager()
		err := manager.Cleanup()

		if err != nil {
			t.Errorf("Cleanup() should not error with no plugins: %v", err)
		}
	})

	t.Run("Plugin with empty name", func(t *testing.T) {
		plugin := NewMockPlugin("", "1.0.0")
		manager := NewManager()

		err := manager.Register(plugin)
		if err != nil {
			t.Errorf("Register() should allow plugin with empty name: %v", err)
		}
	})

	t.Run("Plugin with special characters in name", func(t *testing.T) {
		plugin := NewMockPlugin("plugin-with-special-chars-123", "1.0.0")
		manager := NewManager()

		err := manager.Register(plugin)
		if err != nil {
			t.Errorf("Register() should allow plugin with special characters: %v", err)
		}

		found := false
		for _, p := range manager.plugins {
			if p.Name() == "plugin-with-special-chars-123" {
				found = true
				break
			}
		}

		if !found {
			t.Error("Plugin with special characters not found in manager")
		}
	})
}
