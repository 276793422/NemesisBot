// Simple integration test for async exec tool
package main

import (
	"context"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/tools"
)

func main() {
	fmt.Println("=== 异步执行工具测试 ===\n")

	// Test 1: Sync execution (should work)
	fmt.Println("测试 1: 同步执行 dir 命令")
	execTool := tools.NewExecTool("", false)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	result := execTool.Execute(ctx, map[string]interface{}{
		"command": "dir",
	})

	fmt.Printf("结果: %s\n", result.ForUser)
	fmt.Printf("错误: %v\n\n", result.IsError)

	// Test 2: Async execution with notepad
	fmt.Println("测试 2: 异步执行 notepad.exe")
	asyncTool := tools.NewAsyncExecTool("", false)
	ctx2, cancel2 := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel2()

	result2 := asyncTool.Execute(ctx2, map[string]interface{}{
		"command": "notepad.exe",
	})

	fmt.Printf("结果: %s\n", result2.ForUser)
	fmt.Printf("错误: %v\n\n", result2.IsError)

	// Test 3: Async execution with calc
	fmt.Println("测试 3: 异步执行 calc.exe")
	ctx3, cancel3 := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel3()

	result3 := asyncTool.Execute(ctx3, map[string]interface{}{
		"command":       "calc.exe",
		"wait_seconds": 3.0,
	})

	fmt.Printf("结果: %s\n", result3.ForUser)
	fmt.Printf("错误: %v\n\n", result3.IsError)

	fmt.Println("=== 测试完成 ===")
}
