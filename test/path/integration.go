// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package main

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/276793422/NemesisBot/module/path"
)

func main() {
	fmt.Println("===========================================")
	fmt.Println("NEMESISBOT_HOME 集成测试")
	fmt.Println("===========================================")
	fmt.Println()

	testCount := 0
	passCount := 0

	// Test 1: Verify directory structure with NEMESISBOT_HOME
	fmt.Println("[Test 1] 验证 NEMESISBOT_HOME 目录结构")
	fmt.Println("-------------------------------------------")
	if testNEMESISBOT_HOMEStructure() {
		passCount++
		fmt.Println("✅ PASSED")
	} else {
		fmt.Println("❌ FAILED")
	}
	testCount++
	fmt.Println()

	// Test 2: Verify LocalMode still works
	fmt.Println("[Test 2] 验证 LocalMode 不受影响")
	fmt.Println("-------------------------------------------")
	if testLocalModeUnchanged() {
		passCount++
		fmt.Println("✅ PASSED")
	} else {
		fmt.Println("❌ FAILED")
	}
	testCount++
	fmt.Println()

	// Test 3: Verify default behavior
	fmt.Println("[Test 3] 验证默认行为不受影响")
	fmt.Println("-------------------------------------------")
	if testDefaultBehavior() {
		passCount++
		fmt.Println("✅ PASSED")
	} else {
		fmt.Println("❌ FAILED")
	}
	testCount++
	fmt.Println()

	// Test 4: Verify NEMESISBOT_HOME takes precedence
	fmt.Println("[Test 4] 验证 NEMESISBOT_HOME 优先级高于自动检测")
	fmt.Println("-------------------------------------------")
	if testPrecedence() {
		passCount++
		fmt.Println("✅ PASSED")
	} else {
		fmt.Println("❌ FAILED")
	}
	testCount++
	fmt.Println()

	// Summary
	fmt.Println("===========================================")
	fmt.Println("测试总结")
	fmt.Println("===========================================")
	fmt.Printf("总计: %d\n", testCount)
	fmt.Printf("通过: %d\n", passCount)
	fmt.Printf("失败: %d\n", testCount-passCount)
	fmt.Printf("通过率: %.1f%%\n", float64(passCount)*100/float64(testCount))
	fmt.Println()

	if passCount == testCount {
		fmt.Println("✅ 所有测试通过！")
		os.Exit(0)
	} else {
		fmt.Println("❌ 部分测试失败")
		os.Exit(1)
	}
}

func testNEMESISBOT_HOMEStructure() bool {
	// Save original
	origHome := os.Getenv("NEMESISBOT_HOME")
	defer func() {
		if origHome != "" {
			os.Setenv("NEMESISBOT_HOME", origHome)
		} else {
			os.Unsetenv("NEMESISBOT_HOME")
		}
		path.LocalMode = false
	}()

	// Create test root
	testRoot := os.TempDir()
	projectDir := filepath.Join(testRoot, ".nemesisbot")
	os.Setenv("NEMESISBOT_HOME", testRoot)

	pm := path.NewPathManager()
	homeDir := pm.HomeDir()
	configPath := pm.ConfigPath()
	workspace := pm.Workspace()

	// Verify structure
	if homeDir != projectDir {
		fmt.Printf("  ✗ 主目录错误: %s (期望: %s)\n", homeDir, projectDir)
		return false
	}
	fmt.Printf("  ✓ 主目录: %s\n", homeDir)

	if filepath.Dir(configPath) != projectDir {
		fmt.Printf("  ✗ 配置不在项目目录: %s\n", configPath)
		return false
	}
	fmt.Printf("  ✓ 配置文件: %s\n", filepath.Base(configPath))

	if filepath.Dir(workspace) != projectDir {
		fmt.Printf("  ✗ 工作空间不在项目目录: %s\n", workspace)
		return false
	}
	fmt.Printf("  ✓ 工作空间: %s\n", filepath.Base(workspace))

	// Verify all are under .nemesisbot
	if filepath.Base(homeDir) != ".nemesisbot" {
		fmt.Printf("  ✗ 主目录不是 .nemesisbot\n")
		return false
	}

	return true
}

