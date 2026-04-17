// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/276793422/NemesisBot/module/utils"
)

const (
	defaultFileMaxSize = 10 * 1024 * 1024 // 10 MB per file
)

// DownloadSkillTreeFromGitHub downloads all files in a skill directory from GitHub
// using the Trees API. It lists the full tree, filters for files under the given
// directory prefix, and downloads each file individually.
//
// Parameters:
//   - apiBaseURL: GitHub API base URL (e.g. "https://api.github.com")
//   - rawBaseURL: raw content base URL (e.g. "https://raw.githubusercontent.com")
//   - repo: repository in "owner/repo" format
//   - branch: branch name (e.g. "main")
//   - dirPrefix: directory prefix to filter (e.g. "skills/pdf" or "skills/owner/slug")
//   - targetDir: local directory to write files into
//   - maxFileSize: maximum size per file in bytes (0 = use default 10MB)
func DownloadSkillTreeFromGitHub(ctx context.Context, client *http.Client,
	apiBaseURL, rawBaseURL, repo, branch, dirPrefix, targetDir string,
	maxFileSize int64,
) error {
	// Ensure trailing slash for consistent prefix matching
	if !strings.HasSuffix(dirPrefix, "/") {
		dirPrefix += "/"
	}

	if maxFileSize <= 0 {
		maxFileSize = defaultFileMaxSize
	}

	apiURL := fmt.Sprintf("%s/repos/%s/git/trees/%s?recursive=1", apiBaseURL, repo, branch)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, apiURL, nil)
	if err != nil {
		return fmt.Errorf("failed to create trees request: %w", err)
	}
	req.Header.Set("Accept", "application/vnd.github.v3+json")

	resp, err := utils.DoRequestWithRetry(client, req)
	if err != nil {
		return fmt.Errorf("failed to fetch tree: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		return fmt.Errorf("GitHub Trees API HTTP %d: %s", resp.StatusCode, string(body))
	}

	// Stream-decode the tree response
	blobPaths, err := decodeTreeBlobPaths(ctx, resp.Body, dirPrefix)
	if err != nil {
		return err
	}

	if len(blobPaths) == 0 {
		return fmt.Errorf("no files found under %s in %s", dirPrefix, repo)
	}

	// Download each file and write to targetDir
	if err := os.MkdirAll(targetDir, 0o755); err != nil {
		return fmt.Errorf("failed to create target directory: %w", err)
	}

	for _, blobPath := range blobPaths {
		relativePath := strings.TrimPrefix(blobPath, dirPrefix)
		if relativePath == "" {
			continue
		}

		destPath := filepath.Join(targetDir, relativePath)

		// Security: ensure destPath is within targetDir
		if !utils.IsPathWithinDir(destPath, targetDir) {
			return fmt.Errorf("path traversal detected: %s", relativePath)
		}

		// Create parent directory
		if err := os.MkdirAll(filepath.Dir(destPath), 0o755); err != nil {
			return fmt.Errorf("failed to create directory for %s: %w", relativePath, err)
		}

		rawURL := fmt.Sprintf("%s/%s/%s/%s", rawBaseURL, repo, branch, blobPath)
		data, err := DownloadFile(ctx, client, rawURL, maxFileSize)
		if err != nil {
			return fmt.Errorf("failed to download %s: %w", relativePath, err)
		}

		if err := os.WriteFile(destPath, data, 0o644); err != nil {
			return fmt.Errorf("failed to write %s: %w", relativePath, err)
		}
	}

	return nil
}

// decodeTreeBlobPaths stream-decodes a GitHub Trees API response and returns
// the paths of all blob entries that start with dirPrefix.
func decodeTreeBlobPaths(ctx context.Context, body io.Reader, dirPrefix string) ([]string, error) {
	decoder := json.NewDecoder(body)

	if t, err := decoder.Token(); err != nil {
		return nil, fmt.Errorf("failed to read tree response: %w", err)
	} else if t != json.Delim('{') {
		return nil, fmt.Errorf("expected JSON object, got %v", t)
	}

	type treeEntry struct {
		Path string `json:"path"`
		Type string `json:"type"`
	}

	var blobPaths []string

	for decoder.More() {
		keyToken, err := decoder.Token()
		if err != nil {
			break
		}
		key, _ := keyToken.(string)

		switch key {
		case "tree":
			if t, err := decoder.Token(); err != nil {
				break
			} else if t != json.Delim('[') {
				break
			}

			for decoder.More() {
				// Check context cancellation
				select {
				case <-ctx.Done():
					return nil, fmt.Errorf("tree decode cancelled: %w", ctx.Err())
				default:
				}

				var entry treeEntry
				if err := decoder.Decode(&entry); err != nil {
					continue
				}
				if entry.Type != "blob" {
					continue
				}
				if !strings.HasPrefix(entry.Path, dirPrefix) {
					continue
				}
				blobPaths = append(blobPaths, entry.Path)
			}
			decoder.Token() // consume ]

		default:
			var discard interface{}
			decoder.Decode(&discard)
		}
	}

	return blobPaths, nil
}

// DownloadFile downloads a single file from a URL with size limit.
func DownloadFile(ctx context.Context, client *http.Client, url string, maxSize int64) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	resp, err := utils.DoRequestWithRetry(client, req)
	if err != nil {
		return nil, fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return nil, fmt.Errorf("HTTP %d: %s", resp.StatusCode, string(body))
	}

	body, err := io.ReadAll(io.LimitReader(resp.Body, maxSize))
	if err != nil {
		return nil, fmt.Errorf("failed to read response: %w", err)
	}

	return body, nil
}
