// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// IRC Channel - RFC 1459 IRC protocol via TCP (pure stdlib)

package channels

import (
	"bufio"
	"context"
	"crypto/tls"
	"fmt"
	"net"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/utils"
)

// IRCConfig configures the IRC channel.
type IRCConfig struct {
	Server      string   // "irc.libera.chat:6697"
	TLS         bool     // true for SSL
	Nick        string   // "NemesisBot"
	Password    string   // optional server password
	Channel     string   // "#nemesisbot"
	ChannelName string   // "irc"
	AllowFrom   []string // sender allowlist (nick patterns)
}

// IRCChannel implements the Channel interface for IRC protocol using a raw
// TCP connection (with optional TLS) and the text-based IRC protocol.
type IRCChannel struct {
	*BaseChannel
	config    IRCConfig
	ctx       context.Context
	cancel    context.CancelFunc
	conn      net.Conn
	wg        sync.WaitGroup
	writeMu   sync.Mutex // serializes IRC writes
}

// NewIRCChannel creates a new IRC channel instance.
func NewIRCChannel(cfg IRCConfig, messageBus *bus.MessageBus) (*IRCChannel, error) {
	if cfg.Server == "" {
		return nil, fmt.Errorf("irc server is required")
	}
	if cfg.Nick == "" {
		return nil, fmt.Errorf("irc nick is required")
	}
	if cfg.Channel == "" {
		return nil, fmt.Errorf("irc channel is required")
	}

	// Apply defaults
	cfg.Channel = ensureHashPrefix(cfg.Channel)
	if cfg.ChannelName == "" {
		cfg.ChannelName = "irc"
	}

	base := NewBaseChannel(cfg.ChannelName, cfg, messageBus, cfg.AllowFrom)

	return &IRCChannel{
		BaseChannel: base,
		config:      cfg,
	}, nil
}

// Start connects to the IRC server, registers, joins the channel, and starts
// the read loop.
func (c *IRCChannel) Start(ctx context.Context) error {
	logger.InfoCF("irc", "Starting IRC channel", map[string]interface{}{
		"server":  c.config.Server,
		"nick":    c.config.Nick,
		"channel": c.config.Channel,
		"tls":     c.config.TLS,
	})

	c.ctx, c.cancel = context.WithCancel(ctx)

	// Connect to server
	if err := c.connect(); err != nil {
		return fmt.Errorf("irc connection failed: %w", err)
	}

	// Register and join
	c.register()
	c.join(c.config.Channel)

	// Start read loop
	c.wg.Add(1)
	go c.readLoop()

	c.setRunning(true)
	logger.InfoC("irc", "IRC channel started")
	return nil
}

// Stop gracefully stops the IRC channel by sending QUIT and closing the connection.
func (c *IRCChannel) Stop(ctx context.Context) error {
	logger.InfoC("irc", "Stopping IRC channel")

	if c.cancel != nil {
		c.cancel()
	}

	if c.conn != nil {
		c.sendRaw("QUIT :NemesisBot shutting down")
		c.conn.Close()
		c.conn = nil
	}

	c.wg.Wait()
	c.setRunning(false)
	logger.InfoC("irc", "IRC channel stopped")
	return nil
}

// Send sends a PRIVMSG to the IRC channel or a specific target.
func (c *IRCChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.IsRunning() {
		return fmt.Errorf("irc channel not running")
	}

	target := msg.ChatID
	if target == "" {
		target = c.config.Channel
	}

	logger.DebugCF("irc", "Sending message", map[string]interface{}{
		"target":  target,
		"preview": utils.Truncate(msg.Content, 50),
	})

	// Split long messages to avoid IRC line length limits (512 bytes max)
	lines := splitMessage(msg.Content, 400)
	for _, line := range lines {
		if err := c.sendCommand("PRIVMSG %s :%s", target, line); err != nil {
			return fmt.Errorf("failed to send IRC message: %w", err)
		}
	}

	return nil
}

// connect establishes the TCP (or TLS) connection to the IRC server.
func (c *IRCChannel) connect() error {
	var conn net.Conn
	var err error

	if c.config.TLS {
		tlsConfig := &tls.Config{
			ServerName: strings.Split(c.config.Server, ":")[0],
		}
		conn, err = tls.DialWithDialer(
			&net.Dialer{Timeout: 15 * time.Second},
			"tcp",
			c.config.Server,
			tlsConfig,
		)
	} else {
		conn, err = net.DialTimeout("tcp", c.config.Server, 15*time.Second)
	}

	if err != nil {
		return fmt.Errorf("dial failed: %w", err)
	}

	c.conn = conn
	return nil
}

// register sends NICK and USER commands to the IRC server.
func (c *IRCChannel) register() {
	if c.config.Password != "" {
		c.sendRaw("PASS " + c.config.Password)
	}
	c.sendRaw(fmt.Sprintf("NICK %s", c.config.Nick))
	c.sendRaw(fmt.Sprintf("USER %s 0 * :NemesisBot", c.config.Nick))
}

// join sends the JOIN command for a channel.
func (c *IRCChannel) join(channel string) {
	c.sendRaw("JOIN " + channel)
}

// readLoop reads lines from the IRC connection and dispatches them.
func (c *IRCChannel) readLoop() {
	defer c.wg.Done()

	scanner := bufio.NewScanner(c.conn)
	scanner.Buffer(make([]byte, 1024), 512)

	for scanner.Scan() {
		if c.ctx.Err() != nil {
			return
		}

		line := scanner.Text()
		if line == "" {
			continue
		}

		c.handleLine(line)
	}

	if c.ctx.Err() == nil {
		logger.WarnC("irc", "IRC connection closed unexpectedly, attempting reconnect")
		c.reconnect()
	}
}

