// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package discovery

import (
	"encoding/json"
	"fmt"
	"time"
)

const (
	// ProtocolVersion is the discovery protocol version
	ProtocolVersion = "1.0"
)

// MessageType represents the type of discovery message
type MessageType string

const (
	MessageTypeAnnounce MessageType = "announce"
	MessageTypeBye      MessageType = "bye"
)

// DiscoveryMessage represents a UDP broadcast message
type DiscoveryMessage struct {
	Version      string      `json:"version"`      // Protocol version
	Type         MessageType `json:"type"`         // Message type
	NodeID       string      `json:"node_id"`      // Unique node identifier
	Name         string      `json:"name"`         // Human-readable name
	Addresses    []string    `json:"addresses"`    // List of IP addresses (multiple NICs support)
	RPCPort      int         `json:"rpc_port"`     // RPC port number
	Role         string      `json:"role"`         // Cluster role: manager, coordinator, worker, observer, standby
	Category     string      `json:"category"`     // Business category: design, development, testing, etc.
	Tags         []string    `json:"tags"`         // Custom tags
	Capabilities []string    `json:"capabilities"` // List of capabilities
	Timestamp    int64       `json:"timestamp"`    // Unix timestamp
}

// NewAnnounceMessage creates a new announce message
func NewAnnounceMessage(nodeID, name string, addresses []string, rpcPort int, role, category string, tags []string, capabilities []string) *DiscoveryMessage {
	return &DiscoveryMessage{
		Version:      ProtocolVersion,
		Type:         MessageTypeAnnounce,
		NodeID:       nodeID,
		Name:         name,
		Addresses:    addresses,
		RPCPort:      rpcPort,
		Role:         role,
		Category:     category,
		Tags:         tags,
		Capabilities: capabilities,
		Timestamp:    time.Now().Unix(),
	}
}

// NewByeMessage creates a new bye message
func NewByeMessage(nodeID string) *DiscoveryMessage {
	return &DiscoveryMessage{
		Version:   ProtocolVersion,
		Type:      MessageTypeBye,
		NodeID:    nodeID,
		Timestamp: time.Now().Unix(),
	}
}

// Validate validates the discovery message
func (m *DiscoveryMessage) Validate() error {
	if m.Version != ProtocolVersion {
		return fmt.Errorf("unsupported protocol version: %s", m.Version)
	}

	if m.NodeID == "" {
		return fmt.Errorf("node_id is required")
	}

	if m.Type == MessageTypeAnnounce {
		if m.Name == "" {
			return fmt.Errorf("name is required for announce")
		}
		if len(m.Addresses) == 0 {
			return fmt.Errorf("addresses is required for announce")
		}
		if m.RPCPort == 0 {
			return fmt.Errorf("rpc_port is required for announce")
		}
	}

	return nil
}

// IsExpired checks if the message is expired (older than 2 minutes)
func (m *DiscoveryMessage) IsExpired() bool {
	return time.Now().Unix()-m.Timestamp > 120
}

// Bytes returns the message as JSON bytes
func (m *DiscoveryMessage) Bytes() ([]byte, error) {
	return json.Marshal(m)
}

// String returns a string representation
func (m *DiscoveryMessage) String() string {
	return fmt.Sprintf("DiscoveryMessage{type=%s, node_id=%s, name=%s, addresses=%v, rpc_port=%d, role=%s, category=%s, tags=%v, caps=%v}",
		m.Type, m.NodeID, m.Name, m.Addresses, m.RPCPort, m.Role, m.Category, m.Tags, m.Capabilities)
}
