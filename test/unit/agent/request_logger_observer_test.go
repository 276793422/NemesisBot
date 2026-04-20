package agent_test

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/agent"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/observer"
	"github.com/276793422/NemesisBot/module/providers"
)

// =============================================
// 辅助函数
// =============================================

// enabledLogCfg 返回一个启用了 LLM 日志的配置
func enabledLogCfg() *config.LoggingConfig {
	return &config.LoggingConfig{
		LLM: &config.LLMLogConfig{
			Enabled:     true,
			LogDir:      "requests",
			DetailLevel: "full",
		},
	}
}

// disabledLogCfg 返回一个禁用 LLM 日志的配置
func disabledLogCfg() *config.LoggingConfig {
	return &config.LoggingConfig{
		LLM: &config.LLMLogConfig{Enabled: false},
	}
}

// readSessionFiles 读取会话目录下的所有文件名和内容
func readSessionFiles(t *testing.T, dir string) map[string]string {
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
		content, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			t.Fatalf("failed to read %s: %v", e.Name(), err)
		}
		files[e.Name()] = string(content)
	}
	return files
}

// findSessionDir 在 requests/ 目录下找到唯一的 session 子目录
func findSessionDir(t *testing.T, workspace string) string {
	t.Helper()
	logDir := filepath.Join(workspace, "requests")
	entries, err := os.ReadDir(logDir)
	if err != nil {
		t.Fatalf("failed to read log dir: %v", err)
	}
	for _, e := range entries {
		if e.IsDir() {
			return filepath.Join(logDir, e.Name())
		}
	}
	t.Fatal("no session directory found")
	return ""
}

// findAllSessionDirs 在 requests/ 目录下找到所有 session 子目录
func findAllSessionDirs(t *testing.T, workspace string) []string {
	t.Helper()
	logDir := filepath.Join(workspace, "requests")
	entries, err := os.ReadDir(logDir)
	if err != nil {
		t.Fatalf("failed to read log dir: %v", err)
	}
	var dirs []string
	for _, e := range entries {
		if e.IsDir() {
			dirs = append(dirs, filepath.Join(logDir, e.Name()))
		}
	}
	return dirs
}

// simulateFullConversation 模拟一次完整对话的所有事件
func simulateFullConversation(ctx context.Context, obs *agent.RequestLoggerObserver, traceID string, rounds int) {
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   traceID,
		Timestamp: time.Now(),
		Data: &observer.ConversationStartData{
			SessionKey: "test-session",
			Channel:    "web",
			ChatID:     "chat-123",
			SenderID:   "user",
			Content:    "Please help me with something",
		},
	})

	for i := 1; i <= rounds; i++ {
		obs.OnEvent(ctx, observer.ConversationEvent{
			Type:      observer.EventLLMRequest,
			TraceID:   traceID,
			Timestamp: time.Now(),
			Data: &observer.LLMRequestData{
				Round:        i,
				Model:        "gpt-4",
				ProviderName: "openai",
				APIKey:       "sk-test123",
				APIBase:      "https://api.openai.com/v1",
				HTTPHeaders:  map[string]string{"Content-Type": "application/json"},
				FullConfig:   map[string]interface{}{"max_tokens": 8192},
				Messages:     []providers.Message{{Role: "user", Content: "test"}},
			},
		})

		if i < rounds {
			// 中间轮：返回 tool calls
			obs.OnEvent(ctx, observer.ConversationEvent{
				Type:      observer.EventLLMResponse,
				TraceID:   traceID,
				Timestamp: time.Now(),
				Data: &observer.LLMResponseData{
					Round:        i,
					Duration:     500 * time.Millisecond,
					Content:      "",
					ToolCalls:    []providers.ToolCall{{ID: "tc-1", Name: "read_file", Arguments: map[string]interface{}{"path": "/tmp/test.txt"}}},
					Usage:        &providers.UsageInfo{PromptTokens: 100, CompletionTokens: 50, TotalTokens: 150},
					FinishReason: "tool_calls",
				},
			})
			obs.OnEvent(ctx, observer.ConversationEvent{
				Type:      observer.EventToolCall,
				TraceID:   traceID,
				Timestamp: time.Now(),
				Data: &observer.ToolCallData{
					ToolName:  "read_file",
					Arguments: map[string]interface{}{"path": "/tmp/test.txt"},
					Success:   true,
					Duration:  50 * time.Millisecond,
					LLMRound:  i,
					ChainPos:  0,
				},
			})
		} else {
			// 最后一轮：返回纯文本响应
			obs.OnEvent(ctx, observer.ConversationEvent{
				Type:      observer.EventLLMResponse,
				TraceID:   traceID,
				Timestamp: time.Now(),
				Data: &observer.LLMResponseData{
					Round:        i,
					Duration:     300 * time.Millisecond,
					Content:      "Here is your answer",
					Usage:        &providers.UsageInfo{PromptTokens: 200, CompletionTokens: 100, TotalTokens: 300},
					FinishReason: "stop",
				},
			})
		}
	}

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationEnd,
		TraceID:   traceID,
		Timestamp: time.Now(),
		Data: &observer.ConversationEndData{
			SessionKey:    "test-session",
			Channel:       "web",
			ChatID:        "chat-123",
			TotalRounds:   rounds,
			TotalDuration: 2 * time.Second,
			Content:       "Here is your answer",
		},
	})
}

