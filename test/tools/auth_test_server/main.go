// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
//
// Simple HTTP server for testing auth OAuth flows

package main

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"strconv"
)

func main() {
	port := 8082
	if len(os.Args) > 1 {
		if p, err := strconv.Atoi(os.Args[1]); err == nil {
			port = p
		}
	}

	mux := http.NewServeMux()

	// OAuth endpoints
	mux.HandleFunc("/auth/device", handleDeviceCode)
	mux.HandleFunc("/auth/token", handleTokenExchange)
	mux.HandleFunc("/auth/callback", handleCallback)

	server := &http.Server{
		Addr:    ":" + strconv.Itoa(port),
		Handler: mux,
	}

	log.Printf("Auth test server running on port %d\n", port)
	log.Printf("Device code endpoint: http://localhost:%d/auth/device\n", port)
	log.Printf("Token endpoint: http://localhost:%d/auth/token\n", port)
	log.Printf("Callback endpoint: http://localhost:%d/auth/callback\n", port)

	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatalf("Server error: %v\n", err)
	}
}

func handleDeviceCode(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]interface{}{
		"device_auth_id": "test_device_id",
		"user_code":      "TEST-CODE",
		"interval":       5,
	})
}

func handleTokenExchange(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")

	// Mock JWT token
	mockToken := "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ0ZXN0X3VzZXJfaWQifQ.signature"

	json.NewEncoder(w).Encode(map[string]interface{}{
		"access_token":  mockToken,
		"refresh_token": "refresh_" + mockToken,
		"token_type":    "Bearer",
		"expires_in":    3600,
		"id_token":      mockToken,
	})
}

func handleCallback(w http.ResponseWriter, r *http.Request) {
	code := r.URL.Query().Get("code")
	state := r.URL.Query().Get("state")

	w.Header().Set("Content-Type", "text/html")
	w.WriteHeader(http.StatusOK)
	fmt.Fprintf(w, `<html><body><h2>Authentication successful!</h2>
<p>Code: %s</p><p>State: %s</p><p>You can close this window.</p></body></html>`, code, state)
}
