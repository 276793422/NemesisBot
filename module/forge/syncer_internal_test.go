package forge

import (
	"context"
	"os"
	"testing"
	"time"
)

// --- sanitizeNodeID tests ---

func TestSanitizeNodeID_Alphanumeric(t *testing.T) {
	result := sanitizeNodeID("node-123_ABC")
	if result != "node-123_ABC" {
		t.Errorf("Alphanumeric + hyphen + underscore should pass through: %s", result)
	}
}

func TestSanitizeNodeID_SpecialChars(t *testing.T) {
	result := sanitizeNodeID("node@#$%123!")
	if result != "node____123_" {
		t.Errorf("Special chars should be replaced with underscore: %s", result)
	}
}

func TestSanitizeNodeID_Empty(t *testing.T) {
	result := sanitizeNodeID("")
	if result != "unknown" {
		t.Errorf("Empty string should become 'unknown': %s", result)
	}
}

func TestSanitizeNodeID_AllSpecialChars(t *testing.T) {
	result := sanitizeNodeID("@#$%")
	// Special chars become underscores, but "____" is not empty, so it stays
	if result == "@#$%" {
		t.Error("Special chars should be sanitized")
	}
	// Only truly empty result becomes "unknown"
	if result != "____" {
		t.Errorf("Expected '____', got '%s'", result)
	}
}

func TestSanitizeNodeID_Unicode(t *testing.T) {
	result := sanitizeNodeID("node-测试-123")
	// Chinese chars should be replaced
	if result == "node-测试-123" {
		t.Error("Unicode characters should be sanitized")
	}
}

// --- Syncer tests ---

func TestNewSyncer(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)
	if syncer == nil {
		t.Fatal("NewSyncer returned nil")
	}
}

func TestSyncer_IsEnabled_NoBridge(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	if syncer.IsEnabled() {
		t.Error("Syncer should not be enabled without bridge")
	}
}

// mockClusterForgeBridge for in-package tests
type mockClusterForgeBridge struct {
	enabled    bool
	peers      []PeerInfo
	shareErr   error
	shareCalls int
}

func (m *mockClusterForgeBridge) ShareToPeer(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	m.shareCalls++
	if m.shareErr != nil {
		return nil, m.shareErr
	}
	return []byte(`{"status":"ok"}`), nil
}

func (m *mockClusterForgeBridge) GetOnlinePeers() []PeerInfo {
	return m.peers
}

func (m *mockClusterForgeBridge) IsClusterEnabled() bool {
	return m.enabled
}

func TestSyncer_IsEnabled_WithBridge(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	syncer.SetBridge(&mockClusterForgeBridge{enabled: true})
	if !syncer.IsEnabled() {
		t.Error("Syncer should be enabled when bridge is set and cluster is running")
	}
}

func TestSyncer_IsEnabled_BridgeClusterDisabled(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	syncer.SetBridge(&mockClusterForgeBridge{enabled: false})
	if syncer.IsEnabled() {
		t.Error("Syncer should not be enabled when cluster is not running")
	}
}

func TestSyncer_ReceiveReflection(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	payload := map[string]interface{}{
		"content":   "# Test Report\nSome content",
		"filename":  "2026-04-25.md",
		"from":      "node-abc",
		"timestamp": time.Now().UTC().Format(time.RFC3339),
	}

	if err := syncer.ReceiveReflection(payload); err != nil {
		t.Fatalf("ReceiveReflection failed: %v", err)
	}

	// Verify file was created
	paths, err := syncer.GetRemoteReflections()
	if err != nil {
		t.Fatalf("GetRemoteReflections failed: %v", err)
	}
	if len(paths) != 1 {
		t.Fatalf("Expected 1 remote reflection, got %d", len(paths))
	}
}

func TestSyncer_ReceiveReflection_MissingContent(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	payload := map[string]interface{}{
		"filename": "test.md",
	}
	if err := syncer.ReceiveReflection(payload); err == nil {
		t.Error("Should error on missing content")
	}
}

func TestSyncer_ReceiveReflection_PathTraversalPrevention(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	payload := map[string]interface{}{
		"content":  "test content",
		"filename": "../../../etc/passwd",
	}
	// Should not error (filepath.Base strips traversal), but file should be in remote dir
	if err := syncer.ReceiveReflection(payload); err != nil {
		t.Fatalf("Unexpected error: %v", err)
	}

	// Verify the file was saved with sanitized name (passwd without .md extension)
	remoteDir := tmpDir + "/reflections/remote"
	entries, err := os.ReadDir(remoteDir)
	if err != nil {
		t.Fatalf("Failed to read remote dir: %v", err)
	}
	if len(entries) != 1 {
		t.Fatalf("Expected 1 file, got %d", len(entries))
	}
	// The file should be named something like "unknown_passwd" (from sanitized node ID + base filename)
	t.Logf("File created: %s", entries[0].Name())
}

