//go:build !cross_compile

package process

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
)

// ProcessManager 进程管理器
type ProcessManager struct {
	mu          sync.RWMutex
	children    map[string]*ChildProcess
	executor    PlatformExecutor
	wsServer    *websocket.WebSocketServer
	keyGen      *websocket.KeyGenerator
	nextID      int64
	ctx         context.Context
	cancel      context.CancelFunc
}

// NewProcessManager 创建进程管理器
func NewProcessManager() *ProcessManager {
	ctx, cancel := context.WithCancel(context.Background())

	// 创建共享的 KeyGenerator
	keyGen := websocket.NewKeyGenerator()

	return &ProcessManager{
		children: make(map[string]*ChildProcess),
		executor: GetPlatformExecutor(nil),
		keyGen:   keyGen,
		wsServer: websocket.NewWebSocketServer(keyGen),
		nextID:   0,
		ctx:      ctx,
		cancel:   cancel,
	}
}

// Start 启动进程管理器
func (m *ProcessManager) Start() error {
	log.Printf("[ProcessManager] Starting...")

	// 启动 WebSocket 服务器
	if err := m.wsServer.Start(); err != nil {
		return fmt.Errorf("failed to start WebSocket server: %w", err)
	}

	// 启动监控循环
	go m.monitorLoop()

	return nil
}

// Stop 停止进程管理器
func (m *ProcessManager) Stop() error {
	log.Printf("[ProcessManager] Stopping...")

	m.cancel()

	// 终止所有子进程
	m.mu.Lock()
	for id, child := range m.children {
		log.Printf("[ProcessManager] Terminating child: %s", id)
		m.executor.TerminateChild(child)
		m.executor.Cleanup(child)
	}
	m.children = make(map[string]*ChildProcess)
	m.mu.Unlock()

	// 停止 WebSocket 服务器
	return m.wsServer.Stop()
}

// SpawnChild 创建子进程
func (m *ProcessManager) SpawnChild(windowType string, data interface{}) (string, <-chan interface{}, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	// 1. 生成子进程 ID
	m.nextID++
	childID := fmt.Sprintf("child-%d", m.nextID)

	log.Printf("[ProcessManager] Spawning child: %s (type: %s)", childID, windowType)

	// 2. 获取当前可执行文件路径
	exePath, err := os.Executable()
	if err != nil {
		return "", nil, fmt.Errorf("failed to get executable path: %w", err)
	}

	// 3. 创建子进程
	args := []string{"--multiple", "--child-id", childID, "--window-type", windowType}
	child, err := m.executor.SpawnChild(exePath, args)
	if err != nil {
		return "", nil, fmt.Errorf("failed to spawn child: %w", err)
	}

	child.ID = childID
	m.children[childID] = child

	log.Printf("[ProcessManager] Child %s created (PID: %d)", childID, child.PID)

	// Read stderr from child in background
	go func() {
		buf := make([]byte, 4096)
		for {
			n, err := child.Stderr.reader.Read(buf)
			if n > 0 {
				log.Printf("[ProcessManager] Child %s stderr: %s", childID, string(buf[:n]))
			}
			if err != nil {
				break
			}
		}
	}()

	// 4. 执行握手
	handshakeResult, err := ParentHandshake(child)
	if err != nil {
		m.executor.TerminateChild(child)
		delete(m.children, childID)
		return "", nil, fmt.Errorf("handshake failed: %w", err)
	}

	if !handshakeResult.Success {
		m.executor.TerminateChild(child)
		delete(m.children, childID)
		return "", nil, fmt.Errorf("handshake failed")
	}

	log.Printf("[ProcessManager] Handshake completed with child %s", childID)

	// 5. 生成 WebSocket 密钥
	wsPort := m.wsServer.GetPort()
	wsKey, err := m.keyGen.Generate(childID, child.PID)
	if err != nil {
		m.executor.TerminateChild(child)
		delete(m.children, childID)
		return "", nil, fmt.Errorf("failed to generate WS key: %w", err)
	}

	log.Printf("[ProcessManager] WS key generated for child %s: port=%d", childID, wsPort)

	// 6. 发送密钥到子进程
	if err := SendWSKey(child, wsKey.Key, wsPort, wsKey.Path); err != nil {
		m.executor.TerminateChild(child)
		delete(m.children, childID)
		return "", nil, fmt.Errorf("failed to send WS key: %w", err)
	}

	log.Printf("[ProcessManager] WS key sent to child %s", childID)

	// 6.5 发送窗口数据到子进程
	if err := SendWindowData(child, data); err != nil {
		m.executor.TerminateChild(child)
		delete(m.children, childID)
		return "", nil, fmt.Errorf("failed to send window data: %w", err)
	}

	log.Printf("[ProcessManager] Window data sent to child %s", childID)

	// 7. 创建结果通道
	resultCh := make(chan interface{}, 1)

	// 8. 等待子进程连接和发送结果
	go m.waitForChildResult(childID, resultCh)

	return childID, resultCh, nil
}

