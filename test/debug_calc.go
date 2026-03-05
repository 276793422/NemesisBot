// Debug calc.exe test
package main

import (
	"fmt"
	"os/exec"
	"strings"
	"time"
)

func main() {
	fmt.Println("=== calc.exe 进程检查测试 ===")

	// Start calc
	fmt.Println("步骤 1: 启动计算器")
	cmd := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command",
		"Start-Process calc.exe")

	if err := cmd.Start(); err != nil {
		fmt.Printf("启动失败: %v\n", err)
		return
	}

	fmt.Println("计算器已启动")

	// Wait
	fmt.Println("\n步骤 2: 等待 3 秒...")
	time.Sleep(3 * time.Second)

	// Check process
	fmt.Println("\n步骤 3: 检查 calc 进程")
	output, err := exec.Command("tasklist", "/FI", "IMAGENAME eq calc.exe", "/NH").Output()
	if err != nil {
		fmt.Printf("无法检查进程: %v\n", err)
		return
	}

	outputStr := string(output)
	fmt.Printf("tasklist 输出长度: %d\n", len(outputStr))

	if strings.Contains(strings.ToLower(outputStr), "calc.exe") {
		fmt.Println("✅ 发现计算器进程")
	} else {
		fmt.Println("❌ 未发现计算器进程")
		fmt.Printf("输出内容: %s\n", outputStr[:min(300, len(outputStr))])
	}

	// Try another method
	fmt.Println("\n步骤 4: 使用 Get-Process 检查")
	output2, err2 := exec.Command("powershell", "-Command", "Get-Process -Name calc -ErrorAction SilentlyContinue").Output()
	if err2 != nil {
		fmt.Printf("Get-Process 失败: %v\n", err2)
	} else {
		fmt.Printf("Get-Process 输出: %s\n", string(output2))
	}

	fmt.Println("\n=== 测试完成 ===")
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
