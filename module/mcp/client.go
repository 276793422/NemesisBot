// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package mcp implements the Model Context Protocol (MCP) client.
package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
	"sync/atomic"

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/mcp/transport"
)

// client is the concrete implementation of the Client interface.
type client struct {
	config          *ServerConfig
	transport       transport.Transport
	protocolVersion string
	serverInfo      *ServerInfo
	capabilities    ServerCapabilities

	mu          sync.RWMutex
	reqID       int64
	closed      bool
	initialized bool
}

// NewClient creates a new MCP client with the given server configuration.
func NewClient(cfg *ServerConfig) (Client, error) {
	if cfg == nil {
		return nil, fmt.Errorf("server config cannot be nil")
	}
	if cfg.Name == "" {
		return nil, fmt.Errorf("server name cannot be empty")
	}

	// Create stdio transport
	trans, err := transport.NewStdioTransport(cfg.Command, cfg.Args, cfg.Env)
	if err != nil {
		return nil, fmt.Errorf("failed to create stdio transport: %w", err)
	}

	return &client{
		config:    cfg,
		transport: trans,
		reqID:     0,
	}, nil
}

// Initialize performs the MCP handshake with the server.
func (c *client) Initialize(ctx context.Context) (*InitializeResult, error) {
	c.mu.Lock()
	if c.initialized {
		c.mu.Unlock()
		return nil, fmt.Errorf("client already initialized")
	}
	c.mu.Unlock()

	logger.InfoCF("mcp.client", "Initializing MCP client",
		map[string]interface{}{
			"server":  c.config.Name,
			"command": c.config.Command,
			"args":    c.config.Args,
		})

	// Connect transport (starts subprocess)
	if err := c.transport.Connect(ctx); err != nil {
		return nil, fmt.Errorf("failed to connect transport: %w", err)
	}

	// Send initialize request
	req := &JSONRPCRequest{
		JSONRPC: JSONRPCVersion,
		ID:      c.nextID(),
		Method:  "initialize",
		Params: InitializeParams{
			ProtocolVersion: ProtocolVersion,
			Capabilities: ClientCapabilities{
				Tools:     map[string]bool{},
				Resources: map[string]bool{},
				Prompts:   map[string]bool{},
			},
			ClientInfo: ClientInfo{
				Name:    "NemesisBot",
				Version: "1.0.0",
			},
		},
	}

	transportReq, err := convertToTransportRequest(req)
	if err != nil {
		c.transport.Close()
		return nil, fmt.Errorf("failed to convert request: %w", err)
	}

	transportResp, err := c.transport.Send(ctx, transportReq)
	if err != nil {
		c.transport.Close()
		return nil, fmt.Errorf("initialize request failed: %w", err)
	}

	resp := convertFromTransportResponse(transportResp)

	// Parse initialize result
	var result InitializeResult
	if err := decodeResult(resp, &result); err != nil {
		c.transport.Close()
		return nil, fmt.Errorf("failed to decode initialize result: %w", err)
	}

	// Update client state
	c.mu.Lock()
	c.protocolVersion = result.ProtocolVersion
	c.serverInfo = &result.ServerInfo
	c.capabilities = result.Capabilities
	c.initialized = true
	c.mu.Unlock()

	logger.InfoCF("mcp.client", "MCP client initialized successfully",
		map[string]interface{}{
			"server":           c.config.Name,
			"server_name":      result.ServerInfo.Name,
			"server_version":   result.ServerInfo.Version,
			"protocol_version": result.ProtocolVersion,
		})

	// Note: The initialized notification is optional and not required for basic functionality.
	// It can be added later if needed for specific MCP servers that require it.
	// For now, we skip it to avoid timeout issues.

	return &result, nil
}

// ListTools retrieves the list of available tools from the server.
func (c *client) ListTools(ctx context.Context) ([]Tool, error) {
	c.mu.RLock()
	if !c.initialized {
		c.mu.RUnlock()
		return nil, fmt.Errorf("client not initialized")
	}
	c.mu.RUnlock()

	logger.DebugCF("mcp.client", "Listing tools from MCP server",
		map[string]interface{}{
			"server": c.config.Name,
		})

	req := &JSONRPCRequest{
		JSONRPC: JSONRPCVersion,
		ID:      c.nextID(),
		Method:  "tools/list",
	}

	transportReq, err := convertToTransportRequest(req)
	if err != nil {
		return nil, fmt.Errorf("failed to convert request: %w", err)
	}

	transportResp, err := c.transport.Send(ctx, transportReq)
	if err != nil {
		return nil, fmt.Errorf("tools/list request failed: %w", err)
	}

	resp := convertFromTransportResponse(transportResp)

	var result struct {
		Tools []Tool `json:"tools"`
	}
	if err := decodeResult(resp, &result); err != nil {
		return nil, fmt.Errorf("failed to decode tools/list result: %w", err)
	}

	logger.InfoCF("mcp.client", "Retrieved tool list from MCP server",
		map[string]interface{}{
			"server":     c.config.Name,
			"tool_count": len(result.Tools),
		})

	return result.Tools, nil
}

