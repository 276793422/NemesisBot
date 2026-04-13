//go:build !cgo

package main

import (
	"bufio"
	"fmt"
	"log"
	"net"
	"os"
	"strings"
	"sync"

	"websocket-client/src/client"
	"websocket-client/src/config"
)

const defaultListenAddr = "127.0.0.1:19876"

// session holds the state for one connected client.
type session struct {
	mu  sync.Mutex
	cli *client.WebSocketClient
}

func main() {
	listenAddr := defaultListenAddr
	if len(os.Args) > 1 {
		listenAddr = os.Args[1]
	}

	ln, err := net.Listen("tcp", listenAddr)
	if err != nil {
		log.Fatalf("failed to listen on %s: %v", listenAddr, err)
	}
	defer ln.Close()

	fmt.Printf("WSC Bridge listening on %s\n", listenAddr)
	fmt.Println("Commands: INIT <url> [<token>] | SEND <content> | RECV [<timeout_ms>] | DESTROY | QUIT")

	for {
		conn, err := ln.Accept()
		if err != nil {
			log.Printf("accept error: %v", err)
			continue
		}
		go handleConn(conn)
	}
}

func handleConn(conn net.Conn) {
	defer conn.Close()
	s := &session{}
	scanner := bufio.NewScanner(conn)
	// Increase buffer size for large messages
	scanner.Buffer(make([]byte, 0), 1024*1024)

	for scanner.Scan() {
		line := scanner.Text()
		if line == "" {
			continue
		}

		cmd, arg := splitCmd(line)
		var resp string

		switch cmd {
		case "INIT":
			resp = s.handleInit(arg)
		case "SEND":
			resp = s.handleSend(arg)
		case "RECV":
			resp = s.handleRecv(arg)
		case "DESTROY":
			resp = s.handleDestroy()
		case "QUIT":
			s.handleDestroy()
			conn.Write([]byte("BYE\n"))
			return
		default:
			resp = "ERROR unknown command"
		}

		conn.Write([]byte(resp + "\n"))
	}
}

func (s *session) handleInit(arg string) string {
	s.mu.Lock()
	defer s.mu.Unlock()

	// Destroy existing connection
	if s.cli != nil {
		s.cli.Destroy()
		s.cli = nil
	}

	parts := strings.SplitN(arg, " ", 2)
	url := strings.TrimSpace(parts[0])
	if url == "" {
		return "ERROR missing url"
	}

	token := ""
	if len(parts) > 1 {
		token = strings.TrimSpace(parts[1])
	}

	cfg := config.NewConfig(url, token)
	cli := client.New(cfg)

	if err := cli.Start(); err != nil {
		return fmt.Sprintf("ERROR %s", err)
	}

	s.cli = cli
	return "OK"
}

func (s *session) handleSend(content string) string {
	s.mu.Lock()
	cli := s.cli
	s.mu.Unlock()

	if cli == nil {
		return "ERROR not initialized"
	}

	if !cli.IsConnected() {
		return "ERROR not connected"
	}

	if err := cli.Send(content); err != nil {
		return fmt.Sprintf("ERROR %s", err)
	}

	return "OK"
}

func (s *session) handleRecv(arg string) string {
	s.mu.Lock()
	cli := s.cli
	s.mu.Unlock()

	if cli == nil {
		return "ERROR not initialized"
	}

	timeoutMs := 0
	if arg != "" {
		fmt.Sscanf(arg, "%d", &timeoutMs)
	}

	data := cli.Recv(timeoutMs)
	if data == nil {
		return "TIMEOUT"
	}

	return "MSG " + string(data)
}

func (s *session) handleDestroy() string {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.cli != nil {
		s.cli.Destroy()
		s.cli = nil
	}

	return "OK"
}

// splitCmd splits a line into command and argument.
// "SEND hello world" → ("SEND", "hello world")
func splitCmd(line string) (string, string) {
	idx := strings.IndexByte(line, ' ')
	if idx < 0 {
		return strings.ToUpper(line), ""
	}
	return strings.ToUpper(line[:idx]), line[idx+1:]
}
