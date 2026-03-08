// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package handlers

// Logger represents logging functions for RPC operations
type Logger interface {
	LogRPCInfo(msg string, args ...interface{})
	LogRPCError(msg string, args ...interface{})
	LogRPCDebug(msg string, args ...interface{})
}

// Node represents a node in the cluster (minimal interface)
type Node interface {
	GetID() string
	GetName() string
	GetAddress() string
	GetAddresses() []string
	GetRPCPort() int
	GetCapabilities() []string
	GetStatus() string
	IsOnline() bool
}

// Registrar is a function type for registering RPC handlers
type Registrar func(action string, handler func(payload map[string]interface{}) (map[string]interface{}, error))

// ActionSchema defines the complete schema for a single action
type ActionSchema struct {
	Name        string                   `json:"name"`
	Description string                   `json:"description"`
	Parameters  map[string]interface{}   `json:"parameters,omitempty"`
	Returns     map[string]interface{}   `json:"returns,omitempty"`
	Examples    []map[string]interface{} `json:"examples,omitempty"`
}

// RegisterDefaultHandlers registers system default RPC handlers
// These handlers provide basic cluster functionality:
// - ping: health check
// - get_capabilities: return list of cluster capabilities
// - get_info: return cluster information and online peers
// - list_actions: return all available actions with their schema (NEW)
func RegisterDefaultHandlers(clusterHandler Logger, getNodeID func() string, getCapabilities func() []string, getOnlinePeers func() []interface{}, getActionsSchema func() []interface{}, registrar Registrar) {
	// Ping handler
	registrar("ping", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"status":  "ok",
			"node_id": getNodeID(),
		}, nil
	})

	// Get capabilities handler
	registrar("get_capabilities", func(payload map[string]interface{}) (map[string]interface{}, error) {
		caps := getCapabilities()
		return map[string]interface{}{
			"capabilities": caps,
		}, nil
	})

	// Get info handler
	registrar("get_info", func(payload map[string]interface{}) (map[string]interface{}, error) {
		peers := getOnlinePeers()
		peerInfos := make([]map[string]interface{}, 0, len(peers))
		for _, p := range peers {
			if peer, ok := p.(Node); ok {
				peerInfos = append(peerInfos, map[string]interface{}{
					"id":           peer.GetID(),
					"name":         peer.GetName(),
					"capabilities": peer.GetCapabilities(),
					"status":       peer.GetStatus(),
				})
			}
		}

		return map[string]interface{}{
			"node_id": getNodeID(),
			"peers":   peerInfos,
		}, nil
	})

	// List actions handler (NEW)
	registrar("list_actions", func(payload map[string]interface{}) (map[string]interface{}, error) {
		actions := getActionsSchema()
		return map[string]interface{}{
			"actions": actions,
		}, nil
	})

	clusterHandler.LogRPCInfo("Registered default handlers: ping, get_capabilities, get_info, list_actions")
}
