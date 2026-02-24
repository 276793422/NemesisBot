// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package security_test

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/config"
)

// TestMatchPattern 测试通配符模式匹配逻辑
func TestMatchPattern(t *testing.T) {
	tests := []struct {
		pattern  string
		target   string
		expected bool
	}{
		// 精确匹配
		{"/workspace/test.txt", "/workspace/test.txt", true},
		{"/workspace/test.txt", "/workspace/other.txt", false},

		// * 通配符
		{"*.log", "app.log", true},
		{"*.log", "app.txt", false},
		{"test.*", "test.txt", true},
		{"test.*", "test.log", true},
		{"*.key", "secret.key", true},
		{"*.key", "/workspace/secret.key", false}, // * 不匹配包含分隔符的路径

		// ** 通配符（多级目录）
		{"/workspace/**", "/workspace/test.txt", true},
		{"/workspace/**", "/workspace/a/b/c/test.txt", true},
		{"/workspace/**", "/other/test.txt", false},
		{"/**", "/any/path/test.txt", true},

		// 混合通配符 - ** 匹配多级目录
		{"/workspace/**/*.log", "/workspace/a/b/app.log", true},
		{"/workspace/**/*.log", "/workspace/app.log", false}, // 没有中间目录
		{"/workspace/**/*.log", "/workspace/a/b/app.txt", false},

		// Windows 路径
		{"C:\\Users\\**", "C:\\Users\\test\\file.txt", true},
		{"C:\\Users\\*", "C:\\Users\\test", true}, // * 匹配没有分隔符的字符串
	}

	for _, tt := range tests {
		t.Run(tt.pattern+"|"+tt.target, func(t *testing.T) {
			result := matchPattern(tt.pattern, tt.target)
			if result != tt.expected {
				t.Errorf("Pattern %s matching %s: expected %v, got %v", tt.pattern, tt.target, tt.expected, result)
			}
		})
	}
}

// matchPattern 是从 main.go 复制的辅助函数
func matchPattern(pattern, target string) bool {
	// Simple wildcard matching
	// ** matches any number of directories (at least one if between prefix and suffix)
	// * matches any sequence of characters (not including / or \)

	patternParts := strings.Split(pattern, "**")
	if len(patternParts) > 1 {
		// Has ** wildcard
		prefix := patternParts[0]
		suffix := patternParts[1]

		// Check prefix
		if prefix != "" && !strings.HasPrefix(target, prefix) {
			return false
		}

		// For patterns like "/workspace/**", suffix might be empty
		if suffix == "" {
			return true
		}

		// Handle suffix that may contain * wildcard (e.g., "/*.log")
		if strings.HasPrefix(suffix, "/") || strings.HasPrefix(suffix, "\\") {
			// Suffix starts with a separator - ** should have matched directories, now check filename
			// Extract the filename from target (last component)
			lastSlash := strings.LastIndexAny(target, "/\\")
			if lastSlash == -1 {
				return false
			}
			filename := target[lastSlash+1:]

			// Suffix is like "/*.log" - remove leading / and match filename with pattern
			suffixPattern := suffix[1:] // Remove leading /
			if strings.Contains(suffixPattern, "*") {
				// Handle * in suffix pattern
				parts := strings.Split(suffixPattern, "*")
				if len(parts) == 2 {
					// Pattern is like "*.log" - check if filename starts and ends with parts
					if parts[0] != "" && !strings.HasPrefix(filename, parts[0]) {
						return false
					}
					if parts[1] != "" && !strings.HasSuffix(filename, parts[1]) {
						return false
					}
					// Middle part should not contain separators (already checked since we're using filename)
				}
			} else {
				// No wildcard in suffix pattern
				if filename != suffixPattern {
					return false
				}
			}

			// Check that there's at least one directory between prefix and filename
			// Get the part of the path between the prefix and the filename
			afterPrefix := strings.TrimPrefix(target, prefix)
			// Remove filename from afterPrefix to get just the directory path
			dirPath := strings.TrimSuffix(afterPrefix, filename)
			// Check if dirPath contains at least one separator (meaning there's at least one directory level)
			if !strings.Contains(dirPath, "/") && !strings.Contains(dirPath, "\\") {
				return false
			}
		} else {
			// Suffix doesn't start with separator - treat as literal suffix
			if !strings.HasSuffix(target, suffix) {
				return false
			}

			// For ** in the middle, verify there's at least one directory level between
			middle := strings.TrimPrefix(target, prefix)
			middle = strings.TrimSuffix(middle, suffix)
			// Must contain at least one path separator to represent a directory level
			if !strings.Contains(middle, "/") && !strings.Contains(middle, "\\") {
				return false
			}
		}

		return true
	}

	// No ** wildcard, check for * wildcard
	if strings.Contains(pattern, "*") {
		// Simple * wildcard - should not match path separators
		parts := strings.Split(pattern, "*")
		prefix := parts[0]
		suffix := ""
		if len(parts) > 1 {
			suffix = parts[1]
		}

		// Check if target has the prefix
		if !strings.HasPrefix(target, prefix) {
			return false
		}
		if suffix != "" && !strings.HasSuffix(target, suffix) {
			return false
		}

		// Extract the middle part between prefix and suffix
		middle := strings.TrimPrefix(target, prefix)
		if suffix != "" {
			middle = strings.TrimSuffix(middle, suffix)
		}

		// The middle part should not contain path separators
		if strings.Contains(middle, "/") || strings.Contains(middle, "\\") {
			return false
		}

		return true
	}

	// No wildcards, exact match
	return target == pattern
}

