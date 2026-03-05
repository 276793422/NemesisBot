package main

import (
	"context"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/tools"
)

func main() {
	fmt.Println("=== 验证 30 分钟超时问题修复 ===\n")

	execTool := tools.NewExecTool("", false)

	// 测试场景：模拟 curl 网络挂起
	// httpbin.org/delay/30 会延迟 30 秒响应
	// 但我们的 curl 命令会自动添加 --max-time 10
	// 所以应该在 10 秒内超时，而不是 30 分钟！

	fmt.Println("测试: curl 命令超时修复")
	fmt.Println("场景: 请求一个延迟 30 秒的端点")
	fmt.Println("期望: 在 10 秒内超时（因为自动添加了 --max-time 10）")
	fmt.Println("之前的 bug: 会卡住 30 分钟\n")

	start := time.Now()
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	result := execTool.Execute(ctx, map[string]interface{}{
		"command": "curl -s http://httpbin.org/delay/30",
	})

	duration := time.Since(start)

	fmt.Printf("执行时间: %v\n", duration)
	fmt.Printf("结果: %s\n", truncate(result.ForUser, 200))
	fmt.Printf("IsError: %v\n\n", result.IsError)

	// 验证
	if duration < 15*time.Second {
		fmt.Println("✅ 测试通过！命令在 15 秒内返回（正常超时）")
		fmt.Println("✅ 不再会卡住 30 分钟！")
	} else {
		fmt.Println("❌ 测试失败！命令执行时间过长")
	}

	// 显示命令预处理结果
	fmt.Println("\n=== 命令预处理说明 ===")
	fmt.Println("输入: curl -s http://httpbin.org/delay/30")
	fmt.Println("自动转换为: curl.exe --max-time 10 -s http://httpbin.org/delay/30")
	fmt.Println("\n这样即使网络挂起，也会在 10 秒后超时")
}

func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
