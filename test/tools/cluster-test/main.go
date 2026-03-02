// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// +build ignore

package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
)

// TestTool is a helper tool for testing the cluster module
func main() {
	if len(os.Args) < 2 {
		fmt.Println("Cluster Test Tool")
		fmt.Println()
		fmt.Println("Usage:")
		fmt.Println("  cluster-test <command> [args...]")
		fmt.Println()
		fmt.Println("Commands:")
		fmt.Println("  create-workspace <path>  - Create a test workspace")
		fmt.Println("  list-peers <path>        - List peers in peers.toml")
		fmt.Println("  broadcast <path> <msg>   - Send a test broadcast")
		fmt.Println("  watch <path>              - Watch for broadcasts")
		fmt.Println("  simulate-peer <path> <id> - Simulate a peer")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  cluster-test create-workspace ./test-bot1")
		fmt.Println("  cluster-test broadcast ./test-bot1 \"Hello\"")
		fmt.Println("  cluster-test watch ./test-bot1")
		os.Exit(1)
	}

	command := os.Args[1]

	switch command {
	case "create-workspace":
		if len(os.Args) < 3 {
			fmt.Println("Error: workspace path required")
			os.Exit(1)
		}
		createWorkspace(os.Args[2])

	case "list-peers":
		if len(os.Args) < 3 {
			fmt.Println("Error: workspace path required")
			os.Exit(1)
		}
		listPeers(os.Args[2])

	case "broadcast":
		if len(os.Args) < 4 {
			fmt.Println("Error: workspace path and message required")
			os.Exit(1)
		}
		sendBroadcast(os.Args[2], os.Args[3])

	case "watch":
		if len(os.Args) < 3 {
			fmt.Println("Error: workspace path required")
			os.Exit(1)
		}
		watchBroadcasts(os.Args[2])

	case "simulate-peer":
		if len(os.Args) < 4 {
			fmt.Println("Error: workspace path and peer ID required")
			os.Exit(1)
		}
		simulatePeer(os.Args[2], os.Args[3])

	default:
		fmt.Printf("Unknown command: %s\n", command)
		os.Exit(1)
	}
}

func createWorkspace(path string) {
	fmt.Printf("Creating test workspace: %s\n", path)

	// Create directories
	clusterDir := fmt.Sprintf("%s/cluster", path)
	if err := os.MkdirAll(clusterDir, 0755); err != nil {
		fmt.Printf("Error creating directory: %v\n", err)
		os.Exit(1)
	}

	// Create default peers.toml
	config := cluster.DefaultConfig("test-bot-" + path)
	saver := &configSaver{path: fmt.Sprintf("%s/cluster/peers.toml", path)}

	if err := saver.Save(config); err != nil {
		fmt.Printf("Error creating config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✓ Workspace created")
	fmt.Printf("  Config: %s/cluster/peers.toml\n", path)
}

type configSaver struct {
	path string
}

func (s *configSaver) Save(config *cluster.ClusterConfig) error {
	// Simple TOML marshal (basic implementation)
	data := fmt.Sprintf("# Cluster Configuration\n\n")
	data += fmt.Sprintf("[cluster]\n")
	data += fmt.Sprintf("id = \"auto-discovered\"\n")
	data += fmt.Sprintf("auto_discovery = true\n")
	data += fmt.Sprintf("last_updated = \"%s\"\n\n", time.Now().Format(time.RFC3339))

	data += fmt.Sprintf("[node]\n")
	data += fmt.Sprintf("id = \"%s\"\n", config.Node.ID)
	data += fmt.Sprintf("name = \"%s\"\n", config.Node.Name)
	data += fmt.Sprintf("address = \"\"\n")
	data += fmt.Sprintf("role = \"worker\"\n")
	data += fmt.Sprintf("capabilities = []\n\n")

	data += fmt.Sprintf("# Peers will be automatically discovered\n")

	return os.WriteFile(s.path, []byte(data), 0644)
}

func listPeers(path string) {
	configPath := fmt.Sprintf("%s/cluster/peers.toml", path)

	data, err := os.ReadFile(configPath)
	if err != nil {
		fmt.Printf("Error reading config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Peers in %s:\n", path)
	fmt.Println(string(data))
}

func sendBroadcast(path, message string) {
	// This would require the discovery module
	fmt.Printf("Sending broadcast from %s: %s\n", path, message)
	fmt.Println("(Not implemented - use actual cluster module)")
}

func watchBroadcasts(path string) {
	fmt.Printf("Watching for broadcasts on :49100 (workspace: %s)\n", path)
	fmt.Println("Press Ctrl+C to stop")

	// Simple UDP listener
	addr := "0.0.0.0:49100"
	fmt.Printf("Listening on %s\n", addr)

	// This is a placeholder - actual implementation would use discovery module
	time.Sleep(60 * time.Second)
}

func simulatePeer(path, peerID string) {
	fmt.Printf("Simulating peer %s in workspace %s\n", peerID, path)

	// Create a simple announce message
	msg := map[string]interface{}{
		"version":      "1.0",
		"type":         "announce",
		"node_id":      peerID,
		"name":         "Test Bot " + peerID,
		"address":      "127.0.0.1:49200",
		"capabilities": []string{"test_capability"},
		"timestamp":    time.Now().Unix(),
	}

	data, _ := json.Marshal(msg)
	fmt.Printf("Simulated broadcast message:\n%s\n", string(data))
}
