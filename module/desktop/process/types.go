//go:build !cross_compile

package process

import (
	"encoding/json"
	"os"
	"os/exec"
	"time"
)

// PlatformSpecific 平台特定数据接口
type PlatformSpecific interface{}

// ChildProcess 子进程抽象
type ChildProcess struct {
	ID        string
	Cmd       *exec.Cmd
	PID       int
	Stdin     *WriteCloser
	Stdout    *ReadCloser
	Stderr    *ReadCloser
	Platform  PlatformSpecific
	CreatedAt time.Time
}

// WriteCloser 带编码器的写入关闭器
type WriteCloser struct {
	*json.Encoder
	writer *os.File
}

// ReadCloser 带解码器的读取关闭器
type ReadCloser struct {
	*json.Decoder
	reader *os.File
}

// Close 关闭写入器
func (w *WriteCloser) Close() error {
	return w.writer.Close()
}

// Close 关闭读取器
func (r *ReadCloser) Close() error {
	return r.reader.Close()
}

// ProcessStatus 进程状态
type ProcessStatus int

const (
	ProcessStatusStarting ProcessStatus = iota
	ProcessStatusRunning
	ProcessStatusHandshaking
	ProcessStatusConnected
	ProcessStatusTerminated
	ProcessStatusFailed
)

// HandshakeResult 握手结果
type HandshakeResult struct {
	Success bool
	WindowID string
	Error    error
}

// PipeMessage 管道消息
type PipeMessage struct {
	Type    string                 // "handshake", "ws_key", "ack", "error"
	Version string                 // "1.0"
	Data    map[string]interface{} // 附加数据
}

// ChildResult 子进程结果
type ChildResult struct {
	Success bool
	Data    interface{}
	Error   error
}
