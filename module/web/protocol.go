// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Module - Three-Level Dispatch Protocol

package web

import (
	"encoding/json"
	"fmt"
	"time"
)

// ProtocolMessage represents a three-level dispatch protocol message.
// Format: type → module → cmd, with data and timestamp.
type ProtocolMessage struct {
	Type      string          `json:"type"`
	Module    string          `json:"module"`
	Cmd       string          `json:"cmd"`
	Data      json.RawMessage `json:"data,omitempty"`
	Timestamp string          `json:"timestamp,omitempty"`
}

// IsNewProtocol checks whether raw JSON uses the new three-level format
// by looking for the presence of a non-empty "module" field.
func IsNewProtocol(raw []byte) bool {
	var probe struct {
		Module *string `json:"module"`
	}
	if err := json.Unmarshal(raw, &probe); err != nil {
		return false
	}
	return probe.Module != nil && *probe.Module != ""
}

// ParseProtocolMessage parses raw JSON into a ProtocolMessage.
func ParseProtocolMessage(raw []byte) (*ProtocolMessage, error) {
	var msg ProtocolMessage
	if err := json.Unmarshal(raw, &msg); err != nil {
		return nil, fmt.Errorf("failed to parse protocol message: %w", err)
	}
	return &msg, nil
}

// NewProtocolMessage constructs a new three-level protocol message.
func NewProtocolMessage(typeName, module, cmd string, data interface{}) (*ProtocolMessage, error) {
	var rawData json.RawMessage
	if data != nil {
		b, err := json.Marshal(data)
		if err != nil {
			return nil, fmt.Errorf("failed to marshal data: %w", err)
		}
		rawData = b
	}

	return &ProtocolMessage{
		Type:      typeName,
		Module:    module,
		Cmd:       cmd,
		Data:      rawData,
		Timestamp: time.Now().Format(time.RFC3339Nano),
	}, nil
}

// ToJSON serializes the message to JSON bytes.
func (m *ProtocolMessage) ToJSON() ([]byte, error) {
	return json.Marshal(m)
}

// DecodeData decodes the Data field into the provided value.
func (m *ProtocolMessage) DecodeData(v interface{}) error {
	if m.Data == nil {
		return fmt.Errorf("message has no data")
	}
	return json.Unmarshal(m.Data, v)
}
