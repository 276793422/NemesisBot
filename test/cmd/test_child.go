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

// CmdTestChild 测试子进程输出
func CmdTestChild() {
	log.SetFlags(log.LstdFlags | log.Lshortfile)

	fmt.Println("=== 子进程输出测试 ===")

	// 获取可执行文件路径
	exePath, err := os.Executable()
	if err != nil {
		fmt.Printf("❌ Failed to get executable path: %v\n", err)
		return
	}

	fmt.Printf("✓ Executable: %s\n", exePath)

	// 创建子进程，使用 --multiple 参数
	cmd := exec.Command(exePath, "--multiple", "--child-id", "test-001", "--window-type", "approval")

	// 创建管道
	stdin, err := cmd.StdinPipe()
	if err != nil {
		fmt.Printf("❌ Failed to create stdin pipe: %v\n", err)
		return
	}

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		fmt.Printf("❌ Failed to create stdout pipe: %v\n", err)
		return
	}

	stderr, err := cmd.StderrPipe()
	if err != nil {
		fmt.Printf("❌ Failed to create stderr pipe: %v\n", err)
		return
	}

	// 启动进程
	if err := cmd.Start(); err != nil {
		fmt.Printf("❌ Failed to start child: %v\n", err)
		return
	}

	fmt.Printf("✓ Child started (PID: %d)\n", cmd.Process.Pid)

	// 发送握手消息
	handshakeMsg := map[string]interface{}{
		"type":    "handshake",
		"version": "1.0",
		"data": map[string]interface{}{
			"protocol": "anon-pipe-v1",
			"version":  "1.0",
		},
	}

	encoder := json.NewEncoder(stdin)
	if err := encoder.Encode(handshakeMsg); err != nil {
		fmt.Printf("❌ Failed to send handshake: %v\n", err)
		return
	}

	fmt.Println("✓ Handshake sent")

	// 读取 stdout (期望 JSON ACK)
	decoder := json.NewDecoder(stdout)
	var ackMsg map[string]interface{}

	// 使用通道来同时读取 stdout 和 stderr
	done := make(chan bool, 1)
	errChan := make(chan error, 1)

	go func() {
		if err := decoder.Decode(&ackMsg); err != nil {
			errChan <- fmt.Errorf("failed to decode ACK: %w", err)
			return
		}
		done <- true
	}()

	// 同时读取 stderr 输出
	go func() {
		stderrData, _ := io.ReadAll(stderr)
		if len(stderrData) > 0 {
			fmt.Printf("\n[Child stderr]:\n%s\n", string(stderrData))
		}
	}()

	// 等待结果
	select {
	case <-done:
		fmt.Printf("✓ Received ACK: %+v\n", ackMsg)
	case err := <-errChan:
		fmt.Printf("❌ Error: %v\n", err)

		// 读取 stdout 的原始内容来看看到底收到了什么
		stdoutData, _ := io.ReadAll(stdout)
		if len(stdoutData) > 0 {
			fmt.Printf("\n[Child stdout raw]:\n%s\n", string(stdoutData))
		}
	case <-time.After(5 * time.Second):
		fmt.Println("❌ Timeout waiting for ACK")

		// 读取 stdout 的原始内容
		stdoutData, _ := io.ReadAll(stdout)
		if len(stdoutData) > 0 {
			fmt.Printf("\n[Child stdout raw]:\n%s\n", string(stdoutData))
		}
	}

	// 清理
	cmd.Process.Kill()
	cmd.Wait()

	fmt.Println("\n=== 测试完成 ===")
}
