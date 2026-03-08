// Package command implements CLI commands for NemesisBot
package command

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

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
	case "info":
		cmdClusterInfo()
	case "peers":
		cmdClusterPeers()
	case "init":
		cmdClusterInit()
	case "enable":
		cmdClusterEnable()
	case "disable":
		cmdClusterDisable()
	case "start":
		cmdClusterStart()
	case "stop":
		cmdClusterStop()
	case "reset":
		cmdClusterReset()
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
	fmt.Println("  info                   Show or modify current node information")
	fmt.Println("  peers                  Manage configured peer nodes")
	fmt.Println("  init                   Initialize cluster configuration")
	fmt.Println("  reset                  Reset cluster runtime state")
	fmt.Println("  enable                 Enable cluster module")
	fmt.Println("  disable                Disable cluster module")
	fmt.Println("  start                  Start cluster (alias for 'enable')")
	fmt.Println("  stop                   Stop cluster (alias for 'disable')")
	fmt.Println()
	fmt.Println("Configuration:")
	fmt.Println("  nemesisbot cluster config [--udp-port 11949] [--rpc-port 21949]")
	fmt.Println()
	fmt.Println("Node information:")
	fmt.Println("  nemesisbot cluster info")
	fmt.Println("  nemesisbot cluster info --name \"My Bot\"")
	fmt.Println("  nemesisbot cluster info --role manager --address 192.168.1.100")
	fmt.Println()
	fmt.Println("Peer management:")
	fmt.Println("  nemesisbot cluster peers")
	fmt.Println("  nemesisbot cluster peers add --id node-xxx --address 192.168.1.101")
	fmt.Println("  nemesisbot cluster peers remove --id node-xxx")
	fmt.Println("  nemesisbot cluster peers enable --id node-xxx")
	fmt.Println("  nemesisbot cluster peers disable --id node-xxx")
	fmt.Println()
	fmt.Println("Initialization:")
	fmt.Println("  nemesisbot cluster init --name \"My Bot\" --role manager")
	fmt.Println()
	fmt.Println("Reset:")
	fmt.Println("  nemesisbot cluster reset [--hard]    # Soft: clear discovered, Hard: reset all")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot cluster status")
	fmt.Println("  nemesisbot cluster enable")
	fmt.Println("  nemesisbot cluster info --name \"Production Bot 1\"")
	fmt.Println("  nemesisbot cluster init --name \"My Bot\" --role worker")
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
		UDPPort           int  `json:"udp_port"`
		RPCPort           int  `json:"rpc_port"`
		BroadcastInterval int  `json:"broadcast_interval"`
		ShowHelp          bool `json:"-"`
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
		fmt.Println("  --udp-port, -p <port>           UDP port for discovery (default: 11949)")
		fmt.Println("  --rpc-port, -r <port>           WebSocket RPC port (default: 21949)")
		fmt.Println("  --broadcast-interval, -b <sec>  Broadcast interval in seconds (default: 30)")
		fmt.Println("  --help, -h                     Show this help message")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  nemesisbot cluster config")
		fmt.Println("  nemesisbot cluster config --udp-port 11949")
		fmt.Println("  nemesisbot cluster config --rpc-port 21949 --broadcast-interval 60")
		fmt.Println("  nemesisbot cluster config -p 11950 -r 21950 -b 45")
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