// =============================================
// 基础测试
// =============================================

func TestRequestLoggerObserver_Name(t *testing.T) {
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), t.TempDir())
	if obs.Name() != "request_logger" {
		t.Fatalf("expected request_logger, got %s", obs.Name())
	}
}

func TestRequestLoggerObserver_Disabled(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(disabledLogCfg(), dir)

	obs.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-1",
		Data:    &observer.ConversationStartData{Content: "test"},
	})

	logDir := filepath.Join(dir, "requests")
	if _, err := os.Stat(logDir); !os.IsNotExist(err) {
		t.Error("log directory should not exist when logging is disabled")
	}
}

func TestRequestLoggerObserver_NilConfig(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(nil, dir)

	// Should not panic with nil config
	obs.OnEvent(context.Background(), observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-nil",
		Data:    &observer.ConversationStartData{Content: "test"},
	})

	logDir := filepath.Join(dir, "requests")
	if _, err := os.Stat(logDir); !os.IsNotExist(err) {
		t.Error("log directory should not exist with nil config")
	}
}

// =============================================
// 完整生命周期测试 — 对比原 RequestLogger 输出
// =============================================

func TestRequestLoggerObserver_FullConversationLifecycle(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	simulateFullConversation(ctx, obs, "trace-full", 2)

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)

	// 必须有以下文件：
	// 01.request.md     — 用户消息
	// 02.AI.Request.md  — 第 1 轮 LLM 请求
	// 03.AI.Response.md — 第 1 轮 LLM 响应
	// 04.Local.md       — 第 1 轮工具调用
	// 05.AI.Request.md  — 第 2 轮 LLM 请求
	// 06.AI.Response.md — 第 2 轮 LLM 响应
	// 07.response.md    — 最终响应
	expectedCount := 7
	if len(files) != expectedCount {
		t.Errorf("expected %d files, got %d: %v", expectedCount, len(files), fileNames(files))
	}

	// 验证文件类型都存在
	assertHasFile(t, files, ".request.md", "user request file")
	assertHasFile(t, files, ".response.md", "final response file")
	assertHasFile(t, files, ".AI.Request.md", "LLM request files")
	assertHasFile(t, files, ".AI.Response.md", "LLM response files")
	assertHasFile(t, files, ".Local.md", "local operations file")
}

// =============================================
// 逐一验证每个事件对应的文件内容
// =============================================

func TestRequestLoggerObserver_RequestFileContent(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventConversationStart,
		TraceID:   "trace-content",
		Timestamp: time.Date(2026, 4, 21, 10, 0, 0, 0, time.UTC),
		Data: &observer.ConversationStartData{
			SessionKey: "sk-123",
			Channel:    "web",
			ChatID:     "chat-456",
			SenderID:   "user-789",
			Content:    "Hello, this is a test message!",
		},
	})
	// 必须发送 end 事件来关闭（否则 session 可能未 flush）
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-content",
		Data:    &observer.ConversationEndData{TotalRounds: 0, TotalDuration: time.Second},
	})

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)

	// 找到 request 文件
	reqContent := findFileContent(t, files, ".request.md")
	if reqContent == "" {
		t.Fatal("request file not found or empty")
	}

	// 验证关键内容
	assertContains(t, reqContent, "Hello, this is a test message!", "user message content")
	assertContains(t, reqContent, "web", "channel")
	assertContains(t, reqContent, "chat-456", "chat ID")
	assertContains(t, reqContent, "user-789", "sender ID")
}

