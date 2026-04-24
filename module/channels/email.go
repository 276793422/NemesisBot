// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Email Channel - IMAP polling for inbound + SMTP for outbound

package channels

import (
	"context"
	"crypto/tls"
	"fmt"
	"net"
	"net/smtp"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/utils"
)

// EmailConfig configures the email channel with IMAP (inbound) and SMTP (outbound) settings.
type EmailConfig struct {
	// IMAP (inbound)
	IMAPHost     string `json:"imap_host"`
	IMAPPort     int    `json:"imap_port"`     // 993
	IMAPUsername string `json:"imap_username"`
	IMAPPassword string `json:"imap_password"`
	IMAPTLS      bool   `json:"imap_tls"`      // true

	// SMTP (outbound)
	SMTPHost     string `json:"smtp_host"`
	SMTPPort     int    `json:"smtp_port"`     // 587
	SMTPUsername string `json:"smtp_username"`
	SMTPPassword string `json:"smtp_password"`
	SMTPTLS      bool   `json:"smtp_tls"`      // true

	// General
	PollInterval int      `json:"poll_interval"` // seconds, for IMAP polling fallback
	Folder       string   `json:"folder"`        // "INBOX"
	ChannelName  string   `json:"channel_name"`  // "email"
	AllowFrom    []string `json:"allow_from"`
}

// EmailChannel implements the Channel interface for email communication.
// It uses IMAP polling to receive emails and SMTP to send responses.
type EmailChannel struct {
	*BaseChannel
	config       EmailConfig
	ctx          context.Context
	cancel       context.CancelFunc
	imapConn     net.Conn
	smtpClient   *smtp.Client
	seenUIDs     map[string]bool
	seenMu       sync.Mutex
	pollTicker   *time.Ticker
	wg           sync.WaitGroup
	senderMap    sync.Map // chatID -> sender email address (for replies)
	subjectMap   sync.Map // chatID -> original subject (for threading)
}

// NewEmailChannel creates a new Email channel instance.
func NewEmailChannel(cfg EmailConfig, messageBus *bus.MessageBus) (*EmailChannel, error) {
	if cfg.IMAPHost == "" || cfg.SMTPHost == "" {
		return nil, fmt.Errorf("email imap_host and smtp_host are required")
	}
	if cfg.IMAPUsername == "" || cfg.IMAPPassword == "" {
		return nil, fmt.Errorf("email imap_username and imap_password are required")
	}

	// Apply defaults
	if cfg.IMAPPort == 0 {
		cfg.IMAPPort = 993
	}
	if cfg.SMTPPort == 0 {
		cfg.SMTPPort = 587
	}
	if cfg.PollInterval == 0 {
		cfg.PollInterval = 30
	}
	if cfg.Folder == "" {
		cfg.Folder = "INBOX"
	}
	if cfg.ChannelName == "" {
		cfg.ChannelName = "email"
	}
	if cfg.SMTPUsername == "" {
		cfg.SMTPUsername = cfg.IMAPUsername
	}
	if cfg.SMTPPassword == "" {
		cfg.SMTPPassword = cfg.IMAPPassword
	}

	base := NewBaseChannel(cfg.ChannelName, cfg, messageBus, cfg.AllowFrom)

	return &EmailChannel{
		BaseChannel: base,
		config:      cfg,
		seenUIDs:    make(map[string]bool),
	}, nil
}

