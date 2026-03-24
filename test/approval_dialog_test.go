package main

import (
	"context"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/security/approval"
)

func main() {
	fmt.Println("=== 安全审批对话框测试程序 ===\n")

	// 创建审批管理器
	fmt.Println("1. 创建审批管理器...")
	mgr := approval.NewApprovalManager(nil)
	if err := mgr.Start(); err != nil {
		fmt.Printf("❌ 启动失败: %v\n", err)
		return
	}
	defer mgr.Stop()
	fmt.Println("✅ 审批管理器已启动\n")

	// 测试场景 1: 低风险操作（自动批准）
	fmt.Println("=== 测试场景 1: 低风险操作（自动批准） ===")
	testSafeOperation(mgr)

	// 测试场景 2: 高风险操作（没有 handler，自动拒绝）
	fmt.Println("\n=== 测试场景 2: 高风险操作（无 handler，自动拒绝） ===")
	testDangerousOperationNoHandler(mgr)

	// 测试场景 3: 模拟审批（使用模拟 handler）
	fmt.Println("\n=== 测试场景 3: 高风险操作（使用模拟 handler） ===")
	testWithMockHandler(mgr)

	// 测试场景 4: 超时测试
	fmt.Println("\n=== 测试场景 4: 超时测试 ===")
	testTimeout(mgr)

	fmt.Println("\n=== 所有测试完成 ===")
}

func testSafeOperation(mgr approval.ApprovalManager) {
	req := &approval.ApprovalRequest{
		RequestID:      fmt.Sprintf("test-safe-%d", time.Now().Unix()),
		Operation:      "file_read",
		OperationName:  "读取文件",
		Target:         "/tmp/test.txt",
		RiskLevel:      approval.RiskLevelLow,
		Reason:         "测试低风险操作",
		TimeoutSeconds: 30,
		Timestamp:      time.Now().Unix(),
	}

	fmt.Printf("请求 ID: %s\n", req.RequestID)
	fmt.Printf("操作: %s\n", req.OperationName)
	fmt.Printf("目标: %s\n", req.Target)
	fmt.Printf("风险级别: %s\n", req.RiskLevel)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	resp, err := mgr.RequestApproval(ctx, req)
	if err != nil {
		fmt.Printf("❌ 审批请求失败: %v\n", err)
		return
	}

	fmt.Printf("✅ 审批结果: %v\n", resp.Approved)
	fmt.Printf("   耗时: %.2f 秒\n", resp.DurationSeconds)
	fmt.Printf("   超时: %v\n", resp.TimedOut)
}

func testDangerousOperationNoHandler(mgr approval.ApprovalManager) {
	req := &approval.ApprovalRequest{
		RequestID:      fmt.Sprintf("test-dangerous-%d", time.Now().Unix()),
		Operation:      "file_delete",
		OperationName:  "删除文件",
		Target:         "/etc/passwd",
		RiskLevel:      approval.RiskLevelCritical,
		Reason:         "测试高风险操作（无 handler）",
		TimeoutSeconds: 30,
		Timestamp:      time.Now().Unix(),
	}

	fmt.Printf("请求 ID: %s\n", req.RequestID)
	fmt.Printf("操作: %s\n", req.OperationName)
	fmt.Printf("目标: %s\n", req.Target)
	fmt.Printf("风险级别: %s\n", req.RiskLevel)

	ctx := context.Background()

	resp, err := mgr.RequestApproval(ctx, req)
	if err != nil {
		fmt.Printf("❌ 审批请求失败: %v\n", err)
		return
	}

	fmt.Printf("✅ 审批结果: %v (自动拒绝，因为没有设置 handler)\n", resp.Approved)
	fmt.Printf("   耗时: %.2f 秒\n", resp.DurationSeconds)
}

