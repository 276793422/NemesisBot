package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"sync/atomic"
	"time"

	"websocket-client/src/client"
	"websocket-client/src/config"
)

const (
	CmdQuit  = "/quit"
	CmdExit  = "/exit"
	CmdQ     = "/q"
	CmdHelp  = "/help"
	CmdH     = "/h"
	CmdClear = "/clear"
	CmdC     = "/c"
	CmdStats = "/stats"
)

type CLIState struct {
	Running atomic.Bool
}

func main() {
	cfg := config.LoadOrCreateDefault()

	printBanner()
	printConfig(cfg)

	wsClient := client.New(cfg)

	if err := wsClient.Start(); err != nil {
		fmt.Printf("Connect failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Connected! Type your messages below.")
	printHelp()

	state := &CLIState{}
	state.Running.Store(true)

	// Background goroutine: receive messages and print them
	go func() {
		for state.Running.Load() {
			data := wsClient.Recv(500) // poll every 500ms
			if data == nil {
				continue
			}
			var msg client.ServerMessage
			if json.Unmarshal(data, &msg) != nil {
				continue
			}
			printServerMessage(&msg)
		}
	}()

	// Main goroutine: read user input and send
	scanner := bufio.NewScanner(os.Stdin)
	for state.Running.Load() {
		fmt.Print("> ")
		if !scanner.Scan() {
			break
		}

		input := strings.TrimSpace(scanner.Text())
		if input == "" {
			continue
		}

		switch input {
		case CmdQuit, CmdExit, CmdQ:
			state.Running.Store(false)
		case CmdHelp, CmdH:
			printHelp()
		case CmdClear, CmdC:
			fmt.Print("\033[H\033[2J")
		case CmdStats:
			fmt.Printf("Connected: %v\n", wsClient.IsConnected())
		default:
			if err := wsClient.Send(input); err != nil {
				fmt.Printf("Send failed: %v\n", err)
			}
		}
	}

	wsClient.Destroy()
	fmt.Println("Connection closed")
}

func printBanner() {
	fmt.Println()
	fmt.Println("  NemesisBot WebSocket Client v0.2.0 (Go)")
	fmt.Println()
}

func printConfig(cfg *config.Config) {
	fmt.Printf("  Server: %s\n", cfg.Server.URL)
	fmt.Printf("  Reconnect: %v\n", cfg.Reconnect.Enabled)
	fmt.Printf("  Heartbeat: %v\n", cfg.Heartbeat.Enabled)
	fmt.Println()
}

func printHelp() {
	fmt.Println()
	fmt.Printf("  %s - Show this help\n", CmdHelp)
	fmt.Printf("  %s, %s - Exit\n", CmdQuit, CmdExit)
	fmt.Printf("  %s - Show connection status\n", CmdStats)
	fmt.Printf("  %s - Clear screen\n", CmdClear)
	fmt.Println("  Any other text is sent as a message")
	fmt.Println()
}

func printServerMessage(msg *client.ServerMessage) {
	timestamp := time.Now().Format("15:04:05")
	switch msg.Type {
	case "message":
		fmt.Printf("\n[%s] %s: %s\n> ", timestamp, msg.Role, msg.Content)
	case "error":
		fmt.Printf("\n[%s] ERROR: %s\n> ", timestamp, msg.Error)
	}
}
