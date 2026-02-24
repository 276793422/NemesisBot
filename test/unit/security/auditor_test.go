// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package security_test

import (
	"context"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
	. "github.com/276793422/NemesisBot/module/security"
)

func TestDangerLevel(t *testing.T) {
	tests := []struct {
		operation OperationType
		expected  DangerLevel
	}{
		{OpFileRead, DangerLow},
		{OpFileWrite, DangerHigh},
		{OpProcessExec, DangerCritical},
		{OpNetworkRequest, DangerMedium},
	}

	for _, tt := range tests {
		t.Run(string(tt.operation), func(t *testing.T) {
			level := GetDangerLevel(tt.operation)
			if level != tt.expected {
				t.Errorf("Operation %s: expected danger level %v, got %v", tt.operation, tt.expected, level)
			}
		})
	}
}

func TestNewSecurityAuditor(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	}
	auditor := NewSecurityAuditor(cfg)

	if auditor == nil {
		t.Fatal("NewSecurityAuditor returned nil")
	}
}

// TestSecurityFileRules 测试文件安全规则的实际决策
func TestSecurityFileRules(t *testing.T) {
	// 创建测试配置：允许 /workspace/**，拒绝 *.key
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	}
	auditor := NewSecurityAuditor(cfg)

	// 设置文件写入规则 - 注意顺序！具体的规则要放在前面
	auditor.SetRules(OpFileWrite, []config.SecurityRule{
		{Pattern: "*.key", Action: "deny"},        // 具体规则先匹配
		{Pattern: "/workspace/**", Action: "allow"}, // 通用规则后匹配
	})

	tests := []struct {
		name     string
		path     string
		expected bool // true=allowed, false=denied
	}{
		{
			name:     "允许访问 workspace 内的普通文件",
			path:     "/workspace/test.txt",
			expected: true,
		},
		{
			name:     "拒绝访问 workspace 内的 .key 文件（具体规则优先）",
			path:     "/workspace/secret.key",
			expected: false,
		},
		{
			name:     "拒绝访问 workspace 外的文件（默认拒绝）",
			path:     "/etc/passwd",
			expected: false,
		},
		{
			name:     "拒绝访问任意位置的 .key 文件",
			path:     "/tmp/private.key",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := &OperationRequest{
				Type:        OpFileWrite,
				DangerLevel: GetDangerLevel(OpFileWrite),
				User:        "test-user",
				Source:      "test",
				Target:      tt.path,
			}

			allowed, _, _ := auditor.RequestPermission(context.Background(), req)
			if allowed != tt.expected {
				t.Errorf("路径 %s: 期望 %v, 得到 %v", tt.path, tt.expected, allowed)
			}
		})
	}
}

// TestSecurityDirectoryRules 测试目录安全规则的实际决策
func TestSecurityDirectoryRules(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	}
	auditor := NewSecurityAuditor(cfg)

	// 设置目录读取规则
	auditor.SetRules(OpDirRead, []config.SecurityRule{
		{Pattern: "/workspace/**", Action: "allow"},
	})
	auditor.SetRules(OpDirCreate, []config.SecurityRule{
		{Pattern: "/workspace/tmp/**", Action: "allow"},
	})

	tests := []struct {
		name     string
		op       OperationType
		path     string
		expected bool
	}{
		{
			name:     "允许读取 workspace 内的目录",
			op:       OpDirRead,
			path:     "/workspace/src",
			expected: true,
		},
		{
			name:     "拒绝读取 workspace 外的目录",
			op:       OpDirRead,
			path:     "/etc",
			expected: false,
		},
		{
			name:     "允许在 /workspace/tmp 创建目录",
			op:       OpDirCreate,
			path:     "/workspace/tmp/newdir",
			expected: true,
		},
		{
			name:     "拒绝在 /workspace 创建目录（只有 tmp 可以）",
			op:       OpDirCreate,
			path:     "/workspace/newdir",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := &OperationRequest{
				Type:        tt.op,
				DangerLevel: GetDangerLevel(tt.op),
				User:        "test-user",
				Source:      "test",
				Target:      tt.path,
			}

			allowed, _, _ := auditor.RequestPermission(context.Background(), req)
			if allowed != tt.expected {
				t.Errorf("操作 %s on %s: 期望 %v, 得到 %v", tt.op, tt.path, tt.expected, allowed)
			}
		})
	}
}

// TestSecurityWildcardPatterns 测试通配符模式匹配
func TestSecurityWildcardPatterns(t *testing.T) {
	cfg := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	}
	auditor := NewSecurityAuditor(cfg)

	// 设置复杂的通配符规则
	auditor.SetRules(OpFileRead, []config.SecurityRule{
		{Pattern: "/workspace/**/*.log", Action: "allow"},
		{Pattern: "/workspace/**/*.txt", Action: "allow"},
		{Pattern: "C:\\Users\\**\\*.log", Action: "deny"}, // Windows 路径
	})

	tests := []struct {
		name     string
		path     string
		expected bool
	}{
		{
			name:     "通配符 ** 匹配多级目录 - .log 文件",
			path:     "/workspace/a/b/c/app.log",
			expected: true,
		},
		{
			name:     "通配符 ** 匹配多级目录 - .txt 文件",
			path:     "/workspace/src/config.txt",
			expected: true,
		},
		{
			name:     "不匹配 .md 文件",
			path:     "/workspace/README.md",
			expected: false,
		},
		{
			name:     "拒绝 Windows 路径模式",
			path:     "C:\\Users\\test\\app.log",
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := &OperationRequest{
				Type:        OpFileRead,
				DangerLevel: GetDangerLevel(OpFileRead),
				User:        "test-user",
				Source:      "test",
				Target:      tt.path,
			}

			allowed, _, _ := auditor.RequestPermission(context.Background(), req)
			if allowed != tt.expected {
				t.Errorf("路径 %s: 期望 %v, 得到 %v", tt.path, tt.expected, allowed)
			}
		})
	}
}

// TestSecurityDefaultAction 测试默认动作
func TestSecurityDefaultAction(t *testing.T) {
	// 测试默认允许
	cfgAllow := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "allow",
	}
	auditorAllow := NewSecurityAuditor(cfgAllow)

	req := &OperationRequest{
		Type:        OpFileRead,
		DangerLevel: GetDangerLevel(OpFileRead),
		User:        "test-user",
		Source:      "test",
		Target:      "/any/random/path.txt",
	}

	allowed, _, _ := auditorAllow.RequestPermission(context.Background(), req)
	if !allowed {
		t.Error("默认允许模式：应该允许所有操作")
	}

	// 测试默认拒绝
	cfgDeny := &AuditorConfig{
		Enabled:       true,
		DefaultAction: "deny",
	}
	auditorDeny := NewSecurityAuditor(cfgDeny)

	allowed2, _, _ := auditorDeny.RequestPermission(context.Background(), req)
	if allowed2 {
		t.Error("默认拒绝模式：应该拒绝所有操作")
	}
}