// Start connects to IMAP and begins polling for new messages.
func (c *EmailChannel) Start(ctx context.Context) error {
	logger.InfoCF("email", "Starting Email channel", map[string]interface{}{
		"imap_host":     c.config.IMAPHost,
		"imap_port":     c.config.IMAPPort,
		"smtp_host":     c.config.SMTPHost,
		"smtp_port":     c.config.SMTPPort,
		"folder":        c.config.Folder,
		"poll_interval": c.config.PollInterval,
	})

	c.ctx, c.cancel = context.WithCancel(ctx)

	// Perform initial IMAP connection to verify settings
	if err := c.connectIMAP(); err != nil {
		return fmt.Errorf("failed to connect to IMAP server: %w", err)
	}

	// Perform initial SMTP connection to verify settings
	if err := c.connectSMTP(); err != nil {
		logger.WarnCF("email", "SMTP connection failed (will retry on send)", map[string]interface{}{
			"error": err.Error(),
		})
	}

	// Start the polling loop
	c.pollTicker = time.NewTicker(time.Duration(c.config.PollInterval) * time.Second)
	c.wg.Add(1)
	go c.pollLoop()

	c.setRunning(true)
	logger.InfoC("email", "Email channel started")
	return nil
}

// Stop gracefully stops the email channel.
func (c *EmailChannel) Stop(ctx context.Context) error {
	logger.InfoC("email", "Stopping Email channel")

	if c.cancel != nil {
		c.cancel()
	}

	if c.pollTicker != nil {
		c.pollTicker.Stop()
	}

	c.wg.Wait()

	c.disconnectIMAP()
	c.disconnectSMTP()

	c.setRunning(false)
	logger.InfoC("email", "Email channel stopped")
	return nil
}

// Send sends an email via SMTP. The OutboundMessage.ChatID is expected to be
// the recipient email address (stored when the inbound message was received).
func (c *EmailChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.IsRunning() {
		return fmt.Errorf("email channel not running")
	}

	// Determine recipient: use the chatID (which should be the sender's email)
	recipient := msg.ChatID
	if recipient == "" {
		return fmt.Errorf("no recipient email address in chat_id")
	}

	// Load the original subject for threading
	var subject string
	if val, ok := c.subjectMap.Load(msg.ChatID); ok {
		subject = val.(string)
	} else {
		subject = "Re: NemesisBot Response"
	}

	logger.DebugCF("email", "Sending email", map[string]interface{}{
		"to":      recipient,
		"subject": subject,
		"preview": utils.Truncate(msg.Content, 50),
	})

	if err := c.sendEmail(recipient, subject, msg.Content); err != nil {
		return fmt.Errorf("failed to send email: %w", err)
	}

	logger.DebugCF("email", "Email sent successfully", map[string]interface{}{
		"to": recipient,
	})

	return nil
}

// connectIMAP establishes a connection to the IMAP server.
func (c *EmailChannel) connectIMAP() error {
	addr := net.JoinHostPort(c.config.IMAPHost, fmt.Sprintf("%d", c.config.IMAPPort))

	var conn net.Conn
	var err error

	if c.config.IMAPTLS {
		tlsConfig := &tls.Config{
			ServerName: c.config.IMAPHost,
		}
		conn, err = tls.DialWithDialer(
			&net.Dialer{Timeout: 10 * time.Second},
			"tcp",
			addr,
			tlsConfig,
		)
	} else {
		conn, err = net.DialTimeout("tcp", addr, 10*time.Second)
	}

	if err != nil {
		return fmt.Errorf("IMAP connection failed: %w", err)
	}

	c.imapConn = conn

	// Read the greeting
	greeting, err := c.readIMAPLine()
	if err != nil {
		conn.Close()
		c.imapConn = nil
		return fmt.Errorf("failed to read IMAP greeting: %w", err)
	}

	if !strings.HasPrefix(greeting, "* OK") {
		conn.Close()
		c.imapConn = nil
		return fmt.Errorf("unexpected IMAP greeting: %s", greeting)
	}

	// Login
	if err := c.imapCommand("LOGIN", fmt.Sprintf("%s %s",
		c.imapQuote(c.config.IMAPUsername),
		c.imapQuote(c.config.IMAPPassword),
	)); err != nil {
		conn.Close()
		c.imapConn = nil
		return fmt.Errorf("IMAP login failed: %w", err)
	}

	// Select the mailbox
	if err := c.imapCommand("SELECT", c.config.Folder); err != nil {
		conn.Close()
		c.imapConn = nil
		return fmt.Errorf("IMAP SELECT %s failed: %w", c.config.Folder, err)
	}

	logger.DebugCF("email", "IMAP connected and authenticated", map[string]interface{}{
		"host":   c.config.IMAPHost,
		"folder": c.config.Folder,
	})

	return nil
}

