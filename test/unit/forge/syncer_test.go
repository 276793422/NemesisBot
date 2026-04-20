package forge_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/forge"
	"github.com/276793422/NemesisBot/module/tools"
)

// === Syncer Tests ===

func TestSyncerNew(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	if syncer == nil {
		t.Fatal("NewSyncer should return non-nil")
	}
}

func TestSyncerNotEnabledWithoutBridge(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	if syncer.IsEnabled() {
		t.Error("Syncer should not be enabled without a bridge")
	}
}

// mockBridge is a test mock for ClusterForgeBridge
type mockBridge struct {
	peers       []forge.PeerInfo
	clusterRun  bool
	shareFunc   func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error)
	shareCalls  int
}

func (m *mockBridge) ShareToPeer(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	m.shareCalls++
	if m.shareFunc != nil {
		return m.shareFunc(ctx, peerID, action, payload)
	}
	return []byte(`{"status":"ok"}`), nil
}

func (m *mockBridge) GetOnlinePeers() []forge.PeerInfo {
	return m.peers
}

func (m *mockBridge) IsClusterEnabled() bool {
	return m.clusterRun
}

func TestSyncerShareReflection(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	// Create a test report
	refDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(refDir, 0755)
	reportPath := filepath.Join(refDir, "2026-04-20.md")
	os.WriteFile(reportPath, []byte("# Test Report\n\nTool usage stats here."), 0644)

	// Setup mock bridge
	bridge := &mockBridge{
		peers: []forge.PeerInfo{
			{ID: "node-1", Name: "Node1"},
			{ID: "node-2", Name: "Node2"},
		},
		clusterRun: true,
	}
	syncer.SetBridge(bridge)

	if !syncer.IsEnabled() {
		t.Fatal("Syncer should be enabled with bridge and running cluster")
	}

	err := syncer.ShareReflection(context.Background(), reportPath)
	if err != nil {
		t.Fatalf("ShareReflection failed: %v", err)
	}

	if bridge.shareCalls != 2 {
		t.Errorf("Expected 2 share calls (2 peers), got %d", bridge.shareCalls)
	}
}

func TestSyncerShareReflectionNoPeers(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	// Create a test report
	refDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(refDir, 0755)
	reportPath := filepath.Join(refDir, "2026-04-20.md")
	os.WriteFile(reportPath, []byte("# Test Report"), 0644)

	// Setup mock bridge with no peers
	bridge := &mockBridge{
		peers:      []forge.PeerInfo{},
		clusterRun: true,
	}
	syncer.SetBridge(bridge)

	err := syncer.ShareReflection(context.Background(), reportPath)
	if err != nil {
		t.Fatalf("ShareReflection should not error with no peers: %v", err)
	}
}

func TestSyncerReceiveReflection(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	payload := map[string]interface{}{
		"content":   "# Remote Reflection\n\nTool stats from peer.",
		"filename":  "2026-04-19.md",
		"from":      "node-peer1",
		"timestamp": time.Now().UTC().Format(time.RFC3339),
	}

	err := syncer.ReceiveReflection(payload)
	if err != nil {
		t.Fatalf("ReceiveReflection failed: %v", err)
	}

	// Verify file was stored
	remoteDir := filepath.Join(tmpDir, "reflections", "remote")
	entries, err := os.ReadDir(remoteDir)
	if err != nil {
		t.Fatalf("Failed to read remote dir: %v", err)
	}
	if len(entries) != 1 {
		t.Fatalf("Expected 1 remote report, got %d", len(entries))
	}
	if !strings.Contains(entries[0].Name(), "node-peer1") {
		t.Errorf("Expected filename to contain 'node-peer1', got: %s", entries[0].Name())
	}

	// Verify content
	content, _ := os.ReadFile(filepath.Join(remoteDir, entries[0].Name()))
	if !strings.Contains(string(content), "Remote reflection from node-peer1") {
		t.Error("Content should contain source metadata header")
	}
}

func TestSyncerReceiveReflectionInvalidPayload(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	// Missing content
	err := syncer.ReceiveReflection(map[string]interface{}{
		"filename": "test.md",
	})
	if err == nil {
		t.Error("Should error on missing content")
	}

	// Empty content
	err = syncer.ReceiveReflection(map[string]interface{}{
		"content": "",
	})
	if err == nil {
		t.Error("Should error on empty content")
	}
}

