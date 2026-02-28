// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// WebSocket Channel Integration Tests

package channels_test

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gorilla/websocket"
	. "github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

// TestIntegrationWebSocketChannelFullLifecycle tests complete lifecycle with real WebSocket communication
func TestIntegrationWebSocketChannelFullLifecycle(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:    true,
		Host:       "127.0.0.1",
		Port:       49910,
		Path:       "/ws",
		AuthToken:  "",
		SyncToWeb:  false,
	}

	testBus := bus.NewMessageBus()

	// Create channel
	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	// Start channel
	t.Log("Starting WebSocket channel...")
	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	if !channel.IsRunning() {
		t.Fatal("Channel should be running after Start()")
	}

	// Wait for server to fully start
	time.Sleep(200 * time.Millisecond)

	// Connect WebSocket client
	t.Log("Connecting WebSocket client...")
	wsURL := "ws://127.0.0.1:49910/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect WebSocket client: %v", err)
	}
	defer conn.Close()

	// Read welcome message
	_, welcomeMsg, err := conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome message: %v", err)
	}
	t.Logf("Received welcome: %s", string(welcomeMsg))

	// Subscribe to inbound messages
	var inboundContent atomic.Value
	inboundDone := make(chan bool)
	go func() {
		for {
			msg, ok := testBus.ConsumeInbound(ctx)
			if !ok {
				return
			}
			if msg.Channel == "websocket" {
				inboundContent.Store(msg.Content)
				inboundDone <- true
			}
		}
	}()

	// Send message from client
	t.Log("Sending message from client...")
	clientMsg := map[string]interface{}{
		"type":    "message",
		"content": "Hello from integration test!",
	}
	if err := conn.WriteJSON(clientMsg); err != nil {
		t.Fatalf("Failed to send message: %v", err)
	}

	// Wait for message to reach bus
	select {
	case <-inboundDone:
		content := inboundContent.Load()
		if content == nil || content.(string) != "Hello from integration test!" {
			t.Errorf("Expected 'Hello from integration test!', got: %v", content)
		}
		t.Log("✓ Message successfully received on inbound bus")
	case <-time.After(3 * time.Second):
		t.Fatal("Timeout waiting for inbound message")
	}

	// Send response back to client via channel
	t.Log("Sending response to client...")
	outboundMsg := bus.OutboundMessage{
		Channel: "websocket",
		ChatID:  "websocket:client_123",
		Content: "Response from integration test!",
	}

	if err := channel.Send(ctx, outboundMsg); err != nil {
		t.Fatalf("Failed to send outbound message: %v", err)
	}

	// Read response from client
	_, response, err := conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read response from server: %v", err)
	}

	var serverMsg map[string]interface{}
	if err := json.Unmarshal(response, &serverMsg); err != nil {
		t.Fatalf("Failed to parse server message: %v", err)
	}

	if serverMsg["content"] != "Response from integration test!" {
		t.Errorf("Expected 'Response from integration test!', got: %v", serverMsg["content"])
	}
	t.Log("✓ Client received response from server")
}

// TestIntegrationWebSocketChannelBidirectionalCommunication tests bidirectional message flow
func TestIntegrationWebSocketChannelBidirectionalCommunication(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:    true,
		Host:       "127.0.0.1",
		Port:       49911,
		Path:       "/ws",
		AuthToken:  "",
		SyncToWeb:  false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Connect client
	wsURL := "ws://127.0.0.1:49911/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Read welcome message
	_, _, err = conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome: %v", err)
	}

	// Send multiple messages
	messages := []string{
		"Message 1",
		"Message 2",
		"Message 3",
	}

	for _, msgContent := range messages {
		// Send from client
		clientMsg := map[string]interface{}{
			"type":    "message",
			"content": msgContent,
		}
		if err := conn.WriteJSON(clientMsg); err != nil {
			t.Fatalf("Failed to send '%s': %v", msgContent, err)
		}

		// Wait for inbound
		msg, ok := testBus.ConsumeInbound(ctx)
		if !ok {
			t.Fatal("Failed to subscribe to inbound messages")
		}

		if msg.Channel != "websocket" || msg.Content != msgContent {
			t.Errorf("Expected '%s', got: %s", msgContent, msg.Content)
		}

		// Send response
		outboundMsg := bus.OutboundMessage{
			Channel: "websocket",
			ChatID:  msg.ChatID,
			Content: "Response to: " + msgContent,
		}

		if err := channel.Send(ctx, outboundMsg); err != nil {
			t.Fatalf("Failed to send response: %v", err)
		}

		// Read response from client
		_, response, err := conn.ReadMessage()
		if err != nil {
			t.Fatalf("Failed to read response: %v", err)
		}

		var serverMsg map[string]interface{}
		if err := json.Unmarshal(response, &serverMsg); err != nil {
			t.Fatalf("Failed to parse response: %v", err)
		}

		expectedResponse := "Response to: " + msgContent
		if serverMsg["content"] != expectedResponse {
			t.Errorf("Expected '%s', got: %v", expectedResponse, serverMsg["content"])
		}
	}

	t.Log("✓ Bidirectional communication test completed successfully")
}