// disconnectIMAP closes the IMAP connection.
func (c *EmailChannel) disconnectIMAP() {
	if c.imapConn != nil {
		// Try to logout gracefully
		c.imapCommand("LOGOUT", "")
		c.imapConn.Close()
		c.imapConn = nil
	}
}

// connectSMTP establishes a connection to the SMTP server.
func (c *EmailChannel) connectSMTP() error {
	addr := net.JoinHostPort(c.config.SMTPHost, fmt.Sprintf("%d", c.config.SMTPPort))

	var client *smtp.Client
	var err error

	if c.config.SMTPTLS {
		// Use direct TLS connection (port 465)
		tlsConfig := &tls.Config{
			ServerName: c.config.SMTPHost,
		}
		conn, dialErr := tls.DialWithDialer(
			&net.Dialer{Timeout: 10 * time.Second},
			"tcp",
			addr,
			tlsConfig,
		)
		if dialErr != nil {
			return fmt.Errorf("SMTP TLS connection failed: %w", dialErr)
		}
		client, err = smtp.NewClient(conn, c.config.SMTPHost)
	} else {
		// Use STARTTLS (port 587)
		client, err = smtp.Dial(addr)
		if err == nil {
			// Upgrade to TLS via STARTTLS
			tlsConfig := &tls.Config{
				ServerName: c.config.SMTPHost,
			}
			if startTLSErr := client.StartTLS(tlsConfig); startTLSErr != nil {
				client.Close()
				return fmt.Errorf("SMTP STARTTLS failed: %w", startTLSErr)
			}
		}
	}

	if err != nil {
		return fmt.Errorf("SMTP connection failed: %w", err)
	}

	// Authenticate
	auth := smtp.PlainAuth("", c.config.SMTPUsername, c.config.SMTPPassword, c.config.SMTPHost)
	if err := client.Auth(auth); err != nil {
		client.Close()
		return fmt.Errorf("SMTP auth failed: %w", err)
	}

	c.smtpClient = client
	logger.DebugCF("email", "SMTP connected and authenticated", map[string]interface{}{
		"host": c.config.SMTPHost,
	})

	return nil
}

// disconnectSMTP closes the SMTP connection.
func (c *EmailChannel) disconnectSMTP() {
	if c.smtpClient != nil {
		c.smtpClient.Quit()
		c.smtpClient = nil
	}
}

// pollLoop is the main polling loop that checks for new emails.
func (c *EmailChannel) pollLoop() {
	defer c.wg.Done()

	// Poll immediately on start
	c.pollMessages()

	for {
		select {
		case <-c.ctx.Done():
			return
		case <-c.pollTicker.C:
			c.pollMessages()
		}
	}
}

// pollMessages checks for new emails via IMAP.
func (c *EmailChannel) pollMessages() {
	// Ensure we have a connection
	if c.imapConn == nil {
		if err := c.connectIMAP(); err != nil {
			logger.ErrorCF("email", "IMAP reconnection failed", map[string]interface{}{
				"error": err.Error(),
			})
			return
		}
	}

	// Search for all unseen messages
	responses, err := c.imapCommandMulti("SEARCH", "UNSEEN")
	if err != nil {
		logger.ErrorCF("email", "IMAP SEARCH failed", map[string]interface{}{
			"error": err.Error(),
		})
		// Connection may be stale, close it for reconnection on next poll
		c.disconnectIMAP()
		return
	}

	// Parse sequence numbers from SEARCH response
	seqNums := c.parseSearchResults(responses)
	if len(seqNums) == 0 {
		return
	}

	logger.DebugCF("email", "Found new emails", map[string]interface{}{
		"count": len(seqNums),
	})

	// Fetch each message
	for _, seqNum := range seqNums {
		if c.ctx.Err() != nil {
			return
		}
		c.fetchMessage(seqNum)
	}
}

