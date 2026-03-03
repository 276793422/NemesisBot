// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/276793422/NemesisBot/module/cluster"
)

// ClusterRPCTool enables agents to make RPC calls to other bots in the cluster
type ClusterRPCTool struct {
	cluster *cluster.Cluster
}

// NewClusterRPCTool creates a new cluster RPC tool
func NewClusterRPCTool(cluster *cluster.Cluster) *ClusterRPCTool {
	return &ClusterRPCTool{
		cluster: cluster,
	}
}

// Name returns the tool name
func (t *ClusterRPCTool) Name() string {
	return "cluster_rpc"
}

// Description returns the tool description
func (t *ClusterRPCTool) Description() string {
	return "Call other bots in the cluster via RPC. Parameters: peer_id (string, required), action (string, required), data (object, optional)"
}

// Parameters returns the tool parameters schema
func (t *ClusterRPCTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"peer_id": map[string]interface{}{
				"type":        "string",
				"description": "ID of the peer bot to call",
			},
			"action": map[string]interface{}{
				"type":        "string",
				"description": "RPC action to perform",
			},
			"data": map[string]interface{}{
				"type":        "object",
				"description": "Optional data payload for the RPC call",
			},
		},
		"required": []string{"peer_id", "action"},
	}
}

// Execute executes the cluster RPC tool
func (t *ClusterRPCTool) Execute(ctx context.Context, params map[string]interface{}) *ToolResult {
	// Extract parameters
	peerID, ok := params["peer_id"].(string)
	if !ok || peerID == "" {
		return ErrorResult("peer_id is required")
	}

	action, ok := params["action"].(string)
	if !ok || action == "" {
		return ErrorResult("action is required")
	}

	// Extract data (optional)
	var payload map[string]interface{}
	if data, ok := params["data"].(map[string]interface{}); ok {
		payload = data
	} else {
		payload = make(map[string]interface{})
	}

	// Make RPC call with context support
	response, err := t.cluster.CallWithContext(ctx, peerID, action, payload)
	if err != nil {
		return ErrorResult(fmt.Sprintf("RPC call failed: %v", err))
	}

	// Return response as string
	return SilentResult(string(response))
}

// GetAvailablePeers returns information about available peers
func (t *ClusterRPCTool) GetAvailablePeers(ctx context.Context) (string, error) {
	peersIface := t.cluster.GetOnlinePeers()
	if len(peersIface) == 0 {
		return "No other bots currently online", nil
	}

	result := make([]map[string]interface{}, 0, len(peersIface))
	for _, peerIface := range peersIface {
		// Type assert to access node properties
		if peer, ok := peerIface.(interface {
			GetID() string
			GetName() string
			GetCapabilities() []string
			GetStatus() string
		}); ok {
			result = append(result, map[string]interface{}{
				"id":           peer.GetID(),
				"name":         peer.GetName(),
				"capabilities": peer.GetCapabilities(),
				"status":       peer.GetStatus(),
			})
		}
	}

	jsonData, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		return "", fmt.Errorf("failed to marshal peers: %w", err)
	}

	return string(jsonData), nil
}

// GetCapabilities returns all available capabilities in the cluster
func (t *ClusterRPCTool) GetCapabilities(ctx context.Context) (string, error) {
	caps := t.cluster.GetCapabilities()
	if len(caps) == 0 {
		return "No capabilities available", nil
	}

	jsonData, err := json.MarshalIndent(caps, "", "  ")
	if err != nil {
		return "", fmt.Errorf("failed to marshal capabilities: %w", err)
	}

	return string(jsonData), nil
}
