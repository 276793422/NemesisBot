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

// ClientMessage is the JSON structure sent to the NemesisBot WebSocket server.
type ClientMessage struct {
	Type      string `json:"type"`
	Content   string `json:"content"`
	Timestamp string `json:"timestamp,omitempty"`
}

// ServerMessage is the JSON structure received from the NemesisBot WebSocket server.
type ServerMessage struct {
	Type      string `json:"type"`
	Role      string `json:"role,omitempty"`
	Content   string `json:"content,omitempty"`
	Timestamp string `json:"timestamp,omitempty"`
	Error     string `json:"error,omitempty"`
}

// WebSocketClient manages a WebSocket connection to NemesisBot.
type WebSocketClient struct {
	config    *config.Config
	recvQueue *MessageQueue
	running   atomic.Bool
	connected atomic.Bool

	connMu sync.Mutex   // protects conn write operations
	conn   *websocket.Conn

	wg sync.WaitGroup // waits for background goroutines on destroy
}

// New creates a new WebSocketClient with the given configuration.
func New(cfg *config.Config) *WebSocketClient {
	return &WebSocketClient{
		config:    cfg,
		recvQueue: NewMessageQueue(),
	}
}

// Start connects to the WebSocket server and starts the message receive loop.
// It blocks until the connection is established or fails (without reconnection).
// Reconnection runs in a background goroutine.
func (c *WebSocketClient) Start() error {
	c.running.Store(true)

	// First connection attempt (blocking, caller gets immediate feedback)
	if err := c.connect(); err != nil {
		c.running.Store(false)
		return err
	}

	// Start reconnection loop in background
	c.wg.Add(1)
	go c.reconnectLoop()

	return nil
}

// Send sends a text message to the server.
func (c *WebSocketClient) Send(content string) error {
	if !c.running.Load() || !c.connected.Load() {
		return fmt.Errorf("not connected")
	}

	clientMsg := ClientMessage{
		Type:      "message",
		Content:   content,
		Timestamp: time.Now().Format(time.RFC3339),
	}

	jsonData, err := json.Marshal(clientMsg)
	if err != nil {
		return fmt.Errorf("marshal error: %w", err)
	}

	c.connMu.Lock()
	defer c.connMu.Unlock()

	if c.conn == nil {
		return fmt.Errorf("connection lost")
	}

	if err := c.conn.WriteMessage(websocket.TextMessage, jsonData); err != nil {
		return fmt.Errorf("write error: %w", err)
	}

	return nil
}

// Recv retrieves the next received message as a JSON string.
// timeoutMs: <= 0 non-blocking, > 0 wait up to N milliseconds.
// Returns the JSON-encoded ServerMessage, or empty string on timeout/error.
func (c *WebSocketClient) Recv(timeoutMs int) []byte {
	data, ok := c.recvQueue.Dequeue(timeoutMs)
	if !ok {
		return nil
	}
	return data
}

// IsConnected returns whether the client is currently connected.
func (c *WebSocketClient) IsConnected() bool {
	return c.connected.Load()
}

// Destroy disconnects and releases all resources.
func (c *WebSocketClient) Destroy() {
	c.running.Store(false)
	c.closeConn()
	c.recvQueue.Close()
	c.wg.Wait()
}

// connect establishes a new WebSocket connection.
func (c *WebSocketClient) connect() error {
	u, err := url.Parse(c.config.GetURL())
	if err != nil {
		return fmt.Errorf("invalid URL: %w", err)
	}

	conn, _, err := websocket.DefaultDialer.Dial(u.String(), nil)
	if err != nil {
		return fmt.Errorf("connect failed: %w", err)
	}

	c.connMu.Lock()
	c.conn = conn
	c.connMu.Unlock()

	c.connected.Store(true)

	// Start the receiver goroutine
	c.wg.Add(1)
	go c.receiveLoop()

	return nil
}

// receiveLoop reads messages from the WebSocket connection.
// Each goroutine works with the conn that was active when it started.
func (c *WebSocketClient) receiveLoop() {
	defer c.wg.Done()
	defer c.connected.Store(false)

	// Capture the conn this goroutine owns
	c.connMu.Lock()
	conn := c.conn
	c.connMu.Unlock()

	for c.running.Load() {
		_, data, err := conn.ReadMessage()
		if err != nil {
			if c.running.Load() {
				log.Printf("read error: %v", err)
			}
			return
		}

		// Validate it's a valid server message
		var msg ServerMessage
		if err := json.Unmarshal(data, &msg); err != nil {
			continue
		}

		// Re-serialize to ensure clean JSON for the caller
		clean, err := json.Marshal(&msg)
		if err != nil {
			continue
		}

		c.recvQueue.Enqueue(clean)
	}
}

// reconnectLoop handles automatic reconnection.
func (c *WebSocketClient) reconnectLoop() {
	defer c.wg.Done()

	delay := c.config.InitialDelay()

	for c.running.Load() {
		// Wait until we notice disconnection
		for c.connected.Load() && c.running.Load() {
			time.Sleep(500 * time.Millisecond)
		}

		if !c.running.Load() {
			return
		}

		if !c.config.Reconnect.Enabled {
			return
		}

		attempts := 0
		for c.running.Load() {
			if c.config.Reconnect.MaxAttempts > 0 && attempts >= c.config.Reconnect.MaxAttempts {
				log.Printf("max reconnect attempts (%d) reached", c.config.Reconnect.MaxAttempts)
				c.running.Store(false)
				c.recvQueue.Close()
				return
			}

			log.Printf("reconnecting in %v (attempt %d)...", delay, attempts+1)
			time.Sleep(delay)

			if !c.running.Load() {
				return
			}

			c.closeConn()
			// Brief pause to let old receiveLoop goroutine exit
			time.Sleep(200 * time.Millisecond)
			if err := c.connect(); err != nil {
				log.Printf("reconnect failed: %v", err)
				attempts++
				delay = time.Duration(float64(delay) * c.config.Reconnect.DelayMultiplier)
				if delay > c.config.MaxDelay() {
					delay = c.config.MaxDelay()
				}
				continue
			}

			log.Printf("reconnected successfully")
			delay = c.config.InitialDelay() // reset delay on success
			break
		}
	}
}

// closeConn safely closes the current WebSocket connection.
func (c *WebSocketClient) closeConn() {
	c.connMu.Lock()
	defer c.connMu.Unlock()

	if c.conn != nil {
		// Send close frame
		c.conn.WriteMessage(websocket.CloseMessage,
			websocket.FormatCloseMessage(websocket.CloseNormalClosure, ""))
		c.conn.Close()
		c.conn = nil
	}
	c.connected.Store(false)
}
