package desktop

import (
	"context"
	"embed"
	"fmt"
	"log"
	"sync"
	"time"

	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	"github.com/wailsapp/wails/v2/pkg/runtime"
)

//go:embed all:frontend/dist
var assets embed.FS

// ==================== Service Manager Integration ====================

var globalServiceManager interface{}

// SetServiceManager 设置全局 ServiceManager
func SetServiceManager(svcMgr interface{}) {
	globalServiceManager = svcMgr
}

// GetServiceManager 获取全局 ServiceManager
func GetServiceManager() interface{} {
	return globalServiceManager
}

// ==================== Config ====================

// Config Desktop UI 配置
type Config struct {
	Enabled bool
	Debug   bool
}

// ==================== Global App Instance ====================

var globalApp *App

// ==================== App ====================

// App 应用结构
type App struct {
	ctx               context.Context
	approvalManager   *ApprovalManager
	botRunning        bool
	approvalHistory   *ApprovalHistory
	pendingApproval   *ApprovalRequest
	approvalStartTime time.Time
}

// ApprovalRequest 审批请求
type ApprovalRequest struct {
	RequestID      string            `json:"request_id"`
	Operation      string            `json:"operation"`
	OperationName  string            `json:"operation_name"`
	Target         string            `json:"target"`
	RiskLevel      string            `json:"risk_level"`
	Reason         string            `json:"reason"`
	TimeoutSeconds int               `json:"timeout_seconds"`
	Context        map[string]string `json:"context"`
	Timestamp      int64             `json:"timestamp"`
}

// ApprovalResponse 审批响应
type ApprovalResponse struct {
	RequestID       string  `json:"request_id"`
	Approved        bool    `json:"approved"`
	TimedOut        bool    `json:"timed_out"`
	DurationSeconds float64 `json:"duration_seconds"`
	ResponseTime    int64   `json:"response_time"`
}

// ApprovalHistoryItem 审批历史项
type ApprovalHistoryItem struct {
	RequestID       string    `json:"request_id"`
	Operation      string    `json:"operation"`
	OperationName  string    `json:"operation_name"`
	Target         string    `json:"target"`
	RiskLevel      string    `json:"risk_level"`
	Approved       bool      `json:"approved"`
	TimedOut       bool      `json:"timed_out"`
	DurationSeconds float64  `json:"duration_seconds"`
	ResponseTime   int64     `json:"response_time"`
	Timestamp      int64     `json:"timestamp"`
}

// ApprovalStats 审批统计
type ApprovalStats struct {
	TotalRequests int     `json:"total_requests"`
	Approved       int     `json:"approved"`
	Denied         int     `json:"denied"`
	Timeout        int     `json:"timeout"`
	AvgDuration    float64 `json:"avg_duration"`
}

// NewApp 创建应用实例
func NewApp() *App {
	return &App{
		approvalManager: NewApprovalManager(),
		botRunning:     false,
		approvalHistory: NewApprovalHistory(),
	}
}

// Startup 应用启动时调用
func (a *App) Startup(ctx context.Context) {
	a.ctx = ctx
	log.Println("[App] NemesisBot UI starting...")
}

// Shutdown 应用关闭时调用
func (a *App) Shutdown(ctx context.Context) {
	log.Println("[App] NemesisBot UI shutting down...")
}

// ==================== Approval History ====================

// ApprovalHistory 审批历史
type ApprovalHistory struct {
	mu      sync.RWMutex
	history []ApprovalHistoryItem
	maxSize int
}

// NewApprovalHistory 创建审批历史
func NewApprovalHistory() *ApprovalHistory {
	return &ApprovalHistory{
		history: make([]ApprovalHistoryItem, 0, 100),
		maxSize: 100,
	}
}

// AddRecord 添加记录
func (h *ApprovalHistory) AddRecord(item ApprovalHistoryItem) {
	h.mu.Lock()
	defer h.mu.Unlock()

	item.Timestamp = time.Now().Unix()

	// 添加到历史
	h.history = append(h.history, item)

	// 限制大小
	if len(h.history) > h.maxSize {
		h.history = h.history[1:]
	}
}

