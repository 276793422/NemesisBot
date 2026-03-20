package approval

import (
	"context"
	"log"
)

// ApprovalManager 审批管理器接口
//
// 定义了审批管理器的核心功能，包括启动、停止、状态查询和审批请求处理
type ApprovalManager interface {
	// Start 启动审批管理器
	//
	// 初始化审批管理器，准备接收审批请求
	// 返回:
	//   - error: 启动失败时返回错误
	Start() error

	// Stop 停止审批管理器
	//
	// 释放所有资源，包括关闭可能打开的对话框
	// 返回:
	//   - error: 停止失败时返回错误
	Stop() error

	// IsRunning 检查审批管理器是否正在运行
	//
	// 返回:
	//   - bool: true 表示正在运行，false 表示已停止
	IsRunning() bool

	// RequestApproval 请求用户审批
	//
	// 这是核心方法，会弹出模态对话框征求用户同意
	// 参数:
	//   - ctx: 上下文，用于取消操作
	//   - req: 审批请求，包含操作详情
	// 返回:
	//   - *ApprovalResponse: 用户响应结果
	//   - error: 请求失败时返回错误
	//
	// 使用示例:
	//   resp, err := mgr.RequestApproval(context.Background(), &ApprovalRequest{
	//       RequestID: "req-001",
	//       Operation: "file_delete",
	//       Target:    "/etc/passwd",
	//       RiskLevel: "CRITICAL",
	//       Reason:    "System critical file",
	//       TimeoutSeconds: 30,
	//   })
	RequestApproval(ctx context.Context, req *ApprovalRequest) (*ApprovalResponse, error)

	// SetConfig 更新配置
	//
	// 动态更新审批管理器的配置
	// 参数:
	//   - config: 新的配置
	// 返回:
	//   - error: 更新失败时返回错误
	SetConfig(config *ApprovalConfig) error

	// GetConfig 获取当前配置
	//
	// 返回当前的配置对象
	GetConfig() *ApprovalConfig
}

// NewApprovalManager 创建审批管理器
//
// 创建一个新的审批管理器实例
// 参数:
//   - config: 审批配置，如果为 nil 则使用默认配置
// 返回:
//   - ApprovalManager: 审批管理器实例
func NewApprovalManager(config *ApprovalConfig) ApprovalManager {
	if config == nil {
		config = DefaultApprovalConfig()
	}

	return &approvalManagerImpl{
		config:  config,
		running: false,
	}
}

// approvalManagerImpl 审批管理器的默认实现
//
// 使用单进程模式，在当前进程中创建 WebView 窗口
// 这是最简单、最高效的实现方式
type approvalManagerImpl struct {
	config  *ApprovalConfig
	running bool
}

// Start 启动审批管理器（单进程模式）
func (m *approvalManagerImpl) Start() error {
	log.Printf("[approval] Starting approval manager (single-process mode)")
	m.running = true
	return nil
}

// Stop 停止审批管理器
func (m *approvalManagerImpl) Stop() error {
	log.Printf("[approval] Stopping approval manager")
	m.running = false
	return nil
}

// IsRunning 检查是否运行中
func (m *approvalManagerImpl) IsRunning() bool {
	return m.running
}

// SetConfig 更新配置
func (m *approvalManagerImpl) SetConfig(config *ApprovalConfig) error {
	m.config = config
	log.Printf("[approval] Configuration updated: enabled=%v, timeout=%v, min_risk_level=%s, dialog_width=%d, dialog_height=%d",
		config.Enabled,
		config.Timeout,
		config.MinRiskLevel,
		config.DialogWidth,
		config.DialogHeight,
	)
	return nil
}

// GetConfig 获取配置
func (m *approvalManagerImpl) GetConfig() *ApprovalConfig {
	return m.config
}