func TestRequestLoggerObserver_LLMRequestFileContent(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-llmreq",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventLLMRequest,
		TraceID:   "trace-llmreq",
		Timestamp: time.Date(2026, 4, 21, 10, 1, 0, 0, time.UTC),
		Data: &observer.LLMRequestData{
			Round:        1,
			Model:        "gpt-4",
			ProviderName: "openai",
			APIKey:       "sk-test123",
			APIBase:      "https://api.openai.com/v1",
			HTTPHeaders:  map[string]string{"Content-Type": "application/json"},
			FullConfig:   map[string]interface{}{"max_tokens": 8192},
			Messages:     []providers.Message{{Role: "user", Content: "hello"}},
			Tools:        []providers.ToolDefinition{{Type: "function", Function: providers.ToolFunctionDefinition{Name: "read_file"}}},
		},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-llmreq",
		Data:    &observer.ConversationEndData{TotalRounds: 1, TotalDuration: time.Second},
	})

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)

	reqContent := findFileContent(t, files, ".AI.Request.md")
	if reqContent == "" {
		t.Fatal("AI request file not found or empty")
	}

	assertContains(t, reqContent, "gpt-4", "model name")
	assertContains(t, reqContent, "openai", "provider name")
	assertContains(t, reqContent, "application/json", "HTTP header")
	assertContains(t, reqContent, "max_tokens", "config key")
}

func TestRequestLoggerObserver_LLMResponseFileContent(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-llmresp",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:      observer.EventLLMResponse,
		TraceID:   "trace-llmresp",
		Timestamp: time.Now(),
		Data: &observer.LLMResponseData{
			Round:        1,
			Duration:     500 * time.Millisecond,
			Content:      "This is the LLM response",
			Usage:        &providers.UsageInfo{PromptTokens: 100, CompletionTokens: 50, TotalTokens: 150},
			FinishReason: "stop",
		},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-llmresp",
		Data:    &observer.ConversationEndData{TotalRounds: 1, TotalDuration: time.Second},
	})

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)

	respContent := findFileContent(t, files, ".AI.Response.md")
	if respContent == "" {
		t.Fatal("AI response file not found or empty")
	}

	assertContains(t, respContent, "This is the LLM response", "response content")
	assertContains(t, respContent, "100", "prompt tokens")
	assertContains(t, respContent, "150", "total tokens")
	assertContains(t, respContent, "stop", "finish reason")
}

func TestRequestLoggerObserver_ResponseFileContent(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-resp",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-resp",
		Timestamp: time.Date(2026, 4, 21, 10, 5, 0, 0, time.UTC),
		Data: &observer.ConversationEndData{
			TotalRounds:   3,
			TotalDuration: 5 * time.Second,
			Content:       "Final response content here",
			Channel:       "web",
			ChatID:        "chat-final",
		},
	})

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)

	respContent := findFileContent(t, files, ".response.md")
	if respContent == "" {
		t.Fatal("response file not found or empty")
	}

	assertContains(t, respContent, "Final response content here", "final response content")
	assertContains(t, respContent, "web", "channel")
	assertContains(t, respContent, "chat-final", "chat ID")
	assertContains(t, respContent, "3", "LLM rounds")
}

// =============================================
// 工具调用测试
// =============================================

func TestRequestLoggerObserver_ToolCallSuccess(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-tool-ok",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-tool-ok",
		Data: &observer.ToolCallData{
			ToolName:  "read_file",
			Arguments: map[string]interface{}{"path": "/tmp/test.txt"},
			Success:   true,
			Duration:  50 * time.Millisecond,
			LLMRound:  1,
			ChainPos:  0,
		},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-tool-ok",
		Data: &observer.ConversationEndData{
			TotalRounds:   1,
			TotalDuration: time.Second,
			Content:       "done",
			Channel:       "web",
		},
	})

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)

	localContent := findFileContent(t, files, ".Local.md")
	if localContent == "" {
		t.Fatal("Local operations file not found or empty")
	}

	assertContains(t, localContent, "read_file", "tool name")
	assertContains(t, localContent, "Success", "status")
	assertContains(t, localContent, "/tmp/test.txt", "argument value")
}

