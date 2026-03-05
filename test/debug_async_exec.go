// Debug test for async exec tool
package main

import (
	"context"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/tools"
)

func main() {
	fmt.Println("=== 异步执行工具调试测试 ===")
	fmt.Println()

	// Start notepad asynchronously
	fmt.Println("步骤 1: 启动记事本")
	cmd := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command",
		"Start-Process notepad.exe")

	if err := cmd.Start(); err != nil {
		fmt.Printf("启动失败: %v\n", err)
		return
	}

	fmt.Println("记事本已启动")

	// Wait and check process
	fmt.Println("\n步骤 2: 等待 3 秒...")
	time.Sleep(3 * time.Second)

	fmt.Println("\n步骤 3: 检查进程")
	output, err := exec.Command("tasklist", "/FI", "IMAGENAME eq notepad.exe", "/NH").Output()
	if err != nil {
		fmt.Printf("无法检查进程: %v\n", err)
	} else {
		outputStr := string(output)
		fmt.Printf("tasklist 输出长度: %d\n", len(outputStr))

		if strings.Contains(outputStr, "notepad.exe") {
			fmt.Println("发现记事本进程正在运行")
		} else {
			fmt.Println("未发现记事本进程")
			fmt.Printf("输出内容: %s\n", outputStr[:min(200, len(outputStr))])
		}
	}

	// Test with the actual AsyncExecTool
	fmt.Println("\n=== 使用 AsyncExecTool 测试 ===")
	asyncTool := tools.NewAsyncExecTool("", false)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	result := asyncTool.Execute(ctx, map[string]interface{}{
		"command":       "notepad.exe",
		"wait_seconds": 5.0,
	})

	fmt.Println("\nAsyncExecTool 结果:")
	fmt.Printf("ForLLM: %s\n", result.ForLLM)
	fmt.Printf("ForUser: %s\n", result.ForUser)
	fmt.Printf("IsError: %v\n", result.IsError)

	fmt.Println("\n=== 测试完成 ===")
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
