// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/config"
)

type ExecTool struct {
	workingDir          string
	timeout             time.Duration
	denyPatterns        []*regexp.Regexp
	allowPatterns       []*regexp.Regexp
	restrictToWorkspace bool
}

var defaultDenyPatterns = []*regexp.Regexp{
	regexp.MustCompile(`\brm\s+-[rf]{1,2}\b`),
	regexp.MustCompile(`\bdel\s+/[fq]\b`),
	regexp.MustCompile(`\brmdir\s+/s\b`),
	regexp.MustCompile(`\b(format|mkfs|diskpart)\b\s`), // Match disk wiping commands (must be followed by space/args)
	regexp.MustCompile(`\bdd\s+if=`),
	regexp.MustCompile(`>\s*/dev/sd[a-z]\b`), // Block writes to disk devices (but allow /dev/null)
	regexp.MustCompile(`\b(shutdown|reboot|poweroff)\b`),
	regexp.MustCompile(`:\(\)\s*\{.*\};\s*:`),
	regexp.MustCompile(`\$\([^)]+\)`),
	regexp.MustCompile(`\$\{[^}]+\}`),
	regexp.MustCompile("`[^`]+`"),
	regexp.MustCompile(`\|\s*sh\b`),
	regexp.MustCompile(`\|\s*bash\b`),
	regexp.MustCompile(`;\s*rm\s+-[rf]`),
	regexp.MustCompile(`&&\s*rm\s+-[rf]`),
	regexp.MustCompile(`\|\|\s*rm\s+-[rf]`),
	regexp.MustCompile(`>\s*/dev/null\s*>&?\s*\d?`),
	regexp.MustCompile(`<<\s*EOF`),
	regexp.MustCompile(`\$\(\s*cat\s+`),
	regexp.MustCompile(`\$\(\s*curl\s+`),
	regexp.MustCompile(`\$\(\s*wget\s+`),
	regexp.MustCompile(`\$\(\s*which\s+`),
	regexp.MustCompile(`\bsudo\b`),
	regexp.MustCompile(`\bchmod\s+[0-7]{3,4}\b`),
	regexp.MustCompile(`\bchown\b`),
	regexp.MustCompile(`\bpkill\b`),
	regexp.MustCompile(`\bkillall\b`),
	regexp.MustCompile(`\bkill\s+-[9]\b`),
	regexp.MustCompile(`\bcurl\b.*\|\s*(sh|bash)`),
	regexp.MustCompile(`\bwget\b.*\|\s*(sh|bash)`),
	regexp.MustCompile(`\bnpm\s+install\s+-g\b`),
	regexp.MustCompile(`\bpip\s+install\s+--user\b`),
	regexp.MustCompile(`\bapt\s+(install|remove|purge)\b`),
	regexp.MustCompile(`\byum\s+(install|remove)\b`),
	regexp.MustCompile(`\bdnf\s+(install|remove)\b`),
	regexp.MustCompile(`\bdocker\s+run\b`),
	regexp.MustCompile(`\bdocker\s+exec\b`),
	regexp.MustCompile(`\bgit\s+push\b`),
	regexp.MustCompile(`\bgit\s+force\b`),
	regexp.MustCompile(`\bssh\b.*@`),
	regexp.MustCompile(`\beval\b`),
	regexp.MustCompile(`\bsource\s+.*\.sh\b`),
}

func NewExecTool(workingDir string, restrict bool) *ExecTool {
	return NewExecToolWithConfig(workingDir, restrict, nil)
}

func NewExecToolWithConfig(workingDir string, restrict bool, config *config.Config) *ExecTool {
	denyPatterns := make([]*regexp.Regexp, 0)

	enableDenyPatterns := true
	if config != nil {
		execConfig := config.Tools.Exec
		enableDenyPatterns = execConfig.EnableDenyPatterns
		if enableDenyPatterns {
			if len(execConfig.CustomDenyPatterns) > 0 {
				fmt.Printf("Using custom deny patterns: %v\n", execConfig.CustomDenyPatterns)
				for _, pattern := range execConfig.CustomDenyPatterns {
					re, err := regexp.Compile(pattern)
					if err != nil {
						fmt.Printf("Invalid custom deny pattern %q: %v\n", pattern, err)
						continue
					}
					denyPatterns = append(denyPatterns, re)
				}
			} else {
				denyPatterns = append(denyPatterns, defaultDenyPatterns...)
			}
		} else {
			// If deny patterns are disabled, we won't add any patterns, allowing all commands.
			fmt.Println("Warning: deny patterns are disabled. All commands will be allowed.")
		}
	} else {
		denyPatterns = append(denyPatterns, defaultDenyPatterns...)
	}

	return &ExecTool{
		workingDir:          workingDir,
		timeout:             60 * time.Second,
		denyPatterns:        denyPatterns,
		allowPatterns:       nil,
		restrictToWorkspace: restrict,
	}
}

func (t *ExecTool) Name() string {
	return "exec"
}

