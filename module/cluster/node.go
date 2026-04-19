// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"fmt"
	"net"
	"os"
	"strings"
	"sync"
	"time"
)

// Node represents a bot in the cluster
type Node struct {
	ID           string   `toml:"id"`
	Name         string   `toml:"name"`
	Address      string   `toml:"address"`   // Deprecated: Primary IP:Port (for backward compatibility)
	Addresses    []string `toml:"addresses"` // List of all IP addresses
	RPCPort      int      `toml:"rpc_port"`  // RPC port number
	Role         string   `toml:"role"`      // Cluster role
	Category     string   `toml:"category"`  // Business category
	Tags         []string `toml:"tags"`      // Custom tags
	Capabilities []string `toml:"capabilities"`
	Priority     int      `toml:"priority"`

	// Runtime state
	Status          NodeStatus   `toml:"-"`
	LastSeen        time.Time    `toml:"-"`
	TasksCompleted  int          `toml:"-"`
	SuccessRate     float64      `toml:"-"`
	AvgResponseTime int          `toml:"-"`
	LastError       string       `toml:"-"`
	mu              sync.RWMutex `toml:"-"`
}

// NodeStatus represents the current status of a node
type NodeStatus string

const (
	StatusOnline  NodeStatus = "online"
	StatusOffline NodeStatus = "offline"
	StatusUnknown NodeStatus = "unknown"
)

// IsOnline returns true if the node is online
func (n *Node) IsOnline() bool {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return n.Status == StatusOnline
}

// GetStatus returns the current status as string
func (n *Node) GetStatus() string {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return string(n.Status)
}

// GetNodeStatus returns the current status as NodeStatus type
func (n *Node) GetNodeStatus() NodeStatus {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return n.Status
}

// SetStatus updates the node status
func (n *Node) SetStatus(status NodeStatus) {
	n.mu.Lock()
	defer n.mu.Unlock()
	n.Status = status
	n.LastSeen = time.Now()
}

// UpdateLastSeen updates the last seen timestamp
func (n *Node) UpdateLastSeen() {
	n.mu.Lock()
	defer n.mu.Unlock()
	n.LastSeen = time.Now()
	if n.Status != StatusOnline {
		n.Status = StatusOnline
	}
}

// MarkOffline marks the node as offline
func (n *Node) MarkOffline(reason string) {
	n.mu.Lock()
	defer n.mu.Unlock()
	n.Status = StatusOffline
	n.LastError = reason
}

// GetUptime returns the uptime duration
func (n *Node) GetUptime() time.Duration {
	n.mu.RLock()
	defer n.mu.RUnlock()
	if n.LastSeen.IsZero() {
		return 0
	}
	return time.Since(n.LastSeen)
}

// ToConfig converts Node to PeerConfig for TOML serialization
func (n *Node) ToConfig() PeerConfig {
	n.mu.RLock()
	defer n.mu.RUnlock()

	return PeerConfig{
		ID:           n.ID,
		Name:         n.Name,
		Address:      n.Address,   // Primary address for backward compatibility
		Addresses:    n.Addresses, // All addresses
		RPCPort:      n.RPCPort,   // RPC port
		Role:         n.Role,
		Category:     n.Category,
		Tags:         n.Tags,
		Capabilities: n.Capabilities,
		Priority:     n.Priority,
		Status: PeerStatus{
			State:           string(n.Status),
			LastSeen:        n.LastSeen,
			TasksCompleted:  n.TasksCompleted,
			SuccessRate:     n.SuccessRate,
			AvgResponseTime: n.AvgResponseTime,
			LastError:       n.LastError,
		},
	}
}

// HasCapability checks if the node has a specific capability
func (n *Node) HasCapability(capability string) bool {
	n.mu.RLock()
	defer n.mu.RUnlock()
	for _, cap := range n.Capabilities {
		if strings.EqualFold(cap, capability) {
			return true
		}
	}
	return false
}

// GetID returns the node ID (for RPC interface)
func (n *Node) GetID() string {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return n.ID
}

// GetName returns the node name (for RPC interface)
func (n *Node) GetName() string {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return n.Name
}

// GetAddress returns the node address (for RPC interface)
func (n *Node) GetAddress() string {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return n.Address
}

// GetCapabilities returns the node capabilities (for RPC interface)
func (n *Node) GetCapabilities() []string {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return n.Capabilities
}

// GetAddresses returns all IP addresses of the node (for RPC interface)
func (n *Node) GetAddresses() []string {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return n.Addresses
}

// GetRPCPort returns the RPC port of the node (for RPC interface)
func (n *Node) GetRPCPort() int {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return n.RPCPort
}

