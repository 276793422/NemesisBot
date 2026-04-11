//go:build !cross_compile

package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/process"
	"github.com/276793422/NemesisBot/module/desktop/websocket"
)

// TestChildProcessCreation 测试子进程创建
func TestChildProcessCreation(t *testing.T) {
	// 获取当前可执行文件路径
	exePath, err := os.Executable()
	if err != nil {
		t.Fatalf("Failed to get executable path: %v", err)
	}

	// 创建执行器
	executor := process.NewWindowsExecutor(nil)

	// 创建子进程（使用 --multiple 参数）
	child, err := executor.SpawnChild(exePath, []string{
		"--multiple",
		"--child-id",
		"test-001",
		"--window-type",
		"approval",
	})

	if err != nil {
		t.Fatalf("Failed to spawn child: %v", err)
	}

	defer executor.Cleanup(child)
	defer executor.TerminateChild(child)

	t.Logf("Child process created: PID=%d", child.PID)

	// 验证进程是否在运行
	if child.Cmd.Process == nil {
		t.Fatal("Process is nil")
	}

	// 等待一小段时间确保进程启动
	time.Sleep(100 * time.Millisecond)

	// 检查进程是否还在运行
	if child.Cmd.Process.Signal(os.Signal(nil)) != nil {
		t.Error("Child process has exited")
	}
}

// TestParentHandshake 测试父进程握手
func TestParentHandshake(t *testing.T) {
	// 创建模拟子进程
	exePath, err := os.Executable()
	if err != nil {
		t.Fatalf("Failed to get executable path: %v", err)
	}

	executor := process.NewWindowsExecutor(nil)

	child, err := executor.SpawnChild(exePath, []string{
		"--multiple",
		"--child-id",
		"test-002",
	})
	if err != nil {
		t.Fatalf("Failed to spawn child: %v", err)
	}
	defer executor.Cleanup(child)
	defer executor.TerminateChild(child)

	// 等待子进程启动
	time.Sleep(500 * time.Millisecond)

	// 执行握手
	result, err := process.ParentHandshake(child)
	if err != nil {
		t.Fatalf("Handshake failed: %v", err)
	}

	if !result.Success {
		t.Fatal("Handshake not successful")
	}

	t.Log("✓ Parent handshake completed")
}

// TestHandshakeProtocol 测试握手协议
func TestHandshakeProtocol(t *testing.T) {
	// 创建管道对
	pr, pw := io.Pipe()

	// 父进程端：发送握手
	stdin := &process.WriteCloser{
		Encoder: json.NewEncoder(pw),
		writer:  pw.(*os.File),
	}
	defer stdin.Close()

	handshakeMsg := &process.PipeMessage{
		Type:    "handshake",
		Version: "1.0",
		Data: map[string]interface{}{
			"protocol": "anon-pipe-v1",
			"version":  "1.0",
		},
	}

	if err := stdin.Encode(handshakeMsg); err != nil {
		t.Fatalf("Failed to send handshake: %v", err)
	}

	t.Log("✓ Handshake message sent")

	// 子进程端：接收握手
	stdout := &process.ReadCloser{
		Decoder: json.NewDecoder(pr),
		reader:  pr.(*os.File),
	}
	defer stdout.Close()

	// 读取消息（需要超时）
	msgChan := make(chan *process.PipeMessage, 1)
	errChan := make(chan error, 1)

	go func() {
		var msg process.PipeMessage
		if err := stdout.Decode(&msg); err != nil {
			errChan <- err
			return
		}
		msgChan <- &msg
	}()

	select {
	case msg := <-msgChan:
		if msg.Type != "handshake" {
			t.Errorf("Expected handshake, got %s", msg.Type)
		}
		t.Logf("✓ Handshake message received: %+v", msg)
	case err := <-errChan:
		t.Fatalf("Failed to receive handshake: %v", err)
	case <-time.After(3 * time.Second):
		t.Fatal("Timeout waiting for handshake")
	}
}

// TestWebSocketKeyGeneration 测试 WebSocket 密钥生成
func TestWebSocketKeyGeneration(t *testing.T) {
	keyGen := websocket.NewKeyGenerator()

	// 生成密钥
	key1, err := keyGen.Generate(1234)
	if err != nil {
		t.Fatalf("Failed to generate key: %v", err)
	}

	t.Logf("Key generated: %s", key1.Key)

	// 验证密钥
	validatedKey, err := keyGen.Validate(key1.Key)
	if err != nil {
		t.Fatalf("Failed to validate key: %v", err)
	}

	if validatedKey.Key != key1.Key {
		t.Errorf("Key mismatch: %s != %s", validatedKey.Key, key1.Key)
	}

	if validatedKey.ChildPID != 1234 {
		t.Errorf("PID mismatch: %d != 1234", validatedKey.ChildPID)
	}

	t.Log("✓ Key validation successful")

	// 测试密钥撤销
	err = keyGen.Revoke(key1.Key)
	if err != nil {
		t.Fatalf("Failed to revoke key: %v", err)
	}

	_, err = keyGen.Validate(key1.Key)
	if err == nil {
		t.Error("Expected error for revoked key")
	}

	t.Log("✓ Key revocation successful")
}

// TestWebSocketServerStartup 测试 WebSocket 服务器启动
func TestWebSocketServerStartup(t *testing.T) {
	keyGen := websocket.NewKeyGenerator()
	server := websocket.NewWebSocketServer(keyGen)

	// 启动服务器
	if err := server.Start(); err != nil {
		t.Fatalf("Failed to start server: %v", err)
	}
	defer server.Stop()

	port := server.GetPort()
	t.Logf("✓ WebSocket server started on port %d", port)

	if port <= 0 {
		t.Errorf("Invalid port: %d", port)
	}
}
