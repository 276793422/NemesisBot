// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package transport

import (
	"encoding/json"
	"fmt"
	"math/rand"
	"time"
)

const (
	// RPCProtocolVersion is the RPC protocol version
	RPCProtocolVersion = "1.0"
)

// RPCType represents the type of RPC message
type RPCType string

const (
	RPCTypeRequest  RPCType = "request"
	RPCTypeResponse RPCType = "response"
	RPCTypeError    RPCType = "error"
)

// RPCMessage represents a WebSocket RPC message
type RPCMessage struct {
	Version   string                 `json:"version"`         // Protocol version
	ID        string                 `json:"id"`              // Unique message ID
	Type      RPCType                `json:"type"`            // Message type
	From      string                 `json:"from"`            // Sender node ID
	To        string                 `json:"to"`              // Receiver node ID
	Action    string                 `json:"action"`          // Action to perform
	Payload   map[string]interface{} `json:"payload"`         // Action payload
	Timestamp int64                  `json:"timestamp"`       // Unix timestamp
	Error     string                 `json:"error,omitempty"` // Error message if type is "error"
}

// NewRequest creates a new RPC request message
func NewRequest(from, to, action string, payload map[string]interface{}) *RPCMessage {
	return &RPCMessage{
		Version:   RPCProtocolVersion,
		ID:        generateID(),
		Type:      RPCTypeRequest,
		From:      from,
		To:        to,
		Action:    action,
		Payload:   payload,
		Timestamp: 0, // Will be set by sender
	}
}

// NewResponse creates a new RPC response message
func NewResponse(req *RPCMessage, payload map[string]interface{}) *RPCMessage {
	return &RPCMessage{
		Version:   RPCProtocolVersion,
		ID:        req.ID,
		Type:      RPCTypeResponse,
		From:      req.To,
		To:        req.From,
		Action:    req.Action,
		Payload:   payload,
		Timestamp: 0,
	}
}

// NewError creates a new RPC error message
func NewError(req *RPCMessage, errMsg string) *RPCMessage {
	return &RPCMessage{
		Version:   RPCProtocolVersion,
		ID:        req.ID,
		Type:      RPCTypeError,
		From:      req.To,
		To:        req.From,
		Action:    req.Action,
		Timestamp: 0,
		Error:     errMsg,
	}
}

// Validate validates the RPC message
func (m *RPCMessage) Validate() error {
	if m.Version != RPCProtocolVersion {
		return fmt.Errorf("unsupported protocol version: %s", m.Version)
	}

	if m.ID == "" {
		return fmt.Errorf("message ID is required")
	}

	if m.From == "" {
		return fmt.Errorf("from field is required")
	}

	if m.To == "" {
		return fmt.Errorf("to field is required")
	}

	if m.Action == "" {
		return fmt.Errorf("action is required")
	}

	return nil
}

// Bytes returns the message as JSON bytes
func (m *RPCMessage) Bytes() ([]byte, error) {
	return json.Marshal(m)
}

// String returns a string representation
func (m *RPCMessage) String() string {
	return fmt.Sprintf("RPCMessage{id=%s, type=%s, from=%s, to=%s, action=%s}",
		m.ID, m.Type, m.From, m.To, m.Action)
}

// generateID generates a unique message ID
func generateID() string {
	return fmt.Sprintf("msg-%d-%d", time.Now().UnixNano(), rand.Intn(10000))
}
