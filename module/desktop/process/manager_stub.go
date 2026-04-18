//go:build cross_compile

package process

import (
	"context"
	"fmt"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
)

// ProcessManager 进程管理器（cross_compile 存根）
type ProcessManager struct{}

// NewProcessManager 创建进程管理器（存根）
func NewProcessManager() *ProcessManager {
	return &ProcessManager{}
}

// Start 启动进程管理器（存根）
func (m *ProcessManager) Start() error {
	return fmt.Errorf("ProcessManager is not available in cross-compile builds")
}

// Stop 停止进程管理器（存根）
func (m *ProcessManager) Stop() error {
	return nil
}

// SpawnChild 创建子进程（存根）
func (m *ProcessManager) SpawnChild(windowType string, data interface{}) (string, <-chan interface{}, error) {
	return "", nil, fmt.Errorf("ProcessManager is not available in cross-compile builds")
}

// NotifyChild 向子进程发送 Notification（存根）
func (m *ProcessManager) NotifyChild(childID string, method string, params interface{}) error {
	return fmt.Errorf("ProcessManager is not available in cross-compile builds")
}

// CallChild 向子进程发送 Request 并等待 Response（存根）
func (m *ProcessManager) CallChild(ctx context.Context, childID string, method string, params interface{}) (*websocket.Message, error) {
	return nil, fmt.Errorf("ProcessManager is not available in cross-compile builds")
}
