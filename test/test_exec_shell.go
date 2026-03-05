package main

import (
	"context"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/tools"
)

func main() {
	fmt.Println("=== 测试 exec 工具（cmd.exe 版本）===\n")

	execTool := tools.NewExecTool("", false)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// 测试 1: 简单命令
	fmt.Println("测试 1: 执行 dir 命令")
	result1 := execTool.Execute(ctx, map[string]interface{}{
		"command": "dir",
	})
	fmt.Printf("结果: %s\n", truncate(result1.ForUser, 100))
	fmt.Printf("错误: %v\n\n", result1.IsError)

	// 测试 2: curl 命令（应该自动添加 --max-time 10）
	fmt.Println("测试 2: curl 命令（自动添加超时）")
	result2 := execTool.Execute(ctx, map[string]interface{}{
		"command": "curl -s http://httpbin.org/delay/2",
	})
	fmt.Printf("结果: %s\n", truncate(result2.ForUser, 100))
	fmt.Printf("错误: %v\n\n", result2.IsError)

	fmt.Println("=== 测试完成 ===")
	fmt.Println("\n说明:")
	fmt.Println("- 默认使用 cmd.exe（更可靠，不会卡住30分钟）")
	fmt.Println("- curl 命令自动添加 --max-time 10 参数")
	fmt.Println("- 如需使用 PowerShell，编译时添加: -tags powershell")
}

func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
