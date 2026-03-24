package test

import (
	"fmt"
	"log"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/process"
)

// CmdWindow 测试多进程窗口（隐藏命令）
func CmdWindow() {
	log.SetFlags(log.LstdFlags | log.Lshortfile)

	fmt.Println("=== 多进程窗口测试 ===")

	// 创建 ProcessManager
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

	// 创建测试数据
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

	// 创建子进程
	fmt.Println("\n正在创建子进程...")
	childID, resultCh, err := procMgr.SpawnChild("approval", data)
	if err != nil {
		fmt.Printf("❌ Failed to spawn child: %v\n", err)
		return
	}

	fmt.Printf("✓ Child process spawned: %s\n", childID)
	fmt.Println("等待用户响应...")

	// 等待结果
	select {
	case result := <-resultCh:
		fmt.Printf("\n✓ 收到结果:\n")

		// 尝试解析结果
		switch v := result.(type) {
		case map[string]interface{}:
			for key, value := range v {
				fmt.Printf("  %s: %v\n", key, value)
			}
		case []byte:
			// JSON 字节数组
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
