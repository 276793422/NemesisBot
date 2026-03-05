// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package tools

import (
	"context"
	"fmt"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/config"
)

// AsyncExecTool executes commands asynchronously (starts them and returns quickly)
type AsyncExecTool struct {
	workingDir          string
	defaultWaitTime     time.Duration
	denyPatterns        []*regexp.Regexp
	allowPatterns       []*regexp.Regexp
	restrictToWorkspace bool
}

// NewAsyncExecTool creates a new AsyncExecTool
func NewAsyncExecTool(workingDir string, restrict bool) *AsyncExecTool {
	return NewAsyncExecToolWithConfig(workingDir, restrict, nil)
}

// NewAsyncExecToolWithConfig creates a new AsyncExecTool with custom configuration
func NewAsyncExecToolWithConfig(workingDir string, restrict bool, config *config.Config) *AsyncExecTool {
	denyPatterns := make([]*regexp.Regexp, 0)

	enableDenyPatterns := true
	if config != nil {
		execConfig := config.Tools.Exec
		enableDenyPatterns = execConfig.EnableDenyPatterns
		if enableDenyPatterns {
			if len(execConfig.CustomDenyPatterns) > 0 {
				for _, pattern := range execConfig.CustomDenyPatterns {
					re, err := regexp.Compile(pattern)
					if err != nil {
						continue
					}
					denyPatterns = append(denyPatterns, re)
				}
			} else {
				denyPatterns = append(denyPatterns, defaultDenyPatterns...)
			}
		}
	} else {
		denyPatterns = append(denyPatterns, defaultDenyPatterns...)
	}

	return &AsyncExecTool{
		workingDir:          workingDir,
		defaultWaitTime:     3 * time.Second,
		denyPatterns:        denyPatterns,
		allowPatterns:       nil,
		restrictToWorkspace: restrict,
	}
}

func (t *AsyncExecTool) Name() string {
	return "exec_async"
}

func (t *AsyncExecTool) Description() string {
	return `启动应用程序，等待确认启动成功后立即返回。

适用场景：
- 打开 GUI 应用程序（如：notepad.exe, calc.exe, mspaint.exe）
- 启动编辑器（如：code.exe, sublime_text.exe）
- 打开文件管理器（如：explorer.exe）
- 任何不需要等待退出结果的应用程序

行为说明：
- 此工具会启动应用程序，等待 3-5 秒确认启动成功
- 如果应用程序仍在运行，返回成功消息
- 如果应用程序已经退出（闪退），返回失败消息
- 不会等待应用程序退出，可以立即继续执行其他任务

参数：
- command (必需): 要执行的命令
- working_dir (可选): 工作目录
- wait_seconds (可选): 等待确认的秒数（默认 3，范围 1-10）

示例：
- exec_async(command="notepad.exe")
- exec_async(command="calc.exe", wait_seconds=5)
- exec_async(command="notepad.exe README.md")`
}

func (t *AsyncExecTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"command": map[string]interface{}{
				"type":        "string",
				"description": "要启动的应用程序命令",
			},
			"working_dir": map[string]interface{}{
				"type":        "string",
				"description": "工作目录（可选）",
			},
			"wait_seconds": map[string]interface{}{
				"type":        "integer",
				"description": "等待确认启动成功的秒数（默认 3，范围 1-10）",
				"default":     3,
				"minimum":     1,
				"maximum":     10,
			},
		},
		"required": []string{"command"},
	}
}

func (t *AsyncExecTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	command, ok := args["command"].(string)
	if !ok {
		return ErrorResult("command is required")
	}

	cwd := t.workingDir
	if wd, ok := args["working_dir"].(string); ok && wd != "" {
		cwd = wd
	}

	if cwd == "" {
		wd, err := filepath.Abs(cwd)
		if err == nil {
			cwd = wd
		}
	}

	if guardError := t.guardCommand(command, cwd); guardError != "" {
		return ErrorResult(guardError)
	}

	// Get wait time from arguments
	waitTime := t.defaultWaitTime
	if ws, ok := args["wait_seconds"].(float64); ok {
		seconds := int(ws)
		if seconds < 1 {
			seconds = 1
		} else if seconds > 10 {
			seconds = 10
		}
		waitTime = time.Duration(seconds) * time.Second
	}

	// Extract process name for later checking
	processName := extractProcessName(command)

	// Execute command asynchronously
	var cmd *exec.Cmd
	if runtime.GOOS == "windows" {
		// Replace curl with curl.exe
		curlPattern := regexp.MustCompile(`\bcurl\b`)
		command = curlPattern.ReplaceAllString(command, "curl.exe")

		// Use Start-Process for async execution on Windows
		cmd = exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command",
			"Start-Process -FilePath "+escapePowerShellArg(command))
	} else {
		// Unix-like: use sh with background operator
		cmd = exec.Command("sh", "-c", command+" &")
	}

	if cwd != "" {
		cmd.Dir = cwd
	}

	// Start the command
	if err := cmd.Start(); err != nil {
		return ErrorResult(fmt.Sprintf("Failed to start command: %v", err))
	}

	// Wait a bit to let the process start up
	time.Sleep(waitTime)

	// Check if process is still running
	running, err := t.checkProcessRunning(processName)
	if err != nil {
		// If we can't check, assume it started successfully
		return &ToolResult{
			ForLLM:  fmt.Sprintf("Command executed: %s\nProcess started (unable to verify status)", command),
			ForUser: fmt.Sprintf("命令已执行: %s\n进程已启动（无法验证状态）", command),
			IsError: false,
		}
	}

	if running {
		return &ToolResult{
			ForLLM:  fmt.Sprintf("Application started successfully: %s\nProcess is running", command),
			ForUser: fmt.Sprintf("应用程序已启动: %s\n进程正在运行中", command),
			IsError: false,
		}
	} else {
		return &ToolResult{
			ForLLM:  fmt.Sprintf("Application started but exited quickly (may have crashed): %s", command),
			ForUser: fmt.Sprintf("应用程序已启动但快速退出（可能闪退）: %s", command),
			IsError: true,
		}
	}
}

