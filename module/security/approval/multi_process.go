package approval

import (
	"context"
	"fmt"
	"log"
	"strings"
	"time"
)

// ChildProcessFactory 子进程工厂接口
//
// ProcessManager 实现这个接口
type ChildProcessFactory interface {
	// SpawnChild 创建子进程
	// 参数:
	//   - windowType: 窗口类型（如 "approval"）
	//   - data: 窗口数据
	// 返回:
	//   - childID: 子进程 ID
	//   - resultCh: 结果通道
	//   - error: 错误
	SpawnChild(windowType string, data interface{}) (childID string, resultCh <-chan interface{}, err error)
}

// 全局子进程工厂（由 Gateway 设置）
var globalChildProcessFactory ChildProcessFactory

// SetChildProcessFactory 设置全局子进程工厂
func SetChildProcessFactory(factory ChildProcessFactory) {
	globalChildProcessFactory = factory
	log.Printf("[approval] ChildProcessFactory registered")
}

// RequestApproval 请求用户审批（多进程模式实现）
//
// 这个方法会使用 ChildProcessFactory 创建子进程来显示审批窗口
func (m *approvalManagerImpl) RequestApproval(ctx context.Context, req *ApprovalRequest) (*ApprovalResponse, error) {
	if !m.running {
		return nil, fmt.Errorf("approval manager is not running")
	}

	log.Printf("[approval] Requesting approval (multi-process): request_id=%s, operation=%s, target=%s, risk_level=%s",
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

	// 检查是否有 ChildProcessFactory
	if globalChildProcessFactory == nil {
		// 没有 ChildProcessFactory，使用默认行为
		log.Printf("[approval] No ChildProcessFactory set, using default behavior")

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

		log.Printf("[approval] No ChildProcessFactory available, rejecting dangerous operation")
		return &ApprovalResponse{
			RequestID:       req.RequestID,
			Approved:        false,
			TimedOut:        false,
			DurationSeconds: 0.01,
			ResponseTime:    time.Now().Unix(),
		}, nil
	}

	// 使用 ChildProcessFactory 创建审批窗口
	log.Printf("[approval] Creating child process for approval window")

	// 准备审批窗口数据
	windowData := map[string]interface{}{
		"request_id":      req.RequestID,
		"operation":       req.Operation,
		"operation_name":  GetOperationDisplayName(req.Operation),
		"target":          req.Target,
		"risk_level":      req.RiskLevel,
		"reason":          req.Reason,
		"timeout_seconds": req.TimeoutSeconds,
		"context":         req.Context,
		"timestamp":       time.Now().Unix(),
	}

	// 创建子进程
	childID, resultChan, err := globalChildProcessFactory.SpawnChild("approval", windowData)
	if err != nil {
		// 弹窗不支持时，直接拒绝审批请求
		if strings.Contains(err.Error(), "popup not supported") {
			log.Printf("[approval] Popup not supported, rejecting request: %s", req.RequestID)
			return &ApprovalResponse{
				RequestID:       req.RequestID,
				Approved:        false,
				TimedOut:        false,
				DurationSeconds: 0.01,
				ResponseTime:    time.Now().Unix(),
			}, nil
		}
		return nil, fmt.Errorf("failed to create approval window: %w", err)
	}

	log.Printf("[approval] Child process created: child_id=%s", childID)

	// 创建带超时的上下文
	timeout := time.Duration(req.TimeoutSeconds) * time.Second
	if timeout <= 0 {
		timeout = time.Duration(m.config.Timeout) * time.Second
	}

	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	// 等待结果或超时
	select {
	case result := <-resultChan:
		// 解析结果
		resultMap, ok := result.(map[string]interface{})
		if !ok {
			return nil, fmt.Errorf("invalid result type from child process")
		}

		// 结果可能在顶层，也可能嵌套在 "data" 字段中
		var approved bool
		var reason string
		if dataRaw, hasData := resultMap["data"]; hasData {
			if dataMap, ok := dataRaw.(map[string]interface{}); ok {
				approved, _ = dataMap["approved"].(bool)
				reason, _ = dataMap["reason"].(string)
			}
		} else {
			approved, _ = resultMap["approved"].(bool)
			reason, _ = resultMap["reason"].(string)
		}

		resp := &ApprovalResponse{
			RequestID:       req.RequestID,
			Approved:        approved,
			TimedOut:        false,
			DurationSeconds: time.Since(startTime).Seconds(),
			ResponseTime:    time.Now().Unix(),
		}

		log.Printf("[approval] Approval received: request_id=%s, approved=%v, reason=%s, duration=%f",
			resp.RequestID,
			resp.Approved,
			reason,
			resp.DurationSeconds,
		)

		return resp, nil

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
	safeOps := map[string]bool{
		"file_read":       true,
		"dir_list":        true,
		"network_request": true,
		"hardware_i2c":    true,
		"registry_read":   true,
	}

	return safeOps[req.Operation] && req.RiskLevel == "LOW"
}
