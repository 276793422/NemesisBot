package agent_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/observer"
	"github.com/276793422/NemesisBot/module/providers"
	"github.com/276793422/NemesisBot/module/providers/protocoltypes"
)

// =============================================
// 全流程 Mock 测试：AgentLoop + Observer → 验证日志文件写入
// =============================================
// 这些测试通过实际的 AgentLoop 流程（ProcessDirect / ProcessDirectWithChannel）
// 验证 Observer 路径下 RequestLogger 仍然正确写入所有日志文件。
//
// 核心关注点：不能因为增加了 Observer 而丢失原有日志。
// =============================================

// createConfigWithLogging 创建启用了 LLM 日志的配置
func createConfigWithLogging(workspace string) *config.Config {
	return &config.Config{
		Agents: config.AgentsConfig{
			Defaults: config.AgentDefaults{
				LLM:                 "mock/model",
				Workspace:           workspace,
				MaxToolIterations:   10,
				MaxTokens:           4000,
				RestrictToWorkspace: true,
			},
		},
		Logging: &config.LoggingConfig{
			LLM: &config.LLMLogConfig{
				Enabled:     true,
				LogDir:      "requests",
				DetailLevel: "full",
			},
		},
	}
}

// findRequestSessionDir 在 workspace/requests/ 下查找 session 目录
func findRequestSessionDir(t *testing.T, workspace string) string {
	t.Helper()
	logDir := filepath.Join(workspace, "requests")
	entries, err := os.ReadDir(logDir)
	if err != nil {
		t.Fatalf("failed to read log dir %s: %v", logDir, err)
	}
	for _, e := range entries {
		if e.IsDir() {
			return filepath.Join(logDir, e.Name())
		}
	}
	t.Fatal("no session directory found under requests/")
	return ""
}

// findAllRequestSessionDirs 在 workspace/requests/ 下查找所有 session 目录
func findAllRequestSessionDirs(t *testing.T, workspace string) []string {
	t.Helper()
	logDir := filepath.Join(workspace, "requests")
	entries, err := os.ReadDir(logDir)
	if err != nil {
		t.Fatalf("failed to read log dir %s: %v", logDir, err)
	}
	var dirs []string
	for _, e := range entries {
		if e.IsDir() {
			dirs = append(dirs, filepath.Join(logDir, e.Name()))
		}
	}
	return dirs
}

// readDirFiles 读取目录下所有文件的内容
func readDirFiles(t *testing.T, dir string) map[string]string {
	t.Helper()
	files := make(map[string]string)
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("failed to read dir %s: %v", dir, err)
	}
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		data, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			t.Fatalf("failed to read file %s: %v", e.Name(), err)
		}
		files[e.Name()] = string(data)
	}
	return files
}

// hasFileWithSuffix 检查文件 map 中是否有指定后缀的文件
func hasFileWithSuffix(files map[string]string, suffix string) bool {
	for name := range files {
		if strings.HasSuffix(name, suffix) {
			return true
		}
	}
	return false
}

// findFileWithSuffix 找到指定后缀的文件内容
func findFileWithSuffix(files map[string]string, suffix string) string {
	for name, content := range files {
		if strings.HasSuffix(name, suffix) {
			return content
		}
	}
	return ""
}

// countFilesWithSuffix 统计指定后缀的文件数量
func countFilesWithSuffix(files map[string]string, suffix string) int {
	count := 0
	for name := range files {
		if strings.HasSuffix(name, suffix) {
			count++
		}
	}
	return count
}

// =============================================
// Test 1: Observer 路径 — 简单对话（无工具调用）→ 验证日志文件
// =============================================

