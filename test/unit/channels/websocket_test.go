// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// WebSocket Channel Unit Tests

package channels_test

import (
	"context"
	"net/http"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	. "github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/gorilla/websocket"
)

// TestNewWebSocketChannelSuccess tests successful creation of WebSocket channel
func TestNewWebSocketChannelSuccess(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:   true,
		Host:      "127.0.0.1",
		Port:      49901,
		Path:      "/ws",
		AuthToken: "",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	if channel == nil {
		t.Fatal("Expected channel to be created, got nil")
	}

	if channel.Name() != "websocket" {
		t.Errorf("Expected channel name 'websocket', got: %s", channel.Name())
	}
}

// TestWebSocketChannelStartStop tests starting and stopping the WebSocket server
func TestWebSocketChannelStartStop(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:   true,
		Host:      "127.0.0.1",
		Port:      49902,
		Path:      "/ws",
		AuthToken: "",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()
	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start the server
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start WebSocket channel: %v", err)
	}

	if !channel.IsRunning() {
		t.Error("Expected channel to be running after Start()")
	}

	// Give server time to fully start
	time.Sleep(100 * time.Millisecond)

	// Stop the server
	if err := channel.Stop(ctx); err != nil {
		t.Fatalf("Failed to stop WebSocket channel: %v", err)
	}

	if channel.IsRunning() {
		t.Error("Expected channel to not be running after Stop()")
	}
}

// TestWebSocketChannelClientConnection tests client connection
func TestWebSocketChannelClientConnection(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:   true,
		Host:      "127.0.0.1",
		Port:      49903,
		Path:      "/ws",
		AuthToken: "",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()
	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start the server
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start WebSocket channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Give server time to start
	time.Sleep(100 * time.Millisecond)

	// Connect a client
	wsURL := url.URL{
		Scheme: "ws",
		Host:   "127.0.0.1:49903",
		Path:   "/ws",
	}

	conn, _, err := websocket.DefaultDialer.Dial(wsURL.String(), nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket server: %v", err)
	}
	defer conn.Close()

	// Read welcome message
	_, message, err := conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome message: %v", err)
	}

	if !strings.Contains(string(message), "Connected to NemesisBot") {
		t.Errorf("Expected welcome message, got: %s", string(message))
	}
}

// TestWebSocketChannelSingleClientOnly tests that only one client can connect
func TestWebSocketChannelSingleClientOnly(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:   true,
		Host:      "127.0.0.1",
		Port:      49904,
		Path:      "/ws",
		AuthToken: "",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()
	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start the server
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start WebSocket channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Give server time to start
	time.Sleep(100 * time.Millisecond)

	// Connect first client
	wsURL := url.URL{
		Scheme: "ws",
		Host:   "127.0.0.1:49904",
		Path:   "/ws",
	}

	conn1, _, err := websocket.DefaultDialer.Dial(wsURL.String(), nil)
	if err != nil {
		t.Fatalf("Failed to connect first client: %v", err)
	}
	defer conn1.Close()

	// Give first connection time to establish
	time.Sleep(50 * time.Millisecond)

	// Try to connect second client
	conn2, resp, err := websocket.DefaultDialer.Dial(wsURL.String(), nil)
	if err == nil {
		conn2.Close()
		t.Error("Expected error when connecting second client, got nil")
	}

	if resp == nil {
		t.Error("Expected HTTP response for second connection, got nil")
	} else if resp.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("Expected status 503 Service Unavailable, got: %d", resp.StatusCode)
	}
}