// CallTool executes a tool on the MCP server.
func (c *client) CallTool(ctx context.Context, name string, args map[string]interface{}) (*ToolCallResult, error) {
	c.mu.RLock()
	if !c.initialized {
		c.mu.RUnlock()
		return nil, fmt.Errorf("client not initialized")
	}
	c.mu.RUnlock()

	logger.DebugCF("mcp.client", "Calling MCP tool",
		map[string]interface{}{
			"server": c.config.Name,
			"tool":   name,
			"args":   args,
		})

	req := &JSONRPCRequest{
		JSONRPC: JSONRPCVersion,
		ID:      c.nextID(),
		Method:  "tools/call",
		Params: map[string]interface{}{
			"name":      name,
			"arguments": args,
		},
	}

	transportReq, err := convertToTransportRequest(req)
	if err != nil {
		return nil, fmt.Errorf("failed to convert request: %w", err)
	}

	transportResp, err := c.transport.Send(ctx, transportReq)
	if err != nil {
		return nil, fmt.Errorf("tools/call request failed: %w", err)
	}

	resp := convertFromTransportResponse(transportResp)

	var result ToolCallResult
	if err := decodeResult(resp, &result); err != nil {
		return nil, fmt.Errorf("failed to decode tools/call result: %w", err)
	}

	logger.DebugCF("mcp.client", "MCP tool call completed",
		map[string]interface{}{
			"server":   c.config.Name,
			"tool":     name,
			"is_error": result.IsError,
		})

	return &result, nil
}

// ListResources retrieves the list of available resources from the server.
func (c *client) ListResources(ctx context.Context) ([]Resource, error) {
	c.mu.RLock()
	if !c.initialized {
		c.mu.RUnlock()
		return nil, fmt.Errorf("client not initialized")
	}
	c.mu.RUnlock()

	logger.DebugCF("mcp.client", "Listing resources from MCP server",
		map[string]interface{}{
			"server": c.config.Name,
		})

	req := &JSONRPCRequest{
		JSONRPC: JSONRPCVersion,
		ID:      c.nextID(),
		Method:  "resources/list",
	}

	transportReq, err := convertToTransportRequest(req)
	if err != nil {
		return nil, fmt.Errorf("failed to convert request: %w", err)
	}

	transportResp, err := c.transport.Send(ctx, transportReq)
	if err != nil {
		return nil, fmt.Errorf("resources/list request failed: %w", err)
	}

	resp := convertFromTransportResponse(transportResp)

	var result struct {
		Resources []Resource `json:"resources"`
	}
	if err := decodeResult(resp, &result); err != nil {
		return nil, fmt.Errorf("failed to decode resources/list result: %w", err)
	}

	logger.InfoCF("mcp.client", "Retrieved resource list from MCP server",
		map[string]interface{}{
			"server":         c.config.Name,
			"resource_count": len(result.Resources),
		})

	return result.Resources, nil
}

// ReadResource reads the contents of a resource from the server.
func (c *client) ReadResource(ctx context.Context, uri string) (*ResourceContent, error) {
	c.mu.RLock()
	if !c.initialized {
		c.mu.RUnlock()
		return nil, fmt.Errorf("client not initialized")
	}
	c.mu.RUnlock()

	logger.DebugCF("mcp.client", "Reading resource from MCP server",
		map[string]interface{}{
			"server": c.config.Name,
			"uri":    uri,
		})

	req := &JSONRPCRequest{
		JSONRPC: JSONRPCVersion,
		ID:      c.nextID(),
		Method:  "resources/read",
		Params: map[string]interface{}{
			"uri": uri,
		},
	}

	transportReq, err := convertToTransportRequest(req)
	if err != nil {
		return nil, fmt.Errorf("failed to convert request: %w", err)
	}

	transportResp, err := c.transport.Send(ctx, transportReq)
	if err != nil {
		return nil, fmt.Errorf("resources/read request failed: %w", err)
	}

	resp := convertFromTransportResponse(transportResp)

	var result ResourceContent
	if err := decodeResult(resp, &result); err != nil {
		return nil, fmt.Errorf("failed to decode resources/read result: %w", err)
	}

	logger.DebugCF("mcp.client", "Resource read successfully",
		map[string]interface{}{
			"server": c.config.Name,
			"uri":    uri,
		})

	return &result, nil
}

