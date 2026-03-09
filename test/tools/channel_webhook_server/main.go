// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
//
// Mock test tools for channels module testing

package main

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"strconv"
)

// Webhook test server for testing channel webhook endpoints

func main() {
	port := 8083
	if len(os.Args) > 1 {
		if p, err := strconv.Atoi(os.Args[1]); err == nil {
			port = p
		}
	}

	mux := http.NewServeMux()

	// Telegram webhook endpoint
	mux.HandleFunc("/telegram/webhook", func(w http.ResponseWriter, r *http.Request) {
		var update map[string]interface{}
		json.NewDecoder(r.Body).Decode(&update)

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"ok":          true,
			"result":      true,
			"webhook_received": true,
		})
	})

	// LINE webhook endpoint
	mux.HandleFunc("/line/webhook", func(w http.ResponseWriter, r *http.Request) {
		var events []map[string]interface{}
		json.NewDecoder(r.Body).Decode(&events)

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"webhook_received": true,
			"event_count":      len(events),
		})
	})

	// Slack event endpoint
	mux.HandleFunc("/slack/events", func(w http.ResponseWriter, r *http.Request) {
		var payload map[string]interface{}
		json.NewDecoder(r.Body).Decode(&payload)

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"ok": true,
		})
	})

	// Discord webhook
	mux.HandleFunc("/discord/webhook", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]interface{}{
			"message": "Webhook received",
		})
	})

	// OneBot WebSocket upgrade simulation
	mux.HandleFunc("/onebot/ws", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.WriteHeader(http.StatusOK)
		fmt.Fprintf(w, "WebSocket endpoint - would upgrade here\n")
	})

	// File download endpoint
	mux.HandleFunc("/files/download", func(w http.ResponseWriter, r *http.Request) {
		filename := r.URL.Query().Get("file")
		w.Header().Set("Content-Type", "application/octet-stream")
		w.Header().Set("Content-Disposition", fmt.Sprintf("attachment; filename=%s", filename))
		w.Write([]byte("Mock file content"))
	})

	// External channel input/output
	mux.HandleFunc("/external/input", func(w http.ResponseWriter, r *http.Request) {
		var msg map[string]interface{}
		json.NewDecoder(r.Body).Decode(&msg)

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(msg)
	})

	mux.HandleFunc("/external/output", func(w http.ResponseWriter, r *http.Request) {
		var msg map[string]interface{}
		json.NewDecoder(r.Body).Decode(&msg)

		w.WriteHeader(http.StatusOK)
	})

	server := &http.Server{
		Addr:    ":" + strconv.Itoa(port),
		Handler: mux,
	}

	log.Printf("Channel webhook test server running on port %d\n", port)
	log.Printf("Endpoints:\n")
	log.Printf("  - Telegram: http://localhost:%d/telegram/webhook\n", port)
	log.Printf("  - LINE:      http://localhost:%d/line/webhook\n", port)
	log.Printf("  - Slack:     http://localhost:%d/slack/events\n", port)
	log.Printf("  - Discord:   http://localhost:%d/discord/webhook\n", port)
	log.Printf("  - OneBot:    http://localhost:%d/onebot/ws\n", port)
	log.Printf("  - Files:     http://localhost:%d/files/download?file=test.txt\n", port)

	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatalf("Server error: %v\n", err)
	}
}