// extractProcessName extracts the process name from a command
func extractProcessName(command string) string {
	// Trim whitespace
	command = strings.TrimSpace(command)

	// Remove quotes
	command = strings.Trim(command, "\"")

	// Split by spaces and take the first part
	parts := strings.Fields(command)
	if len(parts) == 0 {
		return ""
	}

	// Get the executable name
	execName := parts[0]

	// Extract just the name without path or extension
	execName = filepath.Base(execName)
	execName = strings.TrimSuffix(execName, ".exe")

	return execName
}

// escapePowerShellArg escapes an argument for PowerShell
func escapePowerShellArg(arg string) string {
	// Simple escaping - wrap in quotes if it contains spaces
	if strings.Contains(arg, " ") {
		return "\"" + arg + "\""
	}
	return arg
}

// checkProcessRunning checks if a process is still running
func (t *AsyncExecTool) checkProcessRunning(processName string) (bool, error) {
	if runtime.GOOS == "windows" {
		return t.checkProcessWindows(processName)
	} else {
		return t.checkProcessUnix(processName)
	}
}

// checkProcessWindows checks if a process is running on Windows
func (t *AsyncExecTool) checkProcessWindows(processName string) (bool, error) {
	// Use tasklist command to check if process is running
	cmd := exec.Command("tasklist", "/FI", "IMAGENAME eq "+processName+".exe", "/NH")
	output, err := cmd.Output()
	if err != nil {
		return false, fmt.Errorf("failed to check process: %w", err)
	}

	// If output contains the process name (case-insensitive), it's running
	return strings.Contains(strings.ToLower(string(output)), strings.ToLower(processName)+".exe"), nil
}

// checkProcessUnix checks if a process is running on Unix-like systems
func (t *AsyncExecTool) checkProcessUnix(processName string) (bool, error) {
	// Try pgrep first
	cmd := exec.Command("pgrep", "-x", processName)
	output, err := cmd.Output()
	if err == nil && len(output) > 0 {
		return true, nil
	}

	// Fallback to ps
	cmd = exec.Command("ps", "aux")
	output, err = cmd.Output()
	if err != nil {
		return false, fmt.Errorf("failed to check process: %w", err)
	}

	// Check if process name appears in ps output (case-insensitive)
	return strings.Contains(strings.ToLower(string(output)), strings.ToLower(processName)), nil
}

// guardCommand validates the command against security policies
func (t *AsyncExecTool) guardCommand(command, cwd string) string {
	cmd := strings.TrimSpace(command)
	lower := strings.ToLower(cmd)

	for _, pattern := range t.denyPatterns {
		if pattern.MatchString(lower) {
			return "Command blocked by safety guard (dangerous pattern detected)"
		}
	}

	if len(t.allowPatterns) > 0 {
		allowed := false
		for _, pattern := range t.allowPatterns {
			if pattern.MatchString(lower) {
				allowed = true
				break
			}
		}
		if !allowed {
			return "Command blocked by safety guard (not in allowlist)"
		}
	}

	if t.restrictToWorkspace {
		if strings.Contains(cmd, "..\\") || strings.Contains(cmd, "../") {
			return "Command blocked by safety guard (path traversal detected)"
		}

		cwdPath, err := filepath.Abs(cwd)
		if err != nil {
			return ""
		}

		pathPattern := regexp.MustCompile(`[A-Za-z]:\\[^\\\"']+|/[^\s\"']+`)
		matches := pathPattern.FindAllString(cmd, -1)

		for _, raw := range matches {
			p, err := filepath.Abs(raw)
			if err != nil {
				continue
			}

			rel, err := filepath.Rel(cwdPath, p)
			if err != nil {
				continue
			}

			if strings.HasPrefix(rel, "..") {
				return "Command blocked by safety guard (path outside working dir)"
			}
		}
	}

	return ""
}

func (t *AsyncExecTool) SetDefaultWaitTime(waitTime time.Duration) {
	t.defaultWaitTime = waitTime
}

func (t *AsyncExecTool) SetRestrictToWorkspace(restrict bool) {
	t.restrictToWorkspace = restrict
}

func (t *AsyncExecTool) SetAllowPatterns(patterns []string) error {
	t.allowPatterns = make([]*regexp.Regexp, 0, len(patterns))
	for _, p := range patterns {
		re, err := regexp.Compile(p)
		if err != nil {
			return fmt.Errorf("invalid allow pattern %q: %w", p, err)
		}
		t.allowPatterns = append(t.allowPatterns, re)
	}
	return nil
}