// fetchMessage fetches and processes a single email by sequence number.
func (c *EmailChannel) fetchMessage(seqNum string) {
	// Fetch the message envelope and body
	fetchCmd := fmt.Sprintf("%s (ENVELOPE BODY[HEADER.FIELDS (SUBJECT FROM MESSAGE-ID)])", seqNum)
	responses, err := c.imapCommandMulti("FETCH", fetchCmd)
	if err != nil {
		logger.ErrorCF("email", "IMAP FETCH failed", map[string]interface{}{
			"seq":   seqNum,
			"error": err.Error(),
		})
		return
	}

	// Parse the email headers
	from, subject, messageID := c.parseEmailHeaders(responses)

	// Skip if already seen
	c.seenMu.Lock()
	if messageID != "" && c.seenUIDs[messageID] {
		c.seenMu.Unlock()
		return
	}
	if messageID != "" {
		c.seenUIDs[messageID] = true
	}
	c.seenMu.Unlock()

	// Fetch the body text
	bodyCmd := fmt.Sprintf("%s (BODY[TEXT])", seqNum)
	bodyResponses, err := c.imapCommandMulti("FETCH", bodyCmd)
	if err != nil {
		logger.ErrorCF("email", "IMAP FETCH body failed", map[string]interface{}{
			"seq":   seqNum,
			"error": err.Error(),
		})
		// Use empty body if we can't fetch it
		bodyResponses = nil
	}

	body := c.parseEmailBody(bodyResponses)

	// Extract the email address from the From header
	senderEmail := c.extractEmailAddress(from)
	chatID := senderEmail
	if chatID == "" {
		chatID = "email:unknown"
	}

	// Store sender info for replies
	if senderEmail != "" {
		c.senderMap.Store(chatID, senderEmail)
	}

	// Store subject for reply threading
	replySubject := subject
	if replySubject != "" && !strings.HasPrefix(strings.ToLower(replySubject), "re:") {
		replySubject = "Re: " + replySubject
	}
	if replySubject == "" {
		replySubject = "Re: NemesisBot Response"
	}
	c.subjectMap.Store(chatID, replySubject)

	// Mark as seen
	c.imapCommand("STORE", fmt.Sprintf("%s +FLAGS (\\Seen)", seqNum))

	metadata := map[string]string{
		"platform":   "email",
		"from":       from,
		"subject":    subject,
		"message_id": messageID,
	}

	logger.DebugCF("email", "Received email", map[string]interface{}{
		"from":    from,
		"subject": subject,
		"chat_id": chatID,
		"preview": utils.Truncate(body, 50),
	})

	// Publish to message bus
	c.HandleMessage(senderEmail, chatID, body, nil, metadata)
}

