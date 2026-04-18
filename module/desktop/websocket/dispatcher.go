//go:build !cross_compile

package websocket

import (
	"context"
	"fmt"
	"sync"
)

// HandlerFunc 处理 Request 消息，返回 Response 或 error
type HandlerFunc func(ctx context.Context, msg *Message) (*Message, error)

// NotificationFunc 处理 Notification 消息（无返回值）
type NotificationFunc func(ctx context.Context, msg *Message)

// Dispatcher 可扩展的消息处理器注册表
type Dispatcher struct {
	mu            sync.RWMutex
	handlers      map[string]HandlerFunc
	notifHandlers map[string]NotificationFunc
	fallback      HandlerFunc
}

// NewDispatcher 创建新的 Dispatcher
func NewDispatcher() *Dispatcher {
	return &Dispatcher{
		handlers:      make(map[string]HandlerFunc),
		notifHandlers: make(map[string]NotificationFunc),
	}
}

// Register 注册 Request 处理器
func (d *Dispatcher) Register(method string, fn HandlerFunc) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.handlers[method] = fn
}

// RegisterNotification 注册 Notification 处理器
func (d *Dispatcher) RegisterNotification(method string, fn NotificationFunc) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.notifHandlers[method] = fn
}

// SetFallback 设置未注册方法的兜底处理器
func (d *Dispatcher) SetFallback(fn HandlerFunc) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.fallback = fn
}

// Dispatch 分发消息到对应的处理器
func (d *Dispatcher) Dispatch(ctx context.Context, msg *Message) (*Message, error) {
	d.mu.RLock()
	defer d.mu.RUnlock()

	switch {
	case msg.IsRequest():
		return d.dispatchRequest(ctx, msg)
	case msg.IsNotification():
		d.dispatchNotification(ctx, msg)
		return nil, nil
	default:
		return nil, fmt.Errorf("message is neither request nor notification")
	}
}

func (d *Dispatcher) dispatchRequest(ctx context.Context, msg *Message) (*Message, error) {
	if fn, ok := d.handlers[msg.Method]; ok {
		return fn(ctx, msg)
	}
	if d.fallback != nil {
		return d.fallback(ctx, msg)
	}
	resp, _ := NewErrorResponse(msg.ID, ErrMethodNotFound, "method not found: "+msg.Method, nil)
	return resp, nil
}

func (d *Dispatcher) dispatchNotification(ctx context.Context, msg *Message) {
	if fn, ok := d.notifHandlers[msg.Method]; ok {
		fn(ctx, msg)
		return
	}
	// Notification 无 fallback，静默忽略
}
