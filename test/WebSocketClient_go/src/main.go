package main

import (
	"bufio"
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
	// Load configuration
	cfg := config.LoadOrCreateDefault()

	printBanner()
	printConfig(cfg)

	// Create client
	wsClient := client.New(cfg)
	cliChannel := wsClient.GetCLIMessageChannel()

	// Start client in goroutine
	clientDone := make(chan error, 1)
	go func() {
		clientDone <- wsClient.Start()
	}()

	// Wait a bit for connection
	time.Sleep(500 * time.Millisecond)

	fmt.Println("✅ Ready! Type your messages below.")
	printHelp()

	// Start CLI input in separate goroutine
	state := &CLIState{}
	state.Running.Store(true)

	go runCLILoop(state, cfg, cliChannel, wsClient)

	// Wait for client to finish
	if err := <-clientDone; err != nil {
		fmt.Printf("❌ Client error: %v\n", err)
	}

	fmt.Println("\n🔌 Connection closed")
}

func runCLILoop(state *CLIState, cfg *config.Config, cliChannel chan<- string, wsClient *client.WebSocketClient) {
	scanner := bufio.NewScanner(os.Stdin)
	quitCommand := false

	for state.Running.Load() && !quitCommand {
		printPrompt(cfg)

		if !scanner.Scan() {
			// Input closed (EOF), but keep running to receive responses
			break
		}

		input := strings.TrimSpace(scanner.Text())
		if input == "" {
			continue
		}

		if handleCommand(state, cfg, input, cliChannel, wsClient) {
			// Quit command
			quitCommand = true
			state.Running.Store(false)
			wsClient.Stop()
			break
		}
	}

	if err := scanner.Err(); err != nil {
		fmt.Printf("⚠️  Input error: %v\n", err)
	}

	// Always signal that CLI input is done
	close(cliChannel)
	fmt.Println("📤 CLI loop ended")
}

func handleCommand(state *CLIState, cfg *config.Config, input string, cliChannel chan<- string, wsClient *client.WebSocketClient) bool {
	switch input {
	case CmdQuit, CmdExit, CmdQ:
		return true
	case CmdHelp, CmdH:
		printHelp()
	case CmdClear, CmdC:
		clearScreen()
	case CmdStats:
		fmt.Println("📊 Statistics will be shown on exit")
	default:
		// Send to WebSocket server
		select {
		case cliChannel <- input:
		default:
			fmt.Println("⚠️  Channel full, message dropped")
		}
	}
	return false
}

func printBanner() {
	fmt.Println()
	border := "╔════════════════════════════════════════════════════════╗"
	title := "║  🤖 NemesisBot WebSocket Client v0.1.0 (Go)              "
	fmt.Println(border)
	fmt.Println("║")
	fmt.Println(title)
	fmt.Println("║")
	fmt.Println(border)
	fmt.Println()
}

func printConfig(cfg *config.Config) {
	fmt.Println("📁 Configuration:")
	fmt.Printf("   Server URL: %s\n", cfg.Server.URL)
	reconnectStatus := "❌"
	if cfg.Reconnect.Enabled {
		reconnectStatus = "✅"
	}
	fmt.Printf("   Auto-reconnect: %s\n", reconnectStatus)
	heartbeatStatus := "❌"
	if cfg.Heartbeat.Enabled {
		heartbeatStatus = "✅"
	}
	fmt.Printf("   Heartbeat: %s\n", heartbeatStatus)
	loggingStatus := "❌"
	if cfg.Logging.Enabled {
		loggingStatus = "✅"
	}
	fmt.Printf("   Logging: %s\n", loggingStatus)
	fmt.Println()
}

func printHelp() {
	fmt.Println()
	fmt.Println("📖 Available Commands:")
	fmt.Printf("  %s - Show this help message\n", CmdHelp)
	fmt.Printf("  %s, %s - Exit the client\n", CmdQuit, CmdExit)
	fmt.Printf("  %s - Show connection statistics\n", CmdStats)
	fmt.Printf("  %s - Clear the screen\n", CmdClear)
	fmt.Println("  ... - Any other text will be sent as a message to the server")
	fmt.Println()
}

func printPrompt(cfg *config.Config) {
	promptStr := "➤ "
	if cfg.UI.PromptStyle == "detailed" {
		promptStr = "➤ [Connected] "
	}
	fmt.Print(promptStr)
}

func clearScreen() {
	print("\033[H\033[2J")
}