func TestRequestLoggerObserver_ToolCallFailure(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-tool-fail",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-tool-fail",
		Data: &observer.ToolCallData{
			ToolName:  "write_file",
			Arguments: map[string]interface{}{"path": "/root/secret"},
			Success:   false,
			Duration:  10 * time.Millisecond,
			Error:     "permission denied: access rejected",
			LLMRound:  1,
			ChainPos:  0,
		},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-tool-fail",
		Data: &observer.ConversationEndData{
			TotalRounds:   1,
			TotalDuration: time.Second,
			Content:       "error occurred",
			Channel:       "web",
		},
	})

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)

	localContent := findFileContent(t, files, ".Local.md")
	if localContent == "" {
		t.Fatal("Local operations file not found")
	}

	assertContains(t, localContent, "write_file", "tool name")
	assertContains(t, localContent, "Failed", "failure status")
	assertContains(t, localContent, "permission denied", "error message")
}

func TestRequestLoggerObserver_MultipleToolsMultipleRounds(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-multi",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})

	// Round 1: two tools
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-multi",
		Data: &observer.ToolCallData{
			ToolName: "read_file", Success: true, Duration: 50 * time.Millisecond,
			LLMRound: 1, ChainPos: 0,
			Arguments: map[string]interface{}{"path": "a.txt"},
		},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-multi",
		Data: &observer.ToolCallData{
			ToolName: "write_file", Success: true, Duration: 100 * time.Millisecond,
			LLMRound: 1, ChainPos: 1,
			Arguments: map[string]interface{}{"path": "b.txt"},
		},
	})

	// Round 2: one tool
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-multi",
		Data: &observer.ToolCallData{
			ToolName: "exec", Success: true, Duration: 200 * time.Millisecond,
			LLMRound: 2, ChainPos: 0,
			Arguments: map[string]interface{}{"cmd": "ls"},
		},
	})

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-multi",
		Data: &observer.ConversationEndData{
			TotalRounds:   2,
			TotalDuration: 3 * time.Second,
			Content:       "done",
			Channel:       "web",
		},
	})

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)

	// 应该有两个 Local.md 文件（每轮一个）
	localCount := 0
	for name, content := range files {
		if strings.HasSuffix(name, ".Local.md") {
			localCount++
			if len(content) == 0 {
				t.Errorf("local file %s is empty", name)
			}
		}
	}
	if localCount != 2 {
		t.Errorf("expected 2 Local.md files (one per round), got %d", localCount)
	}
}

// =============================================
// 并发 / 多会话测试
// =============================================

func TestRequestLoggerObserver_MultipleConcurrentConversations(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	// 启动 10 个并发对话
	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			traceID := fmt.Sprintf("trace-concurrent-%d", idx)
			simulateFullConversation(ctx, obs, traceID, 2)
		}(i)
	}
	wg.Wait()

	// 应该有 1-10 个 session 目录（可能因纳秒碰撞合并）
	dirs := findAllSessionDirs(t, dir)
	if len(dirs) == 0 {
		t.Fatal("expected at least 1 session directory")
	}

	// 验证所有 session 目录下都有文件（不是空的）
	totalFiles := 0
	for _, d := range dirs {
		files := readSessionFiles(t, d)
		totalFiles += len(files)
		if len(files) == 0 {
			t.Errorf("session dir %s has no files", d)
		}
	}

	// 至少应该有 request + response 文件
	if totalFiles < 2 {
		t.Errorf("expected at least 2 files total, got %d", totalFiles)
	}

	// 每个 session 目录都应有 request 文件
	for _, d := range dirs {
		files := readSessionFiles(t, d)
		hasReq := false
		hasResp := false
		for name := range files {
			if strings.HasSuffix(name, ".request.md") {
				hasReq = true
			}
			if strings.HasSuffix(name, ".response.md") {
				hasResp = true
			}
		}
		if !hasReq {
			t.Errorf("session %s missing request file", filepath.Base(d))
		}
		if !hasResp {
			t.Errorf("session %s missing response file", filepath.Base(d))
		}
	}
}

func TestRequestLoggerObserver_MultipleSequentialConversations(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	// 5 个顺序对话
	for i := 0; i < 5; i++ {
		traceID := fmt.Sprintf("trace-seq-%d", i)
		obs.OnEvent(ctx, observer.ConversationEvent{
			Type:    observer.EventConversationStart,
			TraceID: traceID,
			Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
		})
		obs.OnEvent(ctx, observer.ConversationEvent{
			Type:    observer.EventConversationEnd,
			TraceID: traceID,
			Data:    &observer.ConversationEndData{
				TotalRounds: 1, TotalDuration: time.Second, Content: "ok", Channel: "web",
			},
		})
	}

	dirs := findAllSessionDirs(t, dir)
	if len(dirs) < 4 {
		t.Errorf("expected at least 4 session directories, got %d", len(dirs))
	}
}

