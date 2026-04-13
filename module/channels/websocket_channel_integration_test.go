// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/gorilla/websocket"
)

// getFreePort returns a free TCP port on localhost
func getFreePort(t *testing.T) int {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("failed to get free port: %v", err)
	}
	defer ln.Close()
	return ln.Addr().(*net.TCPAddr).Port
}

func TestWebSocketChannel_StartStop(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host: "127.0.0.1",
		Port: getFreePort(t),
		Path: "/ws",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	if !ch.IsRunning() {
		t.Error("should be running")
	}

	stopCtx, stopCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer stopCancel()
	if err := ch.Stop(stopCtx); err != nil {
		t.Fatalf("stop failed: %v", err)
	}
	if ch.IsRunning() {
		t.Error("should not be running after stop")
	}
}

func TestWebSocketChannel_ClientConnect(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host: "127.0.0.1",
		Port: getFreePort(t),
		Path: "/ws",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}

	// Extract address from channel
	addr := ch.server.Addr

	// Connect a client
	wsURL := "ws://" + addr + "/ws"
	wsConn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer wsConn.Close()

	// Read welcome message
	wsConn.SetReadDeadline(time.Now().Add(2 * time.Second))
	_, data, err := wsConn.ReadMessage()
	if err != nil {
		t.Fatalf("failed to read welcome: %v", err)
	}

	var welcome ServerMessage
	if err := json.Unmarshal(data, &welcome); err != nil {
		t.Fatalf("failed to unmarshal welcome: %v", err)
	}
	if welcome.Type != MessageTypeMessage {
		t.Errorf("welcome type = %q, want 'message'", welcome.Type)
	}
	if !strings.Contains(welcome.Content, "Connected to NemesisBot") {
		t.Errorf("welcome content = %q", welcome.Content)
	}

	stopCtx, stopCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer stopCancel()
	ch.Stop(stopCtx)
}

func TestWebSocketChannel_SendMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host: "127.0.0.1",
		Port: getFreePort(t),
		Path: "/ws",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}

	// Connect client
	wsURL := "ws://" + ch.server.Addr + "/ws"
	wsConn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer wsConn.Close()

	// Read welcome
	wsConn.SetReadDeadline(time.Now().Add(2 * time.Second))
	wsConn.ReadMessage()

	// Send message to client
	err = ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "test-chat",
		Content: "hello from server",
	})
	if err != nil {
		t.Fatalf("send failed: %v", err)
	}

	// Read the message
	wsConn.SetReadDeadline(time.Now().Add(2 * time.Second))
	_, data, err := wsConn.ReadMessage()
	if err != nil {
		t.Fatalf("failed to read message: %v", err)
	}

	var msg ServerMessage
	if err := json.Unmarshal(data, &msg); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if msg.Content != "hello from server" {
		t.Errorf("content = %q", msg.Content)
	}
	if msg.Role != "assistant" {
		t.Errorf("role = %q", msg.Role)
	}

	stopCtx, stopCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer stopCancel()
	ch.Stop(stopCtx)
}

func TestWebSocketChannel_ReceiveMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host: "127.0.0.1",
		Port: getFreePort(t),
		Path: "/ws",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}

	// Subscribe to inbound
	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx2, cancel2 := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel2()
		if msg, ok := msgBus.ConsumeInbound(ctx2); ok {
			received <- msg
		}
	}()

	// Connect client
	wsURL := "ws://" + ch.server.Addr + "/ws"
	wsConn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer wsConn.Close()

	// Read welcome
	wsConn.SetReadDeadline(time.Now().Add(2 * time.Second))
	wsConn.ReadMessage()

	// Send message from client
	clientMsg := ClientMessage{
		Type:      MessageTypeMessage,
		Content:   "hello from client",
		Timestamp: time.Now(),
	}
	data, _ := json.Marshal(clientMsg)
	if err := wsConn.WriteMessage(websocket.TextMessage, data); err != nil {
		t.Fatalf("failed to write: %v", err)
	}

	// Wait for message to be received via bus
	select {
	case msg := <-received:
		if msg.Content != "hello from client" {
			t.Errorf("content = %q", msg.Content)
		}
		if msg.Channel != "websocket" {
			t.Errorf("channel = %q", msg.Channel)
		}
	case <-time.After(3 * time.Second):
		t.Error("timed out waiting for message")
	}

	stopCtx, stopCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer stopCancel()
	ch.Stop(stopCtx)
}

