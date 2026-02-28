package client

import (
	"encoding/json"
	"fmt"
	"log"
	"net/url"
	"sync"
	"sync/atomic"
	"time"

	"websocket-client/src/config"

	"github.com/gorilla/websocket"
)

type ClientMessage struct {
	Type      string `json:"type"`
	Content   string `json:"content"`
	Timestamp string `json:"timestamp,omitempty"`
}

type ServerMessage struct {
	Type      string `json:"type"`
	Role      string `json:"role,omitempty"`
	Content   string `json:"content,omitempty"`
	Timestamp string `json:"timestamp,omitempty"`
	Error     string `json:"error,omitempty"`
}

type Statistics struct {
	MessagesSent     atomic.Uint64
	MessagesReceived atomic.Uint64
	BytesSent        atomic.Uint64
	BytesReceived    atomic.Uint64
	ReconnectCount   atomic.Uint64
	ConnectedAt      atomic.Int64
}

type WebSocketClient struct {
	config     *config.Config
	stats      *Statistics
	running    atomic.Bool
	cliChannel chan string
}

func New(cfg *config.Config) *WebSocketClient {
	return &WebSocketClient{
		config:     cfg,
		stats:      &Statistics{},
		cliChannel: make(chan string, 100),
	}
}

func (c *WebSocketClient) GetCLIMessageChannel() chan<- string {
	return c.cliChannel
}

func (c *WebSocketClient) Stop() {
	c.running.Store(false)
}

func (c *WebSocketClient) Start() error {
	c.running.Store(true)

	reconnectAttempts := 0
	reconnectDelay := time.Duration(c.config.Reconnect.InitialDelaySec) * time.Second

	for c.running.Load() {
		if c.config.Reconnect.MaxAttempts > 0 && reconnectAttempts >= c.config.Reconnect.MaxAttempts {
			return fmt.Errorf("max reconnect attempts reached")
		}

		if err := c.connectAndRun(); err != nil {
			log.Printf("⚠️  Connection error: %v", err)
			if !c.config.Reconnect.Enabled || !c.running.Load() {
				return err
			}

			reconnectAttempts++
			c.stats.ReconnectCount.Add(1)
			log.Printf("🔄 Reconnecting in %v seconds... (attempt %d)", reconnectDelay.Seconds(), reconnectAttempts)
			time.Sleep(reconnectDelay)

			reconnectDelay = time.Duration(float64(reconnectDelay) * c.config.Reconnect.DelayMultiplier)
			maxDelay := time.Duration(c.config.Reconnect.MaxDelaySec) * time.Second
			if reconnectDelay > maxDelay {
				reconnectDelay = maxDelay
			}
		} else {
			break
		}
	}

	return nil
}