// =============================================
// 边界 / 异常场景测试
// =============================================

func TestRequestLoggerObserver_EndWithoutStart(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	// 直接发送 end 事件（没有对应的 start）— 不应 panic
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-no-start",
		Data:    &observer.ConversationEndData{TotalRounds: 1, TotalDuration: time.Second},
	})

	// 不应创建任何文件
	logDir := filepath.Join(dir, "requests")
	if _, err := os.Stat(logDir); !os.IsNotExist(err) {
		entries, _ := os.ReadDir(logDir)
		if len(entries) > 0 {
			t.Error("should not create session dir for end-without-start")
		}
	}
}

func TestRequestLoggerObserver_ToolCallWithoutStart(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	// 直接发送 tool_call 事件（没有对应的 start）— 不应 panic
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-no-start-tool",
		Data: &observer.ToolCallData{
			ToolName: "read_file", Success: true, LLMRound: 1,
		},
	})
	// 不应 panic
}

func TestRequestLoggerObserver_LLMRequestWithoutStart(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	// 直接发送 llm_request 事件 — 不应 panic
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventLLMRequest,
		TraceID: "trace-no-start-llm",
		Data:    &observer.LLMRequestData{Round: 1, Model: "test"},
	})
}

func TestRequestLoggerObserver_LLMResponseWithoutStart(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventLLMResponse,
		TraceID: "trace-no-start-resp",
		Data:    &observer.LLMResponseData{Round: 1, Content: "test"},
	})
}

func TestRequestLoggerObserver_DoubleEnd(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-double-end",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-double-end",
		Data:    &observer.ConversationEndData{TotalRounds: 1, TotalDuration: time.Second},
	})
	// 第二次 end — 不应 panic
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-double-end",
		Data:    &observer.ConversationEndData{TotalRounds: 1, TotalDuration: time.Second},
	})
}

func TestRequestLoggerObserver_NilData(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	// 发送 nil data — 不应 panic
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-nil",
		Data:    nil,
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-nil",
		Data:    nil,
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventToolCall,
		TraceID: "trace-nil",
		Data:    nil,
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventLLMRequest,
		TraceID: "trace-nil",
		Data:    nil,
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventLLMResponse,
		TraceID: "trace-nil",
		Data:    nil,
	})
}

func TestRequestLoggerObserver_WrongDataType(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	// 发送错误类型的 data — 不应 panic
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-wrong",
		Data:    &observer.ToolCallData{ToolName: "wrong"}, // 错误类型
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-wrong",
		Data:    &observer.ToolCallData{ToolName: "wrong"}, // 错误类型
	})
}

func TestRequestLoggerObserver_EmptyTraceID(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	// 空 traceID — 不应 panic
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "",
		Data:    &observer.ConversationEndData{TotalRounds: 1, TotalDuration: time.Second},
	})
}

func TestRequestLoggerObserver_ZeroTotalDuration(t *testing.T) {
	dir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), dir)
	ctx := context.Background()

	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationStart,
		TraceID: "trace-zero-dur",
		Data:    &observer.ConversationStartData{Channel: "web", Content: "test"},
	})
	// TotalDuration 为 0，应回退到 time.Since(logger.startTime)
	obs.OnEvent(ctx, observer.ConversationEvent{
		Type:    observer.EventConversationEnd,
		TraceID: "trace-zero-dur",
		Data: &observer.ConversationEndData{
			TotalRounds:   1,
			TotalDuration: 0, // 零值
			Content:       "done",
			Channel:       "web",
		},
	})

	sessionDir := findSessionDir(t, dir)
	files := readSessionFiles(t, sessionDir)
	respContent := findFileContent(t, files, ".response.md")
	if respContent == "" {
		t.Fatal("response file not found")
	}
	// 应该有 Total Duration（回退计算）
	assertContains(t, respContent, "Total Duration", "total duration field")
}

// =============================================
// 对比测试：Observer 路径 vs Legacy 路径输出一致
// =============================================

