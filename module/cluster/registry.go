// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"sync"
	"time"
)

// Registry manages the cluster node registry
type Registry struct {
	mu    sync.RWMutex
	nodes map[string]*Node // node_id -> Node
}

// NewRegistry creates a new registry
func NewRegistry() *Registry {
	return &Registry{
		nodes: make(map[string]*Node),
	}
}

// AddOrUpdate adds a new node or updates an existing one
func (r *Registry) AddOrUpdate(node *Node) {
	r.mu.Lock()
	defer r.mu.Unlock()

	if existing, ok := r.nodes[node.ID]; ok {
		// Update existing node - copy data to avoid holding Node lock while Registry is locked
		// We don't call node methods here to avoid nested locking
		existing.mu.Lock()
		existing.LastSeen = time.Now()
		if existing.Status != StatusOnline {
			existing.Status = StatusOnline
		}
		existing.Name = node.Name
		existing.Address = node.Address
		existing.Addresses = node.Addresses
		existing.RPCPort = node.RPCPort
		existing.Capabilities = node.Capabilities
		existing.Role = node.Role
		existing.Category = node.Category
		existing.Tags = node.Tags
		existing.Priority = node.Priority
		existing.mu.Unlock()
	} else {
		// Add new node
		node.mu.Lock()
		node.LastSeen = time.Now()
		node.Status = StatusOnline // New nodes are online by default
		node.mu.Unlock()
		r.nodes[node.ID] = node
	}
}

// Get retrieves a node by ID
func (r *Registry) Get(nodeID string) *Node {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.nodes[nodeID]
}

// GetAll returns all nodes
func (r *Registry) GetAll() []*Node {
	r.mu.RLock()
	defer r.mu.RUnlock()

	nodes := make([]*Node, 0, len(r.nodes))
	for _, node := range r.nodes {
		nodes = append(nodes, node)
	}
	return nodes
}

// GetOnline returns all online nodes
func (r *Registry) GetOnline() []*Node {
	r.mu.RLock()
	defer r.mu.RUnlock()

	nodes := make([]*Node, 0)
	for _, node := range r.nodes {
		if node.IsOnline() {
			nodes = append(nodes, node)
		}
	}
	return nodes
}

// Remove removes a node from the registry
func (r *Registry) Remove(nodeID string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	delete(r.nodes, nodeID)
}

// GetCapabilities returns all unique capabilities from all nodes
func (r *Registry) GetCapabilities() []string {
	r.mu.RLock()
	defer r.mu.RUnlock()

	capMap := make(map[string]bool)
	for _, node := range r.nodes {
		for _, cap := range node.Capabilities {
			capMap[cap] = true
		}
	}

	caps := make([]string, 0, len(capMap))
	for cap := range capMap {
		caps = append(caps, cap)
	}
	return caps
}

// FindByCapability returns nodes that have a specific capability
func (r *Registry) FindByCapability(capability string) []*Node {
	r.mu.RLock()
	defer r.mu.RUnlock()

	nodes := make([]*Node, 0)
	for _, node := range r.nodes {
		if node.HasCapability(capability) {
			nodes = append(nodes, node)
		}
	}
	return nodes
}

// FindByCapabilityOnline returns online nodes that have a specific capability
func (r *Registry) FindByCapabilityOnline(capability string) []*Node {
	r.mu.RLock()
	defer r.mu.RUnlock()

	nodes := make([]*Node, 0)
	for _, node := range r.nodes {
		if node.IsOnline() && node.HasCapability(capability) {
			nodes = append(nodes, node)
		}
	}
	return nodes
}

// MarkOffline marks a node as offline
func (r *Registry) MarkOffline(nodeID string, reason string) {
	r.mu.Lock()
	defer r.mu.Unlock()

	if node, ok := r.nodes[nodeID]; ok {
		node.MarkOffline(reason)
	}
}

// CheckTimeouts checks all nodes and marks those as offline that haven't been seen recently
func (r *Registry) CheckTimeouts(timeout time.Duration) []string {
	r.mu.Lock()
	defer r.mu.Unlock()

	expired := make([]string, 0)
	now := time.Now()

	for _, node := range r.nodes {
		// Acquire node lock to safely check both Status and LastSeen
		// This is necessary because time.Time is not atomic on 32-bit systems
		node.mu.Lock()
		if node.Status == StatusOnline {
			if now.Sub(node.LastSeen) > timeout {
				// Mark node as offline directly without calling MarkOffline to avoid nested lock
				node.Status = StatusOffline
				node.LastError = "timeout"
				expired = append(expired, node.ID)
			}
		}
		node.mu.Unlock()
	}

	return expired
}

// Count returns the total number of nodes
func (r *Registry) Count() int {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return len(r.nodes)
}

// OnlineCount returns the number of online nodes
func (r *Registry) OnlineCount() int {
	r.mu.RLock()
	defer r.mu.RUnlock()

	count := 0
	for _, node := range r.nodes {
		if node.IsOnline() {
			count++
		}
	}
	return count
}