// sendEmail sends an email via SMTP.
func (c *EmailChannel) sendEmail(to, subject, body string) error {
	// Reconnect SMTP if necessary
	if c.smtpClient == nil {
		if err := c.connectSMTP(); err != nil {
			return fmt.Errorf("SMTP reconnection failed: %w", err)
		}
	}

	client := c.smtpClient

	// Set the sender
	if err := client.Mail(c.config.SMTPUsername); err != nil {
		// Connection may be stale, try reconnecting
		c.disconnectSMTP()
		if retryErr := c.connectSMTP(); retryErr != nil {
			return fmt.Errorf("SMTP reconnect after Mail failure: %w", retryErr)
		}
		client = c.smtpClient
		if err := client.Mail(c.config.SMTPUsername); err != nil {
			return fmt.Errorf("SMTP MAIL FROM failed: %w", err)
		}
	}

	// Set the recipient
	if err := client.Rcpt(to); err != nil {
		return fmt.Errorf("SMTP RCPT TO failed: %w", err)
	}

	// Send the message body
	wc, err := client.Data()
	if err != nil {
		return fmt.Errorf("SMTP DATA failed: %w", err)
	}
	defer wc.Close()

	// Compose the email with proper headers
	emailContent := fmt.Sprintf("From: %s\r\n", c.config.SMTPUsername)
	emailContent += fmt.Sprintf("To: %s\r\n", to)
	emailContent += fmt.Sprintf("Subject: %s\r\n", subject)
	emailContent += fmt.Sprintf("Date: %s\r\n", time.Now().Format(time.RFC1123Z))
	emailContent += "MIME-Version: 1.0\r\n"
	emailContent += "Content-Type: text/plain; charset=UTF-8\r\n"
	emailContent += "\r\n"
	emailContent += body

	if _, err := wc.Write([]byte(emailContent)); err != nil {
		return fmt.Errorf("SMTP write body failed: %w", err)
	}

	return nil
}

// imapCommand sends a tagged IMAP command and waits for the response.
func (c *EmailChannel) imapCommand(command, args string) error {
	tag := "NB00"
	var cmd string
	if args != "" {
		cmd = fmt.Sprintf("%s %s %s\r\n", tag, command, args)
	} else {
		cmd = fmt.Sprintf("%s %s\r\n", tag, command)
	}

	if _, err := c.imapConn.Write([]byte(cmd)); err != nil {
		return fmt.Errorf("IMAP write failed: %w", err)
	}

	return c.readUntilTagged(tag)
}

// imapCommandMulti sends a tagged IMAP command and collects all untagged responses
// until the tagged completion response.
func (c *EmailChannel) imapCommandMulti(command, args string) ([]string, error) {
	tag := "NB00"
	var cmd string
	if args != "" {
		cmd = fmt.Sprintf("%s %s %s\r\n", tag, command, args)
	} else {
		cmd = fmt.Sprintf("%s %s\r\n", tag, command)
	}

	if _, err := c.imapConn.Write([]byte(cmd)); err != nil {
		return nil, fmt.Errorf("IMAP write failed: %w", err)
	}

	var responses []string
	for {
		line, err := c.readIMAPLine()
		if err != nil {
			return responses, err
		}

		if strings.HasPrefix(line, tag+" ") {
			// Tagged completion response
			if strings.Contains(line, "OK") {
				return responses, nil
			}
			return responses, fmt.Errorf("IMAP error: %s", line)
		}

		// Untagged response or continuation
		responses = append(responses, line)
	}
}

// readUntilTagged reads IMAP lines until a tagged response is received.
func (c *EmailChannel) readUntilTagged(tag string) error {
	for {
		line, err := c.readIMAPLine()
		if err != nil {
			return err
		}

		if strings.HasPrefix(line, tag+" ") {
			if strings.Contains(line, "OK") {
				return nil
			}
			return fmt.Errorf("IMAP error: %s", line)
		}
	}
}

// readIMAPLine reads a single line from the IMAP connection.
func (c *EmailChannel) readIMAPLine() (string, error) {
	var line []byte
	buf := make([]byte, 1)

	for {
		_, err := c.imapConn.Read(buf)
		if err != nil {
			return "", err
		}

		line = append(line, buf[0])

		// Check for CRLF line ending
		if len(line) >= 2 && line[len(line)-2] == '\r' && line[len(line)-1] == '\n' {
			return string(line[:len(line)-2]), nil
		}
	}
}

// imapQuote wraps a string in quotes for IMAP commands.
func (c *EmailChannel) imapQuote(s string) string {
	return fmt.Sprintf("\"%s\"", strings.ReplaceAll(s, "\"", "\\\""))
}

