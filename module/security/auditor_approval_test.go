package security

import (
	"context"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/security/approval"
)

// TestSecurityAuditor_WithApprovalManager tests the integration of SecurityAuditor with ApprovalManager
func TestSecurityAuditor_WithApprovalManager(t *testing.T) {
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

	// Verify the approval manager is set
	if auditor.GetApprovalManager() == nil {
		t.Error("approval manager should be set")
	}

	// Test a request that requires approval
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

	// This should require approval and trigger the dialog (in test mode, it will timeout)
	ctx := context.Background()
	allowed, err, requestID := auditor.RequestPermission(ctx, req)

	// In test mode without actual UI, the dialog will timeout
	if allowed {
		t.Error("operation should not be allowed (timeout expected)")
	}

	if err == nil {
		t.Error("expected error due to timeout")
	}

	if requestID == "" {
		t.Error("request ID should be generated")
	}

	t.Logf("Request completed: allowed=%v, error=%v, requestID=%s", allowed, err, requestID)
}

// TestSecurityAuditor_ApprovalManagerNotStarted tests behavior when approval manager is not started
func TestSecurityAuditor_ApprovalManagerNotStarted(t *testing.T) {
	// Create an approval manager but don't start it
	approvalConfig := &approval.ApprovalConfig{
		Enabled:       true,
		Timeout:       5 * time.Second,
		MinRiskLevel:  "HIGH",
		DialogWidth:   550,
		DialogHeight:  480,
	}
	approvalMgr := approval.NewApprovalManager(approvalConfig)

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

	// Set the approval manager (not started)
	auditor.SetApprovalManager(approvalMgr)

	// Configure rules to require approval
	rules := []config.SecurityRule{
		{
			Pattern: "/**",  // Match all paths
			Action:  "ask",
		},
	}
	auditor.SetRules(OpFileDelete, rules)

	// Test a request that requires approval
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

	// Should fall back to pending request behavior
	if allowed {
		t.Error("operation should not be allowed")
	}

	if err == nil {
		t.Error("expected ApprovalRequiredError")
	}

	// Should be stored as pending request
	pending := auditor.GetPendingRequests()
	if len(pending) != 1 {
		t.Errorf("expected 1 pending request, got %d", len(pending))
	}

	t.Logf("Request completed: allowed=%v, error=%v, requestID=%s", allowed, err, requestID)
}

// TestSecurityAuditor_NoApprovalManager tests behavior without approval manager
func TestSecurityAuditor_NoApprovalManager(t *testing.T) {
	// Create security auditor without approval manager
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

	// Verify no approval manager is set
	if auditor.GetApprovalManager() != nil {
		t.Error("approval manager should not be set")
	}

	// Configure rules to require approval
	rules := []config.SecurityRule{
		{
			Pattern: "/**",  // Match all paths
			Action:  "ask",
		},
	}
	auditor.SetRules(OpFileDelete, rules)

	// Test a request that requires approval
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

	// Should use standard pending request behavior
	if allowed {
		t.Error("operation should not be allowed")
	}

	if err == nil {
		t.Error("expected ApprovalRequiredError")
	}

	// Should be stored as pending request
	pending := auditor.GetPendingRequests()
	if len(pending) != 1 {
		t.Errorf("expected 1 pending request, got %d", len(pending))
	}

	// Verify we can approve it manually
	approveErr := auditor.ApproveRequest(requestID, "admin")
	if approveErr != nil {
		t.Errorf("failed to approve request: %v", approveErr)
	}

	t.Logf("Request completed: allowed=%v, error=%v, requestID=%s", allowed, err, requestID)
}

// TestConvertContextToStringMap tests the context conversion helper
func TestConvertContextToStringMap(t *testing.T) {
	tests := []struct {
		name     string
		input    map[string]interface{}
		expected map[string]string
	}{
		{
			name:     "empty map",
			input:    map[string]interface{}{},
			expected: map[string]string{},
		},
		{
			name: "simple values",
			input: map[string]interface{}{
				"key1": "value1",
				"key2": 42,
				"key3": true,
			},
			expected: map[string]string{
				"key1": "value1",
				"key2": "42",
				"key3": "true",
			},
		},
		{
			name: "complex values",
			input: map[string]interface{}{
				"map": map[string]string{"subkey": "subvalue"},
				"slice": []int{1, 2, 3},
			},
			expected: map[string]string{
				"map":   "map[subkey:subvalue]",
				"slice": "[1 2 3]",
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := convertContextToStringMap(tt.input)

			if len(result) != len(tt.expected) {
				t.Errorf("expected %d items, got %d", len(tt.expected), len(result))
			}

			for k, expectedVal := range tt.expected {
				if result[k] != expectedVal {
					t.Errorf("key %s: expected %q, got %q", k, expectedVal, result[k])
				}
			}
		})
	}
}
