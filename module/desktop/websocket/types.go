//go:build !cross_compile

package websocket

// ChildConnection 子进程连接
type ChildConnection struct {
	ID        string            // 连接 ID
	Key       string            // 密钥
	SendCh    chan []byte       // 发送通道
	ReceiveCh chan []byte       // 接收通道
	ChildPID  int               // 子进程 PID
	Meta      map[string]string // 元数据
	Dispatcher *Dispatcher      // 消息处理器注册表
}