// parseSearchResults extracts sequence numbers from IMAP SEARCH responses.
func (c *EmailChannel) parseSearchResults(responses []string) []string {
	var seqNums []string
	for _, resp := range responses {
		if strings.HasPrefix(resp, "* SEARCH") {
			parts := strings.Fields(strings.TrimPrefix(resp, "* SEARCH"))
			for _, p := range parts {
				p = strings.TrimSpace(p)
				if p != "" {
					seqNums = append(seqNums, p)
				}
			}
		}
	}
	return seqNums
}

// parseEmailHeaders extracts From, Subject, and Message-ID from FETCH response lines.
func (c *EmailChannel) parseEmailHeaders(responses []string) (from, subject, messageID string) {
	var inLiteral bool
	var headerData strings.Builder

	for _, line := range responses {
		// Detect literal string marker {N}
		if strings.Contains(line, "{") && strings.Contains(line, "}") {
			// Extract literal size
			start := strings.Index(line, "{")
			end := strings.Index(line, "}")
			if start >= 0 && end > start {
				inLiteral = true
				continue
			}
		}

		if inLiteral {
			headerData.WriteString(line)
			headerData.WriteString("\r\n")
			continue
		}

		// Check for header fields in the response
		lower := strings.ToLower(line)
		if strings.HasPrefix(lower, "from:") {
			from = strings.TrimSpace(strings.TrimPrefix(line, "From:"))
			// Remove angle brackets display format if present
			from = strings.TrimSpace(from)
		} else if strings.HasPrefix(lower, "subject:") {
			subject = strings.TrimSpace(strings.TrimPrefix(line, "Subject:"))
		} else if strings.HasPrefix(lower, "message-id:") {
			messageID = strings.TrimSpace(strings.TrimPrefix(line, "Message-ID:"))
			messageID = strings.Trim(messageID, "<> ")
		}
	}

	// Also parse from the header literal data if present
	headerStr := headerData.String()
	for _, hdrLine := range strings.Split(headerStr, "\r\n") {
		lower := strings.ToLower(hdrLine)
		if strings.HasPrefix(lower, "from:") && from == "" {
			from = strings.TrimSpace(strings.TrimPrefix(hdrLine, "From:"))
		} else if strings.HasPrefix(lower, "subject:") && subject == "" {
			subject = strings.TrimSpace(strings.TrimPrefix(hdrLine, "Subject:"))
		} else if strings.HasPrefix(lower, "message-id:") && messageID == "" {
			messageID = strings.TrimSpace(strings.TrimPrefix(hdrLine, "Message-ID:"))
			messageID = strings.Trim(messageID, "<> ")
		}
	}

	return from, subject, messageID
}

// parseEmailBody extracts the text body from FETCH BODY[TEXT] responses.
func (c *EmailChannel) parseEmailBody(responses []string) string {
	if len(responses) == 0 {
		return ""
	}

	var bodyParts []string
	for _, line := range responses {
		// Skip FETCH command echoes and closing parenthesis
		if strings.HasPrefix(line, "* ") && strings.Contains(line, "FETCH") {
			continue
		}
		if line == ")" {
			continue
		}
		bodyParts = append(bodyParts, line)
	}

	body := strings.Join(bodyParts, "\n")
	return strings.TrimSpace(body)
}

// extractEmailAddress extracts the bare email address from a From header value.
// Examples: "John Doe <john@example.com>" -> "john@example.com"
//           "john@example.com" -> "john@example.com"
func (c *EmailChannel) extractEmailAddress(from string) string {
	from = strings.TrimSpace(from)

	// Try to extract from angle brackets
	start := strings.LastIndex(from, "<")
	end := strings.LastIndex(from, ">")
	if start >= 0 && end > start {
		return strings.TrimSpace(from[start+1 : end])
	}

	// No angle brackets, check if it looks like an email
	if strings.Contains(from, "@") {
		return from
	}

	return ""
}

// Ensure EmailChannel implements the Channel interface at compile time.
var _ Channel = (*EmailChannel)(nil)