// TestSecurityRules_AddRules 测试添加规则
func TestSecurityRules_AddRules(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.security.json")

	// 创建基础配置
	cfg := &config.SecurityConfig{
		FileRules: &config.FileSecurityRules{
			Read: []config.SecurityRule{
				{Pattern: "/workspace/", Action: "allow"},
			},
		},
	}

	// 保存初始配置
	err := config.SaveSecurityConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// 添加第一条规则
	newRule1 := config.SecurityRule{Pattern: "*.log", Action: "ask"}
	cfg.FileRules.Read = append(cfg.FileRules.Read, newRule1)
	err = config.SaveSecurityConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// 重新加载并验证
	cfg2, err := config.LoadSecurityConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	if len(cfg2.FileRules.Read) != 2 {
		t.Errorf("Expected 2 read rules, got %d", len(cfg2.FileRules.Read))
	}

	// 验证规则内容
	if cfg2.FileRules.Read[1].Pattern != "*.log" {
		t.Errorf("Expected pattern '*.log', got '%s'", cfg2.FileRules.Read[1].Pattern)
	}
	if cfg2.FileRules.Read[1].Action != "ask" {
		t.Errorf("Expected action 'ask', got '%s'", cfg2.FileRules.Read[1].Action)
	}
}

// TestSecurityRules_RemoveRules 测试删除规则
func TestSecurityRules_RemoveRules(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.security.json")

	// 创建包含多条规则的配置
	cfg := &config.SecurityConfig{
		FileRules: &config.FileSecurityRules{
			Write: []config.SecurityRule{
				{Pattern: "/workspace/**", Action: "allow"},
				{Pattern: "*.key", Action: "deny"},
				{Pattern: "/etc/**", Action: "deny"},
			},
		},
	}

	// 保存配置
	err := config.SaveSecurityConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// 删除第 1 条规则
	if len(cfg.FileRules.Write) < 2 {
		t.Fatalf("Not enough rules to test removal")
	}

	cfg.FileRules.Write = append(cfg.FileRules.Write[:1], cfg.FileRules.Write[2:]...)
	err = config.SaveSecurityConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// 重新加载并验证
	cfg2, err := config.LoadSecurityConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	if len(cfg2.FileRules.Write) != 2 {
		t.Errorf("Expected 2 write rules after removal, got %d", len(cfg2.FileRules.Write))
	}

	// 验证删除的是正确的规则
	if cfg2.FileRules.Write[1].Pattern != "/etc/**" {
		t.Errorf("Expected second rule to be '/etc/**', got '%s'", cfg2.FileRules.Write[1].Pattern)
	}
}

// TestSecurityRules_RuleOrdering 测试规则顺序优先级
func TestSecurityRules_RuleOrdering(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.security.json")

	// 创建规则：具体路径在前，通用通配符在后
	cfg := &config.SecurityConfig{
		FileRules: &config.FileSecurityRules{
			Write: []config.SecurityRule{
				{Pattern: "/workspace/*.key", Action: "deny"},  // 具体路径模式
				{Pattern: "/workspace/**", Action: "allow"},      // 通用通配符
			},
		},
	}

	err := config.SaveSecurityConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// 测试匹配逻辑
	tests := []struct {
		target   string
		expected bool // true=allowed, false=denied
	}{
		{"/workspace/test.txt", true},      // 匹配通用规则 allow
		{"/workspace/secret.key", false},  // 匹配具体规则 deny
		{"/tmp/private.key", false},       // 不匹配任何规则（默认拒绝）
		{"/etc/passwd", false},             // 不匹配任何规则（默认拒绝）
	}

	for _, tt := range tests {
		t.Run(tt.target, func(t *testing.T) {
			allowed, _ := checkRules(cfg, "file_write", tt.target, "HIGH")
			if allowed != tt.expected {
				t.Errorf("Path %s: expected %v, got %v", tt.target, tt.expected, allowed)
			}
		})
	}
}

