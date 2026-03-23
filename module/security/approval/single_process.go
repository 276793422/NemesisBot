//go:build !multi_process

package approval

import (
	"context"
	"fmt"
	"log"
	"time"
)

// 全局审批处理器（由 Wails Desktop UI 设置）
var globalApprovalHandler ApprovalHandler

// ApprovalHandler 审批处理器接口
//
// Wails Desktop UI 实现这个接口来处理审批请求
type ApprovalHandler interface {
	// RequestApproval 请求用户审批
	// 参数:
	//   - req: 审批请求
	// 返回:
	//   - *ApprovalResponse: 用户响应
	//   - error: 错误
	RequestApproval(req *ApprovalRequest) (*ApprovalResponse, error)
}

// SetApprovalHandler 设置全局审批处理器
//
// 这个方法应该由 Wails Desktop UI 在启动时调用
func SetApprovalHandler(handler ApprovalHandler) {
	globalApprovalHandler = handler
	log.Printf("[approval] Approval handler registered: %T", handler)
}

// RequestApproval 请求用户审批（集成 Wails UI 实现）
//
// 这个方法会使用 Wails Desktop UI 的审批系统来显示对话框
// 参数:
//   - ctx: 上下文，用于取消操作
//   - req: 审批请求，包含操作详情
// 返回:
//   - *ApprovalResponse: 用户响应结果
//   - error: 请求失败时返回错误
func (m *approvalManagerImpl) RequestApproval(ctx context.Context, req *ApprovalRequest) (*ApprovalResponse, error) {
	if !m.running {
		return nil, fmt.Errorf("approval manager is not running")
	}

	log.Printf("[approval] Requesting approval: request_id=%s, operation=%s, target=%s, risk_level=%s",
		req.RequestID,
		req.Operation,
		req.Target,
		req.RiskLevel,
	)

	startTime := time.Now()

	// 首先检查 context 是否已取消
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	default:
		// context 仍然有效，继续
	}

	// 检查是否有可用的审批处理器
	if globalApprovalHandler == nil {
		// 如果没有设置处理器（例如在 CLI 模式下），使用默认行为
		log.Printf("[approval] No approval handler set, using default behavior")

		// 对于安全操作，自动批准
		if isSafeOperation(req) {
			log.Printf("[approval] Auto-approving safe operation: %s", req.Operation)
			return &ApprovalResponse{
				RequestID:       req.RequestID,
				Approved:        true,
				TimedOut:        false,
				DurationSeconds: 0.01,
				ResponseTime:    time.Now().Unix(),
			}, nil
		}

		// 对于危险操作，没有 UI 时拒绝
		log.Printf("[approval] No UI available, rejecting dangerous operation")
		return &ApprovalResponse{
			RequestID:       req.RequestID,
			Approved:        false,
			TimedOut:        false,
			DurationSeconds: 0.01,
			ResponseTime:    time.Now().Unix(),
		}, nil
	}

	// 使用 Wails Desktop UI 的审批处理器
	log.Printf("[approval] Using Wails Desktop UI for approval")

	// 创建带超时的上下文
	timeout := time.Duration(req.TimeoutSeconds) * time.Second
	if timeout <= 0 {
		timeout = time.Duration(m.config.Timeout) * time.Second
	}

	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	// 在 goroutine 中调用处理器
	resultChan := make(chan *ApprovalResponse, 1)
	errChan := make(chan error, 1)

	go func() {
		resp, err := globalApprovalHandler.RequestApproval(req)
		if err != nil {
			errChan <- err
		} else {
			resultChan <- resp
		}
	}()

	// 等待结果或超时
	select {
	case resp := <-resultChan:
		resp.DurationSeconds = time.Since(startTime).Seconds()
		log.Printf("[approval] Approval received: request_id=%s, approved=%v, duration=%f",
			resp.RequestID,
			resp.Approved,
			resp.DurationSeconds,
		)
		return resp, nil

	case err := <-errChan:
		return nil, fmt.Errorf("approval request failed: %w", err)

	case <-ctx.Done():
		resp := &ApprovalResponse{
			RequestID:       req.RequestID,
			Approved:        false,
			TimedOut:        true,
			DurationSeconds: time.Since(startTime).Seconds(),
			ResponseTime:    time.Now().Unix(),
		}

		log.Printf("[approval] Approval timed out: request_id=%s, duration=%f",
			resp.RequestID,
			resp.DurationSeconds,
		)

		return resp, nil
	}
}

// isSafeOperation 判断操作是否安全
//
// 安全操作可以在没有 UI 的情况下自动批准
func isSafeOperation(req *ApprovalRequest) bool {
	// 读取操作通常是安全的
	safeOps := map[string]bool{
		"file_read":        true,
		"dir_list":         true,
		"network_request":  true,
		"hardware_i2c":     true,
		"registry_read":    true,
	}

	return safeOps[req.Operation] && req.RiskLevel == "LOW"
}