// GetHistory 获取历史记录
func (h *ApprovalHistory) GetHistory(limit int) []ApprovalHistoryItem {
	h.mu.RLock()
	defer h.mu.RUnlock()

	if limit <= 0 || limit > len(h.history) {
		limit = len(h.history)
	}

	start := len(h.history) - limit
	if start < 0 {
		start = 0
	}

	result := make([]ApprovalHistoryItem, limit)
	copy(result, h.history[start:])
	return result
}

// GetStats 获取统计信息
func (h *ApprovalHistory) GetStats() ApprovalStats {
	h.mu.RLock()
	defer h.mu.RUnlock()

	stats := ApprovalStats{
		TotalRequests: len(h.history),
		Approved:       0,
		Denied:         0,
		Timeout:        0,
		AvgDuration:    0,
	}

	totalDuration := 0.0

	for _, item := range h.history {
		if item.Approved {
			stats.Approved++
		} else if item.TimedOut {
			stats.Timeout++
		} else {
			stats.Denied++
		}
		totalDuration += item.DurationSeconds
	}

	if stats.TotalRequests > 0 {
		stats.AvgDuration = totalDuration / float64(stats.TotalRequests)
	}

	return stats
}

// ClearHistory 清空历史
func (h *ApprovalHistory) ClearHistory() {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.history = make([]ApprovalHistoryItem, 0, 100)
}

// ==================== Approval Manager ====================

// ApprovalManager 审批管理器
type ApprovalManager struct {
	mu              sync.RWMutex
	ctx             context.Context
	pendingRequests map[string]*ApprovalRequest
	currentRequest  *ApprovalRequest
	responseChan    chan *ApprovalResponse
}

// NewApprovalManager 创建审批管理器
func NewApprovalManager() *ApprovalManager {
	return &ApprovalManager{
		pendingRequests: make(map[string]*ApprovalRequest),
		responseChan:    make(chan *ApprovalResponse, 1),
	}
}

// ShowApproval 显示审批对话框
func (m *ApprovalManager) ShowApproval(req ApprovalRequest) (*ApprovalResponse, error) {
	m.mu.Lock()

	log.Printf("[Approval] Showing approval dialog: %s (Operation: %s, Risk: %s)",
		req.RequestID, req.Operation, req.RiskLevel)

	// 存储当前请求
	m.currentRequest = &req
	m.pendingRequests[req.RequestID] = &req
	m.mu.Unlock()

	// 设置超时
	timeout := time.Duration(req.TimeoutSeconds) * time.Second
	startTime := time.Now()

	// 等待响应
	select {
	case resp := <-m.responseChan:
		resp.DurationSeconds = time.Since(startTime).Seconds()
		log.Printf("[Approval] Received response: %s - Approved: %v",
			resp.RequestID, resp.Approved)

		m.mu.Lock()
		delete(m.pendingRequests, req.RequestID)
		m.currentRequest = nil
		m.mu.Unlock()

		return resp, nil

	case <-time.After(timeout):
		// 超时
		resp := &ApprovalResponse{
			RequestID:       req.RequestID,
			Approved:        false,
			TimedOut:        true,
			DurationSeconds: time.Since(startTime).Seconds(),
			ResponseTime:    time.Now().Unix(),
		}

		log.Printf("[Approval] Request timed out: %s", req.RequestID)

		m.mu.Lock()
		delete(m.pendingRequests, req.RequestID)
		m.currentRequest = nil
		m.mu.Unlock()

		return resp, nil
	}
}

// SubmitApproval 提交审批决定
func (m *ApprovalManager) SubmitApproval(response ApprovalResponse) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	log.Printf("[Approval] Submitting: %s - Approved: %v, TimedOut: %v",
		response.RequestID, response.Approved, response.TimedOut)

	// 发送响应到通道
	select {
	case m.responseChan <- &response:
		// 成功发送
	default:
		// 通道已满或已关闭
		log.Printf("[Approval] Warning: Response channel full or closed for %s", response.RequestID)
	}

	return nil
}

// GetCurrentRequest 获取当前待处理请求
func (m *ApprovalManager) GetCurrentRequest() *ApprovalRequest {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.currentRequest
}

// GetPendingRequests 获取所有待处理请求
func (m *ApprovalManager) GetPendingRequests() []*ApprovalRequest {
	m.mu.RLock()
	defer m.mu.RUnlock()

	requests := make([]*ApprovalRequest, 0, len(m.pendingRequests))
	for _, req := range m.pendingRequests {
		requests = append(requests, req)
	}
	return requests
}

