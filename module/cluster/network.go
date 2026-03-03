// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// Package cluster provides network utility functions for subnet matching
package cluster

import (
	"fmt"
	"net"
	"strings"
)

// NetworkInterface represents a local network interface with its subnet
type NetworkInterface struct {
	IP        string
	Mask      string
	NetworkIP string // The network address (IP & Mask)
}

// GetLocalNetworkInterfaces returns all local network interfaces with their subnet masks
func GetLocalNetworkInterfaces() ([]NetworkInterface, error) {
	var interfaces []NetworkInterface

	netInterfaces, err := net.Interfaces()
	if err != nil {
		return nil, err
	}

	for _, iface := range netInterfaces {
		// Skip down interfaces and loopback
		if iface.Flags&net.FlagUp == 0 || iface.Flags&net.FlagLoopback != 0 {
			continue
		}

		addrs, err := iface.Addrs()
		if err != nil {
			continue
		}

		for _, addr := range addrs {
			var ipNet *net.IPNet
			switch v := addr.(type) {
			case *net.IPNet:
				ipNet = v
			case *net.IPAddr:
				ipNet = &net.IPNet{
					IP:   v.IP,
					Mask: net.IPv4Mask(255, 255, 255, 255), // Default /24
				}
			}

			// Only include IPv4 addresses
			if ipNet != nil && ipNet.IP.To4() != nil {
				// Get the network address (IP & Mask)
				networkIP := ipNet.IP.Mask(ipNet.Mask).String()

				interfaces = append(interfaces, NetworkInterface{
					IP:        ipNet.IP.String(),
					Mask:      net.IP(ipNet.Mask).String(),
					NetworkIP: networkIP,
				})
			}
		}
	}

	return interfaces, nil
}

// IsSameSubnet checks if two IP addresses are in the same subnet
// Uses real subnet masks from local network interfaces
// Returns true if they are in the same subnet, false otherwise
func IsSameSubnet(ip1, ip2 string) bool {
	// Get all local network interfaces
	localInterfaces, err := GetLocalNetworkInterfaces()
	if err != nil || len(localInterfaces) == 0 {
		// Fallback to simple /24 assumption
		return isSameSubnetSimple(ip1, ip2)
	}

	// Parse the target IPs
	parsedIP1 := net.ParseIP(ip1)
	parsedIP2 := net.ParseIP(ip2)
	if parsedIP1 == nil || parsedIP2 == nil {
		return false
	}

	// Check if both IPs belong to any of the local subnets
	for _, localIface := range localInterfaces {
		// Parse the mask to get its size
		parsedMask := net.ParseIP(localIface.Mask)
		if parsedMask == nil {
			continue
		}

		maskSize, _ := net.IPMask(parsedMask).Size()

		// Construct CIDR notation
		cidr := fmt.Sprintf("%s/%d", localIface.NetworkIP, maskSize)

		_, ipNet, err := net.ParseCIDR(cidr)
		if err != nil {
			continue
		}

		// Check if both IPs are in this subnet
		if ipNet.Contains(parsedIP1) && ipNet.Contains(parsedIP2) {
			return true
		}
	}

	return false
}

// isSameSubnetSimple is a fallback that assumes /24 subnet mask
func isSameSubnetSimple(ip1, ip2 string) bool {
	parts1 := strings.Split(ip1, ".")
	parts2 := strings.Split(ip2, ".")

	if len(parts1) < 4 || len(parts2) < 4 {
		return false
	}

	// Compare first three octets
	return parts1[0] == parts2[0] &&
		parts1[1] == parts2[1] &&
		parts1[2] == parts2[2]
}