func TestObserverFullFlow_SimpleConversation_LogsFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Hello! How can I help you?"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	// 创建 ObserverManager + RequestLoggerObserver
	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	response, err := loop.ProcessDirectWithChannel(ctx, "Hi there", "session-1", "web", "chat-001")
	if err != nil {
		t.Fatalf("ProcessDirectWithChannel failed: %v", err)
	}
	if response == "" {
		t.Fatal("expected non-empty response")
	}

	// 验证日志文件已创建
	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	// 必须有以下文件：
	// request.md     — 用户消息
	// AI.Request.md  — LLM 请求
	// AI.Response.md — LLM 响应
	// response.md    — 最终响应
	if !hasFileWithSuffix(files, ".request.md") {
		t.Error("missing request.md file")
	}
	if !hasFileWithSuffix(files, ".AI.Request.md") {
		t.Error("missing AI.Request.md file")
	}
	if !hasFileWithSuffix(files, ".AI.Response.md") {
		t.Error("missing AI.Response.md file")
	}
	if !hasFileWithSuffix(files, ".response.md") {
		t.Error("missing response.md file")
	}

	// 验证文件内容
	reqContent := findFileWithSuffix(files, ".request.md")
	if !strings.Contains(reqContent, "Hi there") {
		t.Error("request file should contain user message 'Hi there'")
	}

	respContent := findFileWithSuffix(files, ".response.md")
	if !strings.Contains(respContent, "Hello! How can I help you?") {
		t.Error("response file should contain the final response")
	}

	aiRespContent := findFileWithSuffix(files, ".AI.Response.md")
	if !strings.Contains(aiRespContent, "Hello! How can I help you?") {
		t.Error("AI response file should contain LLM response")
	}
}

// =============================================
// Test 2: Legacy 路径（无 Observer）— 简单对话 → 验证日志文件
// =============================================

func TestLegacyFullFlow_SimpleConversation_LogsFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Hello from legacy path!"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)
	// 不注入 ObserverManager → 走 legacy 路径

	ctx := context.Background()
	response, err := loop.ProcessDirectWithChannel(ctx, "Hi legacy", "session-legacy", "web", "chat-002")
	if err != nil {
		t.Fatalf("ProcessDirectWithChannel failed: %v", err)
	}
	if response == "" {
		t.Fatal("expected non-empty response")
	}

	// 验证日志文件已创建（与 Observer 路径相同的文件）
	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	if !hasFileWithSuffix(files, ".request.md") {
		t.Error("missing request.md file (legacy)")
	}
	if !hasFileWithSuffix(files, ".AI.Request.md") {
		t.Error("missing AI.Request.md file (legacy)")
	}
	if !hasFileWithSuffix(files, ".AI.Response.md") {
		t.Error("missing AI.Response.md file (legacy)")
	}
	if !hasFileWithSuffix(files, ".response.md") {
		t.Error("missing response.md file (legacy)")
	}
}

// =============================================
// Test 3: Observer 路径 vs Legacy 路径 — 文件类型对比
// 确保两种路径产生完全相同的文件类型
// =============================================

func TestObserverVsLegacy_SameFileTypes(t *testing.T) {
	// Observer 路径
	obsWorkspace := t.TempDir()
	obsCfg := createConfigWithLogging(obsWorkspace)
	obsBus := bus.NewMessageBus()
	obsProvider := &mockLLMProvider{
		responses: []string{"Observer response"},
	}
	obsLoop := agent.NewAgentLoop(obsCfg, obsBus, obsProvider)
	obsMgr := observer.NewManager()
	obsMgr.Register(agent.NewRequestLoggerObserver(obsCfg.Logging, obsWorkspace))
	obsLoop.SetObserverManager(obsMgr)

	obsResp, err := obsLoop.ProcessDirectWithChannel(context.Background(), "Test message", "obs-session", "web", "chat-obs")
	if err != nil {
		t.Fatalf("observer path failed: %v", err)
	}
	_ = obsResp

	// Legacy 路径
	legWorkspace := t.TempDir()
	legCfg := createConfigWithLogging(legWorkspace)
	legBus := bus.NewMessageBus()
	legProvider := &mockLLMProvider{
		responses: []string{"Legacy response"},
	}
	legLoop := agent.NewAgentLoop(legCfg, legBus, legProvider)

	legResp, err := legLoop.ProcessDirectWithChannel(context.Background(), "Test message", "leg-session", "web", "chat-leg")
	if err != nil {
		t.Fatalf("legacy path failed: %v", err)
	}
	_ = legResp

	// 读取两边的文件
	obsSessionDir := findRequestSessionDir(t, obsWorkspace)
	obsFiles := readDirFiles(t, obsSessionDir)

	legSessionDir := findRequestSessionDir(t, legWorkspace)
	legFiles := readDirFiles(t, legSessionDir)

	// 比较文件数量
	if len(obsFiles) != len(legFiles) {
		obsNames := fileKeys(obsFiles)
		legNames := fileKeys(legFiles)
		t.Errorf("file count mismatch: observer=%d (%v), legacy=%d (%v)",
			len(obsFiles), obsNames, len(legFiles), legNames)
	}

	// 比较文件后缀集合
	obsSuffixes := collectFileSuffixes(obsFiles)
	legSuffixes := collectFileSuffixes(legFiles)
	for suffix := range legSuffixes {
		if !obsSuffixes[suffix] {
			t.Errorf("observer path missing file type: %s", suffix)
		}
	}
	for suffix := range obsSuffixes {
		if !legSuffixes[suffix] {
			t.Errorf("observer path has extra file type: %s", suffix)
		}
	}
}