func (t *ExecTool) Description() string {
	return `执行命令并等待完成，返回完整输出。

适用场景：
- 需要返回输出的命令（如：cat, ls, grep, curl）
- 编译和构建命令（如：go build, make）
- 任何需要等待结果完成的命令

行为说明：
- 此工具会等待命令执行完成（最多 60 秒超时）
- 返回完整的标准输出和错误输出
- 命令执行完成前无法继续执行其他操作

重要提示：
- 对于 GUI 应用程序（如 notepad.exe, calc.exe），请使用 exec_async 工具
- exec_async 会立即返回，不会等待应用程序退出

示例：
- exec(command="dir")
- exec(command="cat README.md")
- exec(command="curl https://api.example.com")`
}

func (t *ExecTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"command": map[string]interface{}{
				"type":        "string",
				"description": "The shell command to execute",
			},
			"working_dir": map[string]interface{}{
				"type":        "string",
				"description": "Optional working directory for the command",
			},
		},
		"required": []string{"command"},
	}
}

func (t *ExecTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	command, ok := args["command"].(string)
	if !ok {
		return ErrorResult("command is required")
	}

	cwd := t.workingDir
	if wd, ok := args["working_dir"].(string); ok && wd != "" {
		cwd = wd
	}

	if cwd == "" {
		wd, err := os.Getwd()
		if err == nil {
			cwd = wd
		}
	}

	if guardError := t.guardCommand(command, cwd); guardError != "" {
		return ErrorResult(guardError)
	}

	// Preprocess command for Windows
	if runtime.GOOS == "windows" {
		command = t.preprocessWindowsCommand(command)
	}

	// timeout == 0 means no timeout
	var cmdCtx context.Context
	var cancel context.CancelFunc
	if t.timeout > 0 {
		cmdCtx, cancel = context.WithTimeout(ctx, t.timeout)
	} else {
		cmdCtx, cancel = context.WithCancel(ctx)
	}
	defer cancel()

	var cmd *exec.Cmd
	var err error
	if runtime.GOOS == "windows" {
		// Windows: use platform-specific implementation (cmd.exe or PowerShell)
		cmd, err = t.buildWindowsCommand(cmdCtx, command)
		if err != nil {
			return ErrorResult(fmt.Sprintf("Failed to build command: %v", err))
		}
	} else {
		// Unix-like systems: use sh -c
		cmd = exec.CommandContext(cmdCtx, "sh", "-c", command)
	}

	if cwd != "" {
		cmd.Dir = cwd
	}

	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	err = cmd.Run()
	output := stdout.String()
	if stderr.Len() > 0 {
		output += "\nSTDERR:\n" + stderr.String()
	}

	if err != nil {
		if cmdCtx.Err() == context.DeadlineExceeded {
			msg := fmt.Sprintf("Command timed out after %v", t.timeout)
			return &ToolResult{
				ForLLM:  msg,
				ForUser: msg,
				IsError: true,
			}
		}
		output += fmt.Sprintf("\nExit code: %v", err)
	}

	if output == "" {
		output = "(no output)"
	}

	maxLen := 10000
	if len(output) > maxLen {
		output = output[:maxLen] + fmt.Sprintf("\n... (truncated, %d more chars)", len(output)-maxLen)
	}

	if err != nil {
		return &ToolResult{
			ForLLM:  output,
			ForUser: output,
			IsError: true,
		}
	}

	return &ToolResult{
		ForLLM:  output,
		ForUser: output,
		IsError: false,
	}
}