func TestSyncerGetRemoteReflections(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	// No remote dir yet
	paths, err := syncer.GetRemoteReflections()
	if err != nil {
		t.Fatalf("Should not error: %v", err)
	}
	if len(paths) != 0 {
		t.Errorf("Expected 0 remote reports, got %d", len(paths))
	}

	// Create remote dir with a report
	remoteDir := filepath.Join(tmpDir, "reflections", "remote")
	os.MkdirAll(remoteDir, 0755)
	os.WriteFile(filepath.Join(remoteDir, "remote_report.md"), []byte("content"), 0644)
	os.WriteFile(filepath.Join(remoteDir, "notes.txt"), []byte("not md"), 0644) // Should be excluded

	paths, err = syncer.GetRemoteReflections()
	if err != nil {
		t.Fatalf("Should not error: %v", err)
	}
	if len(paths) != 1 {
		t.Errorf("Expected 1 remote report (only .md), got %d", len(paths))
	}
}

// === Sanitizer Tests ===

func TestSanitizerSanitizeReport(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	sanitizer := forge.NewReportSanitizer(cfg)

	content := "api_key: sk-12345abcdef\ntoken: bearer-xyz\npassword: mysecret123"
	result := sanitizer.SanitizeReport(content)

	if strings.Contains(result, "sk-12345abcdef") {
		t.Error("API key should be redacted")
	}
	if strings.Contains(result, "bearer-xyz") {
		t.Error("Token should be redacted")
	}
	if strings.Contains(result, "mysecret123") {
		t.Error("Password should be redacted")
	}
	if !strings.Contains(result, "[REDACTED]") {
		t.Error("Should contain [REDACTED] replacement")
	}
}

func TestSanitizerRedactSecrets(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	sanitizer := forge.NewReportSanitizer(cfg)

	tests := []struct {
		name     string
		input    string
		badSubstr string
	}{
		{"api_key colon", "api_key: sk-abc123", "sk-abc123"},
		{"api_key equals", "api_key=sk-abc123", "sk-abc123"},
		{"token quoted", `token: "my-secret-token"`, "my-secret-token"},
		{"password colon", "password: p@ssw0rd!", "p@ssw0rd"},
		{"secret colon", "secret: abc123def", "abc123def"},
		{"credential equals", "credential=mycreds", "mycreds"},
		{"key colon", "key: value123", "value123"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := sanitizer.SanitizeReport(tt.input)
			if strings.Contains(result, tt.badSubstr) {
				t.Errorf("Sensitive value '%s' should be redacted in: %s", tt.badSubstr, result)
			}
		})
	}
}

func TestSanitizerCleanPaths(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	sanitizer := forge.NewReportSanitizer(cfg)

	tests := []struct {
		name     string
		input    string
		expected string
		badSubstr string
	}{
		{
			name:     "windows user path",
			input:    "File at C:\\Users\\john\\documents\\config.json",
			badSubstr: "C:\\Users\\john",
		},
		{
			name:     "unix home path",
			input:    "File at /home/alice/workspace/test.json",
			badSubstr: "/home/alice",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := sanitizer.SanitizeReport(tt.input)
			if strings.Contains(result, tt.badSubstr) {
				t.Errorf("Path should be cleaned, still contains '%s': %s", tt.badSubstr, result)
			}
			if !strings.Contains(result, "~/") {
				t.Errorf("Path should be replaced with ~/: %s", result)
			}
		})
	}
}