// =============================================
// Test 4: Observer 路径 — 带工具调用的对话 → 验证 Local.md
// =============================================

func TestObserverFullFlow_WithToolCalls_LogsFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()

	// 第一次调用返回 tool_call，第二次返回文本
	provider := &mockLLMProvider{}
	provider.SetCustomResponse(&providers.LLMResponse{
		Content:      "",
		FinishReason: "tool_calls",
		ToolCalls: []protocoltypes.ToolCall{
			{
				ID:   "tc-001",
				Name: "read_file",
				Arguments: map[string]interface{}{
					"path": "/tmp/test.txt",
				},
			},
		},
	})

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	// ProcessDirect will call the LLM, which returns a tool_call
	// The tool system will execute it, then call LLM again
	response, err := loop.ProcessDirectWithChannel(ctx, "Read the test file", "session-tool", "web", "chat-tool")
	// Tool execution may or may not succeed depending on the tool being registered
	// The key point is that the observer should log the events regardless
	_ = response
	_ = err

	// Verify session directory was created
	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	// 必须至少有 request + response 文件
	if !hasFileWithSuffix(files, ".request.md") {
		t.Error("missing request.md file")
	}
	if !hasFileWithSuffix(files, ".response.md") {
		t.Error("missing response.md file")
	}

	// 必须有 AI.Request.md（至少一个 LLM 请求）
	if !hasFileWithSuffix(files, ".AI.Request.md") {
		t.Error("missing AI.Request.md file")
	}

	// 必须有 AI.Response.md（至少一个 LLM 响应）
	if !hasFileWithSuffix(files, ".AI.Response.md") {
		t.Error("missing AI.Response.md file")
	}

	// 验证 request 文件包含用户消息
	reqContent := findFileWithSuffix(files, ".request.md")
	if !strings.Contains(reqContent, "Read the test file") {
		t.Error("request file should contain user message")
	}
}

// =============================================
// Test 5: Observer 路径 — LLM 错误 → 验证错误路径仍然写日志
// =============================================

func TestObserverFullFlow_LLMError_LogsFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{}
	provider.SetError(true)

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	_, err := loop.ProcessDirectWithChannel(ctx, "This will fail", "session-err", "web", "chat-err")
	if err == nil {
		t.Fatal("expected error from LLM")
	}

	// 即使 LLM 出错，也应该有日志文件
	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	// 至少要有 request.md（用户消息在 conversation_start 时写入）
	if !hasFileWithSuffix(files, ".request.md") {
		t.Error("missing request.md file even though error occurred")
	}

	// 应该有 response.md（错误信息作为最终响应）
	if !hasFileWithSuffix(files, ".response.md") {
		t.Error("missing response.md file (error response)")
	}

	// 验证 response 文件包含错误信息
	respContent := findFileWithSuffix(files, ".response.md")
	if !strings.Contains(respContent, "Error") {
		t.Error("response file should contain error information")
	}
}

// =============================================
// Test 6: Legacy 路径 — LLM 错误 → 验证错误路径仍然写日志
// =============================================

