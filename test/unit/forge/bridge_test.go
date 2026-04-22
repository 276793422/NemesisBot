package forge_test

import (
	"context"
	"errors"
	"testing"

	"github.com/276793422/NemesisBot/module/forge"
)

// Tests for ClusterForgeBridge interface behavior using mockBridge
// (defined in syncer_test.go). The real bridge adapts cluster.Cluster,
// but we test the interface contract here.

func TestBridge_InterfaceContract(t *testing.T) {
	// Verify mockBridge satisfies the ClusterForgeBridge interface
	var _ forge.ClusterForgeBridge = &mockBridge{}
}

func TestBridge_IsClusterEnabled_True(t *testing.T) {
	bridge := &mockBridge{
		clusterRun: true,
		peers: []forge.PeerInfo{
			{ID: "node-1", Name: "Worker-1"},
		},
	}

	if !bridge.IsClusterEnabled() {
		t.Error("Bridge should report cluster enabled")
	}
}

func TestBridge_IsClusterEnabled_False(t *testing.T) {
	bridge := &mockBridge{
		clusterRun: false,
	}

	if bridge.IsClusterEnabled() {
		t.Error("Bridge should report cluster disabled")
	}
}

func TestBridge_GetOnlinePeers(t *testing.T) {
	expectedPeers := []forge.PeerInfo{
		{ID: "node-1", Name: "Worker-1"},
		{ID: "node-2", Name: "Worker-2"},
	}
	bridge := &mockBridge{
		clusterRun: true,
		peers:      expectedPeers,
	}

	peers := bridge.GetOnlinePeers()
	if len(peers) != 2 {
		t.Fatalf("Expected 2 peers, got %d", len(peers))
	}
	if peers[0].ID != "node-1" || peers[1].ID != "node-2" {
		t.Errorf("Peer IDs mismatch: %+v", peers)
	}
	if peers[0].Name != "Worker-1" || peers[1].Name != "Worker-2" {
		t.Errorf("Peer names mismatch: %+v", peers)
	}
}

func TestBridge_GetOnlinePeers_Empty(t *testing.T) {
	bridge := &mockBridge{
		clusterRun: true,
		peers:      nil,
	}

	peers := bridge.GetOnlinePeers()
	if len(peers) != 0 {
		t.Errorf("Expected 0 peers, got %d", len(peers))
	}
}

func TestBridge_ShareToPeer(t *testing.T) {
	bridge := &mockBridge{
		clusterRun: true,
		peers: []forge.PeerInfo{
			{ID: "node-1", Name: "Worker-1"},
		},
	}

	resp, err := bridge.ShareToPeer(context.Background(), "node-1", "forge_share", map[string]interface{}{
		"content": "test report",
	})
	if err != nil {
		t.Fatalf("ShareToPeer should succeed: %v", err)
	}
	if string(resp) != `{"status":"ok"}` {
		t.Errorf("Unexpected response: %s", string(resp))
	}
	if bridge.shareCalls != 1 {
		t.Errorf("Expected 1 share call, got %d", bridge.shareCalls)
	}
}

func TestBridge_ShareToPeer_Error(t *testing.T) {
	bridge := &mockBridge{
		clusterRun: true,
		peers: []forge.PeerInfo{
			{ID: "node-1", Name: "Worker-1"},
		},
		shareFunc: func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
			return nil, errors.New("connection refused")
		},
	}

	_, err := bridge.ShareToPeer(context.Background(), "node-1", "forge_share", nil)
	if err == nil {
		t.Error("ShareToPeer should fail with custom error")
	}
	if err.Error() != "connection refused" {
		t.Errorf("Expected 'connection refused' error, got: %v", err)
	}
}

func TestBridge_IntegrationWithSyncer(t *testing.T) {
	// Verify bridge integrates properly with Syncer
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(tmpDir + "/registry.json")
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	// Without bridge, syncer should not be enabled
	if syncer.IsEnabled() {
		t.Error("Syncer should not be enabled without bridge")
	}

	// Set bridge with cluster running
	bridge := &mockBridge{
		clusterRun: true,
		peers:      []forge.PeerInfo{{ID: "node-1", Name: "Worker-1"}},
	}
	syncer.SetBridge(bridge)

	// Now should be enabled
	if !syncer.IsEnabled() {
		t.Error("Syncer should be enabled after bridge injection")
	}
}

func TestBridge_IntegrationWithForge(t *testing.T) {
	// Verify Forge.SetBridge cascades to Syncer
	f, _ := newTestForge(t)

	bridge := &mockBridge{
		clusterRun: true,
		peers:      []forge.PeerInfo{{ID: "peer-1", Name: "Peer1"}},
	}
	f.SetBridge(bridge)

	syncer := f.GetSyncer()
	if syncer == nil {
		t.Fatal("Syncer should not be nil")
	}
	if !syncer.IsEnabled() {
		t.Error("Syncer should be enabled after Forge.SetBridge")
	}
}
