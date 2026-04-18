//go:build !cross_compile

package websocket

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

// WebSocketClient WebSocket 客户端
type WebSocketClient struct {
	id        string
	key       string
	serverURL string
	conn      *websocket.Conn
	sendCh    chan []byte
	receiveCh chan interface{}
	done      chan struct{}
	mu        sync.RWMutex

	// 新协议：Dispatcher 用于处理收到的 Request/Notification
	dispatcher *Dispatcher

	// 新协议：pending map 用于 Request-Response 关联
	pending   map[string]chan *Message
	pendingMu sync.RWMutex
}

// NewWebSocketClient 创建 WebSocket 客户端
func NewWebSocketClient(wsKey *WebSocketKey) *WebSocketClient {
	serverURL := fmt.Sprintf("ws://127.0.0.1:%d%s", wsKey.Port, wsKey.Path)

	return &WebSocketClient{
		id:         wsKey.Key,
		key:        wsKey.Key,
		serverURL:  serverURL,
		sendCh:     make(chan []byte, 10),
		receiveCh:  make(chan interface{}, 10),
		done:       make(chan struct{}),
		dispatcher: NewDispatcher(),
		pending:    make(map[string]chan *Message),
	}
}

// Connect 连接到服务器
func (c *WebSocketClient) Connect() error {
	log.Printf("[WebSocketClient-%s] Connecting to %s", c.id, c.serverURL)

	conn, _, err := websocket.DefaultDialer.Dial(c.serverURL, nil)
	if err != nil {
		return err
	}

	c.mu.Lock()
	c.conn = conn
	c.mu.Unlock()

	log.Printf("[WebSocketClient-%s] Connected", c.id)

	// 发送认证消息
	authMsg := map[string]interface{}{
		"type": "auth",
		"key":  c.key,
	}

	if err := c.conn.WriteJSON(authMsg); err != nil {
		c.conn.Close()
		return err
	}

	log.Printf("[WebSocketClient-%s] Authenticated", c.id)

	// 启动读写循环
	go c.readLoop()
	go c.writeLoop()

	return nil
}

// readLoop 读取循环（双协议检测）
func (c *WebSocketClient) readLoop() {
	defer func() {
		close(c.done)
	}()

	for {
		_, message, err := c.conn.ReadMessage()
		if err != nil {
			log.Printf("[WebSocketClient-%s] Read error: %v", c.id, err)
			return
		}

		// 尝试解析为新协议消息
		var msg Message
		if err := json.Unmarshal(message, &msg); err == nil && msg.JSONRPC == Version {
			c.handleProtocolMessage(&msg)
			continue
		}

		// 旧协议 fallback
		var data interface{}
		if err := json.Unmarshal(message, &data); err != nil {
			log.Printf("[WebSocketClient-%s] JSON decode error: %v", c.id, err)
			continue
		}

		select {
		case c.receiveCh <- data:
		case <-time.After(5 * time.Second):
			log.Printf("[WebSocketClient-%s] Receive channel full", c.id)
		}
	}
}

// handleProtocolMessage 处理新协议消息
func (c *WebSocketClient) handleProtocolMessage(msg *Message) {
	switch {
	case msg.IsResponse():
		// Response：路由到 pending map
		c.pendingMu.RLock()
		ch, ok := c.pending[msg.ID]
		c.pendingMu.RUnlock()
		if ok {
			select {
			case ch <- msg:
			default:
				log.Printf("[WebSocketClient-%s] Pending channel full for id=%s", c.id, msg.ID)
			}
		} else {
			log.Printf("[WebSocketClient-%s] No pending request for id=%s", c.id, msg.ID)
		}

	case msg.IsRequest() || msg.IsNotification():
		// Request/Notification：路由到 Dispatcher
		if c.dispatcher != nil {
			resp, err := c.dispatcher.Dispatch(context.Background(), msg)
			if err != nil {
				log.Printf("[WebSocketClient-%s] Dispatch error: %v", c.id, err)
			}
			// 如果是 Request 且有响应，发回去
			if msg.IsRequest() && resp != nil {
				if sendErr := c.sendRaw(resp); sendErr != nil {
					log.Printf("[WebSocketClient-%s] Failed to send response: %v", c.id, sendErr)
				}
			}
		}

	default:
		log.Printf("[WebSocketClient-%s] Unhandled protocol message: %+v", c.id, msg)
	}
}