func TestLegacyFullFlow_LLMError_LogsFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{}
	provider.SetError(true)

	loop := agent.NewAgentLoop(cfg, msgBus, provider)
	// 不注入 Observer

	ctx := context.Background()
	_, err := loop.ProcessDirectWithChannel(ctx, "This will fail", "session-err-leg", "web", "chat-err-leg")
	if err == nil {
		t.Fatal("expected error from LLM")
	}

	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	if !hasFileWithSuffix(files, ".request.md") {
		t.Error("missing request.md file (legacy error path)")
	}
	if !hasFileWithSuffix(files, ".response.md") {
		t.Error("missing response.md file (legacy error path)")
	}
}

// =============================================
// Test 7: Observer 路径 — 多个顺序对话 → 每个都生成独立日志
// =============================================

func TestObserverFullFlow_MultipleConversations(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Response 1", "Response 2", "Response 3"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	for i := 0; i < 3; i++ {
		resp, err := loop.ProcessDirectWithChannel(ctx, "Message", "session-multi", "web", "chat-multi")
		if err != nil {
			t.Errorf("conversation %d failed: %v", i, err)
		}
		if resp == "" {
			t.Errorf("conversation %d returned empty response", i)
		}
	}

	// 应该有多个 session 目录
	dirs := findAllRequestSessionDirs(t, workspace)
	if len(dirs) < 2 {
		t.Errorf("expected at least 2 session directories, got %d", len(dirs))
	}

	// 每个 session 目录都应该有 request + response 文件
	for _, dir := range dirs {
		files := readDirFiles(t, dir)
		if !hasFileWithSuffix(files, ".request.md") {
			t.Errorf("session %s missing request.md", filepath.Base(dir))
		}
		if !hasFileWithSuffix(files, ".response.md") {
			t.Errorf("session %s missing response.md", filepath.Base(dir))
		}
	}
}

// =============================================
// Test 8: 禁用日志 → Observer 路径不创建日志文件
// =============================================

func TestObserverFullFlow_LoggingDisabled_NoFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	cfg.Logging.LLM.Enabled = false // 禁用日志

	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	_, err := loop.ProcessDirectWithChannel(ctx, "No log", "session-nolog", "web", "chat-nolog")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// 日志目录不应存在
	logDir := filepath.Join(workspace, "requests")
	if _, err := os.Stat(logDir); !os.IsNotExist(err) {
		entries, _ := os.ReadDir(logDir)
		if len(entries) > 0 {
			t.Error("log directory should be empty when logging is disabled")
		}
	}
}

// =============================================
// Test 9: 完整对话 — 验证 AI.Request.md 包含关键信息
// =============================================

func TestObserverFullFlow_AIRequestFileContainsModelInfo(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"AI response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	_, err := loop.ProcessDirectWithChannel(ctx, "What model are you?", "session-model", "web", "chat-model")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	aiReqContent := findFileWithSuffix(files, ".AI.Request.md")
	if aiReqContent == "" {
		t.Fatal("AI request file not found")
	}

	// AI.Request.md 应该包含模型名称（来自 config defaults.LLM = "mock/model"）
	if !strings.Contains(aiReqContent, "mock/model") {
		t.Error("AI request file should contain model name 'mock/model'")
	}
}

// =============================================
// Test 10: 验证 channel/chatID 信息正确写入
// =============================================

func TestObserverFullFlow_ChannelInfoInFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Channel test response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	_, err := loop.ProcessDirectWithChannel(ctx, "Hello", "session-ch", "discord", "chat-discord-123")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	// request.md 应包含 channel 和 chatID
	reqContent := findFileWithSuffix(files, ".request.md")
	if !strings.Contains(reqContent, "discord") {
		t.Error("request file should contain channel 'discord'")
	}

	// response.md 也应包含 channel 和 chatID
	respContent := findFileWithSuffix(files, ".response.md")
	if !strings.Contains(respContent, "discord") {
		t.Error("response file should contain channel 'discord'")
	}
}

// =============================================
// Test 11: 端到端对比 — Observer 和 Legacy 的文件内容质量
// =============================================