// ==================== Desktop API ====================

// DesktopInfo 桌面信息
type DesktopInfo struct {
	Version     string `json:"version"`
	Environment string `json:"environment"`
	BotState    string `json:"bot_state"`
	Uptime      string `json:"uptime"`
}

// GetDesktopInfo 获取桌面信息
func (a *App) GetDesktopInfo() *DesktopInfo {
	botState := "stopped"
	if a.botRunning {
		botState = "running"
	}

	return &DesktopInfo{
		Version:     "1.0.0",
		Environment: "production",
		BotState:    botState,
		Uptime:      formatUptime(time.Since(time.Now())),
	}
}

// StartBot 启动 Bot
func (a *App) StartBot() error {
	log.Println("[Desktop] Starting bot...")
	a.botRunning = true
	return nil
}

// StopBot 停止 Bot
func (a *App) StopBot() error {
	log.Println("[Desktop] Stopping bot...")
	a.botRunning = false
	return nil
}

// ==================== Approval API ====================

// ShowApproval 显示审批对话框（公开方法）
func (a *App) ShowApproval(req ApprovalRequest) (*ApprovalResponse, error) {
	return a.approvalManager.ShowApproval(req)
}

// SubmitApproval 提交审批决定（公开方法）
func (a *App) SubmitApproval(response ApprovalResponse) error {
	// 计算持续时间
	if !a.approvalStartTime.IsZero() {
		response.DurationSeconds = time.Since(a.approvalStartTime).Seconds()
	}
	response.ResponseTime = time.Now().Unix()

	// 添加到历史记录
	item := ApprovalHistoryItem{
		RequestID:       response.RequestID,
		Approved:        response.Approved,
		TimedOut:        response.TimedOut,
		DurationSeconds: response.DurationSeconds,
		ResponseTime:    response.ResponseTime,
	}

	// 获取请求详情
	if req := a.approvalManager.GetCurrentRequest(); req != nil && req.RequestID == response.RequestID {
		item.Operation = req.Operation
		item.OperationName = req.Operation
		item.Target = req.Target
		item.RiskLevel = req.RiskLevel
	}

	a.approvalHistory.AddRecord(item)

	// 清理待处理的请求
	a.approvalManager.mu.Lock()
	delete(a.approvalManager.pendingRequests, response.RequestID)
	a.approvalManager.currentRequest = nil
	a.approvalManager.mu.Unlock()

	a.pendingApproval = nil

	// 提交到管理器（但不实际发送响应，因为我们使用事件模式）
	log.Printf("[App] Approval submitted: %s - Approved: %v", response.RequestID, response.Approved)

	return nil
}

// GetPendingApprovals 获取待审批列表（公开方法）
func (a *App) GetPendingApprovals() []*ApprovalRequest {
	return a.approvalManager.GetPendingRequests()
}

// GetApprovalHistory 获取审批历史（公开方法）
func (a *App) GetApprovalHistory(limit int) []ApprovalHistoryItem {
	return a.approvalHistory.GetHistory(limit)
}

// GetApprovalStats 获取审批统计（公开方法）
func (a *App) GetApprovalStats() ApprovalStats {
	return a.approvalHistory.GetStats()
}

// ClearApprovalHistory 清空审批历史（公开方法）
func (a *App) ClearApprovalHistory() {
	a.approvalHistory.ClearHistory()
}

// ==================== 模拟请求 API ====================

// SimulatedRequest 模拟审批请求类型
type SimulatedRequest struct {
	ID           string `json:"id"`
	Name         string `json:"name"`
	Description  string `json:"description"`
	Operation    string `json:"operation"`
	OperationName string `json:"operation_name"`
	ExampleTarget string `json:"example_target"`
	RiskLevel    string `json:"risk_level"`
}