// TestSecurityRules_InvalidAction 测试无效的动作
func TestSecurityRules_InvalidAction(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.security.json")

	cfg := &config.SecurityConfig{
		FileRules: &config.FileSecurityRules{
			Read: []config.SecurityRule{},
		},
	}

	// 尝试添加无效动作
	invalidRule := config.SecurityRule{Pattern: "/test/**", Action: "invalid"}
	cfg.FileRules.Read = append(cfg.FileRules.Read, invalidRule)

	// 保存和加载应该能成功（配置不验证动作有效性）
	err := config.SaveSecurityConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save config with invalid action: %v", err)
	}

	// 验证可以加载
	cfg2, err := config.LoadSecurityConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	if len(cfg2.FileRules.Read) != 1 {
		t.Error("Rule should be saved even with invalid action")
	}
}

// TestSecurityRules_ConfigPersistence 测试配置持久化
func TestSecurityRules_ConfigPersistence(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.security.json")

	// 创建完整配置
	cfg := &config.SecurityConfig{
		DefaultAction:         "deny",
		LogAllOperations:      true,
		FileRules: &config.FileSecurityRules{
			Read: []config.SecurityRule{
				{Pattern: "/workspace/**", Action: "allow"},
			},
			Write: []config.SecurityRule{
				{Pattern: "/workspace/**", Action: "allow"},
				{Pattern: "*.key", Action: "deny"},
			},
		},
		DirectoryRules: &config.DirectorySecurityRules{
			Read: []config.SecurityRule{
				{Pattern: "/workspace/**", Action: "allow"},
			},
			Create: []config.SecurityRule{
				{Pattern: "/workspace/tmp/**", Action: "allow"},
			},
		},
	}

	// 保存
	err := config.SaveSecurityConfig(configPath, cfg)
	if err != nil {
		t.Fatalf("Failed to save config: %v", err)
	}

	// 验证文件存在
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		t.Fatal("Config file was not created")
	}

	// 重新加载
	cfg2, err := config.LoadSecurityConfig(configPath)
	if err != nil {
		t.Fatalf("Failed to load config: %v", err)
	}

	// 验证所有内容都正确保存
	if cfg2.DefaultAction != cfg.DefaultAction {
		t.Errorf("DefaultAction mismatch: expected %s, got %s", cfg.DefaultAction, cfg2.DefaultAction)
	}

	if len(cfg2.FileRules.Write) != 2 {
		t.Errorf("FileRules.Write count mismatch: expected 2, got %d", len(cfg2.FileRules.Write))
	}

	if cfg2.FileRules.Write[1].Pattern != "*.key" {
		t.Errorf("Rule not preserved: expected '*.key', got '%s'", cfg2.FileRules.Write[1].Pattern)
	}
}

// checkRules 模拟规则检查逻辑（从 main.go 复制并简化）
func checkRules(cfg *config.SecurityConfig, opType, target string, dangerLevel string) (bool, string) {
	var ruleCategory interface{}

	switch opType {
	case "file_read":
		if cfg.FileRules == nil || len(cfg.FileRules.Read) == 0 {
			return false, "No file read rules configured"
		}
		ruleCategory = cfg.FileRules.Read
	case "file_write":
		if cfg.FileRules == nil || len(cfg.FileRules.Write) == 0 {
			return false, "No file write rules configured"
		}
		ruleCategory = cfg.FileRules.Write
	case "file_delete":
		if cfg.FileRules == nil || len(cfg.FileRules.Delete) == 0 {
			return false, "No file delete rules configured"
		}
		ruleCategory = cfg.FileRules.Delete
	case "dir_read":
		if cfg.DirectoryRules == nil || len(cfg.DirectoryRules.Read) == 0 {
			return false, "No directory read rules configured"
		}
		ruleCategory = cfg.DirectoryRules.Read
	case "dir_create":
		if cfg.DirectoryRules == nil || len(cfg.DirectoryRules.Create) == 0 {
			return false, "No directory create rules configured"
		}
		ruleCategory = cfg.DirectoryRules.Create
	case "dir_delete":
		if cfg.DirectoryRules == nil || len(cfg.DirectoryRules.Delete) == 0 {
			return false, "No directory delete rules configured"
		}
		ruleCategory = cfg.DirectoryRules.Delete
	default:
		return false, "Unknown operation type"
	}

	rules, ok := ruleCategory.([]config.SecurityRule)
	if !ok {
		return false, "Invalid rule format"
	}

	// 按顺序检查规则（第一个匹配的获胜）
	for i, rule := range rules {
		matched := matchPattern(rule.Pattern, target)
		if matched {
			if rule.Action == "allow" {
				return true, fmt.Sprintf("Matched rule [%d]: %s → allow", i, rule.Pattern)
			}
			return false, fmt.Sprintf("Matched rule [%d]: %s → deny", i, rule.Pattern)
		}
	}

	return false, "No matching rule found"
}
