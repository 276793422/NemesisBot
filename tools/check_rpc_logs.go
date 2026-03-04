// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// +build ignore

package main

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Println("Usage: go run check_rpc_logs.go <workspace_path>")
		fmt.Println("Example: go run check_rpc_logs.go ~/nemesisbot")
		os.Exit(1)
	}

	workspace := os.Args[1]
	if strings.HasPrefix(workspace, "~/") {
		homeDir, _ := os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	logDir := filepath.Join(workspace, "logs", "cluster")

	// Check log files
	logFiles := map[string]string{
		"Daemon Log":    filepath.Join(logDir, "daemon.log"),
		"RPC Log":       filepath.Join(logDir, "rpc.log"),
		"Discovery Log": filepath.Join(logDir, "discovery.log"),
	}

	fmt.Println("═══════════════════════════════════════════════════")
	fmt.Println("       RPC 日志诊断工具")
	fmt.Println("═══════════════════════════════════════════════════")
	fmt.Printf("工作区: %s\n\n", workspace)

	for name, logPath := range logFiles {
		fmt.Printf("【%s】%s\n", name, logPath)

		if _, err := os.Stat(logPath); os.IsNotExist(err) {
			fmt.Println("  ❌ 文件不存在")
			fmt.Println("")
			continue
		}

		file, err := os.Open(logPath)
		if err != nil {
			fmt.Printf("  ❌ 无法打开文件: %v\n\n", err)
			continue
		}

		// Get file size
		stat, _ := file.Stat()
		size := stat.Size()
		fmt.Printf("  📄 文件大小: %d 字节\n", size)

		// Count lines and show last few
		scanner := bufio.NewScanner(file)
		var lines []string
		lineCount := 0
		for scanner.Scan() {
			lines = append(lines, scanner.Text())
			lineCount++
		}
		file.Close()

		fmt.Printf("  📊 总行数: %d\n", lineCount)

		// Show last 5 lines if any
		if len(lines) > 0 {
			fmt.Println("  📝 最近 5 行日志:")
			start := len(lines) - 5
			if start < 0 {
				start = 0
			}
			for i := start; i < len(lines); i++ {
				fmt.Printf("     %s\n", lines[i])
			}
		}
		fmt.Println("")
	}

	// Search for RPC-related logs
	fmt.Println("═══════════════════════════════════════════════════")
	fmt.Println("       RPC 相关日志搜索")
	fmt.Println("═══════════════════════════════════════════════════")
	fmt.Println("")

	searchPatterns := []struct {
		name    string
		pattern string
		logFile string
	}{
		{"Client 发送 hello 请求", "RPC ->", "daemon.log"},
		{"Server 接收请求", "Received request: action=hello", "rpc.log"},
		{"Server 无 handler", "No handler for action 'hello'", "rpc.log"},
		{"Server 发送响应", "Sending response: action=hello", "rpc.log"},
		{"RPC 错误", "RPC.*Error", "rpc.log"},
	}

	for _, search := range searchPatterns {
		logPath := filepath.Join(logDir, search.logFile)
		fmt.Printf("【%s】在 %s 中\n", search.name, search.logFile)

		file, err := os.Open(logPath)
		if err != nil {
			fmt.Println("  ❌ 无法打开文件")
			fmt.Println("")
			continue
		}
		defer file.Close()

		scanner := bufio.NewScanner(file)
		found := false
		count := 0

		for scanner.Scan() {
			line := scanner.Text()
			if strings.Contains(line, search.pattern) {
				if !found {
					found = true
					fmt.Println("  ✅ 找到匹配:")
				}
				if count < 3 { // 只显示前3条
					fmt.Printf("     %s\n", line)
				}
				count++
			}
		}

		if !found {
			fmt.Println("  ❌ 未找到匹配")
		} else if count > 3 {
			fmt.Printf("  ... 还有 %d 条匹配\n", count-3)
		}
		fmt.Println("")
	}

	// Check for recent activity
	fmt.Println("═══════════════════════════════════════════════════")
	fmt.Println("       最近活动检查")
	fmt.Println("═══════════════════════════════════════════════════")
	fmt.Println("")

	for name, logPath := range logFiles {
		file, err := os.Open(logPath)
		if err != nil {
			continue
		}

		scanner := bufio.NewScanner(file)
		var lastLine string
		for scanner.Scan() {
			lastLine = scanner.Text()
		}
		file.Close()

		if lastLine != "" {
			// Extract timestamp from log line
			// Format: [2026-01-02 15:04:05] [INFO] message
			parts := strings.SplitN(lastLine, "]", 2)
			if len(parts) >= 1 {
				timestampStr := strings.Trim(parts[0], "[]")
				timestamp, err := time.Parse("2006-01-02 15:04:05", timestampStr)
				if err == nil {
					duration := time.Since(timestamp)
					fmt.Printf("【%s】最后活动: %s (%s 前)\n",
						name, timestamp.Format("2006-01-02 15:04:05"),
						duration.Round(time.Second))
				}
			}
		}
	}

	fmt.Println("")
	fmt.Println("═══════════════════════════════════════════════════")
	fmt.Println("       建议")
	fmt═════════════════════════════════════════════════════")
	fmt.Println("")
	fmt.Println("1. 检查 rpc.log 中是否有 'Received request: action=hello'")
	fmt.Println("   - 如果有，说明 Server 接收到了请求")
	fmt.Println("   - 检查是否有 'No handler for action 'hello''")
	fmt.Println("   - 检查是否有 'Sending response: action=hello'")
	fmt.Println("")
	fmt.Println("2. 检查 daemon.log 中是否有 'RPC ->' 开头的日志")
	fmt.Println("   - 如果有 'Response:' 说明收到了响应")
	fmt.Println("   - 如果有 'Error:' 说明出现了错误")
	fmt.Println("")
	fmt.Println("3. 如果 Server 收到请求但 Client 没收到响应:")
	fmt.Println("   - 可能是网络问题")
	fmt.Println("   - 可能是连接超时")
	fmt.Println("   - 可能是响应格式问题")
	fmt.Println("")
	fmt.Println("4. 解决方案: 注册 'hello' handler")
	fmt.Println("   在 Server 启动时添加:")
	fmt.Println("   cluster.RegisterRPCHandler('hello', func(payload map[string]interface{}) (map[string]interface{}, error) {")
	fmt.Println("       return map[string]interface{}{")
	fmt.Println("           'greeting': 'Hello from ' + payload['from"].(string),")
	fmt.Println("           'timestamp': time.Now().Format(time.RFC3339),")
	fmt.Println("       }, nil")
	fmt.Println("   })")
}
