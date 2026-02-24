// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package transport provides the transport layer for MCP client communication.
// Different transport mechanisms (stdio, HTTP, etc.) implement this interface.
package transport

import (
	"context"
	"encoding/json"
)

// JSONRPCRequest represents a JSON-RPC 2.0 request.
// These types are defined locally to avoid circular import with the mcp package.
type JSONRPCRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      interface{}     `json:"id,omitempty"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params,omitempty"`
}

// JSONRPCResponse represents a JSON-RPC 2.0 response.
type JSONRPCResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      interface{}     `json:"id"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *RPCError       `json:"error,omitempty"`
}

// RPCError represents a JSON-RPC error object.
type RPCError struct {
	Code    int         `json:"code"`
	Message string      `json:"message"`
	Data    interface{} `json:"data,omitempty"`
}

// Transport defines the interface for MCP transport mechanisms.
// Transports handle the low-level communication with MCP servers.
type Transport interface {
	// Connect establishes the connection to the MCP server.
	// For stdio transports, this starts the subprocess.
	// For HTTP transports, this establishes the HTTP connection.
	Connect(ctx context.Context) error

	// Close terminates the connection to the MCP server.
	// For stdio transports, this stops the subprocess.
	// For HTTP transports, this closes the HTTP connection.
	Close() error

	// Send sends a JSON-RPC request and returns the response.
	// This method blocks until a response is received or the context is cancelled.
	Send(ctx context.Context, req *JSONRPCRequest) (*JSONRPCResponse, error)

	// IsConnected returns true if the transport is currently connected.
	IsConnected() bool

	// Name returns the transport type name (e.g., "stdio", "http").
	// Used for logging and debugging.
	Name() string
}