func testWithMockHandler(mgr approval.ApprovalManager) {
	// 设置模拟 handler
	mockHandler := &MockApprovalHandler{
		approve: true,
		delay:   500 * time.Millisecond,
	}
	approval.SetApprovalHandler(mockHandler)
	defer approval.SetApprovalHandler(nil)

	req := &approval.ApprovalRequest{
		RequestID:      fmt.Sprintf("test-mock-%d", time.Now().Unix()),
		Operation:      "process_kill",
		OperationName:  "终止进程",
		Target:         "PID:12345",
		RiskLevel:      approval.RiskLevelHigh,
		Reason:         "测试模拟审批",
		TimeoutSeconds: 30,
		Timestamp:      time.Now().Unix(),
	}

	fmt.Printf("请求 ID: %s\n", req.RequestID)
	fmt.Printf("操作: %s\n", req.OperationName)
	fmt.Printf("目标: %s\n", req.Target)
	fmt.Printf("风险级别: %s\n", req.RiskLevel)
	fmt.Printf("使用模拟 handler（延迟 500ms）\n")

	ctx := context.Background()

	start := time.Now()
	resp, err := mgr.RequestApproval(ctx, req)
	if err != nil {
		fmt.Printf("❌ 审批请求失败: %v\n", err)
		return
	}

	elapsed := time.Since(start)

	fmt.Printf("✅ 审批结果: %v\n", resp.Approved)
	fmt.Printf("   耗时: %.2f 秒 (实际: %.2f 秒)\n", resp.DurationSeconds, elapsed.Seconds())
	fmt.Printf("   超时: %v\n", resp.TimedOut)
}

func testTimeout(mgr approval.ApprovalManager) {
	// 设置超时的模拟 handler
	mockHandler := &MockApprovalHandler{
		approve: false,
		delay:   5 * time.Second, // 超过请求超时
	}
	approval.SetApprovalHandler(mockHandler)
	defer approval.SetApprovalHandler(nil)

	req := &approval.ApprovalRequest{
		RequestID:      fmt.Sprintf("test-timeout-%d", time.Now().Unix()),
		Operation:      "system_shutdown",
		OperationName:  "系统关机",
		Target:         "localhost",
		RiskLevel:      approval.RiskLevelCritical,
		Reason:         "测试超时机制",
		TimeoutSeconds: 2, // 2 秒超时
		Timestamp:      time.Now().Unix(),
	}

	fmt.Printf("请求 ID: %s\n", req.RequestID)
	fmt.Printf("操作: %s\n", req.OperationName)
	fmt.Printf("目标: %s\n", req.Target)
	fmt.Printf("风险级别: %s\n", req.RiskLevel)
	fmt.Printf("超时设置: %d 秒\n", req.TimeoutSeconds)
	fmt.Printf("模拟 handler 延迟: 5 秒（将导致超时）\n")

	ctx := context.Background()

	start := time.Now()
	resp, err := mgr.RequestApproval(ctx, req)
	if err != nil {
		fmt.Printf("❌ 审批请求失败: %v\n", err)
		return
	}

	elapsed := time.Since(start)

	fmt.Printf("✅ 审批结果: %v\n", resp.Approved)
	fmt.Printf("   耗时: %.2f 秒 (实际: %.2f 秒)\n", resp.DurationSeconds, elapsed.Seconds())
	fmt.Printf("   超时: %v (应该为 true)\n", resp.TimedOut)

	if resp.TimedOut {
		fmt.Printf("✅ 超时机制工作正常\n")
	} else {
		fmt.Printf("❌ 超时机制未生效\n")
	}
}

// MockApprovalHandler 模拟审批处理器
type MockApprovalHandler struct {
	approve bool
	delay   time.Duration
}

func (m *MockApprovalHandler) RequestApproval(req *approval.ApprovalRequest) (*approval.ApprovalResponse, error) {
	fmt.Printf("   [Mock Handler] 收到审批请求，延迟 %v 后返回...\n", m.delay)
	time.Sleep(m.delay)

	return &approval.ApprovalResponse{
		RequestID:       req.RequestID,
		Approved:        m.approve,
		TimedOut:        false,
		DurationSeconds: m.delay.Seconds(),
		ResponseTime:    time.Now().Unix(),
	}, nil
}