// String returns a string representation of the node
func (n *Node) String() string {
	n.mu.RLock()
	defer n.mu.RUnlock()
	return fmt.Sprintf("Node{id=%s, name=%s, address=%s, status=%s}",
		n.ID, n.Name, n.Address, n.Status)
}

// GenerateNodeID generates a unique node ID based on hostname and timestamp
func GenerateNodeID() (string, error) {
	hostname, err := os.Hostname()
	if err != nil {
		return "", err
	}

	// Create simple node ID: hostname-timestamp
	// Users can customize this via config if needed
	// Note: nanosecond precision to prevent collisions when multiple instances
	// start on the same host within the same second.
	timestamp := time.Now().Format("20060102-150405.000000000")
	nodeID := fmt.Sprintf("bot-%s-%s", hostname, timestamp)

	return nodeID, nil
}

// GetAllLocalIPs returns all local IP addresses for broadcast
// Returns non-virtual interfaces only, sorted by priority (Ethernet > WiFi > Other)
// This is used for UDP discovery broadcast to tell other nodes how to reach us
func GetAllLocalIPs() ([]string, error) {
	interfaces, err := net.Interfaces()
	if err != nil {
		// Return empty slice, not error
		return []string{}, nil
	}

	// Collect candidate IPs with their priorities
	var candidates []candidateIP

	for _, iface := range interfaces {
		// Skip down and loopback interfaces
		if iface.Flags&net.FlagUp == 0 || iface.Flags&net.FlagLoopback != 0 {
			continue
		}

		// Skip virtual interfaces (common patterns)
		ifName := iface.Name
		if isVirtualInterface(ifName) {
			continue
		}

		addrs, err := iface.Addrs()
		if err != nil {
			continue
		}

		for _, addr := range addrs {
			var ip net.IP
			switch v := addr.(type) {
			case *net.IPNet:
				ip = v.IP
			case *net.IPAddr:
				ip = v.IP
			}

			// Only consider IPv4, exclude link-local
			if ip != nil && ip.To4() != nil && !ip.IsLinkLocalUnicast() {
				priority := getInterfacePriority(ifName)
				candidates = append(candidates, candidateIP{
					ip:       ip.String(),
					priority: priority,
				})
			}
		}
	}

	// Sort by priority and return IPs in priority order
	sortCandidatesByPriority(candidates)

	// Extract just the IP strings
	result := make([]string, len(candidates))
	for i, c := range candidates {
		result[i] = c.ip
	}

	return result, nil
}

// isVirtualInterface checks if an interface name matches common virtual interface patterns
func isVirtualInterface(name string) bool {
	virtualPatterns := []string{
		"veth", "docker", "br-", "virbr", "tun", "tap",
		"vbox", "vmnet", "utun", "awdl", "llw", "anpi",
		"ipsec", "gif", "stf", "p2p", "lo", "Loopback",
	}

	lowerName := strings.ToLower(name)
	for _, pattern := range virtualPatterns {
		if strings.Contains(lowerName, pattern) {
			return true
		}
	}
	return false
}

// getInterfacePriority returns a priority score for an interface type
// Lower score = higher priority (for sorting broadcast IP list)
func getInterfacePriority(name string) int {
	lowerName := strings.ToLower(name)

	// Priority 1: Ethernet (eth, eno, ens, enp)
	if strings.HasPrefix(lowerName, "eth") ||
		strings.HasPrefix(lowerName, "eno") ||
		strings.HasPrefix(lowerName, "ens") ||
		strings.HasPrefix(lowerName, "enp") {
		return 1
	}

	// Priority 2: WiFi (wlan, wlp)
	if strings.HasPrefix(lowerName, "wlan") ||
		strings.HasPrefix(lowerName, "wlp") {
		return 2
	}

	// Priority 3: Other physical interfaces
	if strings.HasPrefix(lowerName, "en") || strings.HasPrefix(lowerName, "wl") {
		return 3
	}

	// Priority 99: Everything else
	return 99
}

// sortCandidatesByPriority sorts candidates by priority (stable sort)
func sortCandidatesByPriority(candidates []candidateIP) {
	// Simple bubble sort (small number of candidates expected)
	n := len(candidates)
	for i := 0; i < n-1; i++ {
		for j := 0; j < n-i-1; j++ {
			if candidates[j].priority > candidates[j+1].priority {
				candidates[j], candidates[j+1] = candidates[j+1], candidates[j]
			}
		}
	}
}

type candidateIP struct {
	ip       string
	priority int
}