// GetSimulatedRequests 获取模拟请求类型列表
func (a *App) GetSimulatedRequests() []SimulatedRequest {
	return []SimulatedRequest{
		{
			ID:           "REQ-001",
			Name:         "文件删除",
			Description:  "删除指定文件或目录",
			Operation:    "file_delete",
			OperationName: "删除文件",
			ExampleTarget: "C:\\Temp\\test.txt",
			RiskLevel:    "HIGH",
		},
		{
			ID:           "REQ-002",
			Name:         "进程执行",
			Description:  "执行系统命令或程序",
			Operation:    "process_exec",
			OperationName: "执行进程",
			ExampleTarget: "powershell.exe -Command \"Get-Process\"",
			RiskLevel:    "CRITICAL",
		},
		{
			ID:           "REQ-003",
			Name:         "注册表修改",
			Description:  "修改 Windows 注册表项",
			Operation:    "registry_write",
			OperationName: "修改注册表",
			ExampleTarget: "HKEY_LOCAL_MACHINE\\Software\\MyApp",
			RiskLevel:    "MEDIUM",
		},
		{
			ID:           "REQ-004",
			Name:         "网络下载",
			Description:  "从互联网下载文件",
			Operation:    "network_download",
			OperationName: "网络下载",
			ExampleTarget: "https://example.com/file.zip",
			RiskLevel:    "MEDIUM",
		},
		{
			ID:           "REQ-005",
			Name:         "系统关闭",
			Description:  "关闭或重启系统",
			Operation:    "system_shutdown",
			OperationName: "关闭系统",
			ExampleTarget: "shutdown /s /t 0",
			RiskLevel:    "CRITICAL",
		},
	}
}

// SimulateApproval 模拟审批请求
func (a *App) SimulateApproval(requestType string) error {
	// 生成请求ID
	requestID := fmt.Sprintf("REQ-%d", time.Now().UnixNano()%100000)

	// 获取请求类型详情
	requests := a.GetSimulatedRequests()
	var reqType SimulatedRequest
	found := false

	for _, req := range requests {
		if req.Operation == requestType {
			reqType = req
			found = true
			break
		}
	}

	if !found {
		return fmt.Errorf("unknown request type: %s", requestType)
	}

	// 创建请求
	req := ApprovalRequest{
		RequestID:      requestID,
		Operation:      reqType.Operation,
		OperationName:  reqType.OperationName,
		Target:         reqType.ExampleTarget,
		RiskLevel:      reqType.RiskLevel,
		Reason:         "AI Agent 需要执行此操作以完成任务",
		TimeoutSeconds: 30,
		Context: map[string]string{
			"request_type": reqType.ID,
			"timestamp":   fmt.Sprintf("%d", time.Now().Unix()),
		},
		Timestamp: time.Now().Unix(),
	}

	log.Printf("[App] Simulating approval request: %s", req.RequestID)

	// 存储待处理的审批请求
	a.approvalManager.mu.Lock()
	a.approvalManager.currentRequest = &req
	a.approvalManager.pendingRequests[req.RequestID] = &req
	a.approvalManager.mu.Unlock()

	// 存储在 App 中用于快速访问
	a.pendingApproval = &req
	a.approvalStartTime = time.Now()

	// 发送事件到前端显示对话框
	runtime.EventsEmit(a.ctx, "show-approval", req)

	return nil
}

// ==================== Logs API ====================

// LogEntry 日志条目
type LogEntry struct {
	Timestamp string `json:"timestamp"`
	Level     string `json:"level"`
	Module    string `json:"module"`
	Message   string `json:"message"`
}

// LogBuffer 日志缓冲
type LogBuffer struct {
	mu     sync.RWMutex
	entries []LogEntry
	maxSize int
}

// NewLogBuffer 创建日志缓冲
func NewLogBuffer() *LogBuffer {
	buffer := &LogBuffer{
		entries: make([]LogEntry, 0, 1000),
		maxSize: 1000,
	}
	// 添加初始日志
	buffer.Add("INFO", "app", "NemesisBot UI started")
	buffer.Add("INFO", "webview", "WebView initialized")
	buffer.Add("INFO", "server", "Local server running on http://127.0.0.1:49000")
	buffer.Add("INFO", "agent", "Agent loop started")
	buffer.Add("INFO", "security", "Security auditor initialized")
	buffer.Add("INFO", "app", "Ready to accept connections")
	return buffer
}

