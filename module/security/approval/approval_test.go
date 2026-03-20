package approval

import (
	"context"
	"testing"
	"time"
)

// TestApprovalManager_Lifecycle 测试审批管理器的生命周期
func TestApprovalManager_Lifecycle(t *testing.T) {
	mgr := NewApprovalManager(nil)

	// 初始状态应该是未运行
	if mgr.IsRunning() {
		t.Error("manager should not be running initially")
	}

	// 启动管理器
	if err := mgr.Start(); err != nil {
		t.Fatalf("failed to start manager: %v", err)
	}

	if !mgr.IsRunning() {
		t.Error("manager should be running after Start()")
	}

	// 停止管理器
	if err := mgr.Stop(); err != nil {
		t.Fatalf("failed to stop manager: %v", err)
	}

	if mgr.IsRunning() {
		t.Error("manager should not be running after Stop()")
	}
}

// TestApprovalManager_Config 测试配置管理
func TestApprovalManager_Config(t *testing.T) {
	mgr := NewApprovalManager(nil)

	// 获取默认配置
	config := mgr.GetConfig()
	if config == nil {
		t.Fatal("config should not be nil")
	}

	// 验证默认值
	if !config.Enabled {
		t.Error("approval should be enabled by default")
	}

	if config.Timeout != 30*time.Second {
		t.Errorf("default timeout should be 30s, got %v", config.Timeout)
	}

	if config.MinRiskLevel != RiskLevelMedium {
		t.Errorf("default min risk level should be MEDIUM, got %s", config.MinRiskLevel)
	}

	// 更新配置
	newConfig := &ApprovalConfig{
		Enabled:       false,
		Timeout:       60 * time.Second,
		MinRiskLevel:  RiskLevelHigh,
		DialogWidth:   800,
		DialogHeight:  600,
	}

	if err := mgr.SetConfig(newConfig); err != nil {
		t.Fatalf("failed to set config: %v", err)
	}

	// 验证配置已更新
	updatedConfig := mgr.GetConfig()
	if updatedConfig.Enabled {
		t.Error("config should be disabled")
	}

	if updatedConfig.Timeout != 60*time.Second {
		t.Errorf("timeout should be 60s, got %v", updatedConfig.Timeout)
	}
}

