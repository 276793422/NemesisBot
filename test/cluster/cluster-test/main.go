// NemesisBot - Cluster Test Tool
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// This is an independent test tool to verify cluster discovery and communication
// Usage:
//   Terminal 1: go run cmd/cluster-test/main.go --node=A --udp-port=49101 --rpc-port=49201
//   Terminal 2: go run cmd/cluster-test/main.go --node=B --udp-port=49102 --rpc-port=49202
//   Terminal 3: go run cmd/cluster-test/main.go --node=C --udp-port=49103 --rpc-port=49203

package main

import (
	"flag"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
)

var (
	nodeName    = flag.String("node", "", "Node name (e.g., A, B, C)")
	udpPort     = flag.Int("udp-port", 49101, "UDP discovery port (each node should use different port)")
	rpcPort     = flag.Int("rpc-port", 49201, "WebSocket RPC port (must be unique per node)")
	workspace   = flag.String("workspace", "", "Workspace directory (default: ./test-cluster/<node-name>)")
	testRPC     = flag.Bool("test-rpc", false, "Test RPC communication after discovery")
	verbose     = flag.Bool("verbose", false, "Enable verbose logging")
)

func main() {
	flag.Parse()

	if *nodeName == "" {
		fmt.Println("Error: --node flag is required")
		flag.Usage()
		os.Exit(1)
	}

	// Set workspace
	ws := *workspace
	if ws == "" {
		ws = filepath.Join(".", "test-cluster", *nodeName)
	}

	// Create workspace
	if err := os.MkdirAll(ws, 0755); err != nil {
		fmt.Printf("Error creating workspace: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("========================================\n")
	fmt.Printf("Cluster Test Tool - Node %s\n", *nodeName)
	fmt.Printf("========================================\n")
	fmt.Printf("Workspace:     %s\n", ws)
	fmt.Printf("UDP Port:      %d\n", *udpPort)
	fmt.Printf("RPC Port:      %d\n", *rpcPort)
	fmt.Printf("Test RPC:      %v\n", *testRPC)
	fmt.Printf("========================================\n\n")

	// Create cluster instance
	clusterInst, err := cluster.NewCluster(ws)
	if err != nil {
		fmt.Printf("Error creating cluster: %v\n", err)
		os.Exit(1)
	}

	// Set ports
	clusterInst.SetPorts(*udpPort, *rpcPort)

	// Set custom node name for display
	fmt.Printf("[%s] Initializing cluster instance...\n", *nodeName)

	// Start cluster
	fmt.Printf("[%s] Starting cluster...\n", *nodeName)
	if err := clusterInst.Start(); err != nil {
		fmt.Printf("[%s] Error starting cluster: %v\n", *nodeName, err)
		os.Exit(1)
	}

	fmt.Printf("[%s] Cluster started successfully!\n", *nodeName)
	fmt.Printf("[%s] Node ID: %s\n", *nodeName, clusterInst.GetNodeID())
	fmt.Printf("[%s] Address: %s\n\n", *nodeName, clusterInst.GetAddress())

	// Wait for discovery
	fmt.Printf("[%s] Waiting for peer discovery (30 seconds)...\n", *nodeName)
	discoveryDone := make(chan bool)
	go func() {
		ticker := time.NewTicker(2 * time.Second)
		defer ticker.Stop()

		for i := 0; i < 15; i++ {
			<-ticker.C
		 peers := clusterInst.GetOnlinePeers()
			if len(peers) > 0 {
				fmt.Printf("[%s] Discovery successful! Found %d peer(s)\n", *nodeName, len(peers))
			} else {
				if *verbose {
					fmt.Printf("[%s] Still waiting... (%d/15)\n", *nodeName, i+1)
				}
			}
		}
		discoveryDone <- true
	}()

	// Handle shutdown
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	// RPC test loop
	if *testRPC {
		go func() {
			time.Sleep(10 * time.Second) // Wait for discovery
			fmt.Printf("[%s] Starting RPC test...\n", *nodeName)

			for i := 0; i < 5; i++ {
				time.Sleep(5 * time.Second)

				peers := clusterInst.GetOnlinePeers()
				if len(peers) == 0 {
					fmt.Printf("[%s] No peers found for RPC test\n", *nodeName)
					continue
				}

				// Try to call ping on each peer
				for _, peerIface := range peers {
					if peer, ok := peerIface.(*cluster.Node); ok {
						if peer.IsOnline() {
							fmt.Printf("[%s] Testing RPC with peer %s...\n", *nodeName, peer.ID)
							resp, err := clusterInst.Call(peer.ID, "ping", map[string]interface{}{
								"message": fmt.Sprintf("Hello from %s", *nodeName),
							})

							if err != nil {
								fmt.Printf("[%s] RPC Error to %s: %v\n", *nodeName, peer.ID, err)
							} else {
								fmt.Printf("[%s] RPC Success to %s: %s\n", *nodeName, peer.ID, string(resp))
							}
						}
					}
				}
			}

			fmt.Printf("[%s] RPC test completed\n", *nodeName)
		}()
	}

	// Print status periodically
	go func() {
		ticker := time.NewTicker(10 * time.Second)
		defer ticker.Stop()

		for {
			select {
			case <-ticker.C:
				peers := clusterInst.GetOnlinePeers()
				fmt.Printf("[%s] Status: %d peer(s) online\n", *nodeName, len(peers))

				if *verbose && len(peers) > 0 {
					for _, peerIface := range peers {
						if peer, ok := peerIface.(*cluster.Node); ok {
							fmt.Printf("[%s]   - %s (%s) at %s\n", *nodeName, peer.Name, peer.ID, peer.Address)
						}
					}
				}
			case <-sigCh:
				return
			}
		}
	}()

	// Wait for signal
	fmt.Printf("[%s] Press Ctrl+C to shutdown...\n\n", *nodeName)
	<-sigCh

	fmt.Printf("\n[%s] Shutting down...\n", *nodeName)
	if err := clusterInst.Stop(); err != nil {
		fmt.Printf("[%s] Error stopping cluster: %v\n", *nodeName, err)
	} else {
		fmt.Printf("[%s] Shutdown complete\n", *nodeName)
	}
}

// PrintBanner prints a test banner
func PrintBanner() {
	banner := `
╔════════════════════════════════════════════════════════════╗
║          NemesisBot Cluster Test Tool                      ║
║                                                            ║
║  Test UDP discovery and WebSocket RPC communication        ║
╚════════════════════════════════════════════════════════════╝
`
	fmt.Print(banner)
}

func init() {
	PrintBanner()
	fmt.Println("Instructions:")
	fmt.Println("  Open multiple terminals and run:")
	fmt.Println("    Terminal 1: go run cmd/cluster-test/main.go --node=A --udp-port=49101 --rpc-port=49201")
	fmt.Println("    Terminal 2: go run cmd/cluster-test/main.go --node=B --udp-port=49102 --rpc-port=49202")
	fmt.Println("    Terminal 3: go run cmd/cluster-test/main.go --node=C --udp-port=49103 --rpc-port=49203")
	fmt.Println("  Note: Discovery module broadcasts to ports 49100-49110")
	fmt.Println("        so nodes on different ports can discover each other.")
	fmt.Println("")
	fmt.Println("  To test RPC communication, add --test-rpc flag")
	fmt.Println("")
}