// waitForChildResult 等待子进程结果
func (m *ProcessManager) waitForChildResult(childID string, resultCh chan interface{}) {
	// 等待 WebSocket 连接建立（最多等待 10 秒）
	var conn *websocket.ChildConnection
	timeout := time.After(10 * time.Second)
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()

WaitForConnection:
	for {
		select {
		case <-ticker.C:
			conn = m.wsServer.GetConnection(childID)
			if conn != nil {
				break WaitForConnection
			}
		case <-timeout:
			resultCh <- map[string]interface{}{
				"success": false,
				"error":   "timeout waiting for connection",
			}
			return
		}
	}

	if conn == nil {
		resultCh <- map[string]interface{}{
			"success": false,
			"error":   "connection not found",
		}
		return
	}

	log.Printf("[ProcessManager] Connection established for child %s", childID)

	// 等待结果消息
	select {
	case data := <-conn.ReceiveCh:
		// 解析 JSON 字节数据
		var parsed map[string]interface{}
		if err := json.Unmarshal(data, &parsed); err != nil {
			resultCh <- map[string]interface{}{
				"success": false,
				"error":   fmt.Sprintf("failed to parse result: %v", err),
			}
		} else {
			resultCh <- parsed
		}
	case <-time.After(5 * time.Minute):
		resultCh <- map[string]interface{}{
			"success": false,
			"error":   "timeout waiting for result",
		}
	}

	// 清理
	m.TerminateChild(childID)
}

// TerminateChild 终止子进程
func (m *ProcessManager) TerminateChild(childID string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	child, ok := m.children[childID]
	if !ok {
		return fmt.Errorf("child not found: %s", childID)
	}

	log.Printf("[ProcessManager] Terminating child: %s", childID)

	// 终止进程
	if err := m.executor.TerminateChild(child); err != nil {
		log.Printf("[ProcessManager] Failed to terminate child %s: %v", childID, err)
	}

	// 清理资源
	m.executor.Cleanup(child)

	// 移除连接
	m.wsServer.RemoveConnection(childID)

	delete(m.children, childID)

	return nil
}

// monitorLoop 监控循环
func (m *ProcessManager) monitorLoop() {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-m.ctx.Done():
			return
		case <-ticker.C:
			m.cleanupStaleChildren()
		}
	}
}

// cleanupStaleChildren 清理僵尸子进程
func (m *ProcessManager) cleanupStaleChildren() {
	m.mu.Lock()
	defer m.mu.Unlock()

	for id, child := range m.children {
		if child.Cmd.Process == nil {
			continue
		}

		// 检查进程是否还在运行
		if !m.executor.IsProcessAlive(child) {
			log.Printf("[ProcessManager] Child %s is dead, cleaning up", id)
			m.executor.Cleanup(child)
			delete(m.children, id)
		}
	}
}

// GetChild 获取子进程
func (m *ProcessManager) GetChild(childID string) (*ChildProcess, bool) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	child, ok := m.children[childID]
	return child, ok
}