// handleLine parses and dispatches a single IRC protocol line.
func (c *IRCChannel) handleLine(line string) {
	// Handle PING
	if strings.HasPrefix(line, "PING ") {
		pingData := strings.TrimPrefix(line, "PING ")
		c.sendRaw("PONG " + pingData)
		return
	}

	// Parse prefix
	var prefix, command string
	params := line

	if strings.HasPrefix(params, ":") {
		parts := strings.SplitN(params, " ", 2)
		prefix = strings.TrimPrefix(parts[0], ":")
		if len(parts) > 1 {
			params = parts[1]
		} else {
			return
		}
	}

	// Extract command
	parts := strings.SplitN(params, " ", 2)
	command = parts[0]
	if len(parts) > 1 {
		params = parts[1]
	} else {
		params = ""
	}

	switch command {
	case "PRIVMSG":
		c.handlePrivMsg(prefix, params)
	case "001": // RPL_WELCOME
		logger.DebugCF("irc", "Registered with server", map[string]interface{}{
			"nick": c.config.Nick,
		})
	case "433": // ERR_NICKNAMEINUSE
		logger.WarnCF("irc", "Nickname in use, appending underscore", map[string]interface{}{
			"nick": c.config.Nick,
		})
		c.config.Nick += "_"
		c.register()
		c.join(c.config.Channel)
	case "ERROR":
		logger.ErrorCF("irc", "Server error", map[string]interface{}{
			"message": params,
		})
	}
}

// handlePrivMsg processes a PRIVMSG from another IRC user.
func (c *IRCChannel) handlePrivMsg(prefix, params string) {
	// Parse: target :message
	parts := strings.SplitN(params, " :", 2)
	if len(parts) < 2 {
		return
	}

	target := parts[0]
	content := parts[1]

	// Extract nick from prefix (nick!user@host)
	senderNick := prefix
	if idx := strings.Index(prefix, "!"); idx > 0 {
		senderNick = prefix[:idx]
	}

	// Ignore our own messages
	if senderNick == c.config.Nick {
		return
	}

	// Only handle messages to our channel or direct queries
	if target != c.config.Channel && !strings.EqualFold(target, c.config.Nick) {
		return
	}

	// Determine chatID: for channel messages use channel name, for queries use sender nick
	chatID := c.config.Channel
	if strings.EqualFold(target, c.config.Nick) {
		chatID = senderNick
	}

	metadata := map[string]string{
		"platform": "irc",
		"target":   target,
		"sender":   senderNick,
		"is_query": fmt.Sprintf("%v", strings.EqualFold(target, c.config.Nick)),
	}

	logger.DebugCF("irc", "Received message", map[string]interface{}{
		"sender":  senderNick,
		"target":  target,
		"preview": utils.Truncate(content, 50),
	})

	c.HandleMessage(senderNick, chatID, content, nil, metadata)
}

// reconnect attempts to reconnect to the IRC server with exponential backoff.
func (c *IRCChannel) reconnect() {
	backoff := 5 * time.Second
	maxBackoff := 60 * time.Second

	for attempt := 1; ; attempt++ {
		select {
		case <-c.ctx.Done():
			return
		case <-time.After(backoff):
		}

		logger.InfoCF("irc", "Reconnecting", map[string]interface{}{
			"attempt": attempt,
			"backoff": backoff.String(),
		})

		if err := c.connect(); err != nil {
			logger.ErrorCF("irc", "Reconnect failed", map[string]interface{}{
				"error": err.Error(),
			})
			backoff = backoff * 2
			if backoff > maxBackoff {
				backoff = maxBackoff
			}
			continue
		}

		c.register()
		c.join(c.config.Channel)

		c.wg.Add(1)
		go c.readLoop()
		return
	}
}

// sendRaw writes a raw IRC line to the connection.
func (c *IRCChannel) sendRaw(line string) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()

	if c.conn == nil {
		return fmt.Errorf("not connected")
	}

	_, err := fmt.Fprintf(c.conn, "%s\r\n", line)
	if err != nil {
		logger.ErrorCF("irc", "Write error", map[string]interface{}{
			"error": err.Error(),
		})
	}
	return err
}

// sendCommand sends a formatted IRC command.
func (c *IRCChannel) sendCommand(format string, args ...interface{}) error {
	return c.sendRaw(fmt.Sprintf(format, args...))
}

// ensureHashPrefix ensures the channel name starts with #.
func ensureHashPrefix(channel string) string {
	if channel != "" && !strings.HasPrefix(channel, "#") {
		return "#" + channel
	}
	return channel
}

// splitMessage splits a message into lines that fit within the IRC line limit.
func splitMessage(content string, maxLen int) []string {
	if len(content) <= maxLen {
		return []string{content}
	}

	var lines []string
	for len(content) > 0 {
		if len(content) <= maxLen {
			lines = append(lines, content)
			break
		}

		// Try to split at a newline
		if idx := strings.LastIndex(content[:maxLen], "\n"); idx > 0 {
			lines = append(lines, content[:idx])
			content = content[idx+1:]
			continue
		}

		// Try to split at a space
		if idx := strings.LastIndex(content[:maxLen], " "); idx > 0 {
			lines = append(lines, content[:idx])
			content = content[idx+1:]
			continue
		}

		// Hard split
		lines = append(lines, content[:maxLen])
		content = content[maxLen:]
	}

	return lines
}

// Ensure IRCChannel implements the Channel interface at compile time.
var _ Channel = (*IRCChannel)(nil)
