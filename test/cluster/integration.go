// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package main

import (
	"fmt"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
)

func main() {
	fmt.Println("===========================================")
	fmt.Println("NemesisBot Cluster Integration Test")
	fmt.Println("===========================================")
	fmt.Println()

	// Create temporary workspaces
	node1Workspace := "./test/cluster/node1"
	node2Workspace := "./test/cluster/node2"

	// Create workspaces
	if err := os.MkdirAll(node1Workspace+"/cluster", 0755); err != nil {
		fmt.Printf("Error creating workspace 1: %v\n", err)
		os.Exit(1)
	}
	if err := os.MkdirAll(node2Workspace+"/cluster", 0755); err != nil {
		fmt.Printf("Error creating workspace 2: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("[Step 1] Creating cluster instances...")
	fmt.Println()

	// Create node 1
	node1, err := cluster.NewCluster(node1Workspace)
	if err != nil {
		fmt.Printf("Error creating node 1: %v\n", err)
		os.Exit(1)
	}
	// Both nodes use SAME UDP port for discovery (so they can hear each other)
	// but DIFFERENT RPC ports (so they can be distinguished)
	node1.SetPorts(11949, 21949)

	// Add delay to ensure different timestamps (and different node IDs)
	time.Sleep(2 * time.Second)

	// Create node 2
	node2, err := cluster.NewCluster(node2Workspace)
	if err != nil {
		fmt.Printf("Error creating node 2: %v\n", err)
		os.Exit(1)
	}
	// Same UDP port as node 1 for discovery, different RPC port
	node2.SetPorts(11949, 21950)

	fmt.Printf("Node 1: ID=%s, UDP=%d, RPC=%d\n", node1.GetNodeID(), 11949, 21949)
	fmt.Printf("Node 2: ID=%s, UDP=%d, RPC=%d\n", node2.GetNodeID(), 11949, 21950)
	fmt.Println()

	fmt.Println("[Step 2] Starting nodes...")
	fmt.Println()

	// Start nodes
	if err := node1.Start(); err != nil {
		fmt.Printf("Error starting node 1: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("✓ Node 1 started (ID: %s)\n", node1.GetNodeID())

	if err := node2.Start(); err != nil {
		fmt.Printf("Error starting node 2: %v\n", err)
		node1.Stop()
		os.Exit(1)
	}
	fmt.Printf("✓ Node 2 started (ID: %s)\n", node2.GetNodeID())
	fmt.Println()

	// Wait for discovery
	fmt.Println("[Step 3] Waiting for peer discovery (15 seconds)...")
	fmt.Println()

	time.Sleep(15 * time.Second)

	// Check peers
	fmt.Println("[Step 4] Checking peer discovery...")
	fmt.Println()

	registry1 := node1.GetRegistry()
	if reg1, ok := registry1.(*cluster.Registry); ok {
		peers1 := reg1.GetOnline()
		fmt.Printf("Node 1 sees %d online peer(s):\n", len(peers1))
		for _, peer := range peers1 {
			fmt.Printf("  - %s (%s)\n", peer.ID, peer.Address)
		}
	}

	registry2 := node2.GetRegistry()
	if reg2, ok := registry2.(*cluster.Registry); ok {
		peers2 := reg2.GetOnline()
		fmt.Printf("Node 2 sees %d online peer(s):\n", len(peers2))
		for _, peer := range peers2 {
			fmt.Printf("  - %s (%s)\n", peer.ID, peer.Address)
		}
	}
	fmt.Println()

	// Test RPC calls
	fmt.Println("[Step 5] Testing RPC communication...")
	fmt.Println()

	// Use the registry that has peers (could be either node)
	var targetPeer *cluster.Node
	var callerNode *cluster.Cluster

	if len(registry1.(*cluster.Registry).GetOnline()) > 0 {
		peers := registry1.(*cluster.Registry).GetOnline()
		targetPeer = peers[0]
		callerNode = node1
		fmt.Printf("Using Node 1 to call Node 2...\n")
	} else if len(registry2.(*cluster.Registry).GetOnline()) > 0 {
		peers := registry2.(*cluster.Registry).GetOnline()
		targetPeer = peers[0]
		callerNode = node2
		fmt.Printf("Using Node 2 to call Node 1...\n")
	} else {
		fmt.Println("No peers discovered for RPC testing")
		goto skip_rpc
	}

	if targetPeer != nil && callerNode != nil {
		fmt.Printf("Caller: %s\n", callerNode.GetNodeID())
		fmt.Printf("Target: %s (%s)\n", targetPeer.ID, targetPeer.Address)
		fmt.Println()

		// Test 1: Ping RPC
		fmt.Printf("Test 1: Calling 'ping' RPC...\n")
		response, err := callerNode.Call(targetPeer.ID, "ping", nil)
		if err != nil {
			fmt.Printf("  ✗ RPC call failed: %v\n", err)
		} else {
			fmt.Printf("  ✓ RPC call succeeded: %s\n", string(response))
		}
		fmt.Println()

		// Test 2: Get capabilities
		fmt.Printf("Test 2: Calling 'get_capabilities' RPC...\n")
		response, err = callerNode.Call(targetPeer.ID, "get_capabilities", nil)
		if err != nil {
			fmt.Printf("  ✗ RPC call failed: %v\n", err)
		} else {
			fmt.Printf("  ✓ RPC call succeeded: %s\n", string(response))
		}
		fmt.Println()

		// Test 3: Get info
		fmt.Printf("Test 3: Calling 'get_info' RPC...\n")
		response, err = callerNode.Call(targetPeer.ID, "get_info", nil)
		if err != nil {
			fmt.Printf("  ✗ RPC call failed: %v\n", err)
		} else {
			fmt.Printf("  ✓ RPC call succeeded: %s\n", string(response))
		}
	}

skip_rpc:

	fmt.Println()

	// Wait for more RPC tests
	fmt.Println("[Step 6] Running for 30 seconds to monitor communication...")
	fmt.Println("(Press Ctrl+C to stop early)")
	fmt.Println()

	// Setup signal handling
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	// Wait for signal or timeout
	select {
	case <-sigCh:
		fmt.Println("\nReceived interrupt signal...")
	case <-time.After(30 * time.Second):
		fmt.Println("Test duration completed.")
	}

	// Cleanup
	fmt.Println()
	fmt.Println("[Step 7] Stopping nodes...")
	fmt.Println()

	var wg sync.WaitGroup
	wg.Add(2)

	go func() {
		defer wg.Done()
		if err := node1.Stop(); err != nil {
			fmt.Printf("Error stopping node 1: %v\n", err)
		} else {
			fmt.Println("✓ Node 1 stopped")
		}
	}()

	go func() {
		defer wg.Done()
		if err := node2.Stop(); err != nil {
			fmt.Printf("Error stopping node 2: %v\n", err)
		} else {
			fmt.Println("✓ Node 2 stopped")
		}
	}()

	wg.Wait()

	fmt.Println()
	fmt.Println("===========================================")
	fmt.Println("Integration Test Complete")
	fmt.Println("===========================================")
}