func TestSanitizerCleanIPs(t *testing.T) {
	cfg := forge.DefaultForgeConfig()
	sanitizer := forge.NewReportSanitizer(cfg)

	// Public IPs should be replaced
	input := "Server at 203.0.113.50 and backup at 198.51.100.25"
	result := sanitizer.SanitizeReport(input)
	if strings.Contains(result, "203.0.113.50") {
		t.Error("Public IP should be replaced")
	}
	if strings.Contains(result, "198.51.100.25") {
		t.Error("Public IP should be replaced")
	}

	// Private IPs should be preserved
	privateInput := "Internal at 192.168.1.100 and 10.0.0.1 and 172.16.0.1 and 127.0.0.1"
	privateResult := sanitizer.SanitizeReport(privateInput)
	if !strings.Contains(privateResult, "192.168.1.100") {
		t.Error("Private IP 192.168.x.x should be preserved")
	}
	if !strings.Contains(privateResult, "10.0.0.1") {
		t.Error("Private IP 10.x.x.x should be preserved")
	}
	if !strings.Contains(privateResult, "172.16.0.1") {
		t.Error("Private IP 172.16.x.x should be preserved")
	}
	if !strings.Contains(privateResult, "127.0.0.1") {
		t.Error("Loopback 127.x.x.x should be preserved")
	}
}

// === Reflector MergeRemoteReflections Tests ===

func TestReflectorMergeRemoteReflections(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	cfg.Reflection.MinExperiences = 1

	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	// Seed local data
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:local1",
		ToolName:    "read_file",
		Count:       10,
		SuccessRate: 0.9,
		LastSeen:    time.Now().UTC(),
	})
	store.AppendAggregated(&forge.AggregatedExperience{
		PatternHash: "sha256:local2",
		ToolName:    "exec",
		Count:       5,
		SuccessRate: 0.8,
		LastSeen:    time.Now().UTC(),
	})

	// Create remote report with table format
	remoteDir := filepath.Join(tmpDir, "remote")
	os.MkdirAll(remoteDir, 0755)
	remoteReport := filepath.Join(remoteDir, "2026-04-19.md")
	os.WriteFile(remoteReport, []byte("# Remote Report\n\n| read_file | 15 |\n| exec | 8 |\n| web_search | 3 |\n"), 0644)

	merged := reflector.MergeRemoteReflections([]string{remoteReport})

	if merged == nil {
		t.Fatal("MergeRemoteReflections should return non-nil")
	}

	// Should have local patterns
	if len(merged.LocalPatterns) == 0 {
		t.Error("Should have local patterns")
	}

	// Should have merged patterns (local + unique remote)
	if len(merged.MergedPatterns) < 2 {
		t.Errorf("Expected at least 2 merged patterns, got %d", len(merged.MergedPatterns))
	}

	// Should have some common tools (read_file is in both)
	if len(merged.CommonTools) == 0 {
		t.Error("Should have at least one common tool (read_file)")
	}
}

func TestReflectorMergeRemoteEmpty(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()

	store := forge.NewExperienceStore(tmpDir, cfg)
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	reflector := forge.NewReflector(tmpDir, store, registry, cfg)

	// Empty remote reports
	merged := reflector.MergeRemoteReflections(nil)

	if merged == nil {
		t.Fatal("Should return non-nil even with empty input")
	}
	if len(merged.RemotePatterns) != 0 {
		t.Error("Remote patterns should be empty")
	}
}

// === forge_share Tool Tests ===

func TestForgeShareTool(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := forge.NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	// Create a reflection report
	refDir := filepath.Join(workspace, "forge", "reflections")
	os.MkdirAll(refDir, 0755)
	os.WriteFile(filepath.Join(refDir, "2026-04-20.md"), []byte("# Test Report\n\nStats here."), 0644)

	// Setup mock bridge
	bridge := &mockBridge{
		peers:      []forge.PeerInfo{{ID: "node-1", Name: "Peer1"}},
		clusterRun: true,
	}
	f.SetBridge(bridge)

	// Find and execute the forge_share tool
	ftools := forge.NewForgeTools(f)
	for _, tool := range ftools {
		if tool.Name() == "forge_share" {
			result := tool.Execute(context.Background(), map[string]interface{}{})
			if result.IsError {
				t.Errorf("forge_share should succeed: %s", result.ForLLM)
			}
			if !strings.Contains(result.ForLLM, "2026-04-20.md") {
				t.Errorf("Result should mention report name: %s", result.ForLLM)
			}
			return
		}
	}
	t.Fatal("forge_share tool not found")
}

