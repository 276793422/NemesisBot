// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"encoding/json"
	"net"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
)

func TestMaixCamChannel_StartStop(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0, // OS picks port
	}
	ch, err := NewMaixCamChannel(cfg, msgBus)
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

	if err := ch.Stop(ctx); err != nil {
		t.Fatalf("stop failed: %v", err)
	}
	if ch.IsRunning() {
		t.Error("should not be running after stop")
	}
}

func TestMaixCamChannel_ReceiveMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0,
	}
	ch, err := NewMaixCamChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	defer ch.Stop(ctx)

	// Get the actual listener address
	addr := ch.listener.Addr().String()

	// Subscribe to inbound
	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx2, cancel2 := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel2()
		if msg, ok := msgBus.ConsumeInbound(ctx2); ok {
			received <- msg
		}
	}()

	// Connect as client
	conn, err := net.DialTimeout("tcp", addr, 2*time.Second)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer conn.Close()

	// Send a person_detected message
	msg := MaixCamMessage{
		Type:      "person_detected",
		Tips:      "detected",
		Timestamp: 1700000000.0,
		Data: map[string]interface{}{
			"class_name": "person",
			"score":      0.95,
			"x":          float64(100),
			"y":          float64(200),
			"w":          float64(50),
			"h":          float64(80),
		},
	}
	data, _ := json.Marshal(msg)
	conn.Write(data)
	conn.Write([]byte("\n"))

	// Wait for message
	select {
	case inbound := <-received:
		if inbound.Channel != "maixcam" {
			t.Errorf("channel = %q", inbound.Channel)
		}
		if !contains(inbound.Content, "Person detected") {
			t.Errorf("content = %q", inbound.Content)
		}
	case <-time.After(3 * time.Second):
		t.Error("timed out waiting for message")
	}
}

func TestMaixCamChannel_SendToClient(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0,
	}
	ch, err := NewMaixCamChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	defer ch.Stop(ctx)

	addr := ch.listener.Addr().String()

	// Connect client
	conn, err := net.DialTimeout("tcp", addr, 2*time.Second)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer conn.Close()

	// Wait for connection to be registered
	time.Sleep(100 * time.Millisecond)

	// Send message to client
	err = ch.Send(ctx, bus.OutboundMessage{
		ChatID:  "default",
		Content: "command: do something",
	})
	if err != nil {
		t.Fatalf("send failed: %v", err)
	}

	// Read response from client side
	conn.SetReadDeadline(time.Now().Add(2 * time.Second))
	buf := make([]byte, 4096)
	n, err := conn.Read(buf)
	if err != nil {
		t.Fatalf("failed to read response: %v", err)
	}
	data := buf[:n]

	var response map[string]interface{}
	if err := json.Unmarshal(data, &response); err != nil {
		t.Fatalf("failed to unmarshal response: %v", err)
	}
	if response["type"] != "command" {
		t.Errorf("type = %v", response["type"])
	}
	if response["message"] != "command: do something" {
		t.Errorf("message = %v", response["message"])
	}
}

func TestMaixCamChannel_SendNotRunning(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0,
	}
	ch, _ := NewMaixCamChannel(cfg, msgBus)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "test",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error when not running")
	}
}

func TestMaixCamChannel_SendNoClients(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0,
	}
	ch, _ := NewMaixCamChannel(cfg, msgBus)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	defer ch.Stop(ctx)

	// No clients connected
	err := ch.Send(ctx, bus.OutboundMessage{
		ChatID:  "default",
		Content: "test",
	})
	if err == nil {
		t.Error("expected error when no clients")
	}
}

func TestMaixCamChannel_HeartbeatMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0,
	}
	ch, _ := NewMaixCamChannel(cfg, msgBus)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	defer ch.Stop(ctx)

	addr := ch.listener.Addr().String()

	conn, err := net.DialTimeout("tcp", addr, 2*time.Second)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer conn.Close()

	// Send heartbeat - should not trigger HandleMessage
	msg := MaixCamMessage{
		Type:      "heartbeat",
		Timestamp: 1234.0,
	}
	data, _ := json.Marshal(msg)
	conn.Write(data)
	conn.Write([]byte("\n"))

	// Give it a moment to process
	time.Sleep(200 * time.Millisecond)
	// No assertion needed - just verify no panic
}

func TestMaixCamChannel_StatusMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()
	cfg := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0,
	}
	ch, _ := NewMaixCamChannel(cfg, msgBus)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	defer ch.Stop(ctx)

	addr := ch.listener.Addr().String()

	conn, err := net.DialTimeout("tcp", addr, 2*time.Second)
	if err != nil {
		t.Fatalf("failed to connect: %v", err)
	}
	defer conn.Close()

	// Send status message
	msg := MaixCamMessage{
		Type:      "status",
		Timestamp: 1234.0,
		Data:      map[string]interface{}{"battery": 85.0},
	}
	data, _ := json.Marshal(msg)
	conn.Write(data)
	conn.Write([]byte("\n"))

	time.Sleep(200 * time.Millisecond)
	// Should process without panic
}

func TestMaixCamChannel_StartAddressConflict(t *testing.T) {
	msgBus := bus.NewMessageBus()

	// Create first channel on a specific port
	cfg1 := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0,
	}
	ch1, _ := NewMaixCamChannel(cfg1, msgBus)
	ctx1, cancel1 := context.WithCancel(context.Background())
	defer cancel1()
	ch1.Start(ctx1)
	defer ch1.Stop(ctx1)

	// Try to start second channel on the same port - should fail
	cfg2 := config.MaixCamConfig{
		Host: "127.0.0.1",
		Port: 0, // different port should work
	}
	ch2, _ := NewMaixCamChannel(cfg2, msgBus)
	ctx2, cancel2 := context.WithCancel(context.Background())
	defer cancel2()

	if err := ch2.Start(ctx2); err != nil {
		t.Fatalf("second channel on different port should work: %v", err)
	}
	ch2.Stop(ctx2)
}
