//go:build !cross_compile

package integration

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/process"
	"github.com/276793422/NemesisBot/module/desktop/websocket"
)

// TestEndToEndCommunication 测试端到端通信
func TestEndToEndCommunication(t *testing.T) {
	t.Log("Starting end-to-end communication test...")

	// 1. 创建 ProcessManager
	pm := process.NewProcessManager()
	if err := pm.Start(); err != nil {
		t.Fatalf("Failed to start ProcessManager: %v", err)
	}
	defer pm.Stop()

	t.Log("✓ ProcessManager started")

	// 2. 创建子进程（使用 approval 类型）
	data := map[string]interface{}{
		"request_id":     "test-request-001",
		"operation":      "file_delete",
		"operation_name":  "删除文件",
		"target":          "C:\\Temp\\test.txt",
		"risk_level":      "HIGH",
		"reason":          "测试端到端通信",
		"timeout_seconds": 10,
		"context":         map[string]string{},
		"timestamp":       time.Now().Unix(),
	}

	childID, resultCh, err := pm.SpawnChild("approval", data)
	if err != nil {
		t.Fatalf("Failed to spawn child: %v", err)
	}

	t.Logf("✓ Child spawned: %s", childID)

	// 3. 等待结果（设置超时）
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	select {
	case result := <-resultCh:
		t.Logf("✓ Received result from child: %+v", result)

		// 验证结果格式
		resultMap, ok := result.(map[string]interface{})
		if !ok {
			t.Errorf("Expected map[string]interface{}, got %T", result)
		}

	case <-ctx.Done():
		t.Error("✗ Timeout waiting for child result")
	}

	// 4. 清理
	pm.TerminateChild(childID)

	t.Log("✓ End-to-end communication test completed")
}

// TestMultipleChildren 测试多个子进程并发
func TestMultipleChildren(t *testing.T) {
	t.Log("Starting multiple children test...")

	pm := process.NewProcessManager()
	if err := pm.Start(); err != nil {
		t.Fatalf("Failed to start ProcessManager: %v", err)
	}
	defer pm.Stop()

	t.Log("✓ ProcessManager started")

	// 创建 3 个子进程
	numChildren := 3
	children := make([]string, 0, numChildren)
	resultChannels := make([]chan interface{}, 0, numChildren)

	for i := 0; i < numChildren; i++ {
		data := map[string]interface{}{
			"request_id":     fmt.Sprintf("test-request-%03d", i),
			"operation":      "file_read",
			"operation_name":  "读取文件",
			"target":          fmt.Sprintf("C:\\Temp\\test%d.txt", i),
			"risk_level":      "MEDIUM",
			"timeout_seconds": 5,
			"timestamp":       time.Now().Unix(),
		}

		childID, resultCh, err := pm.SpawnChild("approval", data)
		if err != nil {
			t.Logf("Warning: Failed to spawn child %d: %v", i, err)
			continue
		}

		children = append(children, childID)
		resultChannels = append(resultChannels, resultCh)
		t.Logf("✓ Child %d spawned: %s", i, childID)
	}

	if len(children) == 0 {
		t.Fatal("No children were spawned")
	}

	t.Logf("✓ %d children spawned successfully", len(children))

	// 等待所有结果（带超时）
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()

	for i, resultCh := range resultChannels {
		select {
		case result := <-resultCh:
			t.Logf("✓ Child %d returned: %+v", i, result)
		case <-ctx.Done():
			t.Errorf("✗ Timeout waiting for child %d", i)
		}
	}

	// 清理所有子进程
	for _, childID := range children {
		pm.TerminateChild(childID)
	}

	t.Log("✓ Multiple children test completed")
}