func TestRequestLoggerObserver_OutputMatchesLegacy(t *testing.T) {
	// 用 Observer 路径
	obsDir := t.TempDir()
	obs := agent.NewRequestLoggerObserver(enabledLogCfg(), obsDir)
	ctx := context.Background()
	simulateFullConversation(ctx, obs, "trace-compare", 2)

	// 用 Legacy 直接调用
	legacyDir := t.TempDir()
	cfg := enabledLogCfg()
	rl := agent.NewRequestLogger(cfg, legacyDir)
	rl.CreateSession()
	rl.LogUserRequest(agent.UserRequestInfo{
		Timestamp: time.Now(), Channel: "web", SenderID: "user",
		ChatID: "chat-123", Content: "Please help me with something",
	})
	rl.LogLLMRequest(agent.LLMRequestInfo{
		Round: 1, Timestamp: time.Now(), Model: "gpt-4",
		ProviderName: "openai", APIKey: "sk-test123",
		APIBase: "https://api.openai.com/v1",
		HTTPHeaders:  map[string]string{"Content-Type": "application/json"},
		FullConfig:   map[string]interface{}{"max_tokens": 8192},
		Messages:     []providers.Message{{Role: "user", Content: "test"}},
	})
	rl.LogLLMResponse(agent.LLMResponseInfo{
		Round: 1, Timestamp: time.Now(), Duration: 500 * time.Millisecond,
		Content: "", Usage: &providers.UsageInfo{PromptTokens: 100, CompletionTokens: 50, TotalTokens: 150},
		FinishReason: "tool_calls",
	})
	rl.LogLocalOperations(agent.LocalOperationInfo{
		Round: 1, Timestamp: time.Now(),
		Operations: []agent.Operation{{
			Type: "tool_call", Name: "read_file", Status: "Success",
			Arguments: map[string]interface{}{"path": "/tmp/test.txt"}, Duration: 50 * time.Millisecond,
		}},
	})
	rl.LogLLMRequest(agent.LLMRequestInfo{
		Round: 2, Timestamp: time.Now(), Model: "gpt-4",
		ProviderName: "openai",
	})
	rl.LogLLMResponse(agent.LLMResponseInfo{
		Round: 2, Timestamp: time.Now(), Duration: 300 * time.Millisecond,
		Content: "Here is your answer", Usage: &providers.UsageInfo{TotalTokens: 300},
		FinishReason: "stop",
	})
	rl.LogFinalResponse(agent.FinalResponseInfo{
		Timestamp: time.Now(), TotalDuration: 2 * time.Second,
		LLMRounds: 2, Content: "Here is your answer", Channel: "web",
	})
	rl.Close()

	// 比较：两种路径产生的文件数量应该一致
	obsSessionDir := findSessionDir(t, obsDir)
	obsFiles := readSessionFiles(t, obsSessionDir)

	legacySessionDir := findSessionDir(t, legacyDir)
	legacyFiles := readSessionFiles(t, legacySessionDir)

	if len(obsFiles) != len(legacyFiles) {
		t.Errorf("file count mismatch: observer=%d, legacy=%d\nobserver files: %v\nlegacy files: %v",
			len(obsFiles), len(legacyFiles), fileNames(obsFiles), fileNames(legacyFiles))
	}

	// 验证文件后缀名集合一致
	obsSuffixes := collectSuffixes(obsFiles)
	legacySuffixes := collectSuffixes(legacyFiles)
	for suffix := range legacySuffixes {
		if !obsSuffixes[suffix] {
			t.Errorf("observer path missing file with suffix %s", suffix)
		}
	}
}

// =============================================
// 辅助：断言和工具函数
// =============================================

func assertContains(t *testing.T, content, expected, context string) {
	t.Helper()
	if !strings.Contains(content, expected) {
		t.Errorf("expected content to contain %q (%s), but it didn't.\nContent preview: %s",
			expected, context, truncateString(content, 200))
	}
}

func assertHasFile(t *testing.T, files map[string]string, suffix, desc string) {
	t.Helper()
	for name := range files {
		if strings.HasSuffix(name, suffix) {
			return
		}
	}
	t.Errorf("no %s found (suffix %s). Files: %v", desc, suffix, fileNames(files))
}

func findFileContent(t *testing.T, files map[string]string, suffix string) string {
	t.Helper()
	for name, content := range files {
		if strings.HasSuffix(name, suffix) {
			return content
		}
	}
	return ""
}

func fileNames(files map[string]string) []string {
	names := make([]string, 0, len(files))
	for n := range files {
		names = append(names, n)
	}
	return names
}

func collectSuffixes(files map[string]string) map[string]bool {
	suffixes := make(map[string]bool)
	for name := range files {
		// 找到第二个 . 的位置
		parts := strings.SplitN(name, ".", 2)
		if len(parts) > 1 {
			suffixes["."+parts[1]] = true
		}
	}
	return suffixes
}

func truncateString(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
