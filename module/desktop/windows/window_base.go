//go:build !cross_compile

package windows

import (
	"context"
	"log"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/desktop/websocket"
)

// WindowData 窗口数据接口
type WindowData interface {
	Validate() error
	GetType() string
}

// WindowBase 窗口基类
type WindowBase struct {
	ID       string
	Type     string
	Data     WindowData
	WSClient *websocket.WebSocketClient
	ResultCh chan interface{}
	mu       sync.RWMutex
	ctx      context.Context
	cancel   context.CancelFunc
}

// NewWindowBase 创建窗口基类
func NewWindowBase(windowID, windowType string, data WindowData, wsClient *websocket.WebSocketClient) *WindowBase {
	ctx, cancel := context.WithCancel(context.Background())

	return &WindowBase{
		ID:       windowID,
		Type:     windowType,
		Data:     data,
		WSClient: wsClient,
		ResultCh: make(chan interface{}, 1),
		ctx:      ctx,
		cancel:   cancel,
	}
}

// Startup 启动窗口
func (w *WindowBase) Startup(ctx context.Context) error {
	w.ctx = ctx
	log.Printf("[Window-%s] Startup", w.ID)
	return nil
}

// Shutdown 关闭窗口
func (w *WindowBase) Shutdown(ctx context.Context) {
	log.Printf("[Window-%s] Shutdown", w.ID)
	w.cancel()
}

// GetID 获取窗口 ID
func (w *WindowBase) GetID() string {
	return w.ID
}

// GetType 获取窗口类型
func (w *WindowBase) GetType() string {
	return w.Type
}

// GetData 获取窗口数据
func (w *WindowBase) GetData() WindowData {
	return w.Data
}

// SetData 设置窗口数据
func (w *WindowBase) SetData(data WindowData) error {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.Data = data
	return nil
}

// SendResult 发送结果到父进程
func (w *WindowBase) SendResult(result interface{}) {
	log.Printf("[Window-%s] Sending result: %+v", w.ID, result)

	// 通过 WebSocket 发送
	if err := w.WSClient.Send(map[string]interface{}{
		"type":     "result",
		"window":   w.ID,
		"data":     result,
		"timestamp": time.Now().Unix(),
	}); err != nil {
		log.Printf("[Window-%s] Failed to send result: %v", w.ID, err)
		return
	}

	// 同时发送到结果通道（用于本地等待）
	select {
	case w.ResultCh <- result:
	default:
		log.Printf("[Window-%s] Result channel full", w.ID)
	}
}

// ReceiveFromParent 从父进程接收数据
func (w *WindowBase) ReceiveFromParent() (interface{}, error) {
	log.Printf("[Window-%s] Waiting for data from parent...", w.ID)

	data, err := w.WSClient.Receive()
	if err != nil {
		return nil, err
	}

	log.Printf("[Window-%s] Received data from parent", w.ID)
	return data, nil
}

// Bind 返回绑定结构（由子类重写）
func (w *WindowBase) Bind() []interface{} {
	return []interface{}{w}
}