// cmdClusterInfo shows or modifies current node information
func cmdClusterInfo() {
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

	// Load static config (peers.toml)
	peersPath := filepath.Join(workspace, "cluster", "peers.toml")
	staticConfig, err := cluster.LoadStaticConfig(peersPath)
	if err != nil {
		fmt.Printf("Error loading peers.toml: %v\n", err)
		fmt.Println("Hint: Run 'nemesisbot cluster init' to initialize cluster configuration")
		os.Exit(1)
	}

	// Parse command line flags
	type flags struct {
		Name         string
		Role         string
		Category     string
		Tags         string
		Address      string
		Capabilities string
		ShowHelp     bool
	}

	f := &flags{}
	args := os.Args[3:]
	modified := false

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--name", "-n":
			if i+1 >= len(args) {
				fmt.Println("Error: --name requires a value")
				os.Exit(1)
			}
			i++
			f.Name = args[i]
			modified = true
		case "--role", "-r":
			if i+1 >= len(args) {
				fmt.Println("Error: --role requires a value")
				os.Exit(1)
			}
			i++
			f.Role = args[i]
			modified = true
		case "--category", "-c":
			if i+1 >= len(args) {
				fmt.Println("Error: --category requires a value")
				os.Exit(1)
			}
			i++
			f.Category = args[i]
			modified = true
		case "--tags", "-t":
			if i+1 >= len(args) {
				fmt.Println("Error: --tags requires a value")
				os.Exit(1)
			}
			i++
			f.Tags = args[i]
			modified = true
		case "--address", "-a":
			if i+1 >= len(args) {
				fmt.Println("Error: --address requires a value")
				os.Exit(1)
			}
			i++
			f.Address = args[i]
			modified = true
		case "--capabilities":
			if i+1 >= len(args) {
				fmt.Println("Error: --capabilities requires a value")
				os.Exit(1)
			}
			i++
			f.Capabilities = args[i]
			modified = true
		case "--help", "-h":
			f.ShowHelp = true
		default:
			fmt.Printf("Error: unknown option %s\n", args[i])
			fmt.Println("Run 'nemesisbot cluster info --help' for usage")
			os.Exit(1)
		}
	}

	if f.ShowHelp {
		fmt.Println("\nNode Information:")
		fmt.Println()
		fmt.Println("Options:")
		fmt.Println("  --name, -n <name>            Set node name")
		fmt.Println("  --role, -r <role>            Set cluster role")
		fmt.Println("                                manager, coordinator, worker, observer, standby")
		fmt.Println("  --category, -c <category>    Set business category")
		fmt.Println("                                design, development, testing, ops, deployment, analysis, general")
		fmt.Println("  --tags, -t <tags>            Set custom tags (comma-separated)")
		fmt.Println("  --address, -a <address>      Set node address (IP:Port)")
		fmt.Println("  --capabilities <caps>        Set capabilities (comma-separated)")
		fmt.Println("  --help, -h                   Show this help message")
		fmt.Println()
		fmt.Println("Cluster Roles:")
		fmt.Println("  manager       - Cluster manager with high privileges")
		fmt.Println("  coordinator   - Task coordinator with medium-high privileges")
		fmt.Println("  worker        - Task executor with medium privileges")
		fmt.Println("  observer      - Read-only observer with low privileges")
		fmt.Println("  standby       - Standby mode, not participating in cluster")
		fmt.Println()
		fmt.Println("Business Categories:")
		fmt.Println("  design        - Design-related tasks (UI/UX, visual, etc.)")
		fmt.Println("  development   - Development tasks (frontend, backend, fullstack)")
		fmt.Println("  testing       - Testing tasks (QA, automation, performance)")
		fmt.Println("  ops           - Operations tasks (monitoring, logging, alerting)")
		fmt.Println("  deployment    - Deployment tasks (CI/CD, release management)")
		fmt.Println("  analysis      - Analysis tasks (data analysis, log analysis)")
		fmt.Println("  general       - General purpose, uncategorized")
		fmt.Println()
		fmt.Println("Examples:")
		fmt.Println("  nemesisbot cluster info")
		fmt.Println("  nemesisbot cluster info --name \"My Bot\"")
		fmt.Println("  nemesisbot cluster info --role manager --category design")
		fmt.Println("  nemesisbot cluster info --category development --tags \"production,senior\"")
		fmt.Println("  nemesisbot cluster info --address 192.168.1.100:49200")
		fmt.Println()
		return
	}

	// If no flags provided, just show current info
	if !modified {
		fmt.Println("\nCurrent Node Information:")
		fmt.Println()
		fmt.Printf("  ID:           %s\n", staticConfig.Node.ID)
		fmt.Printf("  Name:         %s\n", staticConfig.Node.Name)
		fmt.Printf("  Address:      %s\n", staticConfig.Node.Address)
		fmt.Printf("  Role:         %s\n", staticConfig.Node.Role)
		fmt.Printf("  Category:     %s\n", staticConfig.Node.Category)
		if len(staticConfig.Node.Tags) > 0 {
			fmt.Printf("  Tags:         %s\n", strings.Join(staticConfig.Node.Tags, ", "))
		} else {
			fmt.Printf("  Tags:         (none)\n")
		}
		if len(staticConfig.Node.Capabilities) > 0 {
			fmt.Printf("  Capabilities: %s\n", strings.Join(staticConfig.Node.Capabilities, ", "))
		} else {
			fmt.Printf("  Capabilities: (none)\n")
		}
		fmt.Println()
		fmt.Println("To modify node information, use:")
		fmt.Println("  nemesisbot cluster info [options]")
		return
	}

	// Apply changes
	if f.Name != "" {
		staticConfig.Node.Name = f.Name
	}
	if f.Role != "" {
		staticConfig.Node.Role = f.Role
	}
	if f.Category != "" {
		staticConfig.Node.Category = f.Category
	}
	if f.Tags != "" {
		tags := strings.Split(f.Tags, ",")
		cleanTags := make([]string, 0, len(tags))
		for _, tag := range tags {
			tag = strings.TrimSpace(tag)
			if tag != "" {
				cleanTags = append(cleanTags, tag)
			}
		}
		staticConfig.Node.Tags = cleanTags
	}
	if f.Address != "" {
		staticConfig.Node.Address = f.Address
	}
	if f.Capabilities != "" {
		caps := strings.Split(f.Capabilities, ",")
		cleanCaps := make([]string, 0, len(caps))
		for _, cap := range caps {
			cap = strings.TrimSpace(cap)
			if cap != "" {
				cleanCaps = append(cleanCaps, cap)
			}
		}
		staticConfig.Node.Capabilities = cleanCaps
	}

	// Save config
	if err := cluster.SaveStaticConfig(peersPath, staticConfig); err != nil {
		fmt.Printf("Error saving peers.toml: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✓ Node information updated")
	fmt.Println()
	fmt.Println("New configuration:")
	fmt.Printf("  ID:           %s\n", staticConfig.Node.ID)
	fmt.Printf("  Name:         %s\n", staticConfig.Node.Name)
	fmt.Printf("  Address:      %s\n", staticConfig.Node.Address)
	fmt.Printf("  Role:         %s\n", staticConfig.Node.Role)
	fmt.Printf("  Category:     %s\n", staticConfig.Node.Category)
	if len(staticConfig.Node.Tags) > 0 {
		fmt.Printf("  Tags:         %s\n", strings.Join(staticConfig.Node.Tags, ", "))
	} else {
		fmt.Printf("  Tags:         (none)\n")
	}
	if len(staticConfig.Node.Capabilities) > 0 {
		fmt.Printf("  Capabilities: %s\n", strings.Join(staticConfig.Node.Capabilities, ", "))
	} else {
		fmt.Printf("  Capabilities: (none)\n")
	}
	fmt.Println()
	fmt.Println("Note: Restart the gateway to apply changes")
}

