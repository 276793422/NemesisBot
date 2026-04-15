package main

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"time"

	"github.com/gorilla/websocket"
)

var upgrader = websocket.Upgrader{CheckOrigin: func(r *http.Request) bool { return true }}

type serverMessage struct {
	Type      string `json:"type"`
	Role      string `json:"role,omitempty"`
	Content   string `json:"content,omitempty"`
	Timestamp string `json:"timestamp,omitempty"`
	Error     string `json:"error,omitempty"`
}

type clientMessage struct {
	Type    string `json:"type"`
	Content string `json:"content"`
}

func handleWS(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	defer conn.Close()

	// Send welcome (same as real NemesisBot)
	welcome, _ := json.Marshal(serverMessage{
		Type:      "message",
		Role:      "system",
		Content:   "Connected to NemesisBot WebSocket channel. Client ID: test-001",
		Timestamp: time.Now().Format(time.RFC3339),
	})
	conn.WriteMessage(websocket.TextMessage, welcome)
	log.Println("Client connected, sent welcome")

	for {
		_, data, err := conn.ReadMessage()
		if err != nil {
			log.Printf("Client disconnected: %v", err)
			return
		}

		var msg clientMessage
		if json.Unmarshal(data, &msg) != nil {
			continue
		}

		switch msg.Type {
		case "message":
			// Echo back as assistant
			resp, _ := json.Marshal(serverMessage{
				Type:      "message",
				Role:      "assistant",
				Content:   "Echo: " + msg.Content,
				Timestamp: time.Now().Format(time.RFC3339),
			})
			conn.WriteMessage(websocket.TextMessage, resp)
			log.Printf("Echoed: %s", msg.Content)

		case "ping":
			pong, _ := json.Marshal(serverMessage{
				Type:      "pong",
				Timestamp: time.Now().Format(time.RFC3339),
			})
			conn.WriteMessage(websocket.TextMessage, pong)
		}
	}
}

func main() {
	port := "49999"
	if len(os.Args) > 1 {
		port = os.Args[1]
	}

	http.HandleFunc("/ws", handleWS)
	addr := "127.0.0.1:" + port
	fmt.Printf("Mock WS server on ws://%s/ws\n", addr)
	log.Fatal(http.ListenAndServe(addr, nil))
}
