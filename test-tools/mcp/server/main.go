// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package main implements a simple MCP test server for testing NemesisBot's MCP client.
// This server implements the Model Context Protocol and provides simple test tools.

package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

// JSONRPCRequest represents a JSON-RPC 2.0 request
type JSONRPCRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      interface{}     `json:"id,omitempty"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params,omitempty"`
}

// JSONRPCResponse represents a JSON-RPC 2.0 response
type JSONRPCResponse struct {
	JSONRPC string      `json:"jsonrpc"`
	ID      interface{} `json:"id"`
	Result  interface{} `json:"result,omitempty"`
	Error   *RPCError   `json:"error,omitempty"`
}

// RPCError represents a JSON-RPC error
type RPCError struct {
	Code    int         `json:"code"`
	Message string      `json:"message"`
	Data    interface{} `json:"data,omitempty"`
}

// ServerInfo provides information about the server
type ServerInfo struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// InitializeParams for the initialize request
type InitializeParams struct {
	ProtocolVersion string                 `json:"protocolVersion"`
	Capabilities    map[string]interface{} `json:"capabilities"`
	ClientInfo      map[string]interface{} `json:"clientInfo"`
}

// InitializeResult for the initialize response
type InitializeResult struct {
	ProtocolVersion string             `json:"protocolVersion"`
	Capabilities    ServerCapabilities `json:"capabilities"`
	ServerInfo      ServerInfo         `json:"serverInfo"`
}

// ServerCapabilities describes server capabilities
type ServerCapabilities struct {
	Tools     map[string]bool `json:"tools,omitempty"`
	Resources map[string]bool `json:"resources,omitempty"`
	Prompts   map[string]bool `json:"prompts,omitempty"`
}

// Tool represents a tool definition
type Tool struct {
	Name        string                 `json:"name"`
	Description string                 `json:"description,omitempty"`
	InputSchema map[string]interface{} `json:"inputSchema"`
}

// ToolCallParams for tools/call request
type ToolCallParams struct {
	Name      string                 `json:"name"`
	Arguments map[string]interface{} `json:"arguments"`
}

// ToolContent represents content in a tool call result
type ToolContent struct {
	Type     string `json:"type"`
	Text     string `json:"text,omitempty"`
	Data     string `json:"data,omitempty"`
	MimeType string `json:"mimeType,omitempty"`
}

// ToolCallResult represents the result of a tool call
type ToolCallResult struct {
	Content []ToolContent `json:"content"`
	IsError bool          `json:"isError,omitempty"`
}

// Resource represents a resource
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
}

// Prompt represents a prompt template
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
	Role    string               `json:"role"`
	Content PromptMessageContent `json:"content"`
}

// PromptMessageContent represents content in a prompt message
type PromptMessageContent struct {
	Type string `json:"type"`
	Text string `json:"text,omitempty"`
}

// PromptResult represents the result of getting a prompt
type PromptResult struct {
	Messages []PromptMessage `json:"messages"`
}

// MCPServer is a simple MCP test server
type MCPServer struct {
	tools     map[string]Tool
	resources map[string]Resource
	prompts   map[string]Prompt
}

// NewMCPServer creates a new test MCP server
func NewMCPServer() *MCPServer {
	server := &MCPServer{
		tools:     make(map[string]Tool),
		resources: make(map[string]Resource),
		prompts:   make(map[string]Prompt),
	}
	server.registerTools()
	server.registerResources()
	server.registerPrompts()
	return server
}

// registerTools registers test tools
func (s *MCPServer) registerTools() {
	// Echo tool - echoes back the input text
	s.tools["echo"] = Tool{
		Name:        "echo",
		Description: "Echoes back the input text",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"text": map[string]interface{}{
					"type":        "string",
					"description": "The text to echo back",
				},
			},
			"required": []string{"text"},
		},
	}

	// Add tool - adds two numbers
	s.tools["add"] = Tool{
		Name:        "add",
		Description: "Adds two numbers together",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"a": map[string]interface{}{
					"type":        "number",
					"description": "First number",
				},
				"b": map[string]interface{}{
					"type":        "number",
					"description": "Second number",
				},
			},
			"required": []string{"a", "b"},
		},
	}

	// Reverse tool - reverses a string
	s.tools["reverse"] = Tool{
		Name:        "reverse",
		Description: "Reverses the input string",
		InputSchema: map[string]interface{}{
			"type": "object",
			"properties": map[string]interface{}{
				"text": map[string]interface{}{
					"type":        "string",
					"description": "The text to reverse",
				},
			},
			"required": []string{"text"},
		},
	}

	// Get_time tool - returns current server time
	s.tools["get_time"] = Tool{
		Name:        "get_time",
		Description: "Returns the current server time as a string",
		InputSchema: map[string]interface{}{
			"type":       "object",
			"properties": map[string]interface{}{},
		},
	}
}