func TestObserverVsLegacy_ContentQuality(t *testing.T) {
	userMsg := "Quality test message"
	channel := "test-channel"
	chatID := "chat-quality-001"

	// Observer 路径
	obsWorkspace := t.TempDir()
	obsCfg := createConfigWithLogging(obsWorkspace)
	obsBus := bus.NewMessageBus()
	obsProvider := &mockLLMProvider{responses: []string{"Quality response"}}
	obsLoop := agent.NewAgentLoop(obsCfg, obsBus, obsProvider)
	obsMgr := observer.NewManager()
	obsMgr.Register(agent.NewRequestLoggerObserver(obsCfg.Logging, obsWorkspace))
	obsLoop.SetObserverManager(obsMgr)

	_, err := obsLoop.ProcessDirectWithChannel(context.Background(), userMsg, "q-session", channel, chatID)
	if err != nil {
		t.Fatalf("observer path failed: %v", err)
	}

	// Legacy 路径
	legWorkspace := t.TempDir()
	legCfg := createConfigWithLogging(legWorkspace)
	legBus := bus.NewMessageBus()
	legProvider := &mockLLMProvider{responses: []string{"Quality response"}}
	legLoop := agent.NewAgentLoop(legCfg, legBus, legProvider)

	_, err = legLoop.ProcessDirectWithChannel(context.Background(), userMsg, "q-session", channel, chatID)
	if err != nil {
		t.Fatalf("legacy path failed: %v", err)
	}

	// 读取两边文件
	obsFiles := readDirFiles(t, findRequestSessionDir(t, obsWorkspace))
	legFiles := readDirFiles(t, findRequestSessionDir(t, legWorkspace))

	// 比较关键内容
	obsReq := findFileWithSuffix(obsFiles, ".request.md")
	legReq := findFileWithSuffix(legFiles, ".request.md")
	if !strings.Contains(obsReq, userMsg) {
		t.Error("observer request file missing user message")
	}
	if !strings.Contains(legReq, userMsg) {
		t.Error("legacy request file missing user message")
	}

	obsResp := findFileWithSuffix(obsFiles, ".response.md")
	legResp := findFileWithSuffix(legFiles, ".response.md")
	if !strings.Contains(obsResp, "Quality response") {
		t.Error("observer response file missing response content")
	}
	if !strings.Contains(legResp, "Quality response") {
		t.Error("legacy response file missing response content")
	}

	// 两边都应该有 channel 信息
	if !strings.Contains(obsReq, channel) {
		t.Error("observer request file missing channel")
	}
	if !strings.Contains(legReq, channel) {
		t.Error("legacy request file missing channel")
	}
}

// =============================================
// Test 12: ProcessDirect（使用默认 cli 通道）→ 验证日志
// =============================================

func TestObserverFullFlow_ProcessDirect_LogsFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Direct response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	response, err := loop.ProcessDirect(ctx, "Direct test message", "session-direct")
	if err != nil {
		t.Fatalf("ProcessDirect failed: %v", err)
	}
	if response != "Direct response" {
		t.Errorf("unexpected response: %s", response)
	}

	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	// 验证核心文件
	if !hasFileWithSuffix(files, ".request.md") {
		t.Error("missing request.md")
	}
	if !hasFileWithSuffix(files, ".response.md") {
		t.Error("missing response.md")
	}
	if !hasFileWithSuffix(files, ".AI.Request.md") {
		t.Error("missing AI.Request.md")
	}
	if !hasFileWithSuffix(files, ".AI.Response.md") {
		t.Error("missing AI.Response.md")
	}
}

// =============================================
// Test 13: 两个 Observer（RequestLogger + TraceCollector）→ 验证日志不冲突
// =============================================

func TestObserverFullFlow_MultipleObservers(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Multi-observer response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	// 创建 ObserverManager，注册两个 Observer
	observerMgr := observer.NewManager()

	// Observer 1: RequestLogger
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)

	// Observer 2: 简单的计数 Observer（模拟 TraceCollector）
	countObs := &countingObserver{}
	observerMgr.Register(countObs)

	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	response, err := loop.ProcessDirectWithChannel(ctx, "Test with two observers", "session-multi-obs", "web", "chat-mobs")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if response == "" {
		t.Fatal("expected non-empty response")
	}

	// 验证 RequestLogger 仍然写日志
	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)
	if !hasFileWithSuffix(files, ".request.md") {
		t.Error("missing request.md (multiple observers)")
	}
	if !hasFileWithSuffix(files, ".response.md") {
		t.Error("missing response.md (multiple observers)")
	}

	// 验证计数 Observer 也收到了事件
	if countObs.getCount() == 0 {
		t.Error("counting observer should have received events")
	}
}