// Add 添加日志
func (b *LogBuffer) Add(level, module, message string) {
	b.mu.Lock()
	defer b.mu.Unlock()

	entry := LogEntry{
		Timestamp: time.Now().Format("2006-01-02 15:04:05"),
		Level:     level,
		Module:    module,
		Message:   message,
	}

	b.entries = append(b.entries, entry)

	if len(b.entries) > b.maxSize {
		b.entries = b.entries[1:]
	}
}

// Get 获取日志，支持过滤
func (b *LogBuffer) Get(levelFilter, moduleFilter string, limit int) []LogEntry {
	b.mu.RLock()
	defer b.mu.RUnlock()

	var filtered []LogEntry
	for _, entry := range b.entries {
		if levelFilter != "" && entry.Level != levelFilter {
			continue
		}
		if moduleFilter != "" && entry.Module != moduleFilter {
			continue
		}
		filtered = append(filtered, entry)
	}

	if limit <= 0 || limit > len(filtered) {
		limit = len(filtered)
	}

	start := len(filtered) - limit
	if start < 0 {
		start = 0
	}

	result := make([]LogEntry, limit)
	copy(result, filtered[start:])
	return result
}

// GetLogs 获取日志（公开方法）
func (a *App) GetLogs(level, module string, limit int) []LogEntry {
	// 使用模拟的日志缓冲生成更多日志
	result := []LogEntry{
		{Timestamp: time.Now().Add(-5 * time.Minute).Format("2006-01-02 15:04:05"), Level: "INFO", Module: "app", Message: "NemesisBot UI started"},
		{Timestamp: time.Now().Add(-4 * time.Minute).Format("2006-01-02 15:04:05"), Level: "INFO", Module: "webview", Message: "WebView initialized"},
		{Timestamp: time.Now().Add(-3 * time.Minute).Format("2006-01-02 15:04:05"), Level: "INFO", Module: "server", Message: "Local server running on http://127.0.0.1:49000"},
		{Timestamp: time.Now().Add(-2 * time.Minute).Format("2006-01-02 15:04:05"), Level: "INFO", Module: "agent", Message: "Agent loop started"},
		{Timestamp: time.Now().Add(-1 * time.Minute).Format("2006-01-02 15:04:05"), Level: "INFO", Module: "security", Message: "Security auditor initialized"},
		{Timestamp: time.Now().Format("2006-01-02 15:04:05"), Level: "INFO", Module: "app", Message: "Ready to accept connections"},
	}

	// 应用过滤器
	var filtered []LogEntry
	for _, entry := range result {
		if level != "" && entry.Level != level {
			continue
		}
		if module != "" && entry.Module != module {
			continue
		}
		filtered = append(filtered, entry)
	}

	if limit > 0 && limit < len(filtered) {
		start := len(filtered) - limit
		filtered = filtered[start:]
	}

	return filtered
}

// GetLogModules 获取所有日志模块
func (a *App) GetLogModules() []string {
	return []string{"app", "webview", "server", "agent", "security", "channel", "cluster", "rpc"}
}

// ==================== Chat API ====================

// ChatMessage 聊天消息
type ChatMessage struct {
	ID        string `json:"id"`
	Role      string `json:"role"` // "user" or "assistant"
	Content   string `json:"content"`
	Timestamp int64  `json:"timestamp"`
}

// ChatHistory 聊天历史
type ChatHistory struct {
	mu       sync.RWMutex
	messages []ChatMessage
	maxSize  int
}

// NewChatHistory 创建聊天历史
func NewChatHistory() *ChatHistory {
	return &ChatHistory{
		messages: make([]ChatMessage, 0, 100),
		maxSize:  100,
	}
}

// Add 添加消息
func (h *ChatHistory) Add(role, content string) {
	h.mu.Lock()
	defer h.mu.Unlock()

	msg := ChatMessage{
		ID:        fmt.Sprintf("MSG-%d", time.Now().UnixNano()),
		Role:      role,
		Content:   content,
		Timestamp: time.Now().Unix(),
	}

	h.messages = append(h.messages, msg)

	if len(h.messages) > h.maxSize {
		h.messages = h.messages[1:]
	}
}

// Get 获取消息
func (h *ChatHistory) Get(limit int) []ChatMessage {
	h.mu.RLock()
	defer h.mu.RUnlock()

	if limit <= 0 || limit > len(h.messages) {
		limit = len(h.messages)
	}

	start := len(h.messages) - limit
	if start < 0 {
		start = 0
	}

	result := make([]ChatMessage, limit)
	copy(result, h.messages[start:])
	return result
}