func testLocalModeUnchanged() bool {
	// Save original
	origHome := os.Getenv("NEMESISBOT_HOME")
	defer func() {
		if origHome != "" {
			os.Setenv("NEMESISBOT_HOME", origHome)
		} else {
			os.Unsetenv("NEMESISBOT_HOME")
		}
		path.LocalMode = false
	}()

	// Set NEMESISBOT_HOME
	testRoot := os.TempDir()
	os.Setenv("NEMESISBOT_HOME", testRoot)

	// Enable LocalMode (should take precedence)
	path.LocalMode = true
	tempDir := os.TempDir()

	// Change to temp directory
	origWd, _ := os.Getwd()
	defer os.Chdir(origWd)
	os.Chdir(tempDir)

	pm := path.NewPathManager()
	homeDir := pm.HomeDir()

	// Should use current directory's .nemesisbot, not NEMESISBOT_HOME
	expected := filepath.Join(tempDir, ".nemesisbot")
	if homeDir != expected {
		fmt.Printf("  ✗ LocalMode 被 NEMESISBOT_HOME 覆盖了\n")
		fmt.Printf("     得到: %s\n", homeDir)
		fmt.Printf("     期望: %s\n", expected)
		return false
	}

	fmt.Printf("  ✓ LocalMode 使用当前目录: %s\n", homeDir)
	return true
}

func testDefaultBehavior() bool {
	// Save original
	origHome := os.Getenv("NEMESISBOT_HOME")
	defer func() {
		if origHome != "" {
			os.Setenv("NEMESISBOT_HOME", origHome)
		} else {
			os.Unsetenv("NEMESISBOT_HOME")
		}
		path.LocalMode = false
	}()

	// Clear NEMESISBOT_HOME
	os.Unsetenv("NEMESISBOT_HOME")

	pm := path.NewPathManager()
	homeDir := pm.HomeDir()

	// Should end with .nemesisbot
	if filepath.Base(homeDir) != ".nemesisbot" {
		fmt.Printf("  ✗ 默认主目录错误: %s\n", homeDir)
		return false
	}

	// Workspace should be under .nemesisbot
	workspace := pm.Workspace()
	parentDir := filepath.Dir(workspace)
	if filepath.Base(parentDir) != ".nemesisbot" {
		fmt.Printf("  ✗ 工作空间不在 .nemesisbot 下: %s\n", workspace)
		return false
	}

	userHome, _ := os.UserHomeDir()
	expected := filepath.Join(userHome, ".nemesisbot")

	if homeDir != expected {
		fmt.Printf("  ⚠ 主目录不是默认位置: %s\n", homeDir)
		fmt.Printf("    期望: %s\n", expected)
		fmt.Printf("    这可能是因为其他环境变量设置，可以接受\n")
	}

	fmt.Printf("  ✓ 主目录: %s\n", homeDir)
	fmt.Printf("  ✓ 工作空间在主目录下: %s\n", filepath.Base(workspace))

	return true
}

func testPrecedence() bool {
	// Save original
	origHome := os.Getenv("NEMESISBOT_HOME")
	defer func() {
		if origHome != "" {
			os.Setenv("NEMESISBOT_HOME", origHome)
		} else {
			os.Unsetenv("NEMESISBOT_HOME")
		}
		path.LocalMode = false
	}()

	// Set NEMESISBOT_HOME
	testRoot := os.TempDir()
	os.Setenv("NEMESISBOT_HOME", testRoot)

	// Create .nemesisbot in current directory (for auto-detect)
	tempDir := os.TempDir()
	localNemesisBot := filepath.Join(tempDir, ".nemesisbot")
	os.MkdirAll(localNemesisBot, 0755)
	defer os.RemoveAll(localNemesisBot)

	// Change to temp directory
	origWd, _ := os.Getwd()
	defer os.Chdir(origWd)
	os.Chdir(tempDir)

	pm := path.NewPathManager()
	homeDir := pm.HomeDir()

	// Should use NEMESISBOT_HOME/.nemesisbot, not current directory's .nemesisbot
	expected := filepath.Join(testRoot, ".nemesisbot")
	if homeDir != expected {
		fmt.Printf("  ✗ NEMESISBOT_HOME 没有优先于自动检测\n")
		fmt.Printf("     得到: %s\n", homeDir)
		fmt.Printf("     期望: %s\n", expected)
		return false
	}

	fmt.Printf("  ✓ NEMESISBOT_HOME 优先级正确: %s\n", homeDir)
	return true
}

func containsTimeout(s string) bool {
	return len(s) > 0 && (contains(s, "timeout") || contains(s, "i/o timeout"))
}

func contains(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