func (t *ExecTool) guardCommand(command, cwd string) string {
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

func (t *ExecTool) SetTimeout(timeout time.Duration) {
	t.timeout = timeout
}

func (t *ExecTool) SetRestrictToWorkspace(restrict bool) {
	t.restrictToWorkspace = restrict
}

func (t *ExecTool) SetAllowPatterns(patterns []string) error {
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

// preprocessWindowsCommand preprocesses commands for Windows execution
// - Ensures curl is curl.exe (not an alias)
// - Adds --max-time to curl commands if not present
// - Normalizes Windows file paths (converts forward slashes to backslashes)
// - Fixes path quoting issues for Windows commands
func (t *ExecTool) preprocessWindowsCommand(command string) string {
	// Replace 'curl' with 'curl.exe' to avoid alias issues
	curlPattern := regexp.MustCompile(`\bcurl\.exe\b`)
	if !curlPattern.MatchString(command) {
		bareCurlPattern := regexp.MustCompile(`\bcurl\b`)
		command = bareCurlPattern.ReplaceAllString(command, "curl.exe")
	}

	// Add --max-time to curl if not already present
	// This prevents curl from hanging indefinitely on network issues
	if strings.Contains(strings.ToLower(command), "curl.exe") {
		// Check if --max-time or -m is already present
		hasMaxTime := regexp.MustCompile(`\b--max-time\s*\d+|\b-m\s*\d+`).MatchString(command)
		if !hasMaxTime {
			// Insert --max-time 10 after curl.exe
			curlExePattern := regexp.MustCompile(`\bcurl\.exe\b`)
			command = curlExePattern.ReplaceAllString(command, "curl.exe --max-time 300")
		}
	}

	// Normalize Windows file paths (convert C:/path to C:\path)
	command = t.normalizeWindowsPaths(command)

	// Fix path quoting issues for common Windows commands
	// Some Windows commands (like 'type', 'dir') don't handle quoted paths well
	// when called from cmd.exe with /c
	command = t.fixWindowsPathQuoting(command)

	return command
}

// normalizeWindowsPaths converts forward slashes to backslashes in Windows file paths
// while preserving URLs, network paths, and other path-like strings
func (t *ExecTool) normalizeWindowsPaths(command string) string {
	// Step 1: Extract and protect URLs and network paths from conversion
	// Protect multiple protocol types and network paths
	protected := command
	placeholders := make(map[string]string)
	placeholderIndex := 0

	// Pattern 1: Standard URL protocols (http, https, ftp, ftps, sftp, ws, wss, etc.)
	// Matches: http://..., https://..., ftp://..., etc.
	urlPattern := regexp.MustCompile(`(?:https?|ftps?|sftp|wss?|file|git|ssh)://[^\s"'<>]+`)
	urls := urlPattern.FindAllString(command, -1)
	for _, url := range urls {
		placeholder := fmt.Sprintf("___URL_PLACEHOLDER_%d___", placeholderIndex)
		placeholders[placeholder] = url
		protected = strings.Replace(protected, url, placeholder, 1)
		placeholderIndex++
	}

	// Pattern 2: Git SSH URLs (git@host:path format)
	// Matches: git@github.com:user/repo.git
	gitSshPattern := regexp.MustCompile(`git@[^\s"'<>]+`)
	gitUrls := gitSshPattern.FindAllString(command, -1)
	for _, url := range gitUrls {
		placeholder := fmt.Sprintf("___URL_PLACEHOLDER_%d___", placeholderIndex)
		placeholders[placeholder] = url
		protected = strings.Replace(protected, url, placeholder, 1)
		placeholderIndex++
	}

	// Pattern 3: UNC network paths (\\server\share or //server/share)
	// Matches: \\server\share or //server/share
	uncPattern := regexp.MustCompile(`(?:\\\\|//)[^\s"'<>]+`)
	uncPaths := uncPattern.FindAllString(command, -1)
	for _, path := range uncPaths {
		placeholder := fmt.Sprintf("___URL_PLACEHOLDER_%d___", placeholderIndex)
		placeholders[placeholder] = path
		protected = strings.Replace(protected, path, placeholder, 1)
		placeholderIndex++
	}

	// Step 2: Convert Windows file paths from C:/path to C:\path
	// Pattern matches: C:/path or D:/some/path
	// This handles paths in quotes and without quotes
	pathPattern := regexp.MustCompile(`([A-Za-z]):((?:/[^/\s"']*)+)`)

	protected = pathPattern.ReplaceAllStringFunc(protected, func(match string) string {
		// Extract drive letter and path
		parts := pathPattern.FindStringSubmatch(match)
		if len(parts) < 3 {
			return match
		}

		drive := parts[1]
		path := parts[2]

		// Convert forward slashes to backslashes
		normalized := strings.ReplaceAll(path, "/", "\\")

		return drive + ":" + normalized
	})

	// Step 3: Restore all protected strings
	for placeholder, original := range placeholders {
		protected = strings.Replace(protected, placeholder, original, 1)
	}

	return protected
}

// fixWindowsPathQuoting fixes path quoting issues in Windows commands
func (t *ExecTool) fixWindowsPathQuoting(command string) string {
	// Commands that have issues with quoted paths in cmd.exe
	// These commands expect paths without quotes or with different quoting
	problematicCommands := []string{"type", "dir", "copy", "move", "del", "ren", "md", "rd"}

	// Check if command starts with any problematic command
	for _, cmd := range problematicCommands {
		// Pattern: command "path" or command "path" args
		pattern := regexp.MustCompile(`(?i)^\s*` + cmd + `\s+"([^"]+)"(.*)$`)
		if matches := pattern.FindStringSubmatch(command); matches != nil {
			path := matches[1]
			rest := matches[2]

			// If path has no spaces, remove quotes
			if !strings.Contains(path, " ") {
				command = cmd + " " + path + rest
				break
			}

			// If path has spaces but command doesn't support quotes,
			// try using the short path name (8.3 format)
			// This is a Windows-specific format without spaces
			if shortPath, err := t.getShortPath(path); err == nil {
				command = cmd + " " + shortPath + rest
				break
			}

			// If short path conversion fails, keep the quotes
			// Some commands might still work
		}
	}

	return command
}

// getShortPath converts a long Windows path to short path (8.3 format)
// This helps with commands that don't handle spaces well
func (t *ExecTool) getShortPath(longPath string) (string, error) {
	// Use Windows API to get short path
	// For now, return a simple fallback: try to escape spaces with ^
	if strings.Contains(longPath, " ") {
		// cmd.exe escape character for spaces
		return strings.ReplaceAll(longPath, " ", "^ "), nil
	}
	return longPath, nil
}
