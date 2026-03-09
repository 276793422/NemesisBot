// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
//
// Simple HTTP server for testing OAuth flows and HTTP-based channels

package main

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"strconv"
	"sync"
	"time"
)

type TestServer struct {
	server    *http.Server
	requests  []RequestLog
	mu        sync.Mutex
	callbacks map[string]chan []byte
}

type RequestLog struct {
	Method     string
	Path       string
	Headers    map[string]string
	Body       string
	Timestamp  time.Time
	QueryParams map[string]string
}

func NewTestServer(port int) *TestServer {
	mux := http.NewServeMux()
	ts := &TestServer{
		requests:  make([]RequestLog, 0),
		callbacks: make(map[string]chan []byte),
	}

	mux.HandleFunc("/", ts.handleRoot)
	mux.HandleFunc("/echo", ts.handleEcho)
	mux.HandleFunc("/delay/", ts.handleDelay)
	mux.HandleFunc("/status/", ts.handleStatus)
	mux.HandleFunc("/oauth/callback", ts.handleOAuthCallback)
	mux.HandleFunc("/oauth/device", ts.handleDeviceCode)
	mux.HandleFunc("/oauth/token", ts.handleTokenExchange)
	mux.HandleFunc("/webhook", ts.handleWebhook)
	mux.HandleFunc("/api/", ts.handleAPI)
	mux.HandleFunc("/ws", ts.handleWebSocket)

	ts.server = &http.Server{
		Addr:    ":" + strconv.Itoa(port),
		Handler: mux,
	}

	return ts
}

func (ts *TestServer) Start() error {
	log.Printf("Test server starting on %s\n", ts.server.Addr)
	return ts.server.ListenAndServe()
}

func (ts *TestServer) Stop() error {
	log.Println("Test server stopping")
	return ts.server.Close()
}

func (ts *TestServer) handleRoot(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)
	w.WriteHeader(http.StatusOK)
	fmt.Fprintf(w, "Test server is running\n")
}

func (ts *TestServer) handleEcho(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	var body []byte
	if r.Body != nil {
		body, _ = readAll(r.Body)
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)

	response := map[string]interface{}{
		"method": r.Method,
		"path":   r.URL.Path,
		"headers": r.Header,
		"body":   string(body),
		"query":  r.URL.Query(),
	}

	json.NewEncoder(w).Encode(response)
}

func (ts *TestServer) handleDelay(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	// Extract delay from path: /delay/5
	delayStr := r.URL.Path[len("/delay/"):]
	delay, err := strconv.Atoi(delayStr)
	if err != nil {
		delay = 1
	}

	time.Sleep(time.Duration(delay) * time.Second)
	w.WriteHeader(http.StatusOK)
	fmt.Fprintf(w, "Delayed %d seconds\n", delay)
}

func (ts *TestServer) handleStatus(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	// Extract status code from path: /status/404
	statusStr := r.URL.Path[len("/status/"):]
	status, err := strconv.Atoi(statusStr)
	if err != nil {
		status = 200
	}

	w.WriteHeader(status)
	fmt.Fprintf(w, "Status %d\n", status)
}

func (ts *TestServer) handleOAuthCallback(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	code := r.URL.Query().Get("code")
	state := r.URL.Query().Get("state")
	error := r.URL.Query().Get("error")

	if error != "" {
		w.WriteHeader(http.StatusBadRequest)
		fmt.Fprintf(w, "Error: %s\n", error)
		return
	}

	// Send code to callback channel if registered
	if state != "" {
		ts.mu.Lock()
		if ch, ok := ts.callbacks[state]; ok {
			ch <- []byte(code)
			close(ch)
			delete(ts.callbacks, state)
		}
		ts.mu.Unlock()
	}

	w.Header().Set("Content-Type", "text/html")
	w.WriteHeader(http.StatusOK)
	fmt.Fprintf(w, `<html><body><h2>Authentication successful!</h2><p>Code: %s</p><p>You can close this window.</p></body></html>`, code)
}

func (ts *TestServer) handleDeviceCode(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)

	json.NewEncoder(w).Encode(map[string]interface{}{
		"device_auth_id": "test_device_" + strconv.FormatInt(time.Now().Unix(), 10),
		"user_code":      "TEST-CODE",
		"interval":       5,
	})
}