// writeLoop 写入循环
func (c *WebSocketClient) writeLoop() {
	for {
		select {
		case <-c.done:
			return
		case data := <-c.sendCh:
			c.mu.RLock()
			conn := c.conn
			c.mu.RUnlock()
			if conn == nil {
				return
			}
			if err := conn.WriteMessage(websocket.TextMessage, data); err != nil {
				log.Printf("[WebSocketClient-%s] Write error: %v", c.id, err)
				return
			}
		}
	}
}

// sendRaw 发送原始消息（JSON 编码后发送）
func (c *WebSocketClient) sendRaw(msg *Message) error {
	data, err := json.Marshal(msg)
	if err != nil {
		return fmt.Errorf("marshal message: %w", err)
	}
	select {
	case c.sendCh <- data:
		return nil
	case <-time.After(5 * time.Second):
		return ErrClientSendTimeout
	}
}

// Send 发送消息（旧接口，兼容）
func (c *WebSocketClient) Send(data interface{}) error {
	raw, err := json.Marshal(data)
	if err != nil {
		return err
	}
	select {
	case c.sendCh <- raw:
		return nil
	case <-time.After(5 * time.Second):
		return ErrClientSendTimeout
	}
}

// Notify 发送 Notification（新协议，不等响应）
func (c *WebSocketClient) Notify(method string, params interface{}) error {
	msg, err := NewNotification(method, params)
	if err != nil {
		return fmt.Errorf("create notification: %w", err)
	}
	return c.sendRaw(msg)
}

// Call 发送 Request 并等待 Response（新协议，带 ID 关联）
func (c *WebSocketClient) Call(ctx context.Context, method string, params interface{}) (*Message, error) {
	msg, err := NewRequest(method, params)
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}

	// 注册 pending channel
	respCh := make(chan *Message, 1)
	c.pendingMu.Lock()
	c.pending[msg.ID] = respCh
	c.pendingMu.Unlock()

	defer func() {
		c.pendingMu.Lock()
		delete(c.pending, msg.ID)
		c.pendingMu.Unlock()
	}()

	// 发送
	if err := c.sendRaw(msg); err != nil {
		return nil, fmt.Errorf("send request: %w", err)
	}

	// 等待响应
	select {
	case resp := <-respCh:
		return resp, nil
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-time.After(30 * time.Second):
		return nil, ErrClientCallTimeout
	}
}

// Receive 接收消息（旧接口，阻塞）
func (c *WebSocketClient) Receive() (interface{}, error) {
	select {
	case data := <-c.receiveCh:
		return data, nil
	case <-time.After(30 * time.Second):
		return nil, ErrClientReceiveTimeout
	}
}

// RegisterHandler 注册 Request 处理器（处理父进程发来的 Request）
func (c *WebSocketClient) RegisterHandler(method string, fn HandlerFunc) {
	c.dispatcher.Register(method, fn)
}

// RegisterNotificationHandler 注册 Notification 处理器（处理父进程发来的 Notification）
func (c *WebSocketClient) RegisterNotificationHandler(method string, fn NotificationFunc) {
	c.dispatcher.RegisterNotification(method, fn)
}

// SetFallbackHandler 设置兜底处理器
func (c *WebSocketClient) SetFallbackHandler(fn HandlerFunc) {
	c.dispatcher.SetFallback(fn)
}

// Close 关闭连接
func (c *WebSocketClient) Close() error {
	log.Printf("[WebSocketClient-%s] Closing", c.id)
	close(c.done)

	c.mu.RLock()
	conn := c.conn
	c.mu.RUnlock()

	if conn != nil {
		return conn.Close()
	}

	return nil
}

// Errors
var (
	ErrClientSendTimeout    = &WebSocketError{Code: "SEND_TIMEOUT", Message: "Send timeout"}
	ErrClientReceiveTimeout = &WebSocketError{Code: "RECEIVE_TIMEOUT", Message: "Receive timeout"}
	ErrClientCallTimeout    = &WebSocketError{Code: "CALL_TIMEOUT", Message: "Call timeout"}
)
