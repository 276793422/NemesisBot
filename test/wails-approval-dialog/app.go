package main

import (
	"context"
	"fmt"
	"log"
	"time"
)

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

// App struct
type App struct {
	ctx             context.Context
	pendingRequests map[string]*ApprovalRequest
}

// NewApp creates a new App application struct
func NewApp() *App {
	return &App{
		pendingRequests: make(map[string]*ApprovalRequest),
	}
}

// startup is called when the app starts. The context is saved
// so we can call the runtime methods
func (a *App) startup(ctx context.Context) {
	a.ctx = ctx
	log.Println("Security Approval Dialog started")
}

// GetDemoRequests 获取演示用的审批请求列表
func (a *App) GetDemoRequests() []ApprovalRequest {
	return []ApprovalRequest{
		{
			RequestID:      "REQ-001",
			Operation:      "file_delete",
			OperationName:  "删除文件",
			Target:         "C:\\Important\\data.db",
			RiskLevel:      "HIGH",
			Reason:         "AI Agent 需要清理过期的数据库文件",
			TimeoutSeconds: 30,
			Context: map[string]string{
				"file_size": "256 MB",
				"file_type": "SQLite Database",
			},
			Timestamp: time.Now().Unix(),
		},
		{
			RequestID:      "REQ-002",
			Operation:      "process_exec",
			OperationName:  "执行进程",
			Target:         "powershell.exe -RemoveFile C:\\Temp\\*",
			RiskLevel:      "CRITICAL",
			Reason:         "需要清理临时目录中的所有文件",
			TimeoutSeconds: 60,
			Context: map[string]string{
				"command": "Remove-Item -Force",
			},
			Timestamp: time.Now().Unix(),
		},
		{
			RequestID:      "REQ-003",
			Operation:      "registry_write",
			OperationName:  "修改注册表",
			Target:         "HKEY_LOCAL_MACHINE\\Software\\MyApp\\AutoStart",
			RiskLevel:      "MEDIUM",
			Reason:         "设置应用程序开机自启动",
			TimeoutSeconds: 30,
			Context: map[string]string{
				"value": "1",
			},
			Timestamp: time.Now().Unix(),
		},
		{
			RequestID:      "REQ-004",
			Operation:      "network_download",
			OperationName:  "网络下载",
			Target:         "https://example.com/tool.exe",
			RiskLevel:      "MEDIUM",
			Reason:         "下载并安装系统更新工具",
			TimeoutSeconds: 45,
			Context: map[string]string{
				"size": "15.2 MB",
			},
			Timestamp: time.Now().Unix(),
		},
	}
}

// SubmitApproval 提交审批决定
func (a *App) SubmitApproval(response ApprovalResponse) error {
	log.Printf("Approval received: RequestID=%s, Approved=%v, TimedOut=%v\n",
		response.RequestID, response.Approved, response.TimedOut)

	if response.Approved {
		log.Printf("✅ Request %s APPROVED - Operation will proceed\n", response.RequestID)
		// 这里可以执行被批准的操作
	} else {
		log.Printf("❌ Request %s DENIED - Operation blocked\n", response.RequestID)
		// 操作被拒绝
	}

	return nil
}

// SimulateBackendRequest 模拟后端发送审批请求
func (a *App) SimulateBackendRequest(riskLevel string) ApprovalRequest {
	operations := map[string][]string{
		"LOW":     {"file_read", "读取配置文件"},
		"MEDIUM":  {"file_write", "写入文件"},
		"HIGH":    {"file_delete", "删除文件"},
		"CRITICAL": {"process_exec", "执行系统命令"},
	}

	op := operations[riskLevel]
	if op == nil {
		op = operations["MEDIUM"]
	}

	return ApprovalRequest{
		RequestID:      fmt.Sprintf("REQ-%d", time.Now().UnixNano()%10000),
		Operation:      op[0],
		OperationName:  op[1],
		Target:         fmt.Sprintf("C:\\System\\%s", op[0]),
		RiskLevel:      riskLevel,
		Reason:         "AI Agent 需要执行此操作以完成任务",
		TimeoutSeconds: 30,
		Context:        map[string]string{},
		Timestamp:      time.Now().Unix(),
	}
}

// GetSystemInfo 获取系统信息（用于测试）
func (a *App) GetSystemInfo() map[string]interface{} {
	return map[string]interface{}{
		"version":     "1.0.0",
		"environment": "development",
		"timestamp":   time.Now().Format("2006-01-02 15:04:05"),
	}
}
