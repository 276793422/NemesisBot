// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/gorilla/websocket"
)

func TestOneBotChannel_ConnectAndReceive(t *testing.T) {
	// Create a WebSocket test server that simulates OneBot
	msgBus := bus.NewMessageBus()

	var serverConn *websocket.Conn
	upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var err error
		serverConn, err = upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer serverConn.Close()

		// Keep connection alive briefly
		for {
			_, _, err := serverConn.ReadMessage()
			if err != nil {
				return
			}
		}
	}))
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"

	cfg := config.OneBotConfig{
		Enabled:           true,
		WSUrl:             wsURL,
		ReconnectInterval: 0,
		AllowFrom:         []string{},
	}

	ch, err := NewOneBotChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}

	// Give time for connection
	time.Sleep(100 * time.Millisecond)

	if !ch.IsRunning() {
		t.Error("should be running")
	}

	// Send a message from OneBot server
	if serverConn != nil {
		event := map[string]interface{}{
			"post_type":   "meta_event",
			"meta_event_type": "lifecycle",
			"sub_type":    "connect",
			"self_id":     12345,
			"time":        float64(time.Now().Unix()),
		}
		data, _ := json.Marshal(event)
		serverConn.WriteMessage(websocket.TextMessage, data)
	}

	time.Sleep(100 * time.Millisecond)
	ch.Stop(ctx)
}

func TestOneBotChannel_SendMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()

	sendMsgs := make(chan []byte, 10)
	var serverConn *websocket.Conn
	upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var err error
		serverConn, err = upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer serverConn.Close()

		for {
			_, msg, err := serverConn.ReadMessage()
			if err != nil {
				return
			}
			// Respond to API requests that have echo
			var req map[string]interface{}
			if json.Unmarshal(msg, &req) == nil {
				if echo, ok := req["echo"].(string); ok && echo != "" {
					resp := map[string]interface{}{
						"echo":    echo,
						"retcode": 0,
						"status":  "ok",
						"data":    map[string]interface{}{"user_id": 12345, "nickname": "TestBot"},
					}
					respData, _ := json.Marshal(resp)
					serverConn.WriteMessage(websocket.TextMessage, respData)

					// If it's a send action, also forward to the sendMsgs channel
					if action, ok := req["action"].(string); ok && strings.HasPrefix(action, "send_") {
						sendMsgs <- msg
					}
					continue
				}
			}
			sendMsgs <- msg
		}
	}))
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"

	cfg := config.OneBotConfig{
		Enabled:           true,
		WSUrl:             wsURL,
		ReconnectInterval: 0,
		AllowFrom:         []string{},
	}

	ch, err := NewOneBotChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	defer ch.Stop(ctx)

	time.Sleep(200 * time.Millisecond)

	// Send a message through the channel
	err = ch.Send(ctx, bus.OutboundMessage{
		ChatID:  "private:12345",
		Content: "hello onebot",
	})
	if err != nil {
		t.Fatalf("send failed: %v", err)
	}

	// Verify the server received the API request
	select {
	case data := <-sendMsgs:
		var req map[string]interface{}
		if err := json.Unmarshal(data, &req); err != nil {
			t.Fatalf("failed to unmarshal: %v", err)
		}
		if req["action"] != "send_private_msg" {
			t.Errorf("action = %v", req["action"])
		}
	case <-time.After(2 * time.Second):
		t.Error("timed out waiting for message")
	}
}

func TestOneBotChannel_HandlePrivateMessage(t *testing.T) {
	msgBus := bus.NewMessageBus()

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if msg, ok := msgBus.ConsumeInbound(ctx); ok {
			received <- msg
		}
	}()

	var serverConn *websocket.Conn
	upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var err error
		serverConn, err = upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer serverConn.Close()

		// Read messages and respond to API requests
		for {
			_, msg, err := serverConn.ReadMessage()
			if err != nil {
				return
			}
			var req map[string]interface{}
			if json.Unmarshal(msg, &req) == nil {
				if echo, ok := req["echo"].(string); ok && echo != "" {
					resp := map[string]interface{}{
						"echo":    echo,
						"retcode": 0,
						"status":  "ok",
						"data":    map[string]interface{}{"user_id": 99999, "nickname": "TestBot"},
					}
					respData, _ := json.Marshal(resp)
					serverConn.WriteMessage(websocket.TextMessage, respData)
				}
			}
		}
	}))
	defer server.Close()

	wsURL := "ws" + strings.TrimPrefix(server.URL, "http") + "/ws"

	cfg := config.OneBotConfig{
		Enabled:           true,
		WSUrl:             wsURL,
		ReconnectInterval: 0,
		AllowFrom:         []string{},
	}

	ch, err := NewOneBotChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	if err := ch.Start(ctx); err != nil {
		t.Fatalf("start failed: %v", err)
	}
	defer ch.Stop(ctx)

	time.Sleep(300 * time.Millisecond)

	// Send a private message event from OneBot server
	if serverConn != nil {
		event := oneBotRawEvent{
			PostType:    "message",
			MessageType: "private",
			UserID:      json.RawMessage(`12345`),
			MessageID:   json.RawMessage(`100`),
			RawMessage:  "hello from user",
			Message:     json.RawMessage(`"hello from user"`),
			SelfID:      json.RawMessage(`99999`),
			Time:        json.RawMessage(`1700000000`),
		}
		data, _ := json.Marshal(event)
		serverConn.WriteMessage(websocket.TextMessage, data)
	}

	select {
	case msg := <-received:
		if msg.Content != "hello from user" {
			t.Errorf("content = %q", msg.Content)
		}
		if msg.Channel != "onebot" {
			t.Errorf("channel = %q", msg.Channel)
		}
	case <-time.After(5 * time.Second):
		t.Error("timed out waiting for message")
	}
}

