package workflow

import (
	"fmt"
	"regexp"
	"strings"
	"sync"
)

// Context holds the execution state for a single workflow run.
// It provides variable storage, node results, and template resolution.
type Context struct {
	Variables   map[string]string
	NodeResults map[string]*NodeResult
	Input       map[string]interface{}

	mu sync.RWMutex
}

// NewContext creates a new workflow execution context.
func NewContext(input map[string]interface{}) *Context {
	return &Context{
		Variables:   make(map[string]string),
		NodeResults: make(map[string]*NodeResult),
		Input:       input,
	}
}

// SetVar sets a workflow variable.
func (c *Context) SetVar(key, value string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.Variables[key] = value
}

// GetVar retrieves a workflow variable. Returns empty string if not found.
func (c *Context) GetVar(key string) string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.Variables[key]
}

// SetNodeResult stores the result of a node execution.
func (c *Context) SetNodeResult(nodeID string, result *NodeResult) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.NodeResults[nodeID] = result
}

// GetNodeResult retrieves the result of a previously executed node.
func (c *Context) GetNodeResult(nodeID string) *NodeResult {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.NodeResults[nodeID]
}

// GetAllVariables returns a copy of all workflow variables.
func (c *Context) GetAllVariables() map[string]string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	cp := make(map[string]string, len(c.Variables))
	for k, v := range c.Variables {
		cp[k] = v
	}
	return cp
}

// Resolve resolves template references in a string.
// Supported patterns:
//   - {{variable}}              - resolve from workflow variables
//   - {{node_id}}               - resolve full output of a node
//   - {{node_id.field}}         - resolve a specific field from a node's output
//   - {{input.key}}             - resolve from workflow input
func (c *Context) Resolve(template string) string {
	re := regexp.MustCompile(`\{\{([^}]+)\}\}`)

	return re.ReplaceAllStringFunc(template, func(match string) string {
		key := strings.TrimSpace(match[2 : len(match)-2])

		// Try input.key pattern
		if strings.HasPrefix(key, "input.") {
			field := strings.TrimPrefix(key, "input.")
			if c.Input != nil {
				if val, ok := c.Input[field]; ok {
					return fmt.Sprintf("%v", val)
				}
			}
			return match // unresolved
		}

		// Try node_id.field pattern
		if idx := strings.Index(key, "."); idx >= 0 {
			nodeID := key[:idx]
			field := key[idx+1:]
			result := c.GetNodeResult(nodeID)
			if result != nil && result.Output != nil {
				if m, ok := result.Output.(map[string]interface{}); ok {
					if val, exists := m[field]; exists {
						return fmt.Sprintf("%v", val)
					}
				}
			}
			return match // unresolved
		}

		// Try plain variable
		if val := c.GetVar(key); val != "" {
			return val
		}

		// Try node output (full)
		result := c.GetNodeResult(key)
		if result != nil && result.Output != nil {
			return fmt.Sprintf("%v", result.Output)
		}

		return match // unresolved
	})
}

// Clone creates a shallow copy of the context.
func (c *Context) Clone() *Context {
	c.mu.RLock()
	defer c.mu.RUnlock()

	vars := make(map[string]string, len(c.Variables))
	for k, v := range c.Variables {
		vars[k] = v
	}

	results := make(map[string]*NodeResult, len(c.NodeResults))
	for k, v := range c.NodeResults {
		results[k] = v
	}

	input := make(map[string]interface{}, len(c.Input))
	for k, v := range c.Input {
		input[k] = v
	}

	return &Context{
		Variables:   vars,
		NodeResults: results,
		Input:       input,
	}
}