// cmdClusterPeers manages configured peer nodes
func cmdClusterPeers() {
	if len(os.Args) < 4 {
		cmdClusterPeersList()
		return
	}

	action := os.Args[3]

	switch action {
	case "list", "ls":
		cmdClusterPeersList()
	case "add":
		cmdClusterPeersAdd()
	case "remove", "rm":
		cmdClusterPeersRemove()
	case "enable":
		cmdClusterPeersEnable()
	case "disable":
		cmdClusterPeersDisable()
	case "--help", "-h":
		cmdClusterPeersHelp()
	default:
		fmt.Printf("Unknown peers action: %s\n", action)
		cmdClusterPeersHelp()
	}
}

// cmdClusterPeersHelp shows peers command help
func cmdClusterPeersHelp() {
	fmt.Println("\nPeer Management:")
	fmt.Println()
	fmt.Println("Actions:")
	fmt.Println("  list, ls                    List configured peers")
	fmt.Println("  add                         Add a new peer")
	fmt.Println("  remove, rm                  Remove a peer")
	fmt.Println("  enable                      Enable a peer")
	fmt.Println("  disable                     Disable a peer")
	fmt.Println()
	fmt.Println("Add options:")
	fmt.Println("  --id <id>                   Peer ID (required)")
	fmt.Println("  --name <name>               Peer name")
	fmt.Println("  --address <address>         Peer address (IP:Port)")
	fmt.Println("  --role <role>               Cluster role (manager, coordinator, worker, observer, standby)")
	fmt.Println("  --category <category>       Business category (design, development, testing, etc.)")
	fmt.Println("  --tags <tags>               Custom tags (comma-separated)")
	fmt.Println("  --capabilities <caps>       Capabilities (comma-separated)")
	fmt.Println("  --priority <n>              Priority (default: 0)")
	fmt.Println()
	fmt.Println("Remove/Enable/Disable options:")
	fmt.Println("  --id <id>                   Peer ID (required)")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  nemesisbot cluster peers")
	fmt.Println("  nemesisbot cluster peers add --id node-xxx --name \"Peer 1\" --address 192.168.1.101:49200")
	fmt.Println("  nemesisbot cluster peers add --id node-xxx --category design --tags \"production,senior\"")
	fmt.Println("  nemesisbot cluster peers remove --id node-xxx")
	fmt.Println("  nemesisbot cluster peers enable --id node-xxx")
	fmt.Println()
}