// TestIntegrationWebSocketChannelPingPong tests ping/pong heartbeat
func TestIntegrationWebSocketChannelPingPong(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:    true,
		Host:       "127.0.0.1",
		Port:       49912,
		Path:       "/ws",
		AuthToken:  "",
		SyncToWeb:  false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Connect client
	wsURL := "ws://127.0.0.1:49912/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Read welcome message
	_, _, err = conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome: %v", err)
	}

	// Send ping
	pingMsg := map[string]interface{}{
		"type": "ping",
	}
	if err := conn.WriteJSON(pingMsg); err != nil {
		t.Fatalf("Failed to send ping: %v", err)
	}

	// Read pong
	_, pong, err := conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read pong: %v", err)
	}

	var serverMsg map[string]interface{}
	if err := json.Unmarshal(pong, &serverMsg); err != nil {
		t.Fatalf("Failed to parse pong: %v", err)
	}

	if serverMsg["type"] != "pong" {
		t.Errorf("Expected 'pong', got: %v", serverMsg["type"])
	}

	t.Log("✓ Ping/pong test completed successfully")
}

// TestIntegrationWebSocketChannelReconnection tests client reconnection
func TestIntegrationWebSocketChannelReconnection(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:    true,
		Host:       "127.0.0.1",
		Port:       49913,
		Path:       "/ws",
		AuthToken:  "",
		SyncToWeb:  false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	wsURL := "ws://127.0.0.1:49913/ws"

	// First connection
	t.Log("Connecting first client...")
	conn1, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect first client: %v", err)
	}

	// Read welcome
	_, welcome, err := conn1.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome: %v", err)
	}
	if !strings.Contains(string(welcome), "Connected") {
		t.Errorf("Expected welcome message, got: %s", string(welcome))
	}

	// Disconnect first client
	t.Log("Disconnecting first client...")
	conn1.Close()
	time.Sleep(100 * time.Millisecond)

	// Second connection (should succeed after first disconnects)
	t.Log("Connecting second client...")
	conn2, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect second client: %v", err)
	}
	defer conn2.Close()

	// Read welcome
	_, welcome, err = conn2.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome from second client: %v", err)
	}
	if !strings.Contains(string(welcome), "Connected") {
		t.Errorf("Expected welcome message, got: %s", string(welcome))
	}

	// Send message from second client
	clientMsg := map[string]interface{}{
		"type":    "message",
		"content": "Hello from second client!",
	}
	if err := conn2.WriteJSON(clientMsg); err != nil {
		t.Fatalf("Failed to send message: %v", err)
	}

	// Wait for inbound
	msg, ok := testBus.ConsumeInbound(ctx)
	if !ok {
		t.Fatal("Failed to consume inbound messages")
	}

	if msg.Content != "Hello from second client!" {
		t.Errorf("Expected 'Hello from second client!', got: %s", msg.Content)
	}

	t.Log("✓ Reconnection test completed successfully")
}

// TestIntegrationWebSocketChannelConcurrentAccess tests concurrent message handling
func TestIntegrationWebSocketChannelConcurrentAccess(t *testing.T) {
	cfg := &config.WebSocketChannelConfig{
		Enabled:    true,
		Host:       "127.0.0.1",
		Port:       49914,
		Path:       "/ws",
		AuthToken:  "",
		SyncToWeb:  false,
	}

	testBus := bus.NewMessageBus()

	channel, err := NewWebSocketChannel(cfg, testBus)
	if err != nil {
		t.Fatalf("Failed to create WebSocket channel: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	if err := channel.Start(ctx); err != nil {
		t.Fatalf("Failed to start channel: %v", err)
	}
	defer channel.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Connect client
	wsURL := "ws://127.0.0.1:49914/ws"
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		t.Fatalf("Failed to connect: %v", err)
	}
	defer conn.Close()

	// Read welcome
	_, _, err = conn.ReadMessage()
	if err != nil {
		t.Fatalf("Failed to read welcome: %v", err)
	}

	// Send multiple messages sequentially (WebSocket doesn't support concurrent writes)
	numMessages := 10
	messagesReceived := make(chan string, numMessages)

	// Start receiver goroutine
	go func() {
		for i := 0; i < numMessages; i++ {
			_, response, err := conn.ReadMessage()
			if err != nil {
				t.Errorf("Failed to read response: %v", err)
				return
			}
			var serverMsg map[string]interface{}
			if err := json.Unmarshal(response, &serverMsg); err != nil {
				t.Errorf("Failed to parse response: %v", err)
				return
			}
			if content, ok := serverMsg["content"].(string); ok {
				messagesReceived <- content
			}
		}
	}()

	// Process inbound messages and send responses
	go func() {
		for i := 0; i < numMessages; i++ {
			msg, ok := testBus.ConsumeInbound(ctx)
			if !ok {
				return
			}
			if msg.Channel == "websocket" {
				outboundMsg := bus.OutboundMessage{
					Channel: "websocket",
					ChatID:  msg.ChatID,
					Content: "Response: " + msg.Content,
				}
				if err := channel.Send(ctx, outboundMsg); err != nil {
					t.Errorf("Failed to send response: %v", err)
				}
			}
		}
	}()

	// Send messages sequentially
	for i := 0; i < numMessages; i++ {
		msgContent := fmt.Sprintf("Concurrent message %d", i)
		clientMsg := map[string]interface{}{
			"type":    "message",
			"content": msgContent,
		}

		if err := conn.WriteJSON(clientMsg); err != nil {
			t.Fatalf("Failed to send message: %v", err)
		}
	}

	// Wait for all messages to be processed
	timeout := time.After(10 * time.Second)
	receivedCount := 0
	for {
		select {
		case <-messagesReceived:
			receivedCount++
			if receivedCount == numMessages {
				t.Log("✓ All concurrent messages processed successfully")
				return
			}
		case <-timeout:
			t.Errorf("Timeout: only %d/%d messages received", receivedCount, numMessages)
			return
		}
	}
}