func TestWebSocketChannel_PingPong(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host: "127.0.0.1",
		Port: getFreePort(t),
		Path: "/ws",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}

	// Connect client
	wsURL := "ws://" + ch.server.Addr + "/ws"
	wsConn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer wsConn.Close()

	// Read welcome
	wsConn.SetReadDeadline(time.Now().Add(2 * time.Second))
	wsConn.ReadMessage()

	// Send ping
	pingMsg := ClientMessage{Type: MessageTypePing, Timestamp: time.Now()}
	data, _ := json.Marshal(pingMsg)
	if err := wsConn.WriteMessage(websocket.TextMessage, data); err != nil {
		t.Fatalf("failed to write ping: %v", err)
	}

	// Read pong
	wsConn.SetReadDeadline(time.Now().Add(2 * time.Second))
	_, resp, err := wsConn.ReadMessage()
	if err != nil {
		t.Fatalf("failed to read pong: %v", err)
	}

	var pong ServerMessage
	if err := json.Unmarshal(resp, &pong); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if pong.Type != MessageTypePong {
		t.Errorf("expected pong, got %q", pong.Type)
	}

	stopCtx, stopCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer stopCancel()
	ch.Stop(stopCtx)
}

func TestWebSocketChannel_SendWhenNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host: "127.0.0.1",
		Port: getFreePort(t),
		Path: "/ws",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "test",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error when not running")
	}
}

func TestWebSocketChannel_AuthToken(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host:      "127.0.0.1",
		Port:      getFreePort(t),
		Path:      "/ws",
		AuthToken: "secret123",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}

	// Connect without token - should fail
	wsURL := "ws://" + ch.server.Addr + "/ws"
	_, resp, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err == nil {
		t.Error("expected error connecting without token")
	}
	if resp != nil && resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("status = %d, want %d", resp.StatusCode, http.StatusUnauthorized)
	}

	// Connect with wrong token - should fail
	wsURLWrong := "ws://" + ch.server.Addr + "/ws?token=wrong"
	_, resp, _ = websocket.DefaultDialer.Dial(wsURLWrong, nil)
	if resp != nil && resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("status = %d, want %d", resp.StatusCode, http.StatusUnauthorized)
	}

	// Connect with correct token
	wsURLCorrect := "ws://" + ch.server.Addr + "/ws?token=secret123"
	wsConn, _, err := websocket.DefaultDialer.Dial(wsURLCorrect, nil)
	if err != nil {
		t.Fatalf("failed to connect with correct token: %v", err)
	}
	wsConn.Close()

	stopCtx, stopCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer stopCancel()
	ch.Stop(stopCtx)
}

// Test WebSocketChannel rejects second client when one is connected
func TestWebSocketChannel_RejectSecondClient(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host: "127.0.0.1",
		Port: getFreePort(t),
		Path: "/ws",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}

	wsURL := "ws://" + ch.server.Addr + "/ws"

	// Connect first client
	wsConn1, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("first client failed: %v", err)
	}
	defer wsConn1.Close()

	// Try second client - should be rejected
	_, resp, _ := websocket.DefaultDialer.Dial(wsURL, nil)
	if resp != nil && resp.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("expected 503 for second client, got %d", resp.StatusCode)
	}

	stopCtx, stopCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer stopCancel()
	ch.Stop(stopCtx)
}

// Test send when no client connected
func TestWebSocketChannel_SendNoClient(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := &config.WebSocketChannelConfig{
		Host: "127.0.0.1",
		Port: getFreePort(t),
		Path: "/ws",
	}
	ch, err := NewWebSocketChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}

	// Send without any client
	err = ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "test",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error when no client connected")
	}
	if !strings.Contains(err.Error(), "no client connected") {
		t.Errorf("error = %v", err)
	}

	stopCtx, stopCancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer stopCancel()
	ch.Stop(stopCtx)
}

// httptest server helper for future use
var _ = httptest.NewServer
