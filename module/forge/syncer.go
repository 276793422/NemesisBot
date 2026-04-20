package forge

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// PeerInfo holds basic information about a cluster peer node.
type PeerInfo struct {
	ID   string
	Name string
}

// ClusterForgeBridge is the interface for Forge to communicate with the cluster
// without importing the cluster package directly (avoids circular dependency).
type ClusterForgeBridge interface {
	// ShareToPeer sends an RPC call to a specific peer.
	ShareToPeer(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error)
	// GetOnlinePeers returns currently online peer nodes.
	GetOnlinePeers() []PeerInfo
	// IsClusterEnabled returns true if the cluster subsystem is running.
	IsClusterEnabled() bool
}

// Syncer handles sharing reflection reports across cluster nodes.
type Syncer struct {
	forgeDir string
	registry *Registry
	config   *ForgeConfig
	sanitizer *ReportSanitizer
	bridge   ClusterForgeBridge
	mu       sync.Mutex
}

// NewSyncer creates a new Syncer instance.
func NewSyncer(forgeDir string, registry *Registry, config *ForgeConfig) *Syncer {
	return &Syncer{
		forgeDir:  forgeDir,
		registry:  registry,
		config:    config,
		sanitizer: NewReportSanitizer(config),
	}
}

// SetBridge injects the cluster bridge for RPC communication.
func (s *Syncer) SetBridge(bridge ClusterForgeBridge) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.bridge = bridge
}

// IsEnabled returns true if the syncer can share reflections (bridge present + cluster running).
func (s *Syncer) IsEnabled() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.bridge != nil && s.bridge.IsClusterEnabled()
}

// ShareReflection reads a reflection report, sanitizes it, and broadcasts to all online peers.
func (s *Syncer) ShareReflection(ctx context.Context, reportPath string) error {
	if !s.IsEnabled() {
		return fmt.Errorf("cluster sharing is not enabled")
	}

	// Read the report
	content, err := os.ReadFile(reportPath)
	if err != nil {
		return fmt.Errorf("failed to read report: %w", err)
	}

	// Sanitize sensitive information
	sanitized := s.sanitizer.SanitizeReport(string(content))

	// Get online peers
	s.mu.Lock()
	peers := s.bridge.GetOnlinePeers()
	bridge := s.bridge
	s.mu.Unlock()

	if len(peers) == 0 {
		logger.InfoC("forge", "No online peers to share reflection with")
		return nil
	}

	// Build payload
	filename := filepath.Base(reportPath)
	payload := map[string]interface{}{
		"content":       sanitized,
		"filename":      filename,
		"timestamp":     time.Now().UTC().Format(time.RFC3339),
	}

	successCount := 0
	for _, peer := range peers {
		ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
		_, err := bridge.ShareToPeer(ctx, peer.ID, "forge_share", payload)
		cancel()
		if err != nil {
			logger.WarnCF("forge", "Failed to share reflection with peer", map[string]interface{}{
				"peer_id": peer.ID,
				"error":   err.Error(),
			})
			continue
		}
		successCount++
	}

	logger.InfoCF("forge", "Reflection shared with peers", map[string]interface{}{
		"report":  filename,
		"success": successCount,
		"total":   len(peers),
	})

	return nil
}

