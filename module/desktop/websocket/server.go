//go:build !cross_compile

package websocket

import (
	"context"
	"encoding/json"
	"log"
	"net"
	"net/http"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

// WebSocketServer WebSocket 服务器
type WebSocketServer struct {
	mu          sync.RWMutex
	listener    net.Listener
	port        int
	connections map[string]*ChildConnection
	keyGen      *KeyGenerator
	upgrader    websocket.Upgrader
	ctx         context.Context
	cancel      context.CancelFunc
}

// NewWebSocketServer 创建 WebSocket 服务器
func NewWebSocketServer(keyGen *KeyGenerator) *WebSocketServer {
	ctx, cancel := context.WithCancel(context.Background())

	return &WebSocketServer{
		connections: make(map[string]*ChildConnection),
		keyGen:      keyGen,
		upgrader: websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
			CheckOrigin: func(r *http.Request) bool {
				return true // 只允许本地连接
			},
		},
		ctx:    ctx,
		cancel: cancel,
	}
}

// Start 启动服务器
func (s *WebSocketServer) Start() error {
	log.Printf("[WebSocketServer] Starting...")

	// 动态分配端口
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return err
	}

	s.port = listener.Addr().(*net.TCPAddr).Port
	s.listener = listener

	log.Printf("[WebSocketServer] Listening on port %d", s.port)

	// 启动监听循环
	go s.acceptLoop()

	return nil
}

// acceptLoop 接受连接循环
func (s *WebSocketServer) acceptLoop() {
	// 创建 HTTP mux
	mux := http.NewServeMux()
	mux.HandleFunc("/", s.handleWebSocket)

	server := &http.Server{
		Handler: mux,
	}

	log.Printf("[WebSocketServer] Starting HTTP server on port %d", s.port)

	if err := server.Serve(s.listener); err != nil && err != http.ErrServerClosed {
		log.Printf("[WebSocketServer] Server error: %v", err)
	}
}

// handleWebSocket 处理 WebSocket 连接
func (s *WebSocketServer) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	// 升级到 WebSocket
	wsConn, err := s.upgrader.Upgrade(w, r, nil)
	if err != nil {
		log.Printf("[WebSocketServer] Upgrade failed: %v", err)
		return
	}

	s.handleConnection(wsConn)
}

// handleConnection 处理连接
func (s *WebSocketServer) handleConnection(wsConn *websocket.Conn) {
	// 读取第一个消息进行认证
	var msg map[string]interface{}
	if err := wsConn.ReadJSON(&msg); err != nil {
		log.Printf("[WebSocketServer] Read auth failed: %v", err)
		wsConn.Close()
		return
	}

	// 验证密钥
	key, _ := msg["key"].(string)
	wsKey, err := s.keyGen.Validate(key)
	if err != nil {
		log.Printf("[WebSocketServer] Auth failed: %v", err)
		wsConn.Close()
		return
	}

	log.Printf("[WebSocketServer] Child authenticated: PID=%d", wsKey.ChildPID)

	// 创建连接
	childConn := &ChildConnection{
		ID:        wsKey.Key,
		Key:       wsKey.Key,
		ChildPID:  wsKey.ChildPID,
		SendCh:    make(chan []byte, 10),
		ReceiveCh: make(chan []byte, 10),
		Meta:      make(map[string]string),
	}

	// 同时用 childID 和 UUID 存储
	s.mu.Lock()
	s.connections[wsKey.Key] = childConn
	if wsKey.ChildID != "" {
		s.connections[wsKey.ChildID] = childConn
		childConn.Meta["child_id"] = wsKey.ChildID
		log.Printf("[WebSocketServer] Connection registered: UUID=%s, ChildID=%s", wsKey.Key, wsKey.ChildID)
	}
	s.mu.Unlock()

	// 启动读写循环
	go s.readLoop(wsConn, childConn)
	go s.writeLoop(wsConn, childConn)
}

// readLoop 读取循环
func (s *WebSocketServer) readLoop(wsConn *websocket.Conn, conn *ChildConnection) {
	defer func() {
		s.RemoveConnection(conn.ID)
		wsConn.Close()
	}()

	for {
		select {
		case <-s.ctx.Done():
			return
		default:
			_, message, err := wsConn.ReadMessage()
			if err != nil {
				return
			}

			select {
			case conn.ReceiveCh <- message:
			case <-time.After(5 * time.Second):
				log.Printf("[WebSocketServer] Receive channel full")
			}
		}
	}
}

// writeLoop 写入循环
func (s *WebSocketServer) writeLoop(wsConn *websocket.Conn, conn *ChildConnection) {
	defer func() {
		wsConn.Close()
	}()

	for {
		select {
		case <-s.ctx.Done():
			return
		case message := <-conn.SendCh:
			if err := wsConn.WriteMessage(websocket.TextMessage, message); err != nil {
				return
			}
		}
	}
}

// Stop 停止服务器
func (s *WebSocketServer) Stop() error {
	log.Printf("[WebSocketServer] Stopping...")
	s.cancel()
	if s.listener != nil {
		return s.listener.Close()
	}
	return nil
}

// GetPort 获取端口
func (s *WebSocketServer) GetPort() int {
	return s.port
}

// SendToChild 发送消息到子进程
func (s *WebSocketServer) SendToChild(childID string, data interface{}) error {
	s.mu.RLock()
	conn, ok := s.connections[childID]
	s.mu.RUnlock()

	if !ok {
		return ErrConnectionNotFound
	}

	jsonData, err := json.Marshal(data)
	if err != nil {
		return err
	}

	select {
	case conn.SendCh <- jsonData:
		return nil
	case <-time.After(5 * time.Second):
		return ErrServerSendTimeout
	}
}

// RemoveConnection 移除连接
func (s *WebSocketServer) RemoveConnection(childID string) {
	s.mu.Lock()
	defer s.mu.Unlock()

	if conn, ok := s.connections[childID]; ok {
		// 安全关闭通道
		select {
		case <-conn.SendCh:
		default:
			close(conn.SendCh)
		}
		select {
		case <-conn.ReceiveCh:
		default:
			close(conn.ReceiveCh)
		}
		delete(s.connections, childID)
		log.Printf("[WebSocketServer] Connection removed: %s", childID)
	}
}

// GetConnection 获取连接
func (s *WebSocketServer) GetConnection(childID string) *ChildConnection {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.connections[childID]
}

// Errors
var (
	ErrConnectionNotFound = &WebSocketError{Code: "CONN_NOT_FOUND", Message: "Connection not found"}
	ErrServerSendTimeout  = &WebSocketError{Code: "SEND_TIMEOUT", Message: "Send timeout"}
)
