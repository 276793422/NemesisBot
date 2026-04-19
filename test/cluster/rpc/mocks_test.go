// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc_test

import (
	"context"
	"fmt"
	"net"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/cluster/rpc"
)

// mockNode implements Node interface for testing
type mockNode struct {
	id           string
	name         string
	address      string
	addresses    []string
	rpcPort      int
	capabilities []string
	status       string
	online       bool
	conn         *net.Conn
}

func (m *mockNode) GetID() string             { return m.id }
func (m *mockNode) GetName() string           { return m.name }
func (m *mockNode) GetAddress() string        { return m.address }
func (m *mockNode) GetAddresses() []string    { return m.addresses }
func (m *mockNode) GetRPCPort() int           { return m.rpcPort }
func (m *mockNode) GetCapabilities() []string { return m.capabilities }
func (m *mockNode) GetStatus() string         { return m.status }
func (m *mockNode) IsOnline() bool            { return m.online }

// mockCluster implements Cluster interface for testing
type mockCluster struct {
	nodeID       string
	capabilities []string
	logCalls     []string
	peers        map[string]*mockNode
}

func (m *mockCluster) GetRegistry() interface{}        { return nil }
func (m *mockCluster) GetNodeID() string               { return m.nodeID }
func (m *mockCluster) GetAddress() string              { return "" }
func (m *mockCluster) GetCapabilities() []string       { return m.capabilities }
func (m *mockCluster) GetOnlinePeers() []interface{}   { return nil }
func (m *mockCluster) GetActionsSchema() []interface{} { return []interface{}{} }
func (m *mockCluster) LogRPCInfo(msg string, args ...interface{}) {
	m.logCalls = append(m.logCalls, fmt.Sprintf("INFO: "+msg, args...))
}
func (m *mockCluster) LogRPCError(msg string, args ...interface{}) {
	m.logCalls = append(m.logCalls, fmt.Sprintf("ERROR: "+msg, args...))
}
func (m *mockCluster) LogRPCDebug(msg string, args ...interface{}) {
	m.logCalls = append(m.logCalls, fmt.Sprintf("DEBUG: "+msg, args...))
}
func (m *mockCluster) GetPeer(peerID string) (interface{}, error) {
	if peer, ok := m.peers[peerID]; ok {
		return peer, nil
	}
	return nil, fmt.Errorf("peer not found: %s", peerID)
}
func (m *mockCluster) GetLocalNetworkInterfaces() ([]rpc.LocalNetworkInterface, error) {
	return []rpc.LocalNetworkInterface{
		{IP: "127.0.0.1", Mask: "255.0.0.0"},
	}, nil
}
func (m *mockCluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	return nil, fmt.Errorf("not implemented")
}
func (m *mockCluster) GetTaskResultStorer() rpc.TaskResultStorer { return nil }

// mockRPCChannel implements channels.RPCChannel struct for testing
type mockRPCChannel struct {
	responseChan chan string
	err          error
}

func (m *mockRPCChannel) Input(ctx context.Context, msg *bus.InboundMessage) (<-chan string, error) {
	if m.err != nil {
		return nil, m.err
	}
	return m.responseChan, nil
}

func (m *mockRPCChannel) Send(ctx context.Context, msg *bus.OutboundMessage) error {
	return nil
}

func (m *mockRPCChannel) Start() error { return nil }
func (m *mockRPCChannel) Stop() error  { return nil }
func (m *mockRPCChannel) Close() error { return nil }

// integrationTestCluster implements Cluster interface for integration tests
type integrationTestCluster struct {
	nodeID       string
	capabilities []string
	logCalls     []string
	peers        map[string]*mockNode
}

func (m *integrationTestCluster) GetRegistry() interface{}        { return nil }
func (m *integrationTestCluster) GetNodeID() string               { return m.nodeID }
func (m *integrationTestCluster) GetAddress() string              { return "127.0.0.1:21949" }
func (m *integrationTestCluster) GetCapabilities() []string       { return m.capabilities }
func (m *integrationTestCluster) GetOnlinePeers() []interface{}   { return nil }
func (m *integrationTestCluster) GetActionsSchema() []interface{} { return []interface{}{} }
func (m *integrationTestCluster) LogRPCInfo(msg string, args ...interface{}) {
	m.logCalls = append(m.logCalls, fmt.Sprintf("INFO: "+msg, args...))
}
func (m *integrationTestCluster) LogRPCError(msg string, args ...interface{}) {
	m.logCalls = append(m.logCalls, fmt.Sprintf("ERROR: "+msg, args...))
}
func (m *integrationTestCluster) LogRPCDebug(msg string, args ...interface{}) {
	m.logCalls = append(m.logCalls, fmt.Sprintf("DEBUG: "+msg, args...))
}
func (m *integrationTestCluster) GetPeer(peerID string) (interface{}, error) {
	if peer, ok := m.peers[peerID]; ok {
		return peer, nil
	}
	return nil, fmt.Errorf("peer not found: %s", peerID)
}
func (m *integrationTestCluster) GetLocalNetworkInterfaces() ([]rpc.LocalNetworkInterface, error) {
	return []rpc.LocalNetworkInterface{
		{IP: "127.0.0.1", Mask: "255.0.0.0"},
	}, nil
}
func (m *integrationTestCluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	return nil, fmt.Errorf("not implemented")
}
func (m *integrationTestCluster) GetTaskResultStorer() rpc.TaskResultStorer { return nil }

// Helper method to create RPC client with custom settings
func createRPCClient(cluster *mockCluster) *rpc.Client {
	client := rpc.NewClient(cluster)
	// Configure with faster rate limits for testing
	return client
}

// Helper method to create test payload
func createTestPayload(msg string, id int) map[string]interface{} {
	return map[string]interface{}{
		"message":   msg,
		"id":        id,
		"timestamp": time.Now().Unix(),
	}
}