// Clear 清空消息
func (h *ChatHistory) Clear() {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.messages = make([]ChatMessage, 0, 100)
}

// SendMessage 发送消息
func (a *App) SendMessage(message string) string {
	log.Printf("[Chat] Sending message: %s", message)

	// 添加用户消息到历史
	// TODO: 集成实际的 chat 逻辑
	// 目前返回模拟响应
	time.Sleep(300 * time.Millisecond)

	return "Response from NemesisBot: I received your message - \"" + message + "\""
}

// GetChatHistory 获取聊天历史
func (a *App) GetChatHistory(limit int) []ChatMessage {
	// 返回模拟的聊天历史
	return []ChatMessage{
		{ID: "MSG-1", Role: "assistant", Content: "Hello! I'm NemesisBot. How can I help you today?", Timestamp: time.Now().Add(-10 * time.Minute).Unix()},
	}
}

// ClearChatHistory 清空聊天历史
func (a *App) ClearChatHistory() {
	log.Println("[Chat] Clearing chat history")
	// TODO: 实现清空逻辑
}

// ==================== Settings API ====================

// Setting 设置项
type Setting struct {
	Key         string `json:"key"`
	Value       string `json:"value"`
	Type        string `json:"type"`
	Description string `json:"description"`
}

// ThemeConfig 主题配置
type ThemeConfig struct {
	CurrentTheme string `json:"current_theme"`
	AutoTheme    bool   `json:"auto_theme"`
}

// GetSettings 获取设置
func (a *App) GetSettings() []Setting {
	// TODO: 从配置文件读取
	return []Setting{
		{Key: "bot.name", Value: "NemesisBot", Type: "string", Description: "Bot name"},
		{Key: "security.enabled", Value: "true", Type: "boolean", Description: "Enable security middleware"},
		{Key: "security.restrict_to_workspace", Value: "true", Type: "boolean", Description: "Restrict file access to workspace"},
		{Key: "log.level", Value: "INFO", Type: "enum", Description: "Log level"},
		{Key: "cluster.enabled", Value: "false", Type: "boolean", Description: "Enable cluster mode"},
		{Key: "ui.theme", Value: "dark", Type: "enum", Description: "UI theme (light/dark)"},
		{Key: "ui.auto_theme", Value: "true", Type: "boolean", Description: "Auto switch theme"},
		{Key: "ui.animations", Value: "true", Type: "boolean", Description: "Enable animations"},
	}
}

// UpdateSetting 更新设置
func (a *App) UpdateSetting(key, value string) error {
	log.Printf("[Settings] Updating setting: %s = %s", key, value)

	// TODO: 持久化到配置文件

	// 如果是主题相关设置，发送事件通知前端
	if key == "ui.theme" || key == "ui.auto_theme" {
		themeConfig := ThemeConfig{
			CurrentTheme: value,
			AutoTheme:    false,
		}
		runtime.EventsEmit(a.ctx, "theme-changed", themeConfig)
	}

	return nil
}

// GetThemeConfig 获取主题配置
func (a *App) GetThemeConfig() ThemeConfig {
	return ThemeConfig{
		CurrentTheme: "dark",
		AutoTheme:    true,
	}
}

// SetTheme 设置主题
func (a *App) SetTheme(theme string, auto bool) error {
	log.Printf("[Settings] Setting theme: %s (auto: %v)", theme, auto)

	config := ThemeConfig{
		CurrentTheme: theme,
		AutoTheme:    auto,
	}

	// 发送主题变更事件
	runtime.EventsEmit(a.ctx, "theme-changed", config)

	return nil
}

// ==================== System Status API ====================

// SystemStatus 系统状态
type SystemStatus struct {
	Uptime      string  `json:"uptime"`
	MemoryUsage float64 `json:"memory_usage_mb"`
	CPUUsage    float64 `json:"cpu_usage_percent"`
	ThreadCount int     `json:"thread_count"`
	Version     string  `json:"version"`
	GoVersion   string  `json:"go_version"`
}