// registerResources registers test resources
func (s *MCPServer) registerResources() {
	// Test text resource
	s.resources["test://hello"] = Resource{
		URI:         "test://hello",
		Name:        "hello",
		Description: "A simple hello world resource",
		MimeType:    "text/plain",
	}

	// Test config resource
	s.resources["test://config"] = Resource{
		URI:         "test://config",
		Name:        "config",
		Description: "A test configuration resource",
		MimeType:    "application/json",
	}
}

// registerPrompts registers test prompts
func (s *MCPServer) registerPrompts() {
	// Simple prompt
	s.prompts["greeting"] = Prompt{
		Name:        "greeting",
		Description: "A simple greeting prompt",
		Arguments: []PromptArgument{
			{
				Name:        "name",
				Description: "Name to greet",
				Required:    true,
			},
		},
	}

	// Code review prompt
	s.prompts["code_review"] = Prompt{
		Name:        "code_review",
		Description: "A prompt for code review",
		Arguments: []PromptArgument{
			{
				Name:        "language",
				Description: "Programming language",
				Required:    false,
			},
		},
	}
}

// handleRequest handles an incoming JSON-RPC request
func (s *MCPServer) handleRequest(req JSONRPCRequest) *JSONRPCResponse {
	switch req.Method {
	case "initialize":
		resp := s.handleInitialize(req)
		return &resp
	case "tools/list":
		resp := s.handleToolsList(req)
		return &resp
	case "tools/call":
		resp := s.handleToolsCall(req)
		return &resp
	case "resources/list":
		resp := s.handleResourcesList(req)
		return &resp
	case "resources/read":
		resp := s.handleResourceRead(req)
		return &resp
	case "prompts/list":
		resp := s.handlePromptsList(req)
		return &resp
	case "prompts/get":
		resp := s.handlePromptGet(req)
		return &resp
	// Notifications don't need responses
	case "notifications/initialized":
		fmt.Fprintf(os.Stderr, "[MCP Server] Received initialized notification\n")
		return nil
	default:
		return &JSONRPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error: &RPCError{
				Code:    -32601,
				Message: fmt.Sprintf("Method not found: %s", req.Method),
			},
		}
	}
}

// handleInitialize handles the initialize request
func (s *MCPServer) handleInitialize(req JSONRPCRequest) JSONRPCResponse {
	var params InitializeParams
	if err := json.Unmarshal(req.Params, &params); err != nil {
		return JSONRPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error: &RPCError{
				Code:    -32602,
				Message: "Invalid params",
			},
		}
	}

	result := InitializeResult{
		ProtocolVersion: "2025-06-18",
		Capabilities: ServerCapabilities{
			Tools: map[string]bool{},
		},
		ServerInfo: ServerInfo{
			Name:    "test-mcp-server",
			Version: "1.0.0",
		},
	}

	fmt.Fprintf(os.Stderr, "[MCP Server] Initialized by client: %s\n", params.ClientInfo["name"])

	return JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
		Result:  result,
	}
}

// handleToolsList handles the tools/list request
func (s *MCPServer) handleToolsList(req JSONRPCRequest) JSONRPCResponse {
	tools := make([]Tool, 0, len(s.tools))
	for _, tool := range s.tools {
		tools = append(tools, tool)
	}

	result := map[string]interface{}{
		"tools": tools,
	}

	fmt.Fprintf(os.Stderr, "[MCP Server] Listed %d tools\n", len(tools))

	return JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
		Result:  result,
	}
}

