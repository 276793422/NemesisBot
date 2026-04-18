//go:build !cross_compile

package websocket

import "sync"

// ChildConnection 子进程连接
type ChildConnection struct {
	ID         string            // 连接 ID
	Key        string            // 密钥
	SendCh     chan []byte       // 发送通道
	ChildPID   int               // 子进程 PID
	Meta       map[string]string // 元数据
	Dispatcher *Dispatcher       // 消息处理器注册表
	closeOnce  sync.Once         // 保护 SendCh 关闭
}

// CloseSend 安全关闭 SendCh（sync.Once 防止重复关闭 panic）
func (c *ChildConnection) CloseSend() {
	c.closeOnce.Do(func() {
		close(c.SendCh)
	})
}