// GetSystemStatus 获取系统状态
func (a *App) GetSystemStatus() SystemStatus {
	return SystemStatus{
		Uptime:      "0h 5m 23s",
		MemoryUsage: 45.2,
		CPUUsage:    2.5,
		ThreadCount: 12,
		Version:     "1.0.0",
		GoVersion:   "1.25.7",
	}
}

// KeyboardShortcut 键盘快捷键
type KeyboardShortcut struct {
	Key        string `json:"key"`
	Action     string `json:"action"`
	Description string `json:"description"`
}

// GetKeyboardShortcuts 获取键盘快捷键
func (a *App) GetKeyboardShortcuts() []KeyboardShortcut {
	return []KeyboardShortcut{
		{Key: "Ctrl+1", Action: "navigate_chat", Description: "导航到 Chat 页面"},
		{Key: "Ctrl+2", Action: "navigate_overview", Description: "导航到审批中心"},
		{Key: "Ctrl+3", Action: "navigate_logs", Description: "导航到 Logs 页面"},
		{Key: "Ctrl+4", Action: "navigate_settings", Description: "导航到 Settings 页面"},
		{Key: "Ctrl+K", Action: "focus_chat_input", Description: "聚焦聊天输入框"},
		{Key: "Ctrl+L", Action: "clear_logs", Description: "清空日志"},
		{Key: "Ctrl+H", Action: "clear_chat", Description: "清空聊天历史"},
		{Key: "Escape", Action: "close_dialog", Description: "关闭对话框"},
		{Key: "Ctrl+Q", Action: "quit", Description: "退出应用"},
	}
}

// ExecuteShortcut 执行快捷键操作
func (a *App) ExecuteShortcut(action string) error {
	log.Printf("[Shortcut] Executing: %s", action)

	// 发送快捷键事件到前端
	runtime.EventsEmit(a.ctx, "execute-shortcut", action)

	return nil
}

// ==================== Utility Functions ====================

// formatUptime 格式化运行时间
func formatUptime(d time.Duration) string {
	d = d.Round(time.Second)

	seconds := int(d.Seconds())
	if seconds < 60 {
		return fmt.Sprintf("%ds", seconds)
	}

	minutes := seconds / 60
	seconds = seconds % 60
	if minutes < 60 {
		return fmt.Sprintf("%dm %ds", minutes, seconds)
	}

	hours := minutes / 60
	minutes = minutes % 60
	return fmt.Sprintf("%dh %dm %ds", hours, minutes, seconds)
}

// ==================== Public API ====================

// CheckSystemRequirements 检查系统要求
func CheckSystemRequirements() bool {
	// TODO: 添加实际的系统检查
	// 目前 Wails 在 Windows 上不需要额外的运行时检查
	return true
}

// RunWithServiceManager 使用 ServiceManager 运行 Desktop UI
func RunWithServiceManager(cfg *Config, svcMgr interface{}) error {
	log.Println("[Desktop] Starting Wails Desktop UI with ServiceManager...")

	// 设置全局 ServiceManager
	SetServiceManager(svcMgr)

	// 创建应用实例
	application := NewApp()

	// 运行应用
	err := wails.Run(&options.App{
		Title:  "NemesisBot",
		Width:  1280,
		Height: 800,
		AssetServer: &assetserver.Options{
			Assets: assets,
		},
		BackgroundColour: &options.RGBA{R: 27, G: 38, B: 54, A: 1},
		OnStartup:        application.Startup,
		OnShutdown:       application.Shutdown,
		Bind: []interface{}{
			application,
		},
	})

	if err != nil {
		return fmt.Errorf("failed to run Wails UI: %w", err)
	}

	return nil
}

// Run 运行 Desktop UI（不带 ServiceManager）
func Run() error {
	log.Println("[Desktop] Starting Wails Desktop UI...")

	// 创建应用实例
	application := NewApp()

	// 运行应用
	err := wails.Run(&options.App{
		Title:  "NemesisBot",
		Width:  1280,
		Height: 800,
		AssetServer: &assetserver.Options{
			Assets: assets,
		},
		BackgroundColour: &options.RGBA{R: 27, G: 38, B: 54, A: 1},
		OnStartup:        application.Startup,
		OnShutdown:       application.Shutdown,
		Bind: []interface{}{
			application,
		},
	})

	if err != nil {
		return fmt.Errorf("failed to run Wails UI: %w", err)
	}

	return nil
}