// TestWebSocketChannelMessageExchange tests sending and receiving messages
func TestWebSocketChannelMessageExchange(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:   true,
		Host:      "127.0.0.1",
		Port:      49905,
		Path:      "/ws",
		AuthToken: "",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()
	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start the server
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start WebSocket channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Give server time to start
	time.Sleep(100 * time.Millisecond)

	// Subscribe to inbound messages
	var receivedContent atomic.Value
	go func() {
		for {
			msg, ok := testBus.ConsumeInbound(ctx)
			if !ok {
				return
			}
			if msg.Channel == "websocket" {
				receivedContent.Store(msg.Content)
			}
		}
	}()

	// Connect client
	wsURL := url.URL{
		Scheme: "ws",
		Host:   "127.0.0.1:49905",
		Path:   "/ws",
	}

	conn, _, err := websocket.DefaultDialer.Dial(wsURL.String(), nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket server: %v", err)
	}
	defer conn.Close()

	// Read welcome message
	_, _, err = conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome message: %v", err)
	}

	// Send message from client
	clientMsg := map[string]interface{}{
		"type":    "message",
		"content": "Hello, WebSocket!",
	}
	if err := conn.WriteJSON(clientMsg); err != nil {
		t.Fatalf("Failed to send message: %v", err)
	}

	// Wait for message to be processed
	timeout := time.After(2 * time.Second)
	ticker := time.NewTicker(50 * time.Millisecond)
	defer ticker.Stop()

	for {
		select {
		case <-timeout:
			t.Fatal("Timeout waiting for message to be processed")
		case <-ticker.C:
			content := receivedContent.Load()
			if content != nil {
				if content.(string) != "Hello, WebSocket!" {
					t.Errorf("Expected 'Hello, WebSocket!', got: %s", content.(string))
				}
				return
			}
		}
	}
}

// TestWebSocketChannelSendToClient tests sending messages to the client
func TestWebSocketChannelSendToClient(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:   true,
		Host:      "127.0.0.1",
		Port:      49906,
		Path:      "/ws",
		AuthToken: "",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()
	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start the server
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start WebSocket channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Give server time to start
	time.Sleep(100 * time.Millisecond)

	// Connect client
	wsURL := url.URL{
		Scheme: "ws",
		Host:   "127.0.0.1:49906",
		Path:   "/ws",
	}

	conn, _, err := websocket.DefaultDialer.Dial(wsURL.String(), nil)
	if err != nil {
		t.Fatalf("Failed to connect to WebSocket server: %v", err)
	}
	defer conn.Close()

	// Read welcome message
	_, _, err = conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome message: %v", err)
	}

	// Send message to client via channel
	outboundMsg := bus.OutboundMessage{
		Channel: "websocket",
		ChatID:  "websocket:client_123",
		Content: "Hello from server!",
	}

	if err := channel.Send(ctx, outboundMsg); err != nil {
		t.Fatalf("Failed to send message to client: %v", err)
	}

	// Read message from client
	_, message, err := conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read message from server: %v", err)
	}

	if !strings.Contains(string(message), "Hello from server!") {
		t.Errorf("Expected 'Hello from server!', got: %s", string(message))
	}
}

// TestWebSocketChannelAuthentication tests token authentication
func TestWebSocketChannelAuthentication(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:   true,
		Host:      "127.0.0.1",
		Port:      49907,
		Path:      "/ws",
		AuthToken: "test_token",
		SyncToWeb: false,
	}

	testBus := bus.NewMessageBus()
	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Start the server
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start WebSocket channel: %v", err)
	}
	defer channel.Stop(ctx)

	// Give server time to start
	time.Sleep(100 * time.Millisecond)

	// Try to connect without token
	wsURL := url.URL{
		Scheme: "ws",
		Host:   "127.0.0.1:49907",
		Path:   "/ws",
	}

	conn, resp, err := websocket.DefaultDialer.Dial(wsURL.String(), nil)
	if err == nil {
		conn.Close()
		t.Error("Expected authentication error without token, got nil")
	}

	if resp != nil && resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("Expected status 401 Unauthorized, got: %d", resp.StatusCode)
	}

	// Connect with correct token
	wsURL.RawQuery = "token=test_token"
	conn, _, err = websocket.DefaultDialer.Dial(wsURL.String(), nil)
	if err != nil {
		t.Fatalf("Failed to connect with token: %v", err)
	}
	defer conn.Close()

	// Read welcome message
	_, message, err := conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome message: %v", err)
	}

	if !strings.Contains(string(message), "Connected to NemesisBot") {
		t.Errorf("Expected welcome message, got: %s", string(message))
	}
}
