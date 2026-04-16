package test

import (
	"fmt"
	"log"
	"os"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/process"
)

// CmdWindow 测试多进程窗口（隐藏命令）
// 子命令: test window (审批窗口), test dashboard (Dashboard 窗口)
func CmdWindow() {
	log.SetFlags(log.LstdFlags | log.Lshortfile)

	// 子命令路由
	subCmd := "window"
	if len(os.Args) >= 3 {
		subCmd = os.Args[2]
	}

	switch subCmd {
	case "dashboard":
		CmdDashboard()
	case "dashboard-standalone":
		CmdDashboardStandalone()
	default:
		cmdWindowApproval()
	}
}

// cmdWindowApproval tests the approval window
func cmdWindowApproval() {
	fmt.Println("=== 多进程窗口测试 ===")

	procMgr := process.NewProcessManager()
	if err := procMgr.Start(); err != nil {
		fmt.Printf("❌ Failed to start ProcessManager: %v\n", err)
		return
	}
	defer func() {
		if err := procMgr.Stop(); err != nil {
			log.Printf("ProcessManager cleanup warning: %v", err)
		}
	}()

	fmt.Println("✓ ProcessManager started")

	data := map[string]interface{}{
		"request_id":      "test-req-001",
		"operation":       "file_write",
		"operation_name":  "写入文件",
		"target":          "C:\\Temp\\test.txt",
		"risk_level":      "HIGH",
		"reason":          "测试审批流程",
		"timeout_seconds": float64(30),
		"context": map[string]string{
			"file_size": "1024",
			"mime_type": "text/plain",
		},
		"timestamp": float64(time.Now().Unix()),
	}

	fmt.Println("✓ Test data prepared")

	fmt.Println("\n正在创建子进程...")
	childID, resultCh, err := procMgr.SpawnChild("approval", data)
	if err != nil {
		fmt.Printf("❌ Failed to spawn child: %v\n", err)
		return
	}

	fmt.Printf("✓ Child process spawned: %s\n", childID)
	fmt.Println("等待用户响应...")

	select {
	case result := <-resultCh:
		fmt.Printf("\n✓ 收到结果:\n")
		switch v := result.(type) {
		case map[string]interface{}:
			for key, value := range v {
				fmt.Printf("  %s: %v\n", key, value)
			}
		case []byte:
			fmt.Printf("  Raw (UTF-8): %s\n", string(v))
		case string:
			fmt.Printf("  String: %s\n", v)
		default:
			fmt.Printf("  (unknown type): %v\n", v)
		}
	case <-time.After(35 * time.Second):
		fmt.Println("\n✗ 等待结果超时")
	}

	fmt.Println("\n=== 测试完成 ===")
}

// CmdDashboard 测试 Dashboard 窗口（隐藏命令）
// 用法: nemesisbot test dashboard [--token TOKEN] [--port PORT] [--host HOST]
func CmdDashboard() {
	fmt.Println("=== Dashboard 窗口测试 ===")

	procMgr := process.NewProcessManager()
	if err := procMgr.Start(); err != nil {
		fmt.Printf("❌ Failed to start ProcessManager: %v\n", err)
		return
	}
	defer func() {
		if err := procMgr.Stop(); err != nil {
			log.Printf("ProcessManager cleanup warning: %v", err)
		}
	}()

	fmt.Println("✓ ProcessManager started")

	// 默认参数
	token := "276793422"
	port := 49000
	host := "127.0.0.1"

	// 从命令行参数读取
	args := os.Args
	for i := 3; i < len(args); i++ {
		switch args[i] {
		case "--token":
			if i+1 < len(args) {
				token = args[i+1]
				i++
			}
		case "--port":
			if i+1 < len(args) {
				fmt.Sscanf(args[i+1], "%d", &port)
				i++
			}
		case "--host":
			if i+1 < len(args) {
				host = args[i+1]
				i++
			}
		}
	}

	fmt.Printf("✓ 参数: host=%s port=%d token=%s\n", host, port, token)

	data := map[string]interface{}{
		"token":    token,
		"web_port": float64(port),
		"web_host": host,
	}

	fmt.Println("\n正在启动 Dashboard 窗口...")
	childID, _, err := procMgr.SpawnChild("dashboard", data)
	if err != nil {
		fmt.Printf("❌ Failed to spawn Dashboard: %v\n", err)
		return
	}

	fmt.Printf("✓ Dashboard 窗口已启动: %s\n", childID)
	fmt.Println("窗口将保持打开，关闭窗口后进程退出。")
	fmt.Println("\n按 Ctrl+C 终止...")

	// 阻塞等待
	select {}
}
