//go:build !cross_compile

package websocket

// WebSocketMessage WebSocket 消息
type WebSocketMessage struct {
	Type      string                 // "result", "event", "error"
	WindowID  string                 // 窗口 ID
	Data      map[string]interface{} // 数据
	Timestamp int64                  // 时间戳
}

// ChildConnection 子进程连接
type ChildConnection struct {
	ID       string            // 连接 ID
	Key      string            // 密钥
	SendCh   chan []byte       // 发送通道
	ReceiveCh chan []byte       // 接收通道
	ChildPID int               // 子进程 PID
	Meta     map[string]string // 元数据
}