// cmdClusterPeersList lists configured peers
func cmdClusterPeersList() {
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

	// Load static config
	peersPath := filepath.Join(workspace, "cluster", "peers.toml")
	staticConfig, err := cluster.LoadStaticConfig(peersPath)
	if err != nil {
		fmt.Printf("Error loading peers.toml: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("\nConfigured Peers:")
	fmt.Println()

	if len(staticConfig.Peers) == 0 {
		fmt.Println("  No peers configured")
		fmt.Println()
		fmt.Println("To add a peer, use:")
		fmt.Println("  nemesisbot cluster peers add --id <id> --address <address>")
		return
	}

	for i, peer := range staticConfig.Peers {
		fmt.Printf("  [%d] %s\n", i+1, peer.ID)
		fmt.Printf("      Name:         %s\n", peer.Name)
		fmt.Printf("      Address:      %s\n", peer.Address)
		fmt.Printf("      Role:         %s\n", peer.Role)
		fmt.Printf("      Category:     %s\n", peer.Category)
		if len(peer.Tags) > 0 {
			fmt.Printf("      Tags:         %s\n", strings.Join(peer.Tags, ", "))
		}
		if len(peer.Capabilities) > 0 {
			fmt.Printf("      Capabilities: %s\n", strings.Join(peer.Capabilities, ", "))
		}
		fmt.Printf("      Priority:     %d\n", peer.Priority)
		fmt.Printf("      Status:       %s\n", map[bool]string{true: "✓ Enabled", false: "✗ Disabled"}[peer.Enabled])
		if peer.Status.State != "" {
			fmt.Printf("      State:        %s\n", peer.Status.State)
		}
		fmt.Println()
	}

	fmt.Printf("Total: %d peer(s)\n", len(staticConfig.Peers))
	fmt.Println()
}

// cmdClusterPeersAdd adds a new peer
func cmdClusterPeersAdd() {
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

	// Load static config
	peersPath := filepath.Join(workspace, "cluster", "peers.toml")
	staticConfig, err := cluster.LoadStaticConfig(peersPath)
	if err != nil {
		fmt.Printf("Error loading peers.toml: %v\n", err)
		os.Exit(1)
	}

	// Parse flags
	type flags struct {
		ID           string
		Name         string
		Address      string
		Role         string
		Category     string
		Tags         string
		Capabilities string
		Priority     int
	}

	f := &flags{}
	args := os.Args[4:]

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--id":
			if i+1 >= len(args) {
				fmt.Println("Error: --id requires a value")
				os.Exit(1)
			}
			i++
			f.ID = args[i]
		case "--name", "-n":
			if i+1 >= len(args) {
				fmt.Println("Error: --name requires a value")
				os.Exit(1)
			}
			i++
			f.Name = args[i]
		case "--address", "-a":
			if i+1 >= len(args) {
				fmt.Println("Error: --address requires a value")
				os.Exit(1)
			}
			i++
			f.Address = args[i]
		case "--role", "-r":
			if i+1 >= len(args) {
				fmt.Println("Error: --role requires a value")
				os.Exit(1)
			}
			i++
			f.Role = args[i]
		case "--category", "-c":
			if i+1 >= len(args) {
				fmt.Println("Error: --category requires a value")
				os.Exit(1)
			}
			i++
			f.Category = args[i]
		case "--tags", "-t":
			if i+1 >= len(args) {
				fmt.Println("Error: --tags requires a value")
				os.Exit(1)
			}
			i++
			f.Tags = args[i]
		case "--capabilities":
			if i+1 >= len(args) {
				fmt.Println("Error: --capabilities requires a value")
				os.Exit(1)
			}
			i++
			f.Capabilities = args[i]
		case "--priority", "-p":
			if i+1 >= len(args) {
				fmt.Println("Error: --priority requires a value")
				os.Exit(1)
			}
			i++
			fmt.Sscanf(args[i], "%d", &f.Priority)
		default:
			fmt.Printf("Error: unknown option %s\n", args[i])
			fmt.Println("Run 'nemesisbot cluster peers add --help' for usage")
			os.Exit(1)
		}
	}

	if f.ID == "" {
		fmt.Println("Error: --id is required")
		fmt.Println("Usage: nemesisbot cluster peers add --id <id> [options]")
		os.Exit(1)
	}

	// Check if peer already exists
	for _, peer := range staticConfig.Peers {
		if peer.ID == f.ID {
			fmt.Printf("Error: peer with id '%s' already exists\n", f.ID)
			os.Exit(1)
		}
	}

	// Create new peer
	newPeer := cluster.PeerConfig{
		ID:       f.ID,
		Name:     f.Name,
		Address:  f.Address,
		Role:     f.Role,
		Enabled:  true,
		Priority: f.Priority,
	}

	if newPeer.Name == "" {
		newPeer.Name = "Peer " + f.ID
	}
	if newPeer.Role == "" {
		newPeer.Role = "worker"
	}
	if newPeer.Category == "" {
		newPeer.Category = "general"
	}
	if f.Category != "" {
		newPeer.Category = f.Category
	}
	if f.Tags != "" {
		tags := strings.Split(f.Tags, ",")
		for _, tag := range tags {
			tag = strings.TrimSpace(tag)
			if tag != "" {
				newPeer.Tags = append(newPeer.Tags, tag)
			}
		}
	}
	if f.Capabilities != "" {
		caps := strings.Split(f.Capabilities, ",")
		for _, cap := range caps {
			cap = strings.TrimSpace(cap)
			if cap != "" {
				newPeer.Capabilities = append(newPeer.Capabilities, cap)
			}
		}
	}

	// Add peer
	staticConfig.Peers = append(staticConfig.Peers, newPeer)

	// Save config
	if err := cluster.SaveStaticConfig(peersPath, staticConfig); err != nil {
		fmt.Printf("Error saving peers.toml: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Peer '%s' added\n", f.ID)
	fmt.Printf("  Name:         %s\n", newPeer.Name)
	fmt.Printf("  Address:      %s\n", newPeer.Address)
	fmt.Printf("  Role:         %s\n", newPeer.Role)
	fmt.Printf("  Category:     %s\n", newPeer.Category)
	if len(newPeer.Tags) > 0 {
		fmt.Printf("  Tags:         %s\n", strings.Join(newPeer.Tags, ", "))
	}
	fmt.Printf("  Enabled:      %v\n", newPeer.Enabled)
	fmt.Println()
	fmt.Println("Note: Restart the gateway to apply changes")
}

// cmdClusterPeersRemove removes a peer
func cmdClusterPeersRemove() {
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

	// Load static config
	peersPath := filepath.Join(workspace, "cluster", "peers.toml")
	staticConfig, err := cluster.LoadStaticConfig(peersPath)
	if err != nil {
		fmt.Printf("Error loading peers.toml: %v\n", err)
		os.Exit(1)
	}

	// Parse flags
	peerID := ""
	args := os.Args[4:]

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--id":
			if i+1 >= len(args) {
				fmt.Println("Error: --id requires a value")
				os.Exit(1)
			}
			i++
			peerID = args[i]
		default:
			fmt.Printf("Error: unknown option %s\n", args[i])
			os.Exit(1)
		}
	}

	if peerID == "" {
		fmt.Println("Error: --id is required")
		fmt.Println("Usage: nemesisbot cluster peers remove --id <id>")
		os.Exit(1)
	}

	// Find and remove peer
	found := false
	newPeers := make([]cluster.PeerConfig, 0, len(staticConfig.Peers))
	for _, peer := range staticConfig.Peers {
		if peer.ID == peerID {
			found = true
		} else {
			newPeers = append(newPeers, peer)
		}
	}

	if !found {
		fmt.Printf("Error: peer with id '%s' not found\n", peerID)
		os.Exit(1)
	}

	staticConfig.Peers = newPeers

	// Save config
	if err := cluster.SaveStaticConfig(peersPath, staticConfig); err != nil {
		fmt.Printf("Error saving peers.toml: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Peer '%s' removed\n", peerID)
	fmt.Println()
	fmt.Println("Note: Restart the gateway to apply changes")
}

