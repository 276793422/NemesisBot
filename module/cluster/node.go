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
	ID           string    `toml:"id"`
	Name         string    `toml:"name"`
	Address      string    `toml:"address"`      // Deprecated: Primary IP:Port (for backward compatibility)
	Addresses    []string  `toml:"addresses"`     // List of all IP addresses
	RPCPort      int       `toml:"rpc_port"`      // RPC port number
	Role         string    `toml:"role"`          // Cluster role
	Category     string    `toml:"category"`      // Business category
	Tags         []string  `toml:"tags"`          // Custom tags
	Capabilities []string  `toml:"capabilities"`
	Priority     int       `toml:"priority"`

	// Runtime state
	Status       NodeStatus `toml:"-"`
	LastSeen     time.Time  `toml:"-"`
	TasksCompleted int      `toml:"-"`
	SuccessRate  float64    `toml:"-"`
	AvgResponseTime int     `toml:"-"`
	LastError    string     `toml:"-"`
	mu           sync.RWMutex `toml:"-"`
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
		Address:      n.Address,      // Primary address for backward compatibility
		Addresses:    n.Addresses,    // All addresses
		RPCPort:      n.RPCPort,      // RPC port
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
	return n.ID
}

// GetName returns the node name (for RPC interface)
func (n *Node) GetName() string {
	return n.Name
}

// GetAddress returns the node address (for RPC interface)
func (n *Node) GetAddress() string {
	return n.Address
}

// GetCapabilities returns the node capabilities (for RPC interface)
func (n *Node) GetCapabilities() []string {
	return n.Capabilities
}

// String returns a string representation of the node
func (n *Node) String() string {
	return fmt.Sprintf("Node{id=%s, name=%s, address=%s, status=%s}",
		n.ID, n.Name, n.Address, n.Status)
}

// GenerateNodeID generates a unique node ID based on hostname and timestamp
func GenerateNodeID() (string, error) {
	hostname, err := os.Hostname()
	if err != nil {
		return "", err
	}

	// Get local IP address
	ip, err := getLocalIP()
	if err != nil {
		ip = "unknown"
	}

	// Create node ID: hostname-ip-timestamp
	timestamp := time.Now().Format("20060102-150405")
	nodeID := fmt.Sprintf("bot-%s-%s-%s", hostname, strings.ReplaceAll(ip, ".", "-"), timestamp)

	return nodeID, nil
}

// getLocalIP returns the local IP address
func getLocalIP() (string, error) {
	conn, err := net.Dial("udp", "8.8.8.8:80")
	if err != nil {
		return "", err
	}
	defer conn.Close()

	localAddr := conn.LocalAddr().(*net.UDPAddr)
	return localAddr.IP.String(), nil
}

// GetAllLocalIPs returns all local IP addresses
func GetAllLocalIPs() ([]string, error) {
	var ips []string

	interfaces, err := net.Interfaces()
	if err != nil {
		return nil, err
	}

	for _, iface := range interfaces {
		// Skip down interfaces and loopback
		if iface.Flags&net.FlagUp == 0 || iface.Flags&net.FlagLoopback != 0 {
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

			// Only include IPv4 addresses
			if ip != nil && ip.To4() != nil {
				ips = append(ips, ip.String())
			}
		}
	}

	// If no IPs found, fallback to getLocalIP
	if len(ips) == 0 {
		ip, err := getLocalIP()
		if err != nil {
			return nil, err
		}
		return []string{ip}, nil
	}

	return ips, nil
}