// handleToolsCall handles the tools/call request
func (s *MCPServer) handleToolsCall(req JSONRPCRequest) JSONRPCResponse {
	var params ToolCallParams
	if err := json.Unmarshal(req.Params, &params); err != nil {
		return JSONRPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error: &RPCError{
				Code:    -32602,
				Message: "Invalid params",
			},
		}
	}

	if _, exists := s.tools[params.Name]; !exists {
		return JSONRPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error: &RPCError{
				Code:    -32602,
				Message: fmt.Sprintf("Tool not found: %s", params.Name),
			},
		}
	}

	fmt.Fprintf(os.Stderr, "[MCP Server] Called tool: %s with args: %v\n", params.Name, params.Arguments)

	result := s.executeTool(params.Name, params.Arguments)

	return JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
		Result:  result,
	}
}

// executeTool executes a tool and returns the result
func (s *MCPServer) executeTool(name string, args map[string]interface{}) ToolCallResult {
	switch name {
	case "echo":
		text, ok := args["text"].(string)
		if !ok {
			return ToolCallResult{
				Content: []ToolContent{
					{Type: "text", Text: "Error: 'text' argument must be a string"},
				},
				IsError: true,
			}
		}
		return ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: fmt.Sprintf("Echo: %s", text)},
			},
		}

	case "add":
		a, aOk := getFloat(args["a"])
		b, bOk := getFloat(args["b"])
		if !aOk || !bOk {
			return ToolCallResult{
				Content: []ToolContent{
					{Type: "text", Text: "Error: 'a' and 'b' must be numbers"},
				},
				IsError: true,
			}
		}
		result := a + b
		return ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: fmt.Sprintf("%g + %g = %g", a, b, result)},
			},
		}

	case "reverse":
		text, ok := args["text"].(string)
		if !ok {
			return ToolCallResult{
				Content: []ToolContent{
					{Type: "text", Text: "Error: 'text' argument must be a string"},
				},
				IsError: true,
			}
		}
		// Reverse the string
		runes := []rune(text)
		for i, j := 0, len(runes)-1; i < j; i, j = i+1, j-1 {
			runes[i], runes[j] = runes[j], runes[i]
		}
		return ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: fmt.Sprintf("Reverse: %s -> %s", text, string(runes))},
			},
		}

	case "get_time":
		now := strings.TrimSpace(strings.Replace(fmt.Sprintf("%s", os.Getenv("TIME")), "TIME", "", 1))
		if now == "" {
			now = fmt.Sprintf("%s", "2025-02-20 15:30:45")
		}
		return ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: fmt.Sprintf("Current time: %s", now)},
			},
		}

	default:
		return ToolCallResult{
			Content: []ToolContent{
				{Type: "text", Text: fmt.Sprintf("Unknown tool: %s", name)},
			},
			IsError: true,
		}
	}
}

// handleResourcesList handles the resources/list request
func (s *MCPServer) handleResourcesList(req JSONRPCRequest) JSONRPCResponse {
	resources := make([]Resource, 0, len(s.resources))
	for _, resource := range s.resources {
		resources = append(resources, resource)
	}

	result := map[string]interface{}{
		"resources": resources,
	}

	fmt.Fprintf(os.Stderr, "[MCP Server] Listed %d resources\n", len(resources))

	return JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
		Result:  result,
	}
}

// handleResourceRead handles the resources/read request
func (s *MCPServer) handleResourceRead(req JSONRPCRequest) JSONRPCResponse {
	var params struct {
		URI string `json:"uri"`
	}
	if err := json.Unmarshal(req.Params, &params); err != nil {
		return JSONRPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error: &RPCError{
				Code:    -32602,
				Message: "Invalid params",
			},
		}
	}

	// Find resource
	resource, exists := s.resources[params.URI]
	if !exists {
		return JSONRPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error: &RPCError{
				Code:    -32602,
				Message: fmt.Sprintf("Resource not found: %s", params.URI),
			},
		}
	}

	// Generate content based on URI
	var content string
	switch params.URI {
	case "test://hello":
		content = "Hello, World!"
	case "test://config":
		content = `{"setting": "value", "enabled": true}`
	default:
		content = fmt.Sprintf("Content of %s", params.URI)
	}

	result := ResourceContent{
		URI:      resource.URI,
		MimeType: resource.MimeType,
		Text:     content,
	}

	fmt.Fprintf(os.Stderr, "[MCP Server] Read resource: %s\n", params.URI)

	return JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
		Result:  result,
	}
}