func TestOneBotChannel_DedupFiltering(t *testing.T) {
	cfg := config.OneBotConfig{
		AllowFrom: []string{},
	}
	msgBus := bus.NewMessageBus()
	ch, _ := NewOneBotChannel(cfg, msgBus)

	// First message should not be duplicate
	if ch.isDuplicate("msg1") {
		t.Error("first message should not be duplicate")
	}

	// Same message should be duplicate
	if !ch.isDuplicate("msg1") {
		t.Error("second occurrence should be duplicate")
	}

	// Empty/zero message IDs are not duplicates
	if ch.isDuplicate("") {
		t.Error("empty ID should not be duplicate")
	}
	if ch.isDuplicate("0") {
		t.Error("zero ID should not be duplicate")
	}
}

func TestOneBotChannel_Lifecycle(t *testing.T) {
	cfg := config.OneBotConfig{
		WSUrl:             "ws://127.0.0.1:1", // intentionally invalid
		ReconnectInterval: 0,
		AllowFrom:         []string{},
	}
	msgBus := bus.NewMessageBus()
	ch, err := NewOneBotChannel(cfg, msgBus)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start with invalid URL should fail
	err = ch.Start(ctx)
	if err == nil {
		t.Error("expected error with invalid URL")
	}
}

func TestOneBotChannel_CheckGroupTrigger(t *testing.T) {
	cfg := config.OneBotConfig{
		GroupTriggerPrefix: []string{"/bot", "!bot"},
	}
	msgBus := bus.NewMessageBus()
	ch, _ := NewOneBotChannel(cfg, msgBus)

	tests := []struct {
		content      string
		mentioned    bool
		triggered    bool
		strippedContent string
	}{
		{"/bot hello", false, true, "hello"},
		{"!bot world", false, true, "world"},
		{"normal message", false, false, "normal message"},
		{"anything", true, true, "anything"}, // mention always triggers
		{"/other", false, false, "/other"},
	}

	for _, tt := range tests {
		t.Run(tt.content, func(t *testing.T) {
			triggered, stripped := ch.checkGroupTrigger(tt.content, tt.mentioned)
			if triggered != tt.triggered {
				t.Errorf("triggered = %v, want %v", triggered, tt.triggered)
			}
			if triggered && strings.TrimSpace(stripped) != tt.strippedContent {
				t.Errorf("stripped = %q, want %q", strings.TrimSpace(stripped), tt.strippedContent)
			}
		})
	}
}

func TestOneBotChannel_SendNotRunning(t *testing.T) {
	cfg := config.OneBotConfig{AllowFrom: []string{}}
	msgBus := bus.NewMessageBus()
	ch, _ := NewOneBotChannel(cfg, msgBus)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "test",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error when not running")
	}
}

func TestOneBotChannel_BuildSendRequest(t *testing.T) {
	cfg := config.OneBotConfig{AllowFrom: []string{}}
	msgBus := bus.NewMessageBus()
	ch, _ := NewOneBotChannel(cfg, msgBus)

	tests := []struct {
		chatID       string
		expectedAction string
	}{
		{"group:12345", "send_group_msg"},
		{"private:67890", "send_private_msg"},
		{"99999", "send_private_msg"},
	}

	for _, tt := range tests {
		t.Run(tt.chatID, func(t *testing.T) {
			action, _, err := ch.buildSendRequest(bus.OutboundMessage{
				ChatID:  tt.chatID,
				Content: "test",
			})
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if action != tt.expectedAction {
				t.Errorf("action = %q, want %q", action, tt.expectedAction)
			}
		})
	}
}

func TestOneBotChannel_BuildSendRequest_InvalidID(t *testing.T) {
	cfg := config.OneBotConfig{AllowFrom: []string{}}
	msgBus := bus.NewMessageBus()
	ch, _ := NewOneBotChannel(cfg, msgBus)

	_, _, err := ch.buildSendRequest(bus.OutboundMessage{
		ChatID:  "group:abc",
		Content: "test",
	})
	if err == nil {
		t.Error("expected error for invalid ID")
	}
}

func TestOneBotChannel_ParseMessageSegments_String(t *testing.T) {
	cfg := config.OneBotConfig{AllowFrom: []string{}}
	msgBus := bus.NewMessageBus()
	ch, _ := NewOneBotChannel(cfg, msgBus)

	result := ch.parseMessageSegments(json.RawMessage(`"hello world"`), 0)
	if result.Text != "hello world" {
		t.Errorf("text = %q", result.Text)
	}
}

func TestOneBotChannel_ParseMessageSegments_Array(t *testing.T) {
	cfg := config.OneBotConfig{AllowFrom: []string{}}
	msgBus := bus.NewMessageBus()
	ch, _ := NewOneBotChannel(cfg, msgBus)

	segments := `[{"type":"text","data":{"text":"hello "}}, {"type":"at","data":{"qq":"99999"}}, {"type":"text","data":{"text":"world"}}]`
	result := ch.parseMessageSegments(json.RawMessage(segments), 99999)
	if !strings.Contains(result.Text, "hello") || !strings.Contains(result.Text, "world") {
		t.Errorf("text = %q", result.Text)
	}
	if !result.IsBotMentioned {
		t.Error("should be mentioned")
	}
}

func TestOneBotChannel_ParseMessageSegments_Empty(t *testing.T) {
	cfg := config.OneBotConfig{AllowFrom: []string{}}
	msgBus := bus.NewMessageBus()
	ch, _ := NewOneBotChannel(cfg, msgBus)

	result := ch.parseMessageSegments(json.RawMessage(``), 0)
	if result.Text != "" {
		t.Errorf("expected empty text, got %q", result.Text)
	}
}
