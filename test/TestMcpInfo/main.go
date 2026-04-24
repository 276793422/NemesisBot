// NemesisBot - AI agent
// MCP Inspector Tool — 支持三种传输协议检测 MCP 服务器，获取所有接口信息并格式化输出
package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	mcppkg "github.com/276793422/NemesisBot/module/mcp"
)

// ============================================================
// JSON-RPC 基础类型
// ============================================================

var reqIDCounter int64

func nextReqID() int64 {
	return atomic.AddInt64(&reqIDCounter, 0)
}

type jsonRPCRequest struct {
	JSONRPC string      `json:"jsonrpc"`
	ID      int64       `json:"id"`
	Method  string      `json:"method"`
	Params  interface{} `json:"params,omitempty"`
}

type jsonRPCResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      int64           `json:"id"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *struct {
		Code    int    `json:"code"`
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

// ============================================================
// 主入口
// ============================================================

func main() {
	if len(os.Args) < 2 {
		printUsage()
		os.Exit(1)
	}

	transport := os.Args[1]

	switch transport {
	case "stdio":
		// stdio <name> <command> [args...]
		if len(os.Args) < 4 {
			fmt.Println("用法: TestMcpInfo stdio <名称> <命令> [参数...]")
			fmt.Println()
			fmt.Println("示例:")
			fmt.Println("  TestMcpInfo stdio myserver npx @my/mcp-server")
			fmt.Println("  TestMcpInfo stdio test go run ../mcp/server/main.go")
			os.Exit(1)
		}
		name := os.Args[2]
		cmd := os.Args[3]
		var args []string
		if len(os.Args) > 4 {
			args = os.Args[4:]
		}
		inspectStdio(name, cmd, args)

	case "sse":
		// sse <url>
		if len(os.Args) < 3 {
			fmt.Println("用法: TestMcpInfo sse <SSE端点URL>")
			fmt.Println()
			fmt.Println("示例:")
			fmt.Println("  TestMcpInfo sse http://localhost:3000/sse")
			os.Exit(1)
		}
		inspectSSE(os.Args[2])

	case "http":
		// http <url>
		if len(os.Args) < 3 {
			fmt.Println("用法: TestMcpInfo http <Streamable HTTP端点URL>")
			fmt.Println()
			fmt.Println("示例:")
			fmt.Println("  TestMcpInfo http http://localhost:3000/mcp")
			os.Exit(1)
		}
		inspectHTTP(os.Args[2])

	case "help", "-h", "--help":
		printUsage()

	default:
		fmt.Printf("未知传输类型: %s\n\n", transport)
		printUsage()
		os.Exit(1)
	}
}

func printUsage() {
	fmt.Println("MCP Inspector — 检测 MCP 服务器接口信息")
	fmt.Println()
	fmt.Println("支持三种 MCP 传输协议:")
	fmt.Println()
	fmt.Println("用法:")
	fmt.Println("  TestMcpInfo stdio <名称> <命令> [参数...]       通过 stdio 子进程检测 MCP")
	fmt.Println("  TestMcpInfo sse   <SSE端点URL>                   通过 Server-Sent Events 检测 MCP")
	fmt.Println("  TestMcpInfo http  <HTTP端点URL>                  通过 Streamable HTTP 检测 MCP")
	fmt.Println()
	fmt.Println("示例:")
	fmt.Println("  TestMcpInfo stdio test go run ../mcp/server/main.go")
	fmt.Println("  TestMcpInfo stdio window npx @anthropic/window-mcp")
	fmt.Println("  TestMcpInfo sse http://localhost:3000/sse")
	fmt.Println("  TestMcpInfo http http://localhost:3000/mcp")
	fmt.Println()
	fmt.Println("说明:")
	fmt.Println("  stdio  — 通过启动子进程，使用标准输入/输出通信（最常见）")
	fmt.Println("  sse    — 通过 HTTP Server-Sent Events 协议通信（旧版远程 MCP）")
	fmt.Println("  http   — 通过 Streamable HTTP 协议通信（新版远程 MCP）")
}

// ============================================================
// stdio 传输 — 使用项目 mcp 包
// ============================================================

func inspectStdio(name, command string, args []string) {
	fmt.Printf("传输类型: stdio\n")
	fmt.Printf("正在检测 MCP 服务器: %s\n", name)
	fmt.Printf("命令: %s %s\n", command, strings.Join(args, " "))
	fmt.Println()

	client, err := mcppkg.NewClient(&mcppkg.ServerConfig{
		Name:    name,
		Command: command,
		Args:    args,
		Timeout: 30,
	})
	if err != nil {
		fmt.Printf("创建客户端失败: %v\n", err)
		os.Exit(1)
	}
	defer client.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// 初始化
	fmt.Println("── 初始化 ──")
	initRes, err := client.Initialize(ctx)
	if err != nil {
		fmt.Printf("初始化失败: %v\n", err)
		os.Exit(1)
	}

	printServerInfo(initRes.ServerInfo.Name, initRes.ServerInfo.Version, initRes.ProtocolVersion, initRes.Capabilities)

	// 工具
	fmt.Println()
	fmt.Println("── 工具 (Tools) ──")
	tools, err := client.ListTools(ctx)
	if err != nil {
		fmt.Printf("  获取工具列表失败: %v\n", err)
	} else if len(tools) == 0 {
		fmt.Println("  （无工具）")
	} else {
		fmt.Printf("  共 %d 个工具:\n\n", len(tools))
		for i, t := range tools {
			printTool(i+1, t.Name, t.Description, t.InputSchema)
		}
	}

	// 资源
	fmt.Println("── 资源 (Resources) ──")
	resources, err := client.ListResources(ctx)
	if err != nil {
		fmt.Printf("  获取资源列表失败: %v\n", err)
	} else if len(resources) == 0 {
		fmt.Println("  （无资源）")
	} else {
		fmt.Printf("  共 %d 个资源:\n\n", len(resources))
		for i, r := range resources {
			printResource(i+1, r.URI, r.Name, r.Description, r.MimeType)
		}
	}

	// 提示
	fmt.Println("── 提示 (Prompts) ──")
	prompts, err := client.ListPrompts(ctx)
	if err != nil {
		fmt.Printf("  获取提示列表失败: %v\n", err)
	} else if len(prompts) == 0 {
		fmt.Println("  （无提示）")
	} else {
		fmt.Printf("  共 %d 个提示:\n\n", len(prompts))
		for i, p := range prompts {
			printPrompt(i+1, p.Name, p.Description, convertPromptArgs(p.Arguments))
		}
	}

	// 摘要
	printJSONSummary("stdio",
		initRes.ServerInfo.Name, initRes.ServerInfo.Version, initRes.ProtocolVersion,
		extractCapList(initRes.Capabilities),
		extractToolNames(tools), extractResourceURIs(resources), extractPromptNames(prompts))
}

func convertPromptArgs(args []mcppkg.PromptArgument) []promptArg {
	result := make([]promptArg, len(args))
	for i, a := range args {
		result[i] = promptArg{Name: a.Name, Description: a.Description, Required: a.Required}
	}
	return result
}

func extractCapList(caps mcppkg.ServerCapabilities) []string {
	var list []string
	if caps.Tools != nil {
		list = append(list, "Tools")
	}
	if caps.Resources != nil {
		list = append(list, "Resources")
	}
	if caps.Prompts != nil {
		list = append(list, "Prompts")
	}
	if len(list) == 0 {
		list = append(list, "无")
	}
	return list
}

func extractToolNames(tools []mcppkg.Tool) []string {
	names := make([]string, len(tools))
	for i, t := range tools {
		names[i] = t.Name
	}
	return names
}

func extractResourceURIs(resources []mcppkg.Resource) []string {
	uris := make([]string, len(resources))
	for i, r := range resources {
		uris[i] = r.URI
	}
	return uris
}

func extractPromptNames(prompts []mcppkg.Prompt) []string {
	names := make([]string, len(prompts))
	for i, p := range prompts {
		names[i] = p.Name
	}
	return names
}

// ============================================================
// SSE 传输 — Server-Sent Events
// ============================================================

type sseEvent struct {
	Event string
	Data  string
}

type sseTransport struct {
	baseURL   string
	postURL   string
	client    *http.Client
	cancelSSE context.CancelFunc
	pending   map[int64]chan *jsonRPCResponse
	mu        sync.Mutex
}

func inspectSSE(rawURL string) {
	fmt.Printf("传输类型: SSE (Server-Sent Events)\n")
	fmt.Printf("正在连接: %s\n", rawURL)
	fmt.Println()

	t, err := newSSETransport(rawURL)
	if err != nil {
		fmt.Printf("连接 SSE 失败: %v\n", err)
		os.Exit(1)
	}
	defer t.close()

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	runNetInspection(ctx, t, "sse")
}

func newSSETransport(baseURL string) (*sseTransport, error) {
	t := &sseTransport{
		baseURL: baseURL,
		client:  &http.Client{Timeout: 30 * time.Second},
		pending: make(map[int64]chan *jsonRPCResponse),
	}

	ctx, cancel := context.WithCancel(context.Background())
	t.cancelSSE = cancel

	// 打开 SSE 连接
	req, err := http.NewRequestWithContext(ctx, "GET", baseURL, nil)
	if err != nil {
		cancel()
		return nil, fmt.Errorf("创建请求失败: %w", err)
	}
	req.Header.Set("Accept", "text/event-stream")

	resp, err := t.client.Do(req)
	if err != nil {
		cancel()
		return nil, fmt.Errorf("连接失败: %w", err)
	}
	if resp.StatusCode != http.StatusOK {
		resp.Body.Close()
		cancel()
		return nil, fmt.Errorf("连接返回状态码: %d", resp.StatusCode)
	}

	// 后台读取 SSE 事件
	eventCh := make(chan sseEvent, 100)
	go t.readEvents(resp.Body, eventCh)

	// 等待 endpoint 事件
	timeout := time.After(10 * time.Second)
	for {
		select {
		case <-timeout:
			cancel()
			return nil, fmt.Errorf("等待 endpoint 事件超时（10秒）")
		case ev := <-eventCh:
			if ev.Event == "endpoint" {
				t.postURL = resolveURL(baseURL, ev.Data)
				fmt.Printf("已获取消息端点: %s\n\n", t.postURL)
				// 启动事件分发协程
				go t.dispatchEvents(eventCh)
				return t, nil
			}
		}
	}
}

func (t *sseTransport) readEvents(body io.ReadCloser, ch chan<- sseEvent) {
	defer body.Close()
	defer close(ch)
	scanner := bufio.NewScanner(body)
	scanner.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	var event, data string

	for scanner.Scan() {
		line := scanner.Text()
		if strings.HasPrefix(line, "event:") {
			event = strings.TrimSpace(strings.TrimPrefix(line, "event:"))
		} else if strings.HasPrefix(line, "data:") {
			data = strings.TrimSpace(strings.TrimPrefix(line, "data:"))
		} else if line == "" && data != "" {
			ch <- sseEvent{Event: event, Data: data}
			event = ""
			data = ""
		}
	}
}

func (t *sseTransport) dispatchEvents(ch <-chan sseEvent) {
	for ev := range ch {
		if ev.Event != "message" {
			continue
		}
		var resp jsonRPCResponse
		if err := json.Unmarshal([]byte(ev.Data), &resp); err != nil {
			continue
		}
		t.mu.Lock()
		waitCh, ok := t.pending[resp.ID]
		if ok {
			delete(t.pending, resp.ID)
		}
		t.mu.Unlock()
		if ok {
			waitCh <- &resp
		}
	}
}

func (t *sseTransport) send(ctx context.Context, method string, params interface{}) (*jsonRPCResponse, error) {
	id := atomic.AddInt64(&reqIDCounter, 1)

	t.mu.Lock()
	respCh := make(chan *jsonRPCResponse, 1)
	t.pending[id] = respCh
	t.mu.Unlock()

	reqBody, _ := json.Marshal(jsonRPCRequest{
		JSONRPC: "2.0",
		ID:      id,
		Method:  method,
		Params:  params,
	})

	req, err := http.NewRequestWithContext(ctx, "POST", t.postURL, bytes.NewReader(reqBody))
	if err != nil {
		t.mu.Lock()
		delete(t.pending, id)
		t.mu.Unlock()
		return nil, fmt.Errorf("创建请求失败: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := t.client.Do(req)
	if err != nil {
		t.mu.Lock()
		delete(t.pending, id)
		t.mu.Unlock()
		return nil, fmt.Errorf("发送请求失败: %w", err)
	}
	defer resp.Body.Close()

	// SSE 传输：POST 通常返回 202，实际响应通过 SSE 流返回
	io.Copy(io.Discard, resp.Body)

	select {
	case <-ctx.Done():
		t.mu.Lock()
		delete(t.pending, id)
		t.mu.Unlock()
		return nil, ctx.Err()
	case rpcResp := <-respCh:
		if rpcResp.Error != nil {
			return nil, fmt.Errorf("RPC 错误 [%d]: %s", rpcResp.Error.Code, rpcResp.Error.Message)
		}
		return rpcResp, nil
	}
}

func (t *sseTransport) sendNotification(ctx context.Context, method string, params interface{}) error {
	reqBody, _ := json.Marshal(map[string]interface{}{
		"jsonrpc": "2.0",
		"method":  method,
		"params":  params,
	})

	req, err := http.NewRequestWithContext(ctx, "POST", t.postURL, bytes.NewReader(reqBody))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := t.client.Do(req)
	if err != nil {
		return err
	}
	resp.Body.Close()
	return nil
}

func (t *sseTransport) close() {
	if t.cancelSSE != nil {
		t.cancelSSE()
	}
}

// ============================================================
// Streamable HTTP 传输
// ============================================================

type httpTransport struct {
	endpoint  string
	client    *http.Client
	sessionID string
}

func inspectHTTP(rawURL string) {
	fmt.Printf("传输类型: Streamable HTTP\n")
	fmt.Printf("正在连接: %s\n", rawURL)
	fmt.Println()

	t := &httpTransport{
		endpoint: rawURL,
		client:   &http.Client{Timeout: 30 * time.Second},
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	runNetInspection(ctx, t, "http")
}

func (t *httpTransport) send(ctx context.Context, method string, params interface{}) (*jsonRPCResponse, error) {
	id := atomic.AddInt64(&reqIDCounter, 1)

	reqBody, _ := json.Marshal(jsonRPCRequest{
		JSONRPC: "2.0",
		ID:      id,
		Method:  method,
		Params:  params,
	})

	req, err := http.NewRequestWithContext(ctx, "POST", t.endpoint, bytes.NewReader(reqBody))
	if err != nil {
		return nil, fmt.Errorf("创建请求失败: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	if t.sessionID != "" {
		req.Header.Set("Mcp-Session-Id", t.sessionID)
	}

	resp, err := t.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("发送请求失败: %w", err)
	}
	defer resp.Body.Close()

	// 保存 session ID
	if sid := resp.Header.Get("Mcp-Session-Id"); sid != "" {
		t.sessionID = sid
	}

	ct := resp.Header.Get("Content-Type")

	// SSE 响应流
	if strings.Contains(ct, "text/event-stream") {
		return t.readSSEResponse(resp.Body, id)
	}

	// 直接 JSON 响应
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("读取响应失败: %w", err)
	}

	var rpcResp jsonRPCResponse
	if err := json.Unmarshal(body, &rpcResp); err != nil {
		return nil, fmt.Errorf("解析响应失败: %w\n原始数据: %s", err, string(body))
	}

	if rpcResp.Error != nil {
		return nil, fmt.Errorf("RPC 错误 [%d]: %s", rpcResp.Error.Code, rpcResp.Error.Message)
	}
	return &rpcResp, nil
}

func (t *httpTransport) readSSEResponse(body io.Reader, expectedID int64) (*jsonRPCResponse, error) {
	scanner := bufio.NewScanner(body)
	scanner.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	var data string

	for scanner.Scan() {
		line := scanner.Text()
		if strings.HasPrefix(line, "data:") {
			data = strings.TrimSpace(strings.TrimPrefix(line, "data:"))
		} else if line == "" && data != "" {
			var resp jsonRPCResponse
			if err := json.Unmarshal([]byte(data), &resp); err == nil && resp.ID == expectedID {
				if resp.Error != nil {
					return nil, fmt.Errorf("RPC 错误 [%d]: %s", resp.Error.Code, resp.Error.Message)
				}
				return &resp, nil
			}
			data = ""
		}
	}

	return nil, fmt.Errorf("未在 SSE 响应中找到请求 ID %d 的响应", expectedID)
}

func (t *httpTransport) sendNotification(ctx context.Context, method string, params interface{}) error {
	reqBody, _ := json.Marshal(map[string]interface{}{
		"jsonrpc": "2.0",
		"method":  method,
		"params":  params,
	})

	req, err := http.NewRequestWithContext(ctx, "POST", t.endpoint, bytes.NewReader(reqBody))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	if t.sessionID != "" {
		req.Header.Set("Mcp-Session-Id", t.sessionID)
	}

	resp, err := t.client.Do(req)
	if err != nil {
		return err
	}
	resp.Body.Close()
	return nil
}

func (t *httpTransport) close() {}

// ============================================================
// 网络传输通用检测流程（SSE + HTTP）
// ============================================================

type netTransport interface {
	send(ctx context.Context, method string, params interface{}) (*jsonRPCResponse, error)
	sendNotification(ctx context.Context, method string, params interface{}) error
	close()
}

// MCP 协议结果类型
type mcpInitResult struct {
	ProtocolVersion string `json:"protocolVersion"`
	Capabilities    struct {
		Tools     json.RawMessage `json:"tools,omitempty"`
		Resources json.RawMessage `json:"resources,omitempty"`
		Prompts   json.RawMessage `json:"prompts,omitempty"`
	} `json:"capabilities"`
	ServerInfo struct {
		Name    string `json:"name"`
		Version string `json:"version"`
	} `json:"serverInfo"`
}

type mcpToolDef struct {
	Name        string                 `json:"name"`
	Description string                 `json:"description,omitempty"`
	InputSchema map[string]interface{} `json:"inputSchema,omitempty"`
}

type mcpResourceDef struct {
	URI         string `json:"uri"`
	Name        string `json:"name,omitempty"`
	Description string `json:"description,omitempty"`
	MimeType    string `json:"mimeType,omitempty"`
}

type promptArg struct {
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
	Required    bool   `json:"required,omitempty"`
}

type mcpPromptDef struct {
	Name        string       `json:"name"`
	Description string       `json:"description,omitempty"`
	Arguments   []promptArg  `json:"arguments,omitempty"`
}

type mcpListToolsResult struct {
	Tools []mcpToolDef `json:"tools"`
}

type mcpListResourcesResult struct {
	Resources []mcpResourceDef `json:"resources"`
}

type mcpListPromptsResult struct {
	Prompts []mcpPromptDef `json:"prompts"`
}

func runNetInspection(ctx context.Context, t netTransport, typeName string) {
	// 初始化
	fmt.Println("── 初始化 ──")

	initParams := map[string]interface{}{
		"protocolVersion": "2024-11-05",
		"capabilities":    map[string]interface{}{},
		"clientInfo": map[string]string{
			"name":    "TestMcpInfo",
			"version": "1.0.0",
		},
	}

	resp, err := t.send(ctx, "initialize", initParams)
	if err != nil {
		fmt.Printf("初始化失败: %v\n", err)
		os.Exit(1)
	}

	var initRes mcpInitResult
	if err := json.Unmarshal(resp.Result, &initRes); err != nil {
		fmt.Printf("解析初始化结果失败: %v\n", err)
		os.Exit(1)
	}

	var capList []string
	if initRes.Capabilities.Tools != nil {
		capList = append(capList, "Tools")
	}
	if initRes.Capabilities.Resources != nil {
		capList = append(capList, "Resources")
	}
	if initRes.Capabilities.Prompts != nil {
		capList = append(capList, "Prompts")
	}
	if len(capList) == 0 {
		capList = append(capList, "无")
	}

	fmt.Printf("  服务器:      %s\n", initRes.ServerInfo.Name)
	fmt.Printf("  版本:        %s\n", initRes.ServerInfo.Version)
	fmt.Printf("  协议版本:    %s\n", initRes.ProtocolVersion)
	fmt.Printf("  支持能力:    %s\n", strings.Join(capList, ", "))

	// 发送 initialized 通知
	t.sendNotification(ctx, "notifications/initialized", nil)

	// 工具
	fmt.Println()
	fmt.Println("── 工具 (Tools) ──")
	var toolNames []string
	resp, err = t.send(ctx, "tools/list", nil)
	if err != nil {
		fmt.Printf("  获取工具列表失败: %v\n", err)
	} else {
		var result mcpListToolsResult
		if err := json.Unmarshal(resp.Result, &result); err != nil {
			fmt.Printf("  解析工具列表失败: %v\n", err)
		} else if len(result.Tools) == 0 {
			fmt.Println("  （无工具）")
		} else {
			toolNames = make([]string, len(result.Tools))
			fmt.Printf("  共 %d 个工具:\n\n", len(result.Tools))
			for i, tool := range result.Tools {
				toolNames[i] = tool.Name
				printTool(i+1, tool.Name, tool.Description, tool.InputSchema)
			}
		}
	}

	// 资源
	fmt.Println("── 资源 (Resources) ──")
	var resourceURIs []string
	resp, err = t.send(ctx, "resources/list", nil)
	if err != nil {
		fmt.Printf("  获取资源列表失败: %v\n", err)
	} else {
		var result mcpListResourcesResult
		if err := json.Unmarshal(resp.Result, &result); err != nil {
			fmt.Printf("  解析资源列表失败: %v\n", err)
		} else if len(result.Resources) == 0 {
			fmt.Println("  （无资源）")
		} else {
			resourceURIs = make([]string, len(result.Resources))
			fmt.Printf("  共 %d 个资源:\n\n", len(result.Resources))
			for i, r := range result.Resources {
				resourceURIs[i] = r.URI
				printResource(i+1, r.URI, r.Name, r.Description, r.MimeType)
			}
		}
	}

	// 提示
	fmt.Println("── 提示 (Prompts) ──")
	var promptNames []string
	resp, err = t.send(ctx, "prompts/list", nil)
	if err != nil {
		fmt.Printf("  获取提示列表失败: %v\n", err)
	} else {
		var result mcpListPromptsResult
		if err := json.Unmarshal(resp.Result, &result); err != nil {
			fmt.Printf("  解析提示列表失败: %v\n", err)
		} else if len(result.Prompts) == 0 {
			fmt.Println("  （无提示）")
		} else {
			promptNames = make([]string, len(result.Prompts))
			fmt.Printf("  共 %d 个提示:\n\n", len(result.Prompts))
			for i, p := range result.Prompts {
				promptNames[i] = p.Name
				printPrompt(i+1, p.Name, p.Description, p.Arguments)
			}
		}
	}

	// 摘要
	printJSONSummary(typeName,
		initRes.ServerInfo.Name, initRes.ServerInfo.Version, initRes.ProtocolVersion,
		capList, toolNames, resourceURIs, promptNames)
}

// ============================================================
// 共用输出函数
// ============================================================

func printServerInfo(name, version, protocol string, caps mcppkg.ServerCapabilities) {
	fmt.Printf("  服务器:      %s\n", name)
	fmt.Printf("  版本:        %s\n", version)
	fmt.Printf("  协议版本:    %s\n", protocol)
	fmt.Printf("  支持能力:    %s\n", strings.Join(extractCapList(caps), ", "))
}

func printTool(index int, name, desc string, schema map[string]interface{}) {
	fmt.Printf("  [%d] %s\n", index, name)
	if desc != "" {
		fmt.Printf("      描述: %s\n", desc)
	}
	if schema != nil {
		printSchema("      参数", schema)
	}
	fmt.Println()
}

func printResource(index int, uri, name, desc, mimeType string) {
	fmt.Printf("  [%d] %s\n", index, uri)
	if name != "" {
		fmt.Printf("      名称: %s\n", name)
	}
	if desc != "" {
		fmt.Printf("      描述: %s\n", desc)
	}
	if mimeType != "" {
		fmt.Printf("      类型: %s\n", mimeType)
	}
	fmt.Println()
}

func printPrompt(index int, name, desc string, args []promptArg) {
	fmt.Printf("  [%d] %s\n", index, name)
	if desc != "" {
		fmt.Printf("      描述: %s\n", desc)
	}
	if len(args) > 0 {
		fmt.Printf("      参数:\n")
		for _, arg := range args {
			req := ""
			if arg.Required {
				req = " (必填)"
			}
			aDesc := ""
			if arg.Description != "" {
				aDesc = fmt.Sprintf(" — %s", arg.Description)
			}
			fmt.Printf("        - %s%s%s\n", arg.Name, req, aDesc)
		}
	}
	fmt.Println()
}

func printJSONSummary(transport, serverName, serverVersion, protocol string, caps, toolNames, resourceURIs, promptNames []string) {
	fmt.Println("── JSON 摘要 ──")

	summary := map[string]interface{}{
		"server": map[string]string{
			"name":    serverName,
			"version": serverVersion,
		},
		"protocol_version": protocol,
		"transport":        transport,
		"capabilities":     caps,
	}
	if toolNames != nil {
		summary["tools"] = toolNames
	}
	if resourceURIs != nil {
		summary["resources"] = resourceURIs
	}
	if promptNames != nil {
		summary["prompts"] = promptNames
	}

	jsonData, _ := json.MarshalIndent(summary, "  ", "  ")
	fmt.Printf("  %s\n", string(jsonData))

	fmt.Println()
	fmt.Println("检测完成")
}

func printSchema(prefix string, schema map[string]interface{}) {
	schemaType, _ := schema["type"].(string)
	if schemaType == "" {
		return
	}

	fmt.Printf("%s (%s):\n", prefix, schemaType)

	properties, ok := schema["properties"].(map[string]interface{})
	if !ok || len(properties) == 0 {
		return
	}

	keys := make([]string, 0, len(properties))
	for k := range properties {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	requiredMap := make(map[string]bool)
	if req, ok := schema["required"].([]interface{}); ok {
		for _, r := range req {
			if s, ok := r.(string); ok {
				requiredMap[s] = true
			}
		}
	}

	for _, key := range keys {
		prop, ok := properties[key].(map[string]interface{})
		if !ok {
			continue
		}

		propType, _ := prop["type"].(string)
		desc, _ := prop["description"].(string)

		reqMark := ""
		if requiredMap[key] {
			reqMark = " *必填"
		}

		line := fmt.Sprintf("        - %s", key)
		if propType != "" {
			line += fmt.Sprintf(" (%s)", propType)
		}
		if reqMark != "" {
			line += reqMark
		}
		if desc != "" {
			line += fmt.Sprintf(" — %s", desc)
		}
		fmt.Println(line)

		if enum, ok := prop["enum"].([]interface{}); ok && len(enum) > 0 {
			vals := make([]string, len(enum))
			for i, v := range enum {
				vals[i] = fmt.Sprintf("%v", v)
			}
			fmt.Printf("          可选值: %s\n", strings.Join(vals, ", "))
		}

		if nested, ok := prop["properties"].(map[string]interface{}); ok {
			for nk, nv := range nested {
				nestedProp, ok := nv.(map[string]interface{})
				if !ok {
					continue
				}
				nType, _ := nestedProp["type"].(string)
				nDesc, _ := nestedProp["description"].(string)
				nestedLine := fmt.Sprintf("          .%s", nk)
				if nType != "" {
					nestedLine += fmt.Sprintf(" (%s)", nType)
				}
				if nDesc != "" {
					nestedLine += fmt.Sprintf(" — %s", nDesc)
				}
				fmt.Println(nestedLine)
			}
		}
	}
}

// resolveURL 将相对 URL 解析为绝对 URL
func resolveURL(base, ref string) string {
	if strings.HasPrefix(ref, "http://") || strings.HasPrefix(ref, "https://") {
		return ref
	}
	if !strings.HasPrefix(ref, "/") {
		ref = "/" + ref
	}
	u, err := url.Parse(base)
	if err != nil {
		return ref
	}
	return u.Scheme + "://" + u.Host + ref
}
