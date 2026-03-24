//go:build !cross_compile

package process

import (
	"fmt"
	"log"
	"time"
)

const (
	// ProtocolName 协议名称
	ProtocolName = "anon-pipe-v1"
	// ProtocolVersion 协议版本
	ProtocolVersion = "1.0"
	// HandshakeTimeout 握手超时
	HandshakeTimeout = 3 * time.Second
	// AckTimeout ACK 超时
	AckTimeout = 5 * time.Second
)

// ParentHandshake 父进程握手
func ParentHandshake(child *ChildProcess) (*HandshakeResult, error) {
	log.Printf("[Parent] Starting handshake with child %s", child.ID)

	// 1. 发送握手消息
	handshakeMsg := &PipeMessage{
		Type:    "handshake",
		Version: ProtocolVersion,
		Data: map[string]interface{}{
			"protocol": ProtocolName,
			"version":  ProtocolVersion,
		},
	}

	if err := child.Stdin.Encode(handshakeMsg); err != nil {
		return nil, fmt.Errorf("failed to send handshake: %w", err)
	}
	log.Printf("[Parent] Handshake sent to child %s", child.ID)

	// 2. 等待 ACK
	ackChan := make(chan *PipeMessage, 1)
	errChan := make(chan error, 1)

	go func() {
		var ack PipeMessage
		if err := child.Stdout.Decode(&ack); err != nil {
			errChan <- err
			return
		}
		ackChan <- &ack
	}()

	select {
	case ack := <-ackChan:
		if ack.Type != "ack" {
			return nil, fmt.Errorf("expected ack, got %s", ack.Type)
		}
		log.Printf("[Parent] ACK received from child %s", child.ID)
	case err := <-errChan:
		return nil, fmt.Errorf("failed to receive ACK: %w", err)
	case <-time.After(AckTimeout):
		return nil, fmt.Errorf("ACK timeout")
	}

	return &HandshakeResult{Success: true}, nil
}

// ChildHandshake 子进程握手
// in: 从父进程读取消息（通过 stdin）
// out: 向父进程发送消息（通过 stdout）
func ChildHandshake(in *ReadCloser, out *WriteCloser) (*HandshakeResult, error) {
	// 1. 等待握手消息
	msgChan := make(chan *PipeMessage, 1)
	errChan := make(chan error, 1)

	go func() {
		var msg PipeMessage
		if err := in.Decode(&msg); err != nil {
			errChan <- err
			return
		}
		msgChan <- &msg
	}()

	select {
	case msg := <-msgChan:
		if msg.Type != "handshake" {
			return nil, fmt.Errorf("expected handshake, got %s", msg.Type)
		}
		childLog("Handshake received from parent")
	case err := <-errChan:
		return nil, fmt.Errorf("failed to receive handshake: %w", err)
	case <-time.After(HandshakeTimeout):
		return nil, fmt.Errorf("handshake timeout")
	}

	// 2. 发送 ACK
	ack := &PipeMessage{
		Type:    "ack",
		Version: ProtocolVersion,
		Data:    map[string]interface{}{"status": "ok"},
	}

	if err := out.Encode(ack); err != nil {
		return nil, fmt.Errorf("failed to send ACK: %w", err)
	}
	childLog("ACK sent to parent")

	return &HandshakeResult{Success: true}, nil
}

// SendWSKey 发送 WebSocket 密钥
func SendWSKey(child *ChildProcess, key string, port int, path string) error {
	log.Printf("[Parent] Sending WebSocket key to child %s", child.ID)

	msg := &PipeMessage{
		Type:    "ws_key",
		Version: ProtocolVersion,
		Data: map[string]interface{}{
			"key":  key,
			"port": port,
			"path": path,
		},
	}

	if err := child.Stdin.Encode(msg); err != nil {
		return fmt.Errorf("failed to send WS key: %w", err)
	}

	// 等待 ACK
	return waitForACK(child.Stdout)
}

// ReceiveWSKey 接收 WebSocket 密钥
func ReceiveWSKey(in *ReadCloser, out *WriteCloser) (key string, port int, path string, err error) {
	msgChan := make(chan *PipeMessage, 1)
	errChan := make(chan error, 1)

	go func() {
		var msg PipeMessage
		if e := in.Decode(&msg); e != nil {
			errChan <- e
			return
		}
		msgChan <- &msg
	}()

	select {
	case msg := <-msgChan:
		if msg.Type != "ws_key" {
			err = fmt.Errorf("expected ws_key, got %s", msg.Type)
			return
		}

		key, _ = msg.Data["key"].(string)
		portFloat, _ := msg.Data["port"].(float64)
		port = int(portFloat)
		path, _ = msg.Data["path"].(string)

		childLog("WebSocket key received: key=%s, port=%d, path=%s", key, port, path)
	case e := <-errChan:
		err = fmt.Errorf("failed to receive WS key: %w", e)
		return
	case <-time.After(AckTimeout):
		err = fmt.Errorf("WS key timeout")
		return
	}

	// 发送 ACK
	ack := &PipeMessage{
		Type:    "ack",
		Version: ProtocolVersion,
		Data:    map[string]interface{}{"status": "ok"},
	}

	if e := out.Encode(ack); e != nil {
		err = fmt.Errorf("failed to send ACK: %w", e)
		return
	}
	childLog("ACK sent for WS key")

	return
}

// SendWindowData 发送窗口数据
func SendWindowData(child *ChildProcess, data interface{}) error {
	log.Printf("[Parent] Sending window data to child %s", child.ID)

	msg := &PipeMessage{
		Type:    "window_data",
		Version: ProtocolVersion,
		Data: map[string]interface{}{
			"data": data,
		},
	}

	if err := child.Stdin.Encode(msg); err != nil {
		return fmt.Errorf("failed to send window data: %w", err)
	}

	// 等待 ACK
	return waitForACK(child.Stdout)
}

// ReceiveWindowData 接收窗口数据
func ReceiveWindowData(in *ReadCloser, out *WriteCloser) (interface{}, error) {
	msgChan := make(chan *PipeMessage, 1)
	errChan := make(chan error, 1)

	go func() {
		var msg PipeMessage
		if e := in.Decode(&msg); e != nil {
			errChan <- e
			return
		}
		msgChan <- &msg
	}()

	select {
	case msg := <-msgChan:
		if msg.Type != "window_data" {
			err := fmt.Errorf("expected window_data, got %s", msg.Type)
			return nil, err
		}

		// 发送 ACK
		ack := &PipeMessage{
			Type:    "ack",
			Version: ProtocolVersion,
			Data:    map[string]interface{}{"status": "ok"},
		}

		if e := out.Encode(ack); e != nil {
			err := fmt.Errorf("failed to send ACK: %w", e)
			return nil, err
		}
		childLog("ACK sent for window data")

		// 返回数据
		data, ok := msg.Data["data"].(interface{})
		if !ok {
			return nil, fmt.Errorf("invalid data format")
		}

		childLog("Window data received")
		return data, nil

	case e := <-errChan:
		return nil, fmt.Errorf("failed to receive window data: %w", e)

	case <-time.After(AckTimeout):
		return nil, fmt.Errorf("window data timeout")
	}
}

// waitForACK 等待 ACK
func waitForACK(stdout *ReadCloser) error {
	var ack PipeMessage
	if err := stdout.Decode(&ack); err != nil {
		return err
	}
	if ack.Type != "ack" {
		return fmt.Errorf("expected ack, got %s", ack.Type)
	}
	return nil
}