func TestForgeShareToolNoBridge(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, err := forge.NewForge(workspace, nil)
	if err != nil {
		t.Fatalf("NewForge failed: %v", err)
	}

	// No bridge set

	ftools := forge.NewForgeTools(f)
	for _, tool := range ftools {
		if tool.Name() == "forge_share" {
			result := tool.Execute(context.Background(), map[string]interface{}{})
			if result.IsError {
				t.Errorf("forge_share without bridge should not be error, just info: %s", result.ForLLM)
			}
			if !strings.Contains(result.ForLLM, "未启用") {
				t.Errorf("Should mention cluster not enabled: %s", result.ForLLM)
			}
			return
		}
	}
	t.Fatal("forge_share tool not found")
}

// === Forge Tool Count Test (updated for 7 tools) ===

func TestForgeToolsCountWithShare(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := forge.NewForge(workspace, nil)
	ftools := forge.NewForgeTools(f)

	if len(ftools) != 8 {
		t.Fatalf("Expected 8 tools (including forge_share), got %d", len(ftools))
	}

	expectedNames := []string{"forge_reflect", "forge_create", "forge_update", "forge_list", "forge_evaluate", "forge_build_mcp", "forge_share", "forge_learning_status"}
	for i, expected := range expectedNames {
		if ftools[i].Name() != expected {
			t.Errorf("Tool %d: expected name '%s', got '%s'", i, expected, ftools[i].Name())
		}
	}
}

// === Forge End-to-End Share + Receive Test ===

func TestForgeShareAndReceiveE2E(t *testing.T) {
	tmpDir := t.TempDir()

	// === Node A (sender) ===
	workspaceA := filepath.Join(tmpDir, "nodeA", "workspace")
	os.MkdirAll(workspaceA, 0755)
	forgeA, _ := forge.NewForge(workspaceA, nil)

	// Create a reflection report on A
	refDirA := filepath.Join(workspaceA, "forge", "reflections")
	os.MkdirAll(refDirA, 0755)
	reportContent := "# Reflection\n\napi_key: sk-secret123\nPath: C:\\Users\\admin\\file.txt\nIP: 203.0.113.50\n"
	os.WriteFile(filepath.Join(refDirA, "2026-04-20.md"), []byte(reportContent), 0644)

	// === Node B (receiver) ===
	workspaceB := filepath.Join(tmpDir, "nodeB", "workspace")
	os.MkdirAll(workspaceB, 0755)
	forgeB, _ := forge.NewForge(workspaceB, nil)
	syncerB := forgeB.GetSyncer()

	// Capture what would be sent
	var sentContent string
	bridge := &mockBridge{
		peers:      []forge.PeerInfo{{ID: "nodeB", Name: "NodeB"}},
		clusterRun: true,
		shareFunc: func(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
			// Simulate the payload being sent to B
			if c, ok := payload["content"].(string); ok {
				sentContent = c
				// Simulate B receiving the reflection
				payload["from"] = "nodeA"
				syncerB.ReceiveReflection(payload)
			}
			return []byte(`{"status":"ok"}`), nil
		},
	}
	forgeA.SetBridge(bridge)

	// Share from A
	syncerA := forgeA.GetSyncer()
	err := syncerA.ShareReflection(context.Background(), filepath.Join(refDirA, "2026-04-20.md"))
	if err != nil {
		t.Fatalf("ShareReflection failed: %v", err)
	}

	// Verify sanitization happened
	if strings.Contains(sentContent, "sk-secret123") {
		t.Error("Sent content should have API key redacted")
	}
	if strings.Contains(sentContent, "admin") {
		t.Error("Sent content should have username removed from path")
	}
	if strings.Contains(sentContent, "203.0.113.50") {
		t.Error("Sent content should have public IP replaced")
	}

	// Verify B received the report
	remotePaths, _ := syncerB.GetRemoteReflections()
	if len(remotePaths) != 1 {
		t.Fatalf("Expected 1 remote report on B, got %d", len(remotePaths))
	}

	// Verify content on B side
	bContent, _ := os.ReadFile(remotePaths[0])
	bStr := string(bContent)
	if !strings.Contains(bStr, "Remote reflection from nodeA") {
		t.Error("B should have metadata header from A")
	}
	if !strings.Contains(bStr, "Reflection") {
		t.Error("B should have report content")
	}
}

// === Forge GetSyncer / ReceiveReflection via Forge struct ===

