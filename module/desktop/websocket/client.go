//go:build !cross_compile

package websocket

import (
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
	sendCh    chan interface{}
	receiveCh chan interface{}
	done      chan struct{}
	mu        sync.RWMutex
}

// NewWebSocketClient 创建 WebSocket 客户端
func NewWebSocketClient(wsKey *WebSocketKey) *WebSocketClient {
	serverURL := fmt.Sprintf("ws://127.0.0.1:%d%s", wsKey.Port, wsKey.Path)

	return &WebSocketClient{
		id:        wsKey.Key,
		key:       wsKey.Key,
		serverURL: serverURL,
		sendCh:    make(chan interface{}, 10),
		receiveCh: make(chan interface{}, 10),
		done:      make(chan struct{}),
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

// readLoop 读取循环
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

// writeLoop 写入循环
func (c *WebSocketClient) writeLoop() {
	for {
		select {
		case <-c.done:
			return
		case data := <-c.sendCh:
			if err := c.conn.WriteJSON(data); err != nil {
				log.Printf("[WebSocketClient-%s] Write error: %v", c.id, err)
				return
			}
		}
	}
}

// Send 发送消息
func (c *WebSocketClient) Send(data interface{}) error {
	select {
	case c.sendCh <- data:
		return nil
	case <-time.After(5 * time.Second):
		return ErrClientSendTimeout
	}
}

// Receive 接收消息（阻塞）
func (c *WebSocketClient) Receive() (interface{}, error) {
	select {
	case data := <-c.receiveCh:
		return data, nil
	case <-time.After(30 * time.Second):
		return nil, ErrClientReceiveTimeout
	}
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
)