// TestApprovalRequest_Validation 测试审批请求验证
func TestApprovalRequest_Validation(t *testing.T) {
	tests := []struct {
		name    string
		req     *ApprovalRequest
		wantErr bool
	}{
		{
			name: "valid request",
			req: &ApprovalRequest{
				RequestID:      "req-001",
				Operation:      OperationFileDelete,
				Target:         "/etc/passwd",
				RiskLevel:      RiskLevelCritical,
				Reason:         "System critical file",
				TimeoutSeconds: 30,
			},
			wantErr: false,
		},
		{
			name: "missing request ID",
			req: &ApprovalRequest{
				Operation:      OperationFileDelete,
				Target:         "/etc/passwd",
				RiskLevel:      RiskLevelCritical,
				Reason:         "System critical file",
				TimeoutSeconds: 30,
			},
			wantErr: true,
		},
		{
			name: "missing operation",
			req: &ApprovalRequest{
				RequestID:      "req-002",
				Target:         "/etc/passwd",
				RiskLevel:      RiskLevelCritical,
				Reason:         "System critical file",
				TimeoutSeconds: 30,
			},
			wantErr: true,
		},
		{
			name: "missing target",
			req: &ApprovalRequest{
				RequestID:      "req-003",
				Operation:      OperationFileDelete,
				RiskLevel:      RiskLevelCritical,
				Reason:         "System critical file",
				TimeoutSeconds: 30,
			},
			wantErr: true,
		},
		{
			name: "invalid risk level",
			req: &ApprovalRequest{
				RequestID:      "req-004",
				Operation:      OperationFileDelete,
				Target:         "/etc/passwd",
				RiskLevel:      "INVALID",
				Reason:         "System critical file",
				TimeoutSeconds: 30,
			},
			wantErr: true,
		},
		{
			name: "zero timeout",
			req: &ApprovalRequest{
				RequestID:      "req-005",
				Operation:      OperationFileDelete,
				Target:         "/etc/passwd",
				RiskLevel:      RiskLevelCritical,
				Reason:         "System critical file",
				TimeoutSeconds: 0,
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tt.req.Validate()
			if (err != nil) != tt.wantErr {
				t.Errorf("Validate() error = %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

// TestApprovalResponse_Timing 测试响应时间记录
func TestApprovalResponse_Timing(t *testing.T) {
	resp := &ApprovalResponse{
		RequestID:    "req-001",
		Approved:     true,
		TimedOut:     false,
		ResponseTime: time.Now().Unix(),
	}

	if resp.DurationSeconds > 0 {
		t.Error("duration should be 0 initially")
	}

	// 模拟设置持续时间
	resp.DurationSeconds = 2.5

	if resp.DurationSeconds != 2.5 {
		t.Errorf("duration should be 2.5, got %f", resp.DurationSeconds)
	}
}

// TestApprovalConfig_Default 测试默认配置
func TestApprovalConfig_Default(t *testing.T) {
	config := DefaultApprovalConfig()

	if config == nil {
		t.Fatal("default config should not be nil")
	}

	if !config.Enabled {
		t.Error("approval should be enabled by default")
	}

	if config.Timeout != 30*time.Second {
		t.Errorf("default timeout should be 30s, got %v", config.Timeout)
	}

	if config.MinRiskLevel != RiskLevelMedium {
		t.Errorf("default min risk level should be MEDIUM, got %s", config.MinRiskLevel)
	}

	if config.DialogWidth != 550 {
		t.Errorf("default dialog width should be 550, got %d", config.DialogWidth)
	}

	if config.DialogHeight != 480 {
		t.Errorf("default dialog height should be 480, got %d", config.DialogHeight)
	}
}

// TestGetOperationDisplayName 测试操作名称显示
func TestGetOperationDisplayName(t *testing.T) {
	tests := []struct {
		operation string
		want      string
	}{
		{OperationFileRead, "File Read"},
		{OperationFileWrite, "File Write"},
		{OperationFileDelete, "File Delete"},
		{OperationProcessExec, "Process Execute"},
		{OperationProcessKill, "Process Kill"},
		{OperationNetworkDownload, "Network Download"},
		{"unknown_operation", "unknown_operation"},
	}

	for _, tt := range tests {
		t.Run(tt.operation, func(t *testing.T) {
			got := GetOperationDisplayName(tt.operation)
			if got != tt.want {
				t.Errorf("GetOperationDisplayName() = %v, want %v", got, tt.want)
			}
		})
	}
}

// TestApprovalManager_RequestApproval_NotStarted 测试未启动时的请求
func TestApprovalManager_RequestApproval_NotStarted(t *testing.T) {
	mgr := NewApprovalManager(nil)

	// 不启动管理器，直接请求审批
	req := &ApprovalRequest{
		RequestID:      "req-001",
		Operation:      OperationFileDelete,
		Target:         "/etc/passwd",
		RiskLevel:      RiskLevelCritical,
		Reason:         "Test",
		TimeoutSeconds: 30,
	}

	ctx := context.Background()
	_, err := mgr.RequestApproval(ctx, req)

	if err == nil {
		t.Error("should return error when manager is not running")
	}
}

// TestRiskLevel_Constants 测试风险级别常量
func TestRiskLevel_Constants(t *testing.T) {
	levels := []string{
		RiskLevelLow,
		RiskLevelMedium,
		RiskLevelHigh,
		RiskLevelCritical,
	}

	expected := []string{"LOW", "MEDIUM", "HIGH", "CRITICAL"}

	for i, level := range levels {
		if level != expected[i] {
			t.Errorf("risk level %d = %v, want %v", i, level, expected[i])
		}
	}
}

// TestApprovalContext_Cancel 测试上下文取消
func TestApprovalContext_Cancel(t *testing.T) {
	config := &ApprovalConfig{
		Enabled:       true,
		Timeout:       30 * time.Second,
		MinRiskLevel:  RiskLevelLow,
		DialogWidth:   550,
		DialogHeight:  480,
	}
	mgr := NewApprovalManager(config)

	if err := mgr.Start(); err != nil {
		t.Fatalf("failed to start manager: %v", err)
	}
	defer mgr.Stop()

	req := &ApprovalRequest{
		RequestID:      "req-cancel-001",
		Operation:      OperationFileDelete,
		Target:         "/etc/passwd",
		RiskLevel:      RiskLevelCritical,
		Reason:         "Test context cancellation",
		TimeoutSeconds: 30,
	}

	// 创建可取消的上下文
	ctx, cancel := context.WithCancel(context.Background())

	// 立即取消
	cancel()

	// 请求审批（应该立即返回错误）
	_, err := mgr.RequestApproval(ctx, req)

	if err == nil {
		t.Error("should return error when context is cancelled")
	}

	if err != context.Canceled {
		t.Errorf("error should be context.Canceled, got %v", err)
	}
}
