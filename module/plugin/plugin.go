// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// Package plugin provides a plugin system for NemesisBot
// Plugins can intercept and modify tool behavior without modifying core framework
package plugin

import (
	"context"
	"fmt"
)

// Plugin is the interface that all plugins must implement
type Plugin interface {
	// Name returns the plugin name
	Name() string

	// Version returns the plugin version
	Version() string

	// Init initializes the plugin with configuration
	Init(config map[string]interface{}) error

	// Execute intercepts a tool execution
	// Returns:
	//   - allowed: whether the operation should proceed
	//   - error: if the operation should be blocked with an error
	//   - modified: whether the plugin modified the request/response
	Execute(ctx context.Context, invocation *ToolInvocation) (allowed bool, err error, modified bool)

	// Cleanup is called when the plugin is being unloaded
	Cleanup() error
}

// ToolInvocation represents a tool execution request
type ToolInvocation struct {
	// Tool name
	ToolName string

	// Method being called (for future use, e.g., "Execute", "Stream")
	Method string

	// Original arguments
	Args map[string]interface{}

	// Execution context
	Context context.Context

	// User information
	User      string
	Source    string
	Workspace string

	// Result (can be modified by plugins)
	Result interface{}

	// Error (can be set by plugins to block execution)
	BlockingError error

	// Metadata for plugins to pass information
	Metadata map[string]interface{}
}

// BasePlugin provides common functionality for plugins
type BasePlugin struct {
	name    string
	version string
	config  map[string]interface{}
}

// NewBasePlugin creates a new base plugin with the given name and version.
// This provides default implementations for the Plugin interface that can be embedded
// in custom plugins.
//
// Parameters:
//   - name: The unique name identifying this plugin
//   - version: The plugin version following semantic versioning (e.g., "1.0.0")
//
// Returns:
//
//	A BasePlugin instance with default implementations of Name(), Version(), Init(), and Cleanup()
//
// Example:
//
//	type MyPlugin struct {
//	    *plugin.BasePlugin
//	}
//
//	func NewMyPlugin() *MyPlugin {
//	    return &MyPlugin{
//	        BasePlugin: plugin.NewBasePlugin("my-plugin", "1.0.0"),
//	    }
//	}
func NewBasePlugin(name, version string) *BasePlugin {
	return &BasePlugin{
		name:    name,
		version: version,
		config:  make(map[string]interface{}),
	}
}

func (p *BasePlugin) Name() string {
	return p.name
}

func (p *BasePlugin) Version() string {
	return p.version
}

func (p *BasePlugin) Init(config map[string]interface{}) error {
	p.config = config
	return nil
}

func (p *BasePlugin) Cleanup() error {
	return nil
}

// Manager manages plugin lifecycle
type Manager struct {
	plugins []Plugin
	enabled map[string]bool
}

// NewManager creates a new plugin manager for handling plugin registration,
// execution, and lifecycle management.
//
// Returns:
//
//	A new Manager instance ready to register and execute plugins
//
// The manager provides:
//   - Plugin registration and unregistration
//   - Enable/disable plugins at runtime
//   - Execute all enabled plugins for tool invocations
//   - Thread-safe plugin management
//
// Example:
//
//	manager := plugin.NewManager()
//	securityPlugin := security.NewSecurityPlugin()
//	manager.Register(securityPlugin)
//	allowed, err := manager.Execute(ctx, invocation)
func NewManager() *Manager {
	return &Manager{
		plugins: make([]Plugin, 0),
		enabled: make(map[string]bool),
	}
}

// Register registers a plugin
func (m *Manager) Register(plugin Plugin) error {
	if plugin == nil {
		return fmt.Errorf("plugin cannot be nil")
	}

	name := plugin.Name()
	for _, p := range m.plugins {
		if p.Name() == name {
			return fmt.Errorf("plugin %s already registered", name)
		}
	}

	m.plugins = append(m.plugins, plugin)
	m.enabled[name] = true
	return nil
}

// Unregister unregisters a plugin by name
func (m *Manager) Unregister(name string) error {
	for i, p := range m.plugins {
		if p.Name() == name {
			// Cleanup plugin
			p.Cleanup()

			// Remove from slice
			m.plugins = append(m.plugins[:i], m.plugins[i+1:]...)
			delete(m.enabled, name)
			return nil
		}
	}
	return fmt.Errorf("plugin %s not found", name)
}

// Enable enables a plugin
func (m *Manager) Enable(name string) error {
	if !m.enabled[name] {
		m.enabled[name] = true
	}
	return nil
}

// Disable disables a plugin
func (m *Manager) Disable(name string) error {
	if m.enabled[name] {
		m.enabled[name] = false
	}
	return nil
}

// IsEnabled checks if a plugin is enabled
func (m *Manager) IsEnabled(name string) bool {
	return m.enabled[name]
}

// GetPlugin returns a plugin by name
func (m *Manager) GetPlugin(name string) (Plugin, bool) {
	for _, p := range m.plugins {
		if p.Name() == name && m.enabled[name] {
			return p, true
		}
	}
	return nil, false
}

// ListPlugins returns all registered plugins
func (m *Manager) ListPlugins() []Plugin {
	result := make([]Plugin, 0, len(m.plugins))
	for _, p := range m.plugins {
		if m.enabled[p.Name()] {
			result = append(result, p)
		}
	}
	return result
}

// Execute invokes all enabled plugins for a tool invocation
// Returns (allowed, error)
func (m *Manager) Execute(ctx context.Context, invocation *ToolInvocation) (bool, error) {
	for _, plugin := range m.plugins {
		if !m.enabled[plugin.Name()] {
			continue
		}

		allowed, err, _ := plugin.Execute(ctx, invocation)

		// If plugin denied the operation, stop immediately
		if !allowed {
			if err != nil {
				return false, fmt.Errorf("[%s] %w", plugin.Name(), err)
			}
			return false, fmt.Errorf("[%s] operation denied", plugin.Name())
		}

		// If plugin set a blocking error, stop
		if invocation.BlockingError != nil {
			return false, invocation.BlockingError
		}
	}

	return true, nil
}

// Cleanup cleans up all plugins
func (m *Manager) Cleanup() error {
	for _, plugin := range m.plugins {
		if err := plugin.Cleanup(); err != nil {
			// Log but continue cleaning up other plugins
			fmt.Printf("Error cleaning up plugin %s: %v\n", plugin.Name(), err)
		}
	}
	m.plugins = nil
	m.enabled = nil
	return nil
}
