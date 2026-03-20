package security

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/security/approval"
)

// TestSecurityAuditor_RequireApprovalFlow tests the complete approval flow
func TestSecurityAuditor_RequireApprovalFlow(t *testing.T) {
	// Create an approval manager
	approvalConfig := &approval.ApprovalConfig{
		Enabled:       true,
		Timeout:       5 * time.Second,
		MinRiskLevel:  "HIGH",
		DialogWidth:   550,
		DialogHeight:  480,
	}
	approvalMgr := approval.NewApprovalManager(approvalConfig)

	// Start the approval manager
	if err := approvalMgr.Start(); err != nil {
		t.Fatalf("failed to start approval manager: %v", err)
	}
	defer approvalMgr.Stop()

	// Create security auditor
	auditorConfig := &AuditorConfig{
		Enabled:               true,
		LogAllOperations:      true,
		ApprovalTimeout:       30 * time.Second,
		MaxPendingRequests:    100,
		AuditLogRetentionDays: 30,
		DefaultAction:         "deny",
	}

	auditor := NewSecurityAuditor(auditorConfig)
	defer auditor.Close()

	// Set the approval manager
	auditor.SetApprovalManager(approvalMgr)

	// Set up rules that require approval for critical operations
	rules := []config.SecurityRule{
		{
			Pattern: ".*",
			Action:  "ask",
		},
	}
	auditor.SetRules(OpFileDelete, rules)

	// Test 1: Critical operation should trigger approval dialog
	t.Run("Critical operation triggers approval", func(t *testing.T) {
		req := &OperationRequest{
			Type:        OpFileDelete,
			DangerLevel: DangerCritical,
			User:        "test-user",
			Source:      "test",
			Target:      "/etc/passwd",
			Context: map[string]interface{}{
				"reason": "System critical file",
			},
		}

		ctx := context.Background()
		allowed, err, requestID := auditor.RequestPermission(ctx, req)

		// In test mode, the dialog will timeout
		if allowed {
			t.Error("operation should not be allowed (timeout expected)")
		}

		if err == nil {
			t.Error("expected error due to timeout")
		}

		if requestID == "" {
			t.Error("request ID should be generated")
		}

		t.Logf("Critical operation test: allowed=%v, error=%v, requestID=%s", allowed, err, requestID)
	})

	// Test 2: Lower risk operation should be denied (default action)
	t.Run("Lower risk operation uses default action", func(t *testing.T) {
		req := &OperationRequest{
			Type:        OpFileRead,
			DangerLevel: DangerLow,
			User:        "test-user",
			Source:      "test",
			Target:      "/home/user/file.txt",
		}

		ctx := context.Background()
		allowed, err, requestID := auditor.RequestPermission(ctx, req)

		// Should be denied by default action
		if allowed {
			t.Error("operation should not be allowed (default deny)")
		}

		if err == nil {
			t.Error("expected error for denied operation")
		}

		t.Logf("Lower risk operation test: allowed=%v, error=%v, requestID=%s", allowed, err, requestID)
	})
}

// TestSecurityAuditor_ApprovalManagerContextCancellation tests context cancellation during approval
func TestSecurityAuditor_ApprovalManagerContextCancellation(t *testing.T) {
	// Create an approval manager
	approvalConfig := &approval.ApprovalConfig{
		Enabled:       true,
		Timeout:       30 * time.Second,
		MinRiskLevel:  "HIGH",
		DialogWidth:   550,
		DialogHeight:  480,
	}
	approvalMgr := approval.NewApprovalManager(approvalConfig)

	// Start the approval manager
	if err := approvalMgr.Start(); err != nil {
		t.Fatalf("failed to start approval manager: %v", err)
	}
	defer approvalMgr.Stop()

	// Create security auditor
	auditorConfig := &AuditorConfig{
		Enabled:               true,
		LogAllOperations:      true,
		ApprovalTimeout:       30 * time.Second,
		MaxPendingRequests:    100,
		AuditLogRetentionDays: 30,
		DefaultAction:         "deny",
	}

	auditor := NewSecurityAuditor(auditorConfig)
	defer auditor.Close()

	// Set the approval manager
	auditor.SetApprovalManager(approvalMgr)

	// Set up rules that require approval
	rules := []config.SecurityRule{
		{
			Pattern: ".*",
			Action:  "ask",
		},
	}
	auditor.SetRules(OpFileDelete, rules)

	// Test context cancellation
	req := &OperationRequest{
		Type:        OpFileDelete,
		DangerLevel: DangerCritical,
		User:        "test-user",
		Source:      "test",
		Target:      "/etc/passwd",
	}

	// Create a cancellable context
	ctx, cancel := context.WithCancel(context.Background())

	// Cancel immediately
	cancel()

	// Request permission (should fail immediately)
	allowed, err, requestID := auditor.RequestPermission(ctx, req)

	if allowed {
		t.Error("operation should not be allowed")
	}

	if err == nil {
		t.Error("expected error due to context cancellation")
	}

	if err == context.Canceled {
		t.Logf("Correctly received context.Canceled error")
	}

	t.Logf("Context cancellation test: allowed=%v, error=%v, requestID=%s", allowed, err, requestID)
}

// TestSecurityAuditor_ApprovalManagerReplacement tests replacing the approval manager
func TestSecurityAuditor_ApprovalManagerReplacement(t *testing.T) {
	// Create first approval manager
	approvalConfig1 := &approval.ApprovalConfig{
		Enabled:       true,
		Timeout:       5 * time.Second,
		MinRiskLevel:  "HIGH",
		DialogWidth:   550,
		DialogHeight:  480,
	}
	approvalMgr1 := approval.NewApprovalManager(approvalConfig1)

	// Create security auditor
	auditorConfig := &AuditorConfig{
		Enabled:               true,
		LogAllOperations:      true,
		ApprovalTimeout:       30 * time.Second,
		MaxPendingRequests:    100,
		AuditLogRetentionDays: 30,
		DefaultAction:         "deny",
	}

	auditor := NewSecurityAuditor(auditorConfig)
	defer auditor.Close()

	// Set first approval manager
	auditor.SetApprovalManager(approvalMgr1)
	if auditor.GetApprovalManager() != approvalMgr1 {
		t.Error("first approval manager should be set")
	}

	// Create second approval manager
	approvalConfig2 := &approval.ApprovalConfig{
		Enabled:       true,
		Timeout:       10 * time.Second,
		MinRiskLevel:  "MEDIUM",
		DialogWidth:   800,
		DialogHeight:  480,
	}
	approvalMgr2 := approval.NewApprovalManager(approvalConfig2)

	// Replace with second approval manager
	auditor.SetApprovalManager(approvalMgr2)
	if auditor.GetApprovalManager() != approvalMgr2 {
		t.Error("second approval manager should be set")
	}

	t.Log("Successfully replaced approval manager")
}

// TestSecurityAuditor_ApprovalManagerNil tests setting nil approval manager
func TestSecurityAuditor_ApprovalManagerNil(t *testing.T) {
	// Create security auditor
	auditorConfig := &AuditorConfig{
		Enabled:               true,
		LogAllOperations:      true,
		ApprovalTimeout:       30 * time.Second,
		MaxPendingRequests:    100,
		AuditLogRetentionDays: 30,
		DefaultAction:         "deny",
	}

	auditor := NewSecurityAuditor(auditorConfig)
	defer auditor.Close()

	// Set nil approval manager (should be allowed)
	auditor.SetApprovalManager(nil)
	if auditor.GetApprovalManager() != nil {
		t.Error("approval manager should be nil")
	}

	t.Log("Successfully set nil approval manager")
}