// ListPrompts retrieves the list of available prompts from the server.
func (c *client) ListPrompts(ctx context.Context) ([]Prompt, error) {
	c.mu.RLock()
	if !c.initialized {
		c.mu.RUnlock()
		return nil, fmt.Errorf("client not initialized")
	}
	c.mu.RUnlock()

	logger.DebugCF("mcp.client", "Listing prompts from MCP server",
		map[string]interface{}{
			"server": c.config.Name,
		})

	req := &JSONRPCRequest{
		JSONRPC: JSONRPCVersion,
		ID:      c.nextID(),
		Method:  "prompts/list",
	}

	transportReq, err := convertToTransportRequest(req)
	if err != nil {
		return nil, fmt.Errorf("failed to convert request: %w", err)
	}

	transportResp, err := c.transport.Send(ctx, transportReq)
	if err != nil {
		return nil, fmt.Errorf("prompts/list request failed: %w", err)
	}

	resp := convertFromTransportResponse(transportResp)

	var result struct {
		Prompts []Prompt `json:"prompts"`
	}
	if err := decodeResult(resp, &result); err != nil {
		return nil, fmt.Errorf("failed to decode prompts/list result: %w", err)
	}

	logger.InfoCF("mcp.client", "Retrieved prompt list from MCP server",
		map[string]interface{}{
			"server":       c.config.Name,
			"prompt_count": len(result.Prompts),
		})

	return result.Prompts, nil
}

// GetPrompt retrieves a populated prompt from the server.
func (c *client) GetPrompt(ctx context.Context, name string, args map[string]interface{}) (*PromptResult, error) {
	c.mu.RLock()
	if !c.initialized {
		c.mu.RUnlock()
		return nil, fmt.Errorf("client not initialized")
	}
	c.mu.RUnlock()

	logger.DebugCF("mcp.client", "Getting prompt from MCP server",
		map[string]interface{}{
			"server": c.config.Name,
			"prompt": name,
			"args":   args,
		})

	req := &JSONRPCRequest{
		JSONRPC: JSONRPCVersion,
		ID:      c.nextID(),
		Method:  "prompts/get",
		Params: map[string]interface{}{
			"name":      name,
			"arguments": args,
		},
	}

	transportReq, err := convertToTransportRequest(req)
	if err != nil {
		return nil, fmt.Errorf("failed to convert request: %w", err)
	}

	transportResp, err := c.transport.Send(ctx, transportReq)
	if err != nil {
		return nil, fmt.Errorf("prompts/get request failed: %w", err)
	}

	resp := convertFromTransportResponse(transportResp)

	var result PromptResult
	if err := decodeResult(resp, &result); err != nil {
		return nil, fmt.Errorf("failed to decode prompts/get result: %w", err)
	}

	logger.DebugCF("mcp.client", "Prompt retrieved successfully",
		map[string]interface{}{
			"server":    c.config.Name,
			"prompt":    name,
			"msg_count": len(result.Messages),
		})

	return &result, nil
}

// Close closes the MCP client and terminates the connection.
func (c *client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.closed {
		return nil
	}
	c.closed = true

	logger.InfoCF("mcp.client", "Closing MCP client",
		map[string]interface{}{
			"server": c.config.Name,
		})

	return c.transport.Close()
}

// ServerInfo returns the server information.
func (c *client) ServerInfo() *ServerInfo {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.serverInfo
}

// IsConnected returns true if the client is connected.
func (c *client) IsConnected() bool {
	return c.transport.IsConnected()
}

// nextID generates the next request ID.
func (c *client) nextID() int64 {
	return atomic.AddInt64(&c.reqID, 1)
}

// convertToTransportRequest converts an mcp.JSONRPCRequest to a transport.JSONRPCRequest.
func convertToTransportRequest(req *JSONRPCRequest) (*transport.JSONRPCRequest, error) {
	params, err := json.Marshal(req.Params)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal params: %w", err)
	}
	return &transport.JSONRPCRequest{
		JSONRPC: req.JSONRPC,
		ID:      req.ID,
		Method:  req.Method,
		Params:  params,
	}, nil
}

// convertFromTransportResponse converts a transport.JSONRPCResponse to an mcp.JSONRPCResponse.
func convertFromTransportResponse(resp *transport.JSONRPCResponse) *JSONRPCResponse {
	var err *JSONRPCError
	if resp.Error != nil {
		err = &JSONRPCError{
			Code:    resp.Error.Code,
			Message: resp.Error.Message,
			Data:    resp.Error.Data,
		}
	}
	return &JSONRPCResponse{
		JSONRPC: resp.JSONRPC,
		ID:      resp.ID,
		Result:  resp.Result,
		Error:   err,
	}
}