func TestForgeReceiveReflection(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := forge.NewForge(workspace, nil)

	err := f.ReceiveReflection(map[string]interface{}{
		"content":   "# Remote report",
		"filename":  "remote_test.md",
		"from":      "node-test",
		"timestamp": time.Now().UTC().Format(time.RFC3339),
	})
	if err != nil {
		t.Fatalf("Forge.ReceiveReflection failed: %v", err)
	}

	syncer := f.GetSyncer()
	paths, _ := syncer.GetRemoteReflections()
	if len(paths) != 1 {
		t.Errorf("Expected 1 remote reflection, got %d", len(paths))
	}
}

// === Syncer ShareReflection with specific report path ===

func TestForgeShareToolWithSpecificReport(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := forge.NewForge(workspace, nil)

	// Create specific report
	refDir := filepath.Join(workspace, "forge", "reflections")
	os.MkdirAll(refDir, 0755)
	specificReport := filepath.Join(refDir, "2026-04-19.md")
	os.WriteFile(specificReport, []byte("# Specific report"), 0644)

	bridge := &mockBridge{
		peers:      []forge.PeerInfo{{ID: "p1", Name: "P1"}},
		clusterRun: true,
	}
	f.SetBridge(bridge)

	ftools := forge.NewForgeTools(f)
	for _, tool := range ftools {
		if tool.Name() == "forge_share" {
			result := tool.Execute(context.Background(), map[string]interface{}{
				"report_path": specificReport,
			})
			if result.IsError {
				t.Errorf("Should succeed: %s", result.ForLLM)
			}
			if !strings.Contains(result.ForLLM, "2026-04-19.md") {
				t.Errorf("Should mention specific report: %s", result.ForLLM)
			}
			return
		}
	}
	t.Fatal("forge_share tool not found")
}

// Verify forge_share tool implements the Tool interface correctly
func TestForgeShareToolInterface(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := forge.NewForge(workspace, nil)
	ftools := forge.NewForgeTools(f)

	var shareTool tools.Tool
	for _, tool := range ftools {
		if tool.Name() == "forge_share" {
			shareTool = tool
			break
		}
	}
	if shareTool == nil {
		t.Fatal("forge_share tool not found")
	}

	if shareTool.Description() == "" {
		t.Error("forge_share should have a description")
	}

	params := shareTool.Parameters()
	if params == nil {
		t.Error("forge_share should have parameters")
	}
	if params["type"] != "object" {
		t.Error("Parameters type should be 'object'")
	}
}

// === Security: Path Traversal Prevention ===

func TestSyncerReadReflectionContentPathTraversal(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	// Create a legitimate report
	refDir := filepath.Join(tmpDir, "reflections")
	os.MkdirAll(refDir, 0755)
	os.WriteFile(filepath.Join(refDir, "2026-04-20.md"), []byte("legit content"), 0644)

	// Path traversal attempts should fail
	traversalAttempts := []string{
		"../etc/passwd",
		"..\\windows\\system32\\config\\sam",
		"../../secret.txt",
		"../../../tmp/evil",
	}
	for _, attempt := range traversalAttempts {
		_, err := syncer.ReadReflectionContent(attempt)
		if err == nil {
			t.Errorf("Path traversal should be blocked: %s", attempt)
		}
	}

	// Valid read should work
	content, err := syncer.ReadReflectionContent("2026-04-20.md")
	if err != nil {
		t.Fatalf("Valid read should work: %v", err)
	}
	if content != "legit content" {
		t.Errorf("Content mismatch: got %q", content)
	}
}

// === Security: Filename Sanitization ===

func TestSyncerReceiveReflectionFilenameSanitization(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	// Try to inject path separators in filename
	payload := map[string]interface{}{
		"content":   "# Evil report",
		"filename": "../../evil.md",
		"from":     "node-test",
		"timestamp": time.Now().UTC().Format(time.RFC3339),
	}

	err := syncer.ReceiveReflection(payload)
	if err != nil {
		t.Fatalf("ReceiveReflection should not fail: %v", err)
	}

	// Verify the file was stored in remote dir, not escaped
	remoteDir := filepath.Join(tmpDir, "reflections", "remote")
	entries, _ := os.ReadDir(remoteDir)
	if len(entries) != 1 {
		t.Fatalf("Expected 1 file in remote dir, got %d", len(entries))
	}

	// The filename should be sanitized to just "evil.md" (filepath.Base strips ../)
	if entries[0].Name() != "node-test_evil.md" {
		t.Errorf("Filename should be sanitized, got: %s", entries[0].Name())
	}

	// Verify no file exists outside the remote dir
	if _, err := os.Stat(filepath.Join(tmpDir, "evil.md")); err == nil {
		t.Error("File should NOT exist at traversed path")
	}
}

