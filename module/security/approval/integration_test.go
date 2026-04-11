package approval

import (
	"context"
	"testing"
	"time"
)

// mockChildProcessFactory 模拟子进程工厂（用于测试）
type mockChildProcessFactory struct {
	approve bool
	delay   time.Duration
}

func (m *mockChildProcessFactory) SpawnChild(windowType string, data interface{}) (string, <-chan interface{}, error) {
	resultCh := make(chan interface{}, 1)
	go func() {
		if m.delay > 0 {
			time.Sleep(m.delay)
		}
		resultCh <- map[string]interface{}{
			"approved": m.approve,
			"reason":   "test",
		}
	}()
	return "test-child-1", resultCh, nil
}

// TestApprovalHandlerIntegration 测试审批处理器集成
func TestApprovalHandlerIntegration(t *testing.T) {
	// 创建审批管理器
	mgr := NewApprovalManager(nil)
	if err := mgr.Start(); err != nil {
		t.Fatalf("Failed to start approval manager: %v", err)
	}
	defer mgr.Stop()

	// 设置模拟的子进程工厂
	SetChildProcessFactory(&mockChildProcessFactory{
		approve: true,
		delay:   100 * time.Millisecond,
	})
	defer SetChildProcessFactory(nil)

	// 创建测试请求
	req := &ApprovalRequest{
		RequestID:      "test-integration-001",
		Operation:      "file_delete",
		Target:         "/tmp/test.txt",
		RiskLevel:      "HIGH",
		Reason:         "Test integration",
		TimeoutSeconds: 30,
		Timestamp:      time.Now().Unix(),
	}

	// 请求审批
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	resp, err := mgr.RequestApproval(ctx, req)
	if err != nil {
		t.Fatalf("RequestApproval failed: %v", err)
	}

	// 验证响应
	if resp == nil {
		t.Fatal("Expected response, got nil")
	}

	if resp.RequestID != req.RequestID {
		t.Errorf("Expected request ID %s, got %s", req.RequestID, resp.RequestID)
	}

	if !resp.Approved {
		t.Error("Expected approved=true, got false")
	}

	if resp.TimedOut {
		t.Error("Expected timedOut=false, got true")
	}

	t.Logf("Approval handler integration test passed: approved=%v, duration=%f", resp.Approved, resp.DurationSeconds)
}

// TestApprovalHandlerTimeout 测试超时场景
func TestApprovalHandlerTimeout(t *testing.T) {
	// 创建审批管理器
	mgr := NewApprovalManager(nil)
	if err := mgr.Start(); err != nil {
		t.Fatalf("Failed to start approval manager: %v", err)
	}
	defer mgr.Stop()

	// 设置超时的模拟工厂
	SetChildProcessFactory(&mockChildProcessFactory{
		approve: false,
		delay:   5 * time.Second, // 超过请求超时
	})
	defer SetChildProcessFactory(nil)

	// 创建短超时的请求
	req := &ApprovalRequest{
		RequestID:      "test-timeout-001",
		Operation:      "file_delete",
		Target:         "/etc/passwd",
		RiskLevel:      "CRITICAL",
		Reason:         "Test timeout",
		TimeoutSeconds: 1, // 1 秒超时
		Timestamp:      time.Now().Unix(),
	}

	// 请求审批
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	resp, err := mgr.RequestApproval(ctx, req)
	if err != nil {
		t.Fatalf("RequestApproval failed: %v", err)
	}

	// 验证超时
	if !resp.TimedOut {
		t.Error("Expected timedOut=true, got false")
	}

	if resp.Approved {
		t.Error("Expected approved=false for timeout, got true")
	}

	t.Logf("Approval handler timeout test passed: timedOut=%v", resp.TimedOut)
}

// TestApprovalHandlerNil 测试没有处理器的情况
func TestApprovalHandlerNil(t *testing.T) {
	// 创建审批管理器
	mgr := NewApprovalManager(nil)
	if err := mgr.Start(); err != nil {
		t.Fatalf("Failed to start approval manager: %v", err)
	}
	defer mgr.Stop()

	// 确保没有设置工厂
	SetChildProcessFactory(nil)

	// 创建安全操作请求
	req := &ApprovalRequest{
		RequestID:      "test-nil-handler-001",
		Operation:      "file_read",
		Target:         "/tmp/test.txt",
		RiskLevel:      "LOW",
		Reason:         "Test safe operation",
		TimeoutSeconds: 30,
		Timestamp:      time.Now().Unix(),
	}

	// 请求审批
	ctx := context.Background()
	resp, err := mgr.RequestApproval(ctx, req)
	if err != nil {
		t.Fatalf("RequestApproval failed: %v", err)
	}

	// 安全操作应该自动批准
	if !resp.Approved {
		t.Error("Expected auto-approval for safe operation")
	}

	t.Logf("Approval handler nil test passed: approved=%v", resp.Approved)
}
