package command

import (
	"encoding/json"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"time"
)

// CmdTestChildDetailed 详细测试子进程
func CmdTestChildDetailed() {
	log.SetFlags(log.LstdFlags | log.Lshortfile)

	fmt.Println("=== 详细子进程测试 ===")

	// 获取可执行文件路径
	exePath, err := os.Executable()
	if err != nil {
		fmt.Printf("❌ Failed to get executable path: %v\n", err)
		return
	}

	fmt.Printf("✓ Executable: %s\n", exePath)

	// 创建子进程
	cmd := exec.Command(exePath, "--multiple", "--child-id", "test-detailed", "--window-type", "approval")

	// 创建管道
	stdin, _ := cmd.StdinPipe()
	stdout, _ := cmd.StdoutPipe()
	stderr, _ := cmd.StderrPipe()

	// 启动进程
	if err := cmd.Start(); err != nil {
		fmt.Printf("❌ Failed to start child: %v\n", err)
		return
	}

	fmt.Printf("✓ Child started (PID: %d)\n", cmd.Process.Pid)

	// 并发读取 stderr
	stderrDone := make(chan []byte, 1)
	go func() {
		data, _ := io.ReadAll(stderr)
		stderrDone <- data
	}()

	// 发送握手
	encoder := json.NewEncoder(stdin)
	handshakeMsg := map[string]interface{}{
		"type":    "handshake",
		"version": "1.0",
		"data": map[string]interface{}{
			"protocol": "anon-pipe-v1",
			"version":  "1.0",
		},
	}
	encoder.Encode(handshakeMsg)
	fmt.Println("✓ Handshake sent")

	// 读取响应
	decoder := json.NewDecoder(stdout)
	var ackMsg map[string]interface{}

	// 等待握手 ACK
	decoder.Decode(&ackMsg)
	fmt.Printf("✓ Handshake ACK: %+v\n", ackMsg)

	// 发送 WS key
	wsKeyMsg := map[string]interface{}{
		"type": "ws_key",
		"version": "1.0",
		"data": map[string]interface{}{
			"key":  "test-key-12345",
			"port": float64(12345),
			"path": "/test",
		},
	}
	encoder.Encode(wsKeyMsg)
	fmt.Println("✓ WS key sent")

	// 等待 ACK
	decoder.Decode(&ackMsg)
	fmt.Printf("✓ WS key ACK: %+v\n", ackMsg)

	// 发送窗口数据
	windowData := map[string]interface{}{
		"request_id":      "test-req-001",
		"operation":       "file_write",
		"operation_name":  "写入文件",
		"target":          "C:\\Temp\\test.txt",
		"risk_level":      "HIGH",
		"reason":          "测试审批流程",
		"timeout_seconds": float64(30),
		"context":         map[string]string{},
		"timestamp":       float64(time.Now().Unix()),
	}

	windowMsg := map[string]interface{}{
		"type":    "window_data",
		"version": "1.0",
		"data": map[string]interface{}{
			"data": windowData,
		},
	}
	encoder.Encode(windowMsg)
	fmt.Println("✓ Window data sent")

	// 等待 ACK
	decoder.Decode(&ackMsg)
	fmt.Printf("✓ Window data ACK: %+v\n", ackMsg)

	// 等待子进程运行（5秒）
	fmt.Println("\n等待子进程运行 Wails 窗口（5秒）...")
	time.Sleep(5 * time.Second)

	// 读取 stderr
	stderrData := <-stderrDone
	if len(stderrData) > 0 {
		fmt.Printf("\n[Child stderr]:\n%s\n", string(stderrData))
	}

	// 检查进程状态
	if cmd.Process != nil && cmd.ProcessState != nil {
		fmt.Printf("\n进程退出码: %d\n", cmd.ProcessState.ExitCode())
	}

	// 清理
	cmd.Process.Kill()
	cmd.Wait()

	fmt.Println("\n=== 测试完成 ===")
}