func (c *WebSocketClient) connectAndRun() error {
	fmt.Println("🔄 Connecting...")

	u, err := url.Parse(c.config.GetURL())
	if err != nil {
		return fmt.Errorf("failed to parse URL: %w", err)
	}

	conn, _, err := websocket.DefaultDialer.Dial(u.String(), nil)
	if err != nil {
		return fmt.Errorf("failed to connect: %w", err)
	}
	defer conn.Close()

	fmt.Println("✅ Connected!")

	c.stats.ConnectedAt.Store(time.Now().Unix())

	// Single event loop using channels
	fmt.Println("📤 SINGLE EVENT LOOP - Never send + receive simultaneously")

	// Channel for received messages
	receiveChannel := make(chan []byte, 100)
	errorChannel := make(chan error, 1)

	// Use WaitGroup to wait for receiver goroutine
	var wg sync.WaitGroup
	wg.Add(1)

	// Start receiver goroutine
	go func() {
		defer wg.Done()
		for {
			messageType, data, err := conn.ReadMessage()
			if err != nil {
				errorChannel <- err
				return
			}

			if messageType == websocket.CloseMessage {
				errorChannel <- fmt.Errorf("server closed connection")
				return
			}
			
			fmt.Println("Recv message")
			if messageType == websocket.TextMessage {
				receiveChannel <- data
			}
		}
	}()

	cliClosed := false
	idleTimeout := 30 * time.Second
	cliClosedTime := time.Time{}
	cliChannel := c.cliChannel  // Local variable so we can nil it

	for c.running.Load() {
		select {
		case <-time.After(100 * time.Millisecond):
			// Check idle timeout - only after CLI is closed and running is false
			if cliClosed && !c.running.Load() && !cliClosedTime.IsZero() && time.Since(cliClosedTime) > idleTimeout {
				fmt.Println("⏱️  Idle timeout after CLI closed, exiting")
				conn.WriteMessage(websocket.CloseMessage, []byte{})
				return nil
			}

		case msg, ok := <-cliChannel:
			if !ok {
				// CLI channel closed - set to nil to prevent this case from being selected again
				if !cliClosed {
					fmt.Println("📤 CLI closed, waiting for responses...")
					cliClosed = true
					cliClosedTime = time.Now()
					cliChannel = nil  // Prevent this case from being selected again
				}
				continue
			}

			// Send message to server
			fmt.Printf("📤 [TX] %s\n", msg)

			clientMsg := ClientMessage{
				Type:      "message",
				Content:   msg,
				Timestamp: time.Now().Format(time.RFC3339),
			}

			jsonData, err := json.Marshal(clientMsg)
			if err != nil {
				log.Printf("❌ Failed to marshal message: %v", err)
				continue
			}

			c.stats.BytesSent.Add(uint64(len(jsonData)))

			if err := conn.WriteMessage(websocket.TextMessage, jsonData); err != nil {
				log.Printf("❌ Failed to send message: %v", err)
				// Connection error, but try to receive any pending responses first
				break
			}

			c.stats.MessagesSent.Add(1)
			fmt.Println("✅ Sent")
			config.LogToFile(c.config, msg)

		case data, ok := <-receiveChannel:
			if !ok {
				fmt.Println("Recv Error")
				return nil
			}

			fmt.Printf("📥 [RX] %d bytes\n", len(data))
			c.stats.BytesReceived.Add(uint64(len(data)))

			var serverMsg ServerMessage
			if err := json.Unmarshal(data, &serverMsg); err != nil {
				log.Printf("⚠️  Failed to parse message: %v", err)
				continue
			}

			switch serverMsg.Type {
			case "message":
				c.stats.MessagesReceived.Add(1)
				c.printReceivedMessage(&serverMsg)
				config.LogToFile(c.config, fmt.Sprintf("[%s]: %s", serverMsg.Role, serverMsg.Content))
			case "pong":
				config.LogToFile(c.config, "PONG")
			case "error":
				fmt.Printf("❌ Error: %s\n", serverMsg.Error)
			}

		case err := <-errorChannel:
			log.Printf("⚠️  WebSocket error: %v", err)
			// Don't exit immediately - try to process any remaining messages first
			// Break the select loop to drain receiveChannel
			goto drainMessages
		}
	}

drainMessages:
	// Process any remaining messages in the receiveChannel before exiting
	fmt.Println("📥 Processing remaining messages...")
	drainCount := 0
	maxDrainTime := 2 * time.Second  // Only wait up to 2 seconds for remaining messages
	drainStart := time.Now()

	for time.Since(drainStart) < maxDrainTime {
		select {
		case data, ok := <-receiveChannel:
			if !ok {
				fmt.Println("📥 Receive channel closed")
				return nil
			}

			drainCount++
			fmt.Printf("📥 [RX] %d bytes (draining)\n", len(data))
			c.stats.BytesReceived.Add(uint64(len(data)))

			var serverMsg ServerMessage
			if err := json.Unmarshal(data, &serverMsg); err != nil {
				log.Printf("⚠️  Failed to parse message: %v", err)
				continue
			}

			switch serverMsg.Type {
			case "message":
				c.stats.MessagesReceived.Add(1)
				c.printReceivedMessage(&serverMsg)
				config.LogToFile(c.config, fmt.Sprintf("[%s]: %s", serverMsg.Role, serverMsg.Content))
			case "pong":
				config.LogToFile(c.config, "PONG")
			case "error":
				fmt.Printf("❌ Error: %s\n", serverMsg.Error)
			}

		case <-time.After(100 * time.Millisecond):
			// No message for 100ms, check if we should continue waiting
			if drainCount == 0 {
				// Still haven't drained anything, keep waiting
				continue
			}
			// We've drained some messages, wait a bit more to be safe
			if time.Since(drainStart) < maxDrainTime {
				continue
			}
			// Timeout reached
			fmt.Printf("📥 Drained %d messages, timeout reached\n", drainCount)
			goto done
		}
	}

done:

	// Wait for receiver goroutine to finish
	wg.Wait()

	return nil
}

func (c *WebSocketClient) printReceivedMessage(msg *ServerMessage) {
	timestamp := ""
	if c.config.UI.ShowTimestamp {
		timestamp = fmt.Sprintf("[%s] ", time.Now().Format("15:04:05"))
	}

	var roleStr string
	switch msg.Role {
	case "assistant":
		roleStr = "🤖 Assistant"
	case "user":
		roleStr = "👤 User"
	case "system":
		roleStr = "⚙️  System"
	default:
		roleStr = "📨 Unknown"
	}

	fmt.Printf("%s%s: %s\n", timestamp, roleStr, msg.Content)
}

func (c *WebSocketClient) GetStats() *Statistics {
	return c.stats
}

func (c *WebSocketClient) IsConnected() bool {
	return c.running.Load()
}