// === IP Parsing: Multi-digit octets ===

func TestSanitizerIPWithMultiDigitOctets(t *testing.T) {
	// 172.16.x.x should be private
	if !isPrivateIPPure("172.16.0.1") {
		t.Error("172.16.0.1 should be private")
	}
	// 172.31.x.x should be private
	if !isPrivateIPPure("172.31.255.255") {
		t.Error("172.31.255.255 should be private")
	}
	// 172.32.x.x should NOT be private
	if isPrivateIPPure("172.32.0.1") {
		t.Error("172.32.0.1 should NOT be private")
	}
	// 172.15.x.x should NOT be private
	if isPrivateIPPure("172.15.0.1") {
		t.Error("172.15.0.1 should NOT be private")
	}
	// 172.200.x.x should NOT be private
	if isPrivateIPPure("172.200.0.1") {
		t.Error("172.200.0.1 should NOT be private")
	}
}

// Helper to test the actual IP logic through the sanitizer
func isPrivateIPPure(ip string) bool {
	cfg := forge.DefaultForgeConfig()
	s := forge.NewReportSanitizer(cfg)
	result := s.SanitizeReport("IP: " + ip)
	return strings.Contains(result, ip)
}

// === Security: forge_share tool report_path validation ===

func TestForgeShareToolRejectsArbitraryPath(t *testing.T) {
	tmpDir := t.TempDir()
	workspace := filepath.Join(tmpDir, "workspace")
	os.MkdirAll(workspace, 0755)

	f, _ := forge.NewForge(workspace, nil)

	// Create a sensitive file outside reflections dir
	sensitiveFile := filepath.Join(tmpDir, "secret.txt")
	os.WriteFile(sensitiveFile, []byte("secret data"), 0644)

	// Create reflections dir with a valid report
	refDir := filepath.Join(workspace, "forge", "reflections")
	os.MkdirAll(refDir, 0755)
	os.WriteFile(filepath.Join(refDir, "2026-04-20.md"), []byte("valid report"), 0644)

	// Setup bridge
	bridge := &mockBridge{
		peers:      []forge.PeerInfo{{ID: "p1", Name: "P1"}},
		clusterRun: true,
	}
	f.SetBridge(bridge)

	ftools := forge.NewForgeTools(f)
	for _, tool := range ftools {
		if tool.Name() == "forge_share" {
			// Try to share a file outside reflections dir
			result := tool.Execute(context.Background(), map[string]interface{}{
				"report_path": sensitiveFile,
			})
			if !result.IsError {
				t.Error("Should reject path outside reflections directory")
			}
			if !strings.Contains(result.ForLLM, "reflections") {
				t.Errorf("Error should mention reflections: %s", result.ForLLM)
			}
			return
		}
	}
	t.Fatal("forge_share tool not found")
}

// === SanitizeContent method ===

func TestSyncerSanitizeContent(t *testing.T) {
	tmpDir := t.TempDir()
	cfg := forge.DefaultForgeConfig()
	registry := forge.NewRegistry(filepath.Join(tmpDir, "registry.json"))
	syncer := forge.NewSyncer(tmpDir, registry, cfg)

	content := "api_key: sk-test123\nPath: C:\\Users\\admin\\file.txt\nIP: 203.0.113.50\n"
	sanitized := syncer.SanitizeContent(content)

	if strings.Contains(sanitized, "sk-test123") {
		t.Error("SanitizeContent should redact secrets")
	}
	if strings.Contains(sanitized, "admin") {
		t.Error("SanitizeContent should clean paths")
	}
	if strings.Contains(sanitized, "203.0.113.50") {
		t.Error("SanitizeContent should clean public IPs")
	}
}
