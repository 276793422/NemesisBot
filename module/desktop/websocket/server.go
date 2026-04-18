//go:build !cross_compile

package websocket

import (
	"context"
	"encoding/json"
	"fmt"
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

	// 新协议：父进程侧 Dispatcher，处理子进程发来的 Request/Notification
	dispatcher *Dispatcher

	// 新协议：pending map 用于 CallChild 的 Request-Response 关联
	pending   map[string]chan *Message
	pendingMu sync.RWMutex
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
		ctx:        ctx,
		cancel:     cancel,
		dispatcher: NewDispatcher(),
		pending:    make(map[string]chan *Message),
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
	go s.acceptLoop(listener)

	return nil
}

// acceptLoop 接受连接循环
func (s *WebSocketServer) acceptLoop(ln net.Listener) {
	// 创建 HTTP mux
	mux := http.NewServeMux()
	mux.HandleFunc("/", s.handleWebSocket)

	server := &http.Server{
		Handler: mux,
	}

	log.Printf("[WebSocketServer] Starting HTTP server on port %d", s.port)

	if err := server.Serve(ln); err != nil && err != http.ErrServerClosed {
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

	// 创建连接（附带 Dispatcher）
	childConn := &ChildConnection{
		ID:         wsKey.Key,
		Key:        wsKey.Key,
		ChildPID:   wsKey.ChildPID,
		SendCh:     make(chan []byte, 10),
		ReceiveCh:  make(chan []byte, 10),
		Meta:       make(map[string]string),
		Dispatcher: NewDispatcher(),
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

// readLoop 读取循环（双协议检测）
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

			// 尝试解析为新协议消息
			var msg Message
			if err := json.Unmarshal(message, &msg); err == nil && msg.JSONRPC == Version {
				s.handleServerProtocolMessage(conn, &msg)
				continue
			}

			// 旧协议 fallback：放入 ReceiveCh
			select {
			case conn.ReceiveCh <- message:
			case <-time.After(5 * time.Second):
				log.Printf("[WebSocketServer] Receive channel full")
			}
		}
	}
}

// handleServerProtocolMessage 处理服务器端收到的协议消息
func (s *WebSocketServer) handleServerProtocolMessage(conn *ChildConnection, msg *Message) {
	switch {
	case msg.IsResponse():
		// Response：路由到 pending map（用于 CallChild）
		s.pendingMu.RLock()
		ch, ok := s.pending[msg.ID]
		s.pendingMu.RUnlock()
		if ok {
			select {
			case ch <- msg:
			default:
				log.Printf("[WebSocketServer] Pending channel full for id=%s", msg.ID)
			}
		}

	case msg.IsRequest() || msg.IsNotification():
		// 优先使用连接级 Dispatcher
		if conn.Dispatcher != nil {
			resp, err := conn.Dispatcher.Dispatch(s.ctx, msg)
			if err != nil {
				log.Printf("[WebSocketServer] Connection dispatcher error: %v", err)
			}
			if msg.IsRequest() && resp != nil {
				s.sendToConn(conn, resp)
			}
			return
		}
		// 其次使用服务器级 Dispatcher
		if s.dispatcher != nil {
			resp, err := s.dispatcher.Dispatch(s.ctx, msg)
			if err != nil {
				log.Printf("[WebSocketServer] Server dispatcher error: %v", err)
			}
			if msg.IsRequest() && resp != nil {
				s.sendToConn(conn, resp)
			}
		}

	default:
		log.Printf("[WebSocketServer] Unhandled protocol message: %+v", msg)
	}
}

// sendToConn 向指定连接发送协议消息
func (s *WebSocketServer) sendToConn(conn *ChildConnection, msg *Message) {
	data, err := json.Marshal(msg)
	if err != nil {
		log.Printf("[WebSocketServer] Failed to marshal message: %v", err)
		return
	}
	select {
	case conn.SendCh <- data:
	case <-time.After(5 * time.Second):
		log.Printf("[WebSocketServer] Send channel full for %s", conn.ID)
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
		err := s.listener.Close()
		s.listener = nil
		return err
	}
	return nil
}

// GetPort 获取端口
func (s *WebSocketServer) GetPort() int {
	return s.port
}

// SendToChild 发送消息到子进程（旧接口，兼容）
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

// SendNotification 向子进程发送 Notification（新协议）
func (s *WebSocketServer) SendNotification(childID string, method string, params interface{}) error {
	s.mu.RLock()
	conn, ok := s.connections[childID]
	s.mu.RUnlock()

	if !ok {
		return ErrConnectionNotFound
	}

	msg, err := NewNotification(method, params)
	if err != nil {
		return fmt.Errorf("create notification: %w", err)
	}

	s.sendToConn(conn, msg)
	return nil
}

// CallChild 向子进程发送 Request 并等待 Response（新协议）
func (s *WebSocketServer) CallChild(ctx context.Context, childID string, method string, params interface{}) (*Message, error) {
	s.mu.RLock()
	conn, ok := s.connections[childID]
	s.mu.RUnlock()

	if !ok {
		return nil, ErrConnectionNotFound
	}

	msg, err := NewRequest(method, params)
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}

	// 注册 pending channel
	respCh := make(chan *Message, 1)
	s.pendingMu.Lock()
	s.pending[msg.ID] = respCh
	s.pendingMu.Unlock()

	defer func() {
		s.pendingMu.Lock()
		delete(s.pending, msg.ID)
		s.pendingMu.Unlock()
	}()

	// 发送
	s.sendToConn(conn, msg)

	// 等待响应
	select {
	case resp := <-respCh:
		return resp, nil
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-time.After(30 * time.Second):
		return nil, ErrServerCallTimeout
	}
}

// RegisterHandler 注册处理子进程发来的 Request（服务器级）
func (s *WebSocketServer) RegisterHandler(method string, fn HandlerFunc) {
	s.dispatcher.Register(method, fn)
}

// RegisterNotificationHandler 注册处理子进程发来的 Notification（服务器级）
func (s *WebSocketServer) RegisterNotificationHandler(method string, fn NotificationFunc) {
	s.dispatcher.RegisterNotification(method, fn)
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
	ErrServerCallTimeout  = &WebSocketError{Code: "CALL_TIMEOUT", Message: "Call timeout"}
)
