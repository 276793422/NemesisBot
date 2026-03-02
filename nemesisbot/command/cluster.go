// Package command implements CLI commands for NemesisBot
package command

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/276793422/NemesisBot/module/cluster"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/path"
)

// CmdCluster manages cluster configuration and status
func CmdCluster() {
	if len(os.Args) < 3 {
		ClusterHelp()
		return
	}

	subcommand := os.Args[2]

	switch subcommand {
	case "status":
		cmdClusterStatus()
	case "config":
		cmdClusterConfig()
	case "enable":
		cmdClusterEnable()
	case "disable":
		cmdClusterDisable()
	case "start":
		cmdClusterStart()
	case "stop":
		cmdClusterStop()
	default:
		fmt.Printf("Unknown cluster command: %s\n", subcommand)
		ClusterHelp()
	}
}

// ClusterHelp prints cluster command help
func ClusterHelp() {
	fmt.Println("\nCluster commands:")
	fmt.Println("  status                 Show cluster status and configuration")
	fmt.Println("  config                 Show or modify cluster configuration")
	fmt.Println("  enable                 Enable cluster module")
	fmt.Println("  disable                Disable cluster module")
	fmt.Println("  start                  Start cluster (alias for 'enable')")
	fmt.Println("  stop                   Stop cluster (alias for 'disable')")
	fmt.Println()
	fmt.Println("Config options:")
	fmt.Println("  nemesisbot cluster config")
	fmt.Println("  nemesisbot cluster config --udp-port 49100")
	fmt.Println("  nemesisbot cluster config --rpc-port 49200")
	fmt.Println("  nemesisbot cluster config --broadcast-interval 30")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot cluster status")
	fmt.Println("  nemesisbot cluster enable")
	fmt.Println("  nemesisbot cluster disable")
	fmt.Println("  nemesisbot cluster config --udp-port 49100 --rpc-port 49200")
	fmt.Println()
}

// cmdClusterStatus shows cluster status
func cmdClusterStatus() {
	// Get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Printf("Error resolving home directory: %v\n", err)
		os.Exit(1)
	}

	// Resolve workspace path
	configPath := filepath.Join(homeDir, "config.json")
	cfg, err := config.LoadConfig(configPath)
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Get workspace
	workspace := cfg.Agents.Defaults.Workspace
	if strings.HasPrefix(workspace, "~/") {
		homeDir, _ := os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Load cluster config
	clusterCfg, err := cluster.LoadAppConfig(workspace)
	if err != nil {
		fmt.Printf("Error loading cluster config: %v\n", err)
		os.Exit(1)
	}

	// Show status
	fmt.Println("╔════════════════════════════════════════════════════════════╗")
	fmt.Println("║                    Cluster Status                             ║")
	fmt.Println("╚════════════════════════════════════════════════════════════╝")
	fmt.Println()

	if clusterCfg.Enabled {
		fmt.Println("Status:         🟢 Enabled")
	} else {
		fmt.Println("Status:         ⚪ Disabled")
	}
	fmt.Println()

	fmt.Println("Configuration:")
	fmt.Printf("  UDP Port:             %d\n", clusterCfg.Port)
	fmt.Printf("  RPC Port:             %d\n", clusterCfg.RPCPort)
	fmt.Printf("  Broadcast Interval:  %d seconds\n", clusterCfg.BroadcastInterval)
	fmt.Println()

	// Check peers.toml status
	peersPath := filepath.Join(workspace, "cluster", "peers.toml")
	if _, err := os.Stat(peersPath); err == nil {
		fmt.Println("Runtime State:  ✅ peers.toml exists")
	} else {
		fmt.Println("Runtime State:  ⚪ peers.toml not found (not started yet)")
	}
	fmt.Println()

	fmt.Println("Usage:")
	fmt.Println("  Enable cluster:  nemesisbot cluster enable")
	fmt.Println("  Disable cluster: nemesisbot cluster disable")
	fmt.Println("  Modify config:   nemesisbot cluster config [options]")
}

