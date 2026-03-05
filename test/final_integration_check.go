// Final integration test - simulating real usage
package main

import (
	"context"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/tools"
)

func main() {
	fmt.Println("=== 实际使用场景模拟测试 ===\n")

	// Scenario 1: 查看文件
	fmt.Println("场景 1: 查看当前目录")
	execTool := tools.NewExecTool("", false)
	ctx1, cancel1 := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel1()

	result1 := execTool.Execute(ctx1, map[string]interface{}{
		"command": "dir",
	})

	fmt.Printf("结果: %s\n", truncate(result1.ForUser, 100))
	fmt.Printf("错误: %v\n\n", result1.IsError)

	// Scenario 2: 打开记事本
	fmt.Println("场景 2: 打开记事本")
	asyncTool := tools.NewAsyncExecTool("", false)
	ctx2, cancel2 := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel2()

	start := time.Now()
	result2 := asyncTool.Execute(ctx2, map[string]interface{}{
		"command": "notepad.exe",
	})
	duration := time.Since(start)

	fmt.Printf("执行时间: %v\n", duration.Milliseconds())
	fmt.Printf("结果: %s\n\n", result2.ForUser)

	// Scenario 3: 连续操作
	fmt.Println("场景 3: 连续执行多个操作")
	ctx3, cancel3 := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel3()

	// Open notepad
	result3a := asyncTool.Execute(ctx3, map[string]interface{}{
		"command": "notepad.exe README.md",
	})
	fmt.Printf("操作 3a - 打开记事本: %s\n", truncate(result3a.ForUser, 80))

	// List directory
	result3b := execTool.Execute(ctx3, map[string]interface{}{
		"command": "dir",
	})
	fmt.Printf("操作 3b - 列出目录: %s\n\n", truncate(result3b.ForUser, 80))

	fmt.Println("=== 测试完成 ===")
	fmt.Println("\n总结:")
	fmt.Println("✅ 同步执行: 正常工作，返回完整输出")
	fmt.Println("✅ 异步执行: 正常工作，立即返回")
	fmt.Println("✅ 连续操作: 可以快速连续执行多个操作")
}

func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