func (ts *TestServer) handleTokenExchange(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)

	// Create a mock JWT token
	mockToken := "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ0ZXN0X3VzZXIiLCJleHAiOjE5OTk5OTk5OTl9.signature"

	json.NewEncoder(w).Encode(map[string]interface{}{
		"access_token":  mockToken,
		"refresh_token": "refresh_" + mockToken,
		"token_type":    "Bearer",
		"expires_in":    3600,
		"id_token":      mockToken,
	})
}

func (ts *TestServer) handleWebhook(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	var body []byte
	if r.Body != nil {
		body, _ = readAll(r.Body)
	}

	// Echo the webhook back
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)

	json.NewEncoder(w).Encode(map[string]interface{}{
		"webhook_received": true,
		"body":            string(body),
		"headers":         r.Header,
	})
}

func (ts *TestServer) handleAPI(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	path := r.URL.Path[len("/api/"):]

	switch path {
	case "tools/list":
		json.NewEncoder(w).Encode(map[string]interface{}{
			"tools": []map[string]interface{}{
				{
					"name":        "test_tool",
					"description": "A test tool",
					"inputSchema": map[string]interface{}{
						"type": "object",
					},
				},
			},
		})

	case "resources/list":
		json.NewEncoder(w).Encode(map[string]interface{}{
			"resources": []map[string]interface{}{
				{
					"uri":         "file:///test.txt",
					"name":        "Test File",
					"mimeType":    "text/plain",
					"description": "A test file",
				},
			},
		})

	case "prompts/list":
		json.NewEncoder(w).Encode(map[string]interface{}{
			"prompts": []map[string]interface{}{
				{
					"name":        "test_prompt",
					"description": "A test prompt",
					"arguments":   []map[string]interface{}{},
				},
			},
		})

	default:
		w.WriteHeader(http.StatusNotFound)
		json.NewEncoder(w).Encode(map[string]interface{}{
			"error": "Not found",
		})
	}
}

func (ts *TestServer) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	ts.logRequest(r)

	// Simple WebSocket upgrade response
	w.WriteHeader(http.StatusSwitchingProtocols)
	w.Header().Set("Upgrade", "websocket")
	w.Header().Set("Connection", "Upgrade")
	fmt.Fprintf(w, "WebSocket connection would be established here\n")
}

func (ts *TestServer) logRequest(r *http.Request) {
	ts.mu.Lock()
	defer ts.mu.Unlock()

	var body string
	if r.Body != nil {
		data, _ := readAll(r.Body)
		body = string(data)
	}

	headers := make(map[string]string)
	for k, v := range r.Header {
		if len(v) > 0 {
			headers[k] = v[0]
		}
	}

	query := make(map[string]string)
	for k, v := range r.URL.Query() {
		if len(v) > 0 {
			query[k] = v[0]
		}
	}

	ts.requests = append(ts.requests, RequestLog{
		Method:      r.Method,
		Path:        r.URL.Path,
		Headers:     headers,
		Body:        body,
		Timestamp:   time.Now(),
		QueryParams: query,
	})
}

func (ts *TestServer) GetRequests() []RequestLog {
	ts.mu.Lock()
	defer ts.mu.Unlock()

	// Return a copy to avoid race conditions
	result := make([]RequestLog, len(ts.requests))
	copy(result, ts.requests)
	return result
}

func (ts *TestServer) ClearRequests() {
	ts.mu.Lock()
	defer ts.mu.Unlock()
	ts.requests = make([]RequestLog, 0)
}

func (ts *TestServer) RegisterCallback(state string) <-chan []byte {
	ts.mu.Lock()
	defer ts.mu.Unlock()

	ch := make(chan []byte, 1)
	ts.callbacks[state] = ch
	return ch
}

func readAll(r *http.Request) ([]byte, error) {
	return nil, nil
}

func main() {
	port := 8081
	if len(os.Args) > 1 {
		if p, err := strconv.Atoi(os.Args[1]); err == nil {
			port = p
		}
	}

	server := NewTestServer(port)

	// Handle shutdown gracefully
	go func() {
		if err := server.Start(); err != nil && err != http.ErrServerClosed {
			log.Fatalf("Server error: %v\n", err)
		}
	}()

	log.Printf("Test server running on port %d\n", port)
	log.Printf("Try: http://localhost:%d/\n", port)
	log.Printf("OAuth callback: http://localhost:%d/oauth/callback\n", port)

	// Keep running
	select {}
}