// cmdClusterPeersEnable enables a peer
func cmdClusterPeersEnable() {
	modifyPeerEnabled(true)
}

// cmdClusterPeersDisable disables a peer
func cmdClusterPeersDisable() {
	modifyPeerEnabled(false)
}

// modifyPeerEnabled modifies the enabled status of a peer
func modifyPeerEnabled(enabled bool) {
	action := "enable"
	if !enabled {
		action = "disable"
	}

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
		homeDir, _ = os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Load static config
	peersPath := filepath.Join(workspace, "cluster", "peers.toml")
	staticConfig, err := cluster.LoadStaticConfig(peersPath)
	if err != nil {
		fmt.Printf("Error loading peers.toml: %v\n", err)
		os.Exit(1)
	}

	// Parse flags
	peerID := ""
	args := os.Args[4:]

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--id":
			if i+1 >= len(args) {
				fmt.Println("Error: --id requires a value")
				os.Exit(1)
			}
			i++
			peerID = args[i]
		default:
			fmt.Printf("Error: unknown option %s\n", args[i])
			os.Exit(1)
		}
	}

	if peerID == "" {
		fmt.Println("Error: --id is required")
		fmt.Printf("Usage: nemesisbot cluster peers %s --id <id>\n", action)
		os.Exit(1)
	}

	// Find and modify peer
	found := false
	for i, peer := range staticConfig.Peers {
		if peer.ID == peerID {
			found = true
			staticConfig.Peers[i].Enabled = enabled
			break
		}
	}

	if !found {
		fmt.Printf("Error: peer with id '%s' not found\n", peerID)
		os.Exit(1)
	}

	// Save config
	if err := cluster.SaveStaticConfig(peersPath, staticConfig); err != nil {
		fmt.Printf("Error saving peers.toml: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("✓ Peer '%s' %sd\n", peerID, action)
	fmt.Println()
	fmt.Println("Note: Restart the gateway to apply changes")
}

// cmdClusterInit initializes cluster configuration
func cmdClusterInit() {
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
		homeDir, _ = os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Check if peers.toml already exists
	peersPath := filepath.Join(workspace, "cluster", "peers.toml")
	if _, err := os.Stat(peersPath); err == nil {
		fmt.Println("Cluster configuration already exists.")
		fmt.Println()
		fmt.Print("Do you want to reinitialize? This will overwrite existing configuration. (y/n): ")
		var response string
		fmt.Scanln(&response)
		if response != "y" && response != "Y" {
			fmt.Println("Aborted.")
			return
		}
		fmt.Println("Reinitializing cluster configuration...")
	}

	// Parse flags
	type flags struct {
		Name         string
		Role         string
		Category     string
		Tags         string
		Address      string
		Capabilities string
	}

	f := &flags{}
	args := os.Args[3:]

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--name", "-n":
			if i+1 >= len(args) {
				fmt.Println("Error: --name requires a value")
				os.Exit(1)
			}
			i++
			f.Name = args[i]
		case "--role", "-r":
			if i+1 >= len(args) {
				fmt.Println("Error: --role requires a value")
				os.Exit(1)
			}
			i++
			f.Role = args[i]
		case "--category", "-c":
			if i+1 >= len(args) {
				fmt.Println("Error: --category requires a value")
				os.Exit(1)
			}
			i++
			f.Category = args[i]
		case "--tags", "-t":
			if i+1 >= len(args) {
				fmt.Println("Error: --tags requires a value")
				os.Exit(1)
			}
			i++
			f.Tags = args[i]
		case "--address", "-a":
			if i+1 >= len(args) {
				fmt.Println("Error: --address requires a value")
				os.Exit(1)
			}
			i++
			f.Address = args[i]
		case "--capabilities":
			if i+1 >= len(args) {
				fmt.Println("Error: --capabilities requires a value")
				os.Exit(1)
			}
			i++
			f.Capabilities = args[i]
		case "--help", "-h":
			fmt.Println("\nCluster Initialization:")
			fmt.Println()
			fmt.Println("Options:")
			fmt.Println("  --name, -n <name>            Node name")
			fmt.Println("  --role, -r <role>            Cluster role (manager, coordinator, worker, observer, standby)")
			fmt.Println("  --category, -c <category>    Business category (design, development, testing, etc.)")
			fmt.Println("  --tags, -t <tags>            Custom tags (comma-separated)")
			fmt.Println("  --address, -a <address>      Node address")
			fmt.Println("  --capabilities <caps>        Capabilities (comma-separated)")
			fmt.Println()
			fmt.Println("Cluster Roles:")
			fmt.Println("  manager       - Cluster manager with high privileges")
			fmt.Println("  coordinator   - Task coordinator with medium-high privileges")
			fmt.Println("  worker        - Task executor with medium privileges")
			fmt.Println("  observer      - Read-only observer with low privileges")
			fmt.Println("  standby       - Standby mode, not participating in cluster")
			fmt.Println()
			fmt.Println("Business Categories:")
			fmt.Println("  design        - Design-related tasks")
			fmt.Println("  development   - Development tasks")
			fmt.Println("  testing       - Testing tasks")
			fmt.Println("  ops           - Operations tasks")
			fmt.Println("  deployment    - Deployment tasks")
			fmt.Println("  analysis      - Analysis tasks")
			fmt.Println("  general       - General purpose (default)")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot cluster init")
			fmt.Println("  nemesisbot cluster init --name \"My Bot\"")
			fmt.Println("  nemesisbot cluster init --name \"Design Bot\" --role manager --category design")
			fmt.Println("  nemesisbot cluster init --category development --tags \"production,senior\"")
			fmt.Println()
			return
		default:
			fmt.Printf("Error: unknown option %s\n", args[i])
			fmt.Println("Run 'nemesisbot cluster init --help' for usage")
			os.Exit(1)
		}
	}

	// Generate node ID
	hostname, _ := os.Hostname()
	nodeID := fmt.Sprintf("node-%s-%d", hostname, time.Now().Unix())

	// Set defaults
	nodeName := f.Name
	if nodeName == "" {
		nodeName = "Bot " + nodeID
	}

	nodeRole := f.Role
	if nodeRole == "" {
		nodeRole = "worker"
	}

	nodeCategory := f.Category
	if nodeCategory == "" {
		nodeCategory = "general"
	}

	var tags []string
	if f.Tags != "" {
		tagList := strings.Split(f.Tags, ",")
		for _, tag := range tagList {
			tag = strings.TrimSpace(tag)
			if tag != "" {
				tags = append(tags, tag)
			}
		}
	}

	var capabilities []string
	if f.Capabilities != "" {
		caps := strings.Split(f.Capabilities, ",")
		for _, cap := range caps {
			cap = strings.TrimSpace(cap)
			if cap != "" {
				capabilities = append(capabilities, cap)
			}
		}
	}

	// Create static config
	staticConfig := cluster.CreateStaticConfig(nodeID, nodeName, f.Address)
	staticConfig.Node.Role = nodeRole
	staticConfig.Node.Category = nodeCategory
	staticConfig.Node.Tags = tags
	staticConfig.Node.Capabilities = capabilities

	// Ensure cluster directory exists
	clusterDir := filepath.Join(workspace, "cluster")
	if err := os.MkdirAll(clusterDir, 0755); err != nil {
		fmt.Printf("Error creating cluster directory: %v\n", err)
		os.Exit(1)
	}

	// Save config
	if err := cluster.SaveStaticConfig(peersPath, staticConfig); err != nil {
		fmt.Printf("Error saving peers.toml: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("✓ Cluster configuration initialized")
	fmt.Println()
	fmt.Println("Node Information:")
	fmt.Printf("  ID:           %s\n", staticConfig.Node.ID)
	fmt.Printf("  Name:         %s\n", staticConfig.Node.Name)
	fmt.Printf("  Address:      %s\n", staticConfig.Node.Address)
	fmt.Printf("  Role:         %s\n", staticConfig.Node.Role)
	fmt.Printf("  Category:     %s\n", staticConfig.Node.Category)
	if len(staticConfig.Node.Tags) > 0 {
		fmt.Printf("  Tags:         %s\n", strings.Join(staticConfig.Node.Tags, ", "))
	} else {
		fmt.Printf("  Tags:         (none)\n")
	}
	if len(staticConfig.Node.Capabilities) > 0 {
		fmt.Printf("  Capabilities: %s\n", strings.Join(staticConfig.Node.Capabilities, ", "))
	} else {
		fmt.Printf("  Capabilities: (none)\n")
	}
	fmt.Println()
	fmt.Println("Next steps:")
	fmt.Println("  1. Enable cluster: nemesisbot cluster enable")
	fmt.Println("  2. Start gateway: nemesisbot gateway")
}

// cmdClusterReset resets cluster runtime state
func cmdClusterReset() {
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
		homeDir, _ = os.UserHomeDir()
		workspace = filepath.Join(homeDir, workspace[2:])
	}

	// Parse flags
	hardReset := false
	args := os.Args[3:]

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--hard":
			hardReset = true
		case "--help", "-h":
			fmt.Println("\nCluster Reset:")
			fmt.Println()
			fmt.Println("Options:")
			fmt.Println("  --hard    Perform hard reset (clear all cluster data)")
			fmt.Println()
			fmt.Println("Soft reset:  Clears discovered peers (state.toml)")
			fmt.Println("Hard reset:  Clears all cluster data including peers.toml")
			fmt.Println()
			fmt.Println("Examples:")
			fmt.Println("  nemesisbot cluster reset       # Soft reset")
			fmt.Println("  nemesisbot cluster reset --hard # Hard reset")
			fmt.Println()
			return
		default:
			fmt.Printf("Error: unknown option %s\n", args[i])
			fmt.Println("Run 'nemesisbot cluster reset --help' for usage")
			os.Exit(1)
		}
	}

	if hardReset {
		// Hard reset: remove entire cluster directory
		clusterDir := filepath.Join(workspace, "cluster")
		if _, err := os.Stat(clusterDir); err == nil {
			fmt.Println("Warning: This will delete all cluster configuration and data.")
			fmt.Println()
			fmt.Print("Are you sure? (y/n): ")
			var response string
			fmt.Scanln(&response)
			if response != "y" && response != "Y" {
				fmt.Println("Aborted.")
				return
			}

			if err := os.RemoveAll(clusterDir); err != nil {
				fmt.Printf("Error removing cluster directory: %v\n", err)
				os.Exit(1)
			}

			fmt.Println("✓ Hard reset complete")
			fmt.Println()
			fmt.Println("All cluster data has been deleted.")
			fmt.Println()
			fmt.Println("Next steps:")
			fmt.Println("  1. Initialize cluster: nemesisbot cluster init")
			fmt.Println("  2. Enable cluster: nemesisbot cluster enable")
			fmt.Println("  3. Start gateway: nemesisbot gateway")
		} else {
			fmt.Println("No cluster data found")
		}
	} else {
		// Soft reset: only remove state.toml
		statePath := filepath.Join(workspace, "cluster", "state.toml")
		if _, err := os.Stat(statePath); err == nil {
			if err := os.Remove(statePath); err != nil {
				fmt.Printf("Error removing state.toml: %v\n", err)
				os.Exit(1)
			}

			fmt.Println("✓ Soft reset complete")
			fmt.Println()
			fmt.Println("Discovered peers have been cleared.")
			fmt.Println("Static configuration (peers.toml) is preserved.")
			fmt.Println()
			fmt.Println("Note: Restart the gateway to apply changes")
		} else {
			fmt.Println("No runtime state found (state.toml doesn't exist)")
		}
	}
}