// ReceiveReflection receives and stores a remote reflection report.
func (s *Syncer) ReceiveReflection(payload map[string]interface{}) error {
	content, ok := payload["content"].(string)
	if !ok || content == "" {
		return fmt.Errorf("invalid or missing 'content' in payload")
	}

	filename, _ := payload["filename"].(string)
	if filename == "" {
		filename = fmt.Sprintf("remote_%s.md", time.Now().UTC().Format("2006-01-02_150405"))
	}
	// Sanitize filename: strip any path separators to prevent directory traversal
	filename = filepath.Base(filename)
	if filename == "." || filename == ".." {
		filename = fmt.Sprintf("remote_%s.md", time.Now().UTC().Format("2006-01-02_150405"))
	}

	from, _ := payload["from"].(string)
	// Sanitize 'from' field as well
	from = sanitizeNodeID(from)
	timestamp, _ := payload["timestamp"].(string)

	// Ensure remote directory exists
	remoteDir := filepath.Join(s.forgeDir, "reflections", "remote")
	if err := os.MkdirAll(remoteDir, 0755); err != nil {
		return fmt.Errorf("failed to create remote reflections dir: %w", err)
	}

	// Prefix filename with source node to avoid collisions
	if from != "" {
		filename = fmt.Sprintf("%s_%s", from, filename)
	}

	// Add metadata header
	header := fmt.Sprintf("<!-- Remote reflection from %s at %s -->\n", from, timestamp)
	fullContent := header + content

	destPath := filepath.Join(remoteDir, filename)
	if err := os.WriteFile(destPath, []byte(fullContent), 0644); err != nil {
		return fmt.Errorf("failed to write remote report: %w", err)
	}

	logger.InfoCF("forge", "Received remote reflection", map[string]interface{}{
		"from":     from,
		"filename": filename,
	})

	return nil
}

// GetRemoteReflections returns file paths of all remote reflection reports.
func (s *Syncer) GetRemoteReflections() ([]string, error) {
	remoteDir := filepath.Join(s.forgeDir, "reflections", "remote")
	entries, err := os.ReadDir(remoteDir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}

	var paths []string
	for _, entry := range entries {
		if !entry.IsDir() && filepath.Ext(entry.Name()) == ".md" {
			paths = append(paths, filepath.Join(remoteDir, entry.Name()))
		}
	}
	return paths, nil
}

// GetLocalReflections returns file paths of all local reflection reports for sharing.
func (s *Syncer) GetLocalReflections() ([]string, error) {
	reflectionsDir := filepath.Join(s.forgeDir, "reflections")
	entries, err := os.ReadDir(reflectionsDir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}

	var paths []string
	for _, entry := range entries {
		if !entry.IsDir() && filepath.Ext(entry.Name()) == ".md" {
			paths = append(paths, filepath.Join(reflectionsDir, entry.Name()))
		}
	}
	return paths, nil
}

// GetReflectionsListPayload returns a serializable list of available local reflections.
func (s *Syncer) GetReflectionsListPayload() map[string]interface{} {
	paths, err := s.GetLocalReflections()
	if err != nil {
		return map[string]interface{}{
			"reflections": []string{},
			"error":       err.Error(),
		}
	}

	filenames := make([]string, len(paths))
	for i, p := range paths {
		filenames[i] = filepath.Base(p)
	}

	return map[string]interface{}{
		"reflections": filenames,
		"count":       len(filenames),
	}
}

// ReadReflectionContent reads a specific reflection report content.
func (s *Syncer) ReadReflectionContent(filename string) (string, error) {
	// Sanitize filename: strip path components
	filename = filepath.Base(filename)
	if filename == "." || filename == ".." {
		return "", fmt.Errorf("invalid filename: %s", filename)
	}

	// Only allow reading from local reflections directory
	path := filepath.Join(s.forgeDir, "reflections", filename)
	absPath, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	reflectionsDir := filepath.Join(s.forgeDir, "reflections")
	absDir, err := filepath.Abs(reflectionsDir)
	if err != nil {
		return "", err
	}
	// Security: ensure the resolved path is within the reflections directory
	if !strings.HasPrefix(absPath, absDir+string(filepath.Separator)) {
		return "", fmt.Errorf("invalid path: %s", filename)
	}

	content, err := os.ReadFile(absPath)
	if err != nil {
		return "", err
	}
	return string(content), nil
}

// SanitizeContent sanitizes reflection content before sharing with remote peers.
func (s *Syncer) SanitizeContent(content string) string {
	return s.sanitizer.SanitizeReport(content)
}

// marshalPayload is a helper for tests to serialize data.
func marshalPayload(data map[string]interface{}) []byte {
	b, _ := json.Marshal(data)
	return b
}

// sanitizeNodeID strips unsafe characters from a node ID used in filenames.
func sanitizeNodeID(id string) string {
	id = strings.Map(func(r rune) rune {
		if (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') || r == '-' || r == '_' {
			return r
		}
		return '_'
	}, id)
	if id == "" {
		id = "unknown"
	}
	return id
}