// cmdClusterConfig shows or modifies cluster configuration
func cmdClusterConfig() {
	// Get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Printf("Error resolving home directory: %v\n", err)
		os.Exit(1)
	}

	// Resolve workspace path
	configPath := filepath.Join(homeDir, "config.json")
	cfg, err := config.LoadConfig(configPath)
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Get workspace
	workspace := cfg.Agents.Defaults.Workspace
	if strings.HasPrefix(workspace, "~/") {
		homeDir, _ := os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Load cluster config
	clusterCfg, err := cluster.LoadAppConfig(workspace)
	if err != nil {
		fmt.Printf("Error loading cluster config: %v\n", err)
		os.Exit(1)
	}

	// Parse command line flags
	type flags struct {
		UDPPort           int    `json:"udp_port"`
		RPCPort           int    `json:"rpc_port"`
		BroadcastInterval int    `json:"broadcast_interval"`
		ShowHelp          bool   `json:"-"`
	}

	f := &flags{}
	args := os.Args[3:]

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--udp-port", "-p":
			if i+1 >= len(args) {
				fmt.Println("Error: --udp-port requires a value")
				os.Exit(1)
			}
			i++
			var port int
			if _, err := fmt.Sscanf(args[i], "%d", &port); err != nil {
				fmt.Printf("Error: invalid port number: %s\n", args[i])
				os.Exit(1)
			}
			f.UDPPort = port
		case "--rpc-port", "-r":
			if i+1 >= len(args) {
				fmt.Println("Error: --rpc-port requires a value")
				os.Exit(1)
			}
			i++
			var port int
			if _, err := fmt.Sscanf(args[i], "%d", &port); err != nil {
				fmt.Printf("Error: invalid port number: %s\n", args[i])
				os.Exit(1)
			}
			f.RPCPort = port
		case "--broadcast-interval", "-b":
			if i+1 >= len(args) {
				fmt.Println("Error: --broadcast-interval requires a value")
				os.Exit(1)
			}
			i++
			var interval int
			if _, err := fmt.Sscanf(args[i], "%d", &interval); err != nil {
				fmt.Printf("Error: invalid interval: %s\n", args[i])
				os.Exit(1)
			}
			f.BroadcastInterval = interval
		case "--help", "-h":
			f.ShowHelp = true
		default:
			fmt.Printf("Error: unknown option %s\n", args[i])
			fmt.Println("Run 'nemesisbot cluster config --help' for usage")
			os.Exit(1)
		}
	}

	if f.ShowHelp {
		fmt.Println("\nCluster Configuration:")
		fmt.Println()
		fmt.Println("Options:")
		fmt.Println("  --udp-port, -p <port>           UDP port for discovery (default: 49100)")
		fmt.Println("  --rpc-port, -r <port>           WebSocket RPC port (default: 49200)")
		fmt.Println("  --broadcast-interval, -b <sec>  Broadcast interval in seconds (default: 30)")
		fmt.Println("  --help, -h                     Show this help message")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  nemesisbot cluster config")
		fmt.Println("  nemesisbot cluster config --udp-port 49100")
		fmt.Println("  nemesisbot cluster config --rpc-port 49200 --broadcast-interval 60")
		fmt.Println("  nemesisbot cluster config -p 49101 -r 49201 -b 45")
		fmt.Println()
		return
	}

	// If no flags provided, just show current config
	if f.UDPPort == 0 && f.RPCPort == 0 && f.BroadcastInterval == 0 {
		fmt.Println("\nCurrent Cluster Configuration:")
		fmt.Println()
		fmt.Printf("  Enabled:             %v\n", clusterCfg.Enabled)
		fmt.Printf("  UDP Port:             %d\n", clusterCfg.Port)
		fmt.Printf("  RPC Port:             %d\n", clusterCfg.RPCPort)
		fmt.Printf("  Broadcast Interval:  %d seconds\n", clusterCfg.BroadcastInterval)
		fmt.Println()
		fmt.Println("To modify configuration, use:")
		fmt.Println("  nemesisbot cluster config [options]")
		return
	}

	// Apply changes
	modified := false
	if f.UDPPort > 0 {
		clusterCfg.Port = f.UDPPort
		modified = true
	}
	if f.RPCPort > 0 {
		clusterCfg.RPCPort = f.RPCPort
		modified = true
	}
	if f.BroadcastInterval > 0 {
		clusterCfg.BroadcastInterval = f.BroadcastInterval
		modified = true
	}

	if !modified {
		fmt.Println("No changes to apply.")
		return
	}

	// Save config
	if err := cluster.SaveAppConfig(workspace, clusterCfg); err != nil {
		fmt.Printf("Error saving cluster config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✓ Cluster configuration updated")
	fmt.Println()
	fmt.Println("New configuration:")
	fmt.Printf("  UDP Port:             %d\n", clusterCfg.Port)
	fmt.Printf("  RPC Port:             %d\n", clusterCfg.RPCPort)
	fmt.Printf("  Broadcast Interval:  %d seconds\n", clusterCfg.BroadcastInterval)
	fmt.Println()
	fmt.Println("Note: Changes will take effect when you restart the gateway")
}