// =============================================
// Test 14: Observer 空请求 → 验证不崩溃
// =============================================

func TestObserverFullFlow_EmptyMessage(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()
	provider := &mockLLMProvider{
		responses: []string{"Empty response"},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	// 空消息不应该导致崩溃
	_, _ = loop.ProcessDirect(ctx, "", "session-empty")
}

// =============================================
// Test 15: 完整 2 轮对话（1轮工具调用 + 1轮文本响应）→ 逐一读取全部日志文件内容
// 这是最关键的测试：验证 Observer 路径下，一次完整的带工具调用的对话
// 能够正确写入所有 7 个日志文件，且每个文件内容正确。
// =============================================

// sequentialMockProvider 按顺序返回预定义的 LLMResponse
type sequentialMockProvider struct {
	responses []*providers.LLMResponse
	index     int
}

func (p *sequentialMockProvider) Chat(ctx context.Context, messages []providers.Message, tools []providers.ToolDefinition, model string, options map[string]interface{}) (*providers.LLMResponse, error) {
	if p.index < len(p.responses) {
		resp := p.responses[p.index]
		p.index++
		return resp, nil
	}
	// 超出预设时返回默认文本响应
	return &providers.LLMResponse{Content: "default", FinishReason: "stop"}, nil
}

func (p *sequentialMockProvider) GetDefaultModel() string { return "mock/model" }

func TestObserverFullFlow_TwoRoundConversation_AllSevenFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()

	// Mock LLM:
	//   第 1 次调用 → 返回 tool_call（read_file）
	//   第 2 次调用 → 返回纯文本（最终响应）
	provider := &sequentialMockProvider{
		responses: []*providers.LLMResponse{
			{
				Content:      "",
				FinishReason: "tool_calls",
				ToolCalls: []protocoltypes.ToolCall{
					{
						ID:   "call-001",
						Name: "read_file",
						Arguments: map[string]interface{}{
							"path": "test_data.txt",
						},
					},
				},
			},
			{
				Content:      "文件内容是：Hello World",
				FinishReason: "stop",
				Usage:        &providers.UsageInfo{PromptTokens: 200, CompletionTokens: 50, TotalTokens: 250},
			},
		},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)

	observerMgr := observer.NewManager()
	rlObs := agent.NewRequestLoggerObserver(cfg.Logging, workspace)
	observerMgr.Register(rlObs)
	loop.SetObserverManager(observerMgr)

	ctx := context.Background()
	response, err := loop.ProcessDirectWithChannel(ctx, "请帮我读取 test_data.txt", "session-2round", "web", "chat-2round")
	if err != nil {
		t.Fatalf("ProcessDirectWithChannel failed: %v", err)
	}
	if response == "" {
		t.Fatal("expected non-empty response")
	}

	// ======== 读取所有日志文件 ========
	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	t.Logf("Session dir: %s", sessionDir)
	t.Logf("File count: %d", len(files))
	for name := range files {
		t.Logf("  File: %s", name)
	}

	// 期望 7 个文件：
	//   01.request.md      — 用户消息
	//   02.AI.Request.md   — 第 1 轮 LLM 请求
	//   03.AI.Response.md  — 第 1 轮 LLM 响应（tool_call）
	//   04.Local.md        — 第 1 轮工具执行结果
	//   05.AI.Request.md   — 第 2 轮 LLM 请求
	//   06.AI.Response.md  — 第 2 轮 LLM 响应（最终文本）
	//   07.response.md     — 最终响应

	// ---- 1. 验证 request.md ----
	reqContent := findFileWithSuffix(files, ".request.md")
	if reqContent == "" {
		t.Fatal("request.md not found")
	}
	t.Logf("--- request.md ---\n%s", reqContent)
	if !strings.Contains(reqContent, "请帮我读取 test_data.txt") {
		t.Error("request.md should contain user message '请帮我读取 test_data.txt'")
	}
	if !strings.Contains(reqContent, "web") {
		t.Error("request.md should contain channel 'web'")
	}
	if !strings.Contains(reqContent, "chat-2round") {
		t.Error("request.md should contain chatID 'chat-2round'")
	}

	// ---- 2 & 5. 验证 AI.Request.md（应有 2 个）----
	aiReqCount := countFilesWithSuffix(files, ".AI.Request.md")
	if aiReqCount != 2 {
		t.Errorf("expected 2 AI.Request.md files (one per LLM round), got %d", aiReqCount)
	}

	// ---- 3 & 6. 验证 AI.Response.md（应有 2 个）----
	aiRespCount := countFilesWithSuffix(files, ".AI.Response.md")
	if aiRespCount != 2 {
		t.Errorf("expected 2 AI.Response.md files (one per LLM round), got %d", aiRespCount)
	}

	// ---- 4. 验证 Local.md（工具执行日志）----
	localContent := findFileWithSuffix(files, ".Local.md")
	if localContent == "" {
		t.Fatal("Local.md not found — tool call was not logged!")
	}
	t.Logf("--- Local.md ---\n%s", localContent)
	if !strings.Contains(localContent, "read_file") {
		t.Error("Local.md should contain tool name 'read_file'")
	}

	// ---- 7. 验证 response.md ----
	respContent := findFileWithSuffix(files, ".response.md")
	if respContent == "" {
		t.Fatal("response.md not found")
	}
	t.Logf("--- response.md ---\n%s", respContent)
	if !strings.Contains(respContent, "文件内容是：Hello World") {
		t.Error("response.md should contain final response '文件内容是：Hello World'")
	}
	if !strings.Contains(respContent, "web") {
		t.Error("response.md should contain channel 'web'")
	}

	// ---- 最终统计 ----
	if len(files) != 7 {
		t.Errorf("expected exactly 7 files for a 2-round conversation, got %d: %v",
			len(files), fileKeys(files))
	}
}