// handlePromptsList handles the prompts/list request
func (s *MCPServer) handlePromptsList(req JSONRPCRequest) JSONRPCResponse {
	prompts := make([]Prompt, 0, len(s.prompts))
	for _, prompt := range s.prompts {
		prompts = append(prompts, prompt)
	}

	result := map[string]interface{}{
		"prompts": prompts,
	}

	fmt.Fprintf(os.Stderr, "[MCP Server] Listed %d prompts\n", len(prompts))

	return JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
		Result:  result,
	}
}

// handlePromptGet handles the prompts/get request
func (s *MCPServer) handlePromptGet(req JSONRPCRequest) JSONRPCResponse {
	var params struct {
		Name      string                 `json:"name"`
		Arguments map[string]interface{} `json:"arguments,omitempty"`
	}
	if err := json.Unmarshal(req.Params, &params); err != nil {
		return JSONRPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error: &RPCError{
				Code:    -32602,
				Message: "Invalid params",
			},
		}
	}

	// Find prompt
	_, exists := s.prompts[params.Name]
	if !exists {
		return JSONRPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error: &RPCError{
				Code:    -32602,
				Message: fmt.Sprintf("Prompt not found: %s", params.Name),
			},
		}
	}

	// Generate messages based on prompt
	var messages []PromptMessage
	switch params.Name {
	case "greeting":
		name := "World"
		if n, ok := params.Arguments["name"].(string); ok {
			name = n
		}
		messages = []PromptMessage{
			{
				Role: "user",
				Content: PromptMessageContent{
					Type: "text",
					Text: fmt.Sprintf("Hello, %s!", name),
				},
			},
		}
	case "code_review":
		lang := "Go"
		if l, ok := params.Arguments["language"].(string); ok {
			lang = l
		}
		messages = []PromptMessage{
			{
				Role: "system",
				Content: PromptMessageContent{
					Type: "text",
					Text: fmt.Sprintf("You are a code reviewer for %s code.", lang),
				},
			},
			{
				Role: "user",
				Content: PromptMessageContent{
					Type: "text",
					Text: "Please review the following code.",
				},
			},
		}
	default:
		messages = []PromptMessage{
			{
				Role: "user",
				Content: PromptMessageContent{
					Type: "text",
					Text: fmt.Sprintf("This is the '%s' prompt.", params.Name),
				},
			},
		}
	}

	result := PromptResult{
		Messages: messages,
	}

	fmt.Fprintf(os.Stderr, "[MCP Server] Got prompt: %s with args: %v\n", params.Name, params.Arguments)

	return JSONRPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
		Result:  result,
	}
}

// getFloat is a helper to get a float64 from an interface{}
func getFloat(v interface{}) (float64, bool) {
	switch f := v.(type) {
	case float64:
		return f, true
	case float32:
		return float64(f), true
	case int:
		return float64(f), true
	case int32:
		return float64(f), true
	case int64:
		return float64(f), true
	default:
		return 0, false
	}
}

// Run starts the MCP server and processes requests
func (s *MCPServer) Run() error {
	fmt.Fprintf(os.Stderr, "[MCP Server] Starting test MCP server on stdio...\n")

	// Read from stdin line by line
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {
		line := scanner.Text()

		// Skip empty lines
		if strings.TrimSpace(line) == "" {
			continue
		}

		// Parse request
		var req JSONRPCRequest
		if err := json.Unmarshal([]byte(line), &req); err != nil {
			fmt.Fprintf(os.Stderr, "[MCP Server] Failed to parse request: %v\n", err)
			// Send error response
			errorResp := JSONRPCResponse{
				JSONRPC: "2.0",
				ID:      nil,
				Error: &RPCError{
					Code:    -32700,
					Message: "Parse error",
				},
			}
			errorJSON, _ := json.Marshal(errorResp)
			fmt.Println(string(errorJSON))
			continue
		}

		// Handle request
		resp := s.handleRequest(req)

		// Send response (notifications return nil, so skip sending)
		if resp == nil {
			continue
		}

		respJSON, err := json.Marshal(resp)
		if err != nil {
			fmt.Fprintf(os.Stderr, "[MCP Server] Failed to marshal response: %v\n", err)
			continue
		}

		fmt.Println(string(respJSON))
	}

	if err := scanner.Err(); err != nil {
		return fmt.Errorf("error reading stdin: %w", err)
	}

	return nil
}

func main() {
	server := NewMCPServer()
	if err := server.Run(); err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}
}
