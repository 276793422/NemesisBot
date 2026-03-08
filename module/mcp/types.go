// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package mcp implements the Model Context Protocol (MCP) client.
// MCP is an open protocol that standardizes how AI models connect to external tools and data sources.
// Specification: https://modelcontextprotocol.io/
package mcp

import (
	"context"
	"encoding/json"
	"fmt"
)

// Protocol version constants
const (
	ProtocolVersion = "2025-06-18"
	JSONRPCVersion  = "2.0"
)

// JSONRPCRequest represents a JSON-RPC 2.0 request object
type JSONRPCRequest struct {
	JSONRPC string      `json:"jsonrpc"`
	ID      interface{} `json:"id,omitempty"`
	Method  string      `json:"method"`
	Params  interface{} `json:"params,omitempty"`
}

// JSONRPCResponse represents a JSON-RPC 2.0 response object
type JSONRPCResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      interface{}     `json:"id"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *JSONRPCError   `json:"error,omitempty"`
}

// JSONRPCError represents a JSON-RPC 2.0 error object
type JSONRPCError struct {
	Code    int         `json:"code"`
	Message string      `json:"message"`
	Data    interface{} `json:"data,omitempty"`
}

// Error returns a formatted error string
func (e *JSONRPCError) Error() string {
	if e.Data != nil {
		return fmt.Sprintf("MCP error %d: %s (data: %v)", e.Code, e.Message, e.Data)
	}
	return fmt.Sprintf("MCP error %d: %s", e.Code, e.Message)
}

// Tool represents an MCP tool definition
type Tool struct {
	Name        string                 `json:"name"`
	Description string                 `json:"description,omitempty"`
	InputSchema map[string]interface{} `json:"inputSchema"`
}

// ToolContent represents content in a tool call result
type ToolContent struct {
	Type     string `json:"type"` // "text" | "image" | "resource"
	Text     string `json:"text,omitempty"`
	Data     string `json:"data,omitempty"`
	MimeType string `json:"mimeType,omitempty"`
}

// ToolCallResult represents the result of a tool call
type ToolCallResult struct {
	Content []ToolContent `json:"content"`
	IsError bool          `json:"isError,omitempty"`
}

// ToolCapabilities describes server's tool capabilities
type ToolCapabilities struct {
	ListChanged bool `json:"listChanged,omitempty"`
}

// ResourceCapabilities describes server's resource capabilities
type ResourceCapabilities struct {
	Subscribe   bool `json:"subscribe,omitempty"`
	ListChanged bool `json:"listChanged,omitempty"`
}

// PromptCapabilities describes server's prompt capabilities
type PromptCapabilities struct {
	ListChanged bool `json:"listChanged,omitempty"`
}

// Resource represents a resource available on the MCP server
type Resource struct {
	URI         string `json:"uri"`
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
	MimeType    string `json:"mimeType,omitempty"`
}

// ResourceContent represents content read from a resource
type ResourceContent struct {
	URI      string `json:"uri"`
	MimeType string `json:"mimeType,omitempty"`
	Text     string `json:"text,omitempty"`
	Blob     []byte `json:"blob,omitempty"`
}

// Prompt represents a prompt template available on the MCP server
type Prompt struct {
	Name        string           `json:"name"`
	Description string           `json:"description,omitempty"`
	Arguments   []PromptArgument `json:"arguments,omitempty"`
}

// PromptArgument represents an argument for a prompt
type PromptArgument struct {
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
	Required    bool   `json:"required,omitempty"`
}

// PromptMessage represents a message in a prompt result
type PromptMessage struct {
	Role    string               `json:"role"` // "user" | "assistant" | "system"
	Content PromptMessageContent `json:"content"`
}

// PromptMessageContent represents content in a prompt message
type PromptMessageContent struct {
	Type string `json:"type"` // "text" | "image" | "resource"
	Text string `json:"text,omitempty"`
	Data string `json:"data,omitempty"`
}

// PromptResult represents the result of getting a prompt
type PromptResult struct {
	Messages    []PromptMessage `json:"messages"`
	Description string          `json:"description,omitempty"`
}

// ServerCapabilities describes the server's capabilities
type ServerCapabilities struct {
	Tools     *ToolCapabilities     `json:"tools,omitempty"`
	Resources *ResourceCapabilities `json:"resources,omitempty"`
	Prompts   *PromptCapabilities   `json:"prompts,omitempty"`
}

// ServerInfo provides information about the server
type ServerInfo struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// ClientInfo provides information about the client
type ClientInfo struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// InitializeParams parameters for the initialize request
type InitializeParams struct {
	ProtocolVersion string             `json:"protocolVersion"`
	Capabilities    ClientCapabilities `json:"capabilities"`
	ClientInfo      ClientInfo         `json:"clientInfo"`
}

// InitializeResult result of the initialize request
type InitializeResult struct {
	ProtocolVersion string             `json:"protocolVersion"`
	Capabilities    ServerCapabilities `json:"capabilities"`
	ServerInfo      ServerInfo         `json:"serverInfo"`
}

// ClientCapabilities describes the client's capabilities
type ClientCapabilities struct {
	Tools     map[string]bool `json:"tools,omitempty"`
	Resources map[string]bool `json:"resources,omitempty"`
	Prompts   map[string]bool `json:"prompts,omitempty"`
}

// ServerConfig configuration for an MCP server connection
type ServerConfig struct {
	Name    string   `json:"name"`
	Command string   `json:"command"`
	Args    []string `json:"args"`
	Env     []string `json:"env,omitempty"`
	Timeout int      `json:"timeout"` // seconds, 0 means use global default
}

// Client represents an MCP client connection to a server
type Client interface {
	// Initialize performs the MCP handshake and returns server info
	Initialize(ctx context.Context) (*InitializeResult, error)

	// ListTools retrieves available tools from the server
	ListTools(ctx context.Context) ([]Tool, error)

	// CallTool executes a tool on the server
	CallTool(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error)

	// ListResources retrieves available resources from the server
	ListResources(ctx context.Context) ([]Resource, error)

	// ReadResource reads the contents of a resource
	ReadResource(ctx context.Context, uri string) (*ResourceContent, error)

	// ListPrompts retrieves available prompts from the server
	ListPrompts(ctx context.Context) ([]Prompt, error)

	// GetPrompt retrieves a populated prompt from the server
	GetPrompt(ctx context.Context, name string, args map[string]interface{}) (*PromptResult, error)

	// Close terminates the connection
	Close() error

	// ServerInfo returns the server information (available after Initialize)
	ServerInfo() *ServerInfo

	// IsConnected returns true if the client is connected
	IsConnected() bool
}

// decodeResult decodes a JSON-RPC result into the provided interface
func decodeResult(resp *JSONRPCResponse, result interface{}) error {
	if resp.Error != nil {
		return resp.Error
	}

	if len(resp.Result) == 0 {
		return fmt.Errorf("empty result in response")
	}

	// Unmarshal the raw JSON result into the target interface
	if err := json.Unmarshal(resp.Result, result); err != nil {
		return fmt.Errorf("failed to decode result: %w (raw: %s)", err, string(resp.Result))
	}

	return nil
}
