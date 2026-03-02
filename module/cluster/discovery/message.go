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
	Version      string   `json:"version"`      // Protocol version
	Type         MessageType `json:"type"`         // Message type
	NodeID       string   `json:"node_id"`      // Unique node identifier
	Name         string   `json:"name"`         // Human-readable name
	Address      string   `json:"address"`      // IP:Port (RPC port)
	Capabilities []string `json:"capabilities"` // List of capabilities
	Timestamp    int64    `json:"timestamp"`     // Unix timestamp
}

// NewAnnounceMessage creates a new announce message
func NewAnnounceMessage(nodeID, name, address string, capabilities []string) *DiscoveryMessage {
	return &DiscoveryMessage{
		Version:      ProtocolVersion,
		Type:         MessageTypeAnnounce,
		NodeID:       nodeID,
		Name:         name,
		Address:      address,
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
		if m.Address == "" {
			return fmt.Errorf("address is required for announce")
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
	return fmt.Sprintf("DiscoveryMessage{type=%s, node_id=%s, name=%s, address=%s, caps=%v}",
		m.Type, m.NodeID, m.Name, m.Address, m.Capabilities)
}