func TestSyncer_GetRemoteReflections_NoDir(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	paths, err := syncer.GetRemoteReflections()
	if err != nil {
		t.Fatalf("Should not error on non-existent dir: %v", err)
	}
	if paths != nil {
		t.Errorf("Expected nil for non-existent dir, got %v", paths)
	}
}

func TestSyncer_GetLocalReflections_NoDir(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	paths, err := syncer.GetLocalReflections()
	if err != nil {
		t.Fatalf("Should not error on non-existent dir: %v", err)
	}
	if paths != nil {
		t.Errorf("Expected nil for non-existent dir, got %v", paths)
	}
}

func TestSyncer_GetReflectionsListPayload(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	payload := syncer.GetReflectionsListPayload()
	if payload == nil {
		t.Fatal("Payload should not be nil")
	}
	if payload["count"] != 0 {
		t.Errorf("Expected count 0 for no reflections, got %v", payload["count"])
	}
}

func TestSyncer_ReadReflectionContent(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	// Create a reflection file
	refDir := tmpDir + "/reflections"
	mkdirErr := func() error {
		return os.MkdirAll(refDir, 0755)
	}()
	if mkdirErr != nil {
		t.Fatal(mkdirErr)
	}
	os.WriteFile(refDir+"/test.md", []byte("# Test Report\nContent"), 0644)

	content, err := syncer.ReadReflectionContent("test.md")
	if err != nil {
		t.Fatalf("ReadReflectionContent failed: %v", err)
	}
	if content != "# Test Report\nContent" {
		t.Errorf("Unexpected content: %s", content)
	}
}

func TestSyncer_ReadReflectionContent_InvalidFilename(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	_, err := syncer.ReadReflectionContent("..")
	if err == nil {
		t.Error("Should error on '..' filename")
	}
}

func TestSyncer_SanitizeContent(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	content := "api_key: sk-12345 at C:\\Users\\admin\\project with IP 203.0.113.50"
	sanitized := syncer.SanitizeContent(content)

	if sanitized == content {
		t.Error("Content should be sanitized")
	}
}

func TestSyncer_ShareReflection_NoPeers(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)
	syncer.SetBridge(&mockClusterForgeBridge{
		enabled: true,
		peers:   []PeerInfo{},
	})

	// Create a dummy report
	refDir := tmpDir + "/reflections"
	os.MkdirAll(refDir, 0755)
	os.WriteFile(refDir+"/test.md", []byte("report content"), 0644)

	err := syncer.ShareReflection(context.Background(), refDir+"/test.md")
	if err != nil {
		t.Fatalf("ShareReflection with no peers should not error: %v", err)
	}
}

func TestSyncer_ShareReflection_WithPeers(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	bridge := &mockClusterForgeBridge{
		enabled: true,
		peers: []PeerInfo{
			{ID: "peer-1", Name: "Peer 1"},
			{ID: "peer-2", Name: "Peer 2"},
		},
	}
	syncer.SetBridge(bridge)

	// Create a dummy report
	refDir := tmpDir + "/reflections"
	os.MkdirAll(refDir, 0755)
	os.WriteFile(refDir+"/test.md", []byte("report content with api_key: secret123"), 0644)

	err := syncer.ShareReflection(context.Background(), refDir+"/test.md")
	if err != nil {
		t.Fatalf("ShareReflection failed: %v", err)
	}
	if bridge.shareCalls != 2 {
		t.Errorf("Expected 2 share calls, got %d", bridge.shareCalls)
	}
}

func TestSyncer_ShareReflection_NotEnabled(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := DefaultForgeConfig()
	registry := NewRegistry(tmpDir + "/registry.json")
	syncer := NewSyncer(tmpDir, registry, cfg)

	err := syncer.ShareReflection(context.Background(), "test.md")
	if err == nil {
		t.Error("Should error when not enabled")
	}
}

// --- marshalPayload tests ---

func TestMarshalPayload(t *testing.T) {
	data := map[string]interface{}{"key": "value", "count": 42}
	result := marshalPayload(data)
	if len(result) == 0 {
		t.Error("Result should not be empty")
	}
}