// =============================================
// Test 16: 同场景 Legacy 路径 → 同样验证 7 个文件
// 用于与 Test 15 对比
// =============================================

func TestLegacyFullFlow_TwoRoundConversation_AllSevenFiles(t *testing.T) {
	workspace := t.TempDir()
	cfg := createConfigWithLogging(workspace)
	msgBus := bus.NewMessageBus()

	provider := &sequentialMockProvider{
		responses: []*providers.LLMResponse{
			{
				Content:      "",
				FinishReason: "tool_calls",
				ToolCalls: []protocoltypes.ToolCall{
					{
						ID:        "call-001",
						Name:      "read_file",
						Arguments: map[string]interface{}{"path": "test_data.txt"},
					},
				},
			},
			{
				Content:      "文件内容是：Hello World",
				FinishReason: "stop",
				Usage:        &providers.UsageInfo{PromptTokens: 200, CompletionTokens: 50, TotalTokens: 250},
			},
		},
	}

	loop := agent.NewAgentLoop(cfg, msgBus, provider)
	// 不注入 Observer → 走 legacy 路径

	ctx := context.Background()
	response, err := loop.ProcessDirectWithChannel(ctx, "请帮我读取 test_data.txt", "session-2round-leg", "web", "chat-2round-leg")
	if err != nil {
		t.Fatalf("ProcessDirectWithChannel failed: %v", err)
	}
	if response == "" {
		t.Fatal("expected non-empty response")
	}

	sessionDir := findRequestSessionDir(t, workspace)
	files := readDirFiles(t, sessionDir)

	t.Logf("Legacy session dir: %s", sessionDir)
	t.Logf("Legacy file count: %d", len(files))

	// 同样应该有 7 个文件
	if !hasFileWithSuffix(files, ".request.md") {
		t.Error("legacy: missing request.md")
	}
	if !hasFileWithSuffix(files, ".response.md") {
		t.Error("legacy: missing response.md")
	}
	if !hasFileWithSuffix(files, ".Local.md") {
		t.Error("legacy: missing Local.md (tool call log)")
	}

	aiReqCount := countFilesWithSuffix(files, ".AI.Request.md")
	aiRespCount := countFilesWithSuffix(files, ".AI.Response.md")
	if aiReqCount != 2 {
		t.Errorf("legacy: expected 2 AI.Request.md, got %d", aiReqCount)
	}
	if aiRespCount != 2 {
		t.Errorf("legacy: expected 2 AI.Response.md, got %d", aiRespCount)
	}

	if len(files) != 7 {
		t.Errorf("legacy: expected exactly 7 files, got %d: %v",
			len(files), fileKeys(files))
	}
}

