// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package security_test

import (
	"context"
	"testing"

	. "github.com/276793422/NemesisBot/module/config"
	. "github.com/276793422/NemesisBot/module/security"
)

// TestAskActionMapping 测试 ask 动作被正确映射为 deny
func TestAskActionMapping(t *testing.T) {
	auditor := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	})

	// 设置一个 ask 规则
	auditor.SetRules(OpFileRead, []SecurityRule{
		{Pattern: "*.log", Action: "ask"},
	})

	tests := []struct {
		name     string
		target   string
		expected bool // true=allowed, false=denied
	}{
		{
			name:     "ask 规则应该阻止操作",
			target:   "app.log",
			expected: false, // ask 被映射为 deny
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := &OperationRequest{
				Type:        OpFileRead,
				DangerLevel: GetDangerLevel(OpFileRead),
				User:        "test-user",
				Source:      "test",
				Target:      tt.target,
			}

			allowed, _, _ := auditor.RequestPermission(context.Background(), req)
			if allowed != tt.expected {
				t.Errorf("操作 %s: 期望 %v, 得到 %v", tt.target, tt.expected, allowed)
			}
		})
	}
}

// TestAskVsDenyComparison 测试 ask 和 deny 行为一致
func TestAskVsDenyComparison(t *testing.T) {
	// 创建两个审计器，一个使用 ask，一个使用 deny
	auditorAsk := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	})
	auditorAsk.SetRules(OpFileRead, []SecurityRule{
		{Pattern: "*.log", Action: "ask"},
	})

	auditorDeny := NewSecurityAuditor(&AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	})
	auditorDeny.SetRules(OpFileRead, []SecurityRule{
		{Pattern: "*.log", Action: "deny"},
	})

	target := "app.log"

	reqAsk := &OperationRequest{
		Type:        OpFileRead,
		DangerLevel: GetDangerLevel(OpFileRead),
		User:        "test-user",
		Source:      "test",
		Target:      target,
	}

	reqDeny := &OperationRequest{
		Type:        OpFileRead,
		DangerLevel: GetDangerLevel(OpFileRead),
		User:        "test-user",
		Source:      "test",
		Target:      target,
	}

	allowedAsk, _, _ := auditorAsk.RequestPermission(context.Background(), reqAsk)
	allowedDeny, _, _ := auditorDeny.RequestPermission(context.Background(), reqDeny)

	// ask 和 deny 应该产生相同的结果
	if allowedAsk != allowedDeny {
		t.Errorf("ask 和 deny 行为不一致: ask=%v, deny=%v", allowedAsk, allowedDeny)
	}

	// 两者都应该被阻止
	if allowedAsk {
		t.Error("ask 规则没有阻止操作（期望被映射为 deny）")
	}
}