// cmdClusterEnable enables the cluster module
func cmdClusterEnable() {
	// Get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Printf("Error resolving home directory: %v\n", err)
		os.Exit(1)
	}

	// Resolve workspace path
	configPath := filepath.Join(homeDir, "config.json")
	cfg, err := config.LoadConfig(configPath)
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Get workspace
	workspace := cfg.Agents.Defaults.Workspace
	if strings.HasPrefix(workspace, "~/") {
		homeDir, _ := os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Load cluster config
	clusterCfg, err := cluster.LoadAppConfig(workspace)
	if err != nil {
		fmt.Printf("Error loading cluster config: %v\n", err)
		os.Exit(1)
	}

	if clusterCfg.Enabled {
		fmt.Println("Cluster is already enabled")
		return
	}

	// Enable cluster
	clusterCfg.Enabled = true

	// Save config
	if err := cluster.SaveAppConfig(workspace, clusterCfg); err != nil {
		fmt.Printf("Error saving cluster config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✓ Cluster module enabled")
	fmt.Println()
	fmt.Println("Configuration:")
	fmt.Printf("  UDP Port:             %d\n", clusterCfg.Port)
	fmt.Printf("  RPC Port:             %d\n", clusterCfg.RPCPort)
	fmt.Printf("  Broadcast Interval:  %d seconds\n", clusterCfg.BroadcastInterval)
	fmt.Println()
	fmt.Println("Next steps:")
	fmt.Println("  1. Restart the gateway to start cluster:")
	fmt.Println("     nemesisbot gateway")
}

// cmdClusterDisable disables the cluster module
func cmdClusterDisable() {
	// Get workspace path
	homeDir, err := path.ResolveHomeDir()
	if err != nil {
		fmt.Printf("Error resolving home directory: %v\n", err)
		os.Exit(1)
	}

	// Resolve workspace path
	configPath := filepath.Join(homeDir, "config.json")
	cfg, err := config.LoadConfig(configPath)
	if err != nil {
		fmt.Printf("Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Get workspace
	workspace := cfg.Agents.Defaults.Workspace
	if strings.HasPrefix(workspace, "~/") {
		homeDir, _ := os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Load cluster config
	clusterCfg, err := cluster.LoadAppConfig(workspace)
	if err != nil {
		fmt.Printf("Error loading cluster config: %v\n", err)
		os.Exit(1)
	}

	if !clusterCfg.Enabled {
		fmt.Println("Cluster is already disabled")
		return
	}

	// Disable cluster
	clusterCfg.Enabled = false

	// Save config
	if err := cluster.SaveAppConfig(workspace, clusterCfg); err != nil {
		fmt.Printf("Error saving cluster config: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✓ Cluster module disabled")
	fmt.Println()
	fmt.Println("Note: Restart the gateway to apply changes")
}

// cmdClusterStart is an alias for enable
func cmdClusterStart() {
	cmdClusterEnable()
}

// cmdClusterStop is an alias for disable
func cmdClusterStop() {
	cmdClusterDisable()
}