// =============================================
// Test 17: Observer vs Legacy — 2 轮对话文件数量和类型完全一致
// =============================================

func TestObserverVsLegacy_TwoRoundConversation_IdenticalOutput(t *testing.T) {
	userMsg := "请帮我读取 test_data.txt"

	// Observer 路径
	obsWorkspace := t.TempDir()
	obsCfg := createConfigWithLogging(obsWorkspace)
	obsBus := bus.NewMessageBus()
	obsProvider := &sequentialMockProvider{
		responses: []*providers.LLMResponse{
			{Content: "", FinishReason: "tool_calls", ToolCalls: []protocoltypes.ToolCall{
				{ID: "call-001", Name: "read_file", Arguments: map[string]interface{}{"path": "test.txt"}},
			}},
			{Content: "Done", FinishReason: "stop"},
		},
	}
	obsLoop := agent.NewAgentLoop(obsCfg, obsBus, obsProvider)
	obsMgr := observer.NewManager()
	obsMgr.Register(agent.NewRequestLoggerObserver(obsCfg.Logging, obsWorkspace))
	obsLoop.SetObserverManager(obsMgr)

	_, err := obsLoop.ProcessDirectWithChannel(context.Background(), userMsg, "session-cmp", "web", "chat-cmp")
	if err != nil {
		t.Fatalf("observer path failed: %v", err)
	}

	// Legacy 路径
	legWorkspace := t.TempDir()
	legCfg := createConfigWithLogging(legWorkspace)
	legBus := bus.NewMessageBus()
	legProvider := &sequentialMockProvider{
		responses: []*providers.LLMResponse{
			{Content: "", FinishReason: "tool_calls", ToolCalls: []protocoltypes.ToolCall{
				{ID: "call-001", Name: "read_file", Arguments: map[string]interface{}{"path": "test.txt"}},
			}},
			{Content: "Done", FinishReason: "stop"},
		},
	}
	legLoop := agent.NewAgentLoop(legCfg, legBus, legProvider)

	_, err = legLoop.ProcessDirectWithChannel(context.Background(), userMsg, "session-cmp", "web", "chat-cmp")
	if err != nil {
		t.Fatalf("legacy path failed: %v", err)
	}

	// 比较两边文件
	obsFiles := readDirFiles(t, findRequestSessionDir(t, obsWorkspace))
	legFiles := readDirFiles(t, findRequestSessionDir(t, legWorkspace))

	if len(obsFiles) != len(legFiles) {
		t.Errorf("file count mismatch: observer=%d (%v), legacy=%d (%v)",
			len(obsFiles), fileKeys(obsFiles), len(legFiles), fileKeys(legFiles))
	}

	// 逐类型比较
	for _, suffix := range []string{".request.md", ".AI.Request.md", ".AI.Response.md", ".Local.md", ".response.md"} {
		obsCount := countFilesWithSuffix(obsFiles, suffix)
		legCount := countFilesWithSuffix(legFiles, suffix)
		if obsCount != legCount {
			t.Errorf("file type %s: observer=%d, legacy=%d", suffix, obsCount, legCount)
		}
	}
}

// countingObserver 是一个简单的计数 Observer，用于测试多 Observer 场景
type countingObserver struct {
	count int
}

func (c *countingObserver) Name() string { return "counting" }

func (c *countingObserver) OnEvent(ctx context.Context, event observer.ConversationEvent) {
	c.count++
}

func (c *countingObserver) getCount() int {
	return c.count
}

// =============================================
// 辅助函数
// =============================================

func fileKeys(files map[string]string) []string {
	keys := make([]string, 0, len(files))
	for k := range files {
		keys = append(keys, k)
	}
	return keys
}

func collectFileSuffixes(files map[string]string) map[string]bool {
	suffixes := make(map[string]bool)
	for name := range files {
		parts := strings.SplitN(name, ".", 2)
		if len(parts) > 1 {
			suffixes["."+parts[1]] = true
		}
	}
	return suffixes
}

// Ensure imports are used
var _ = time.Second
