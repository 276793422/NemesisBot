// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package skills

import (
	"archive/zip"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// ClawHubRegistry implements the SkillRegistry interface for ClawHub.ai.
type ClawHubRegistry struct {
	baseURL         string
	authToken       string
	searchPath      string
	skillsPath      string
	downloadPath    string
	timeout         time.Duration
	maxZipSize      int64
	maxResponseSize int64
	client          *http.Client
}

// NewClawHubRegistry creates a new ClawHub registry client.
func NewClawHubRegistry(cfg ClawHubConfig) *ClawHubRegistry {
	timeout := 30 * time.Second
	if cfg.Timeout > 0 {
		timeout = time.Duration(cfg.Timeout) * time.Second
	}

	maxZipSize := int64(50 * 1024 * 1024) // 50MB default
	if cfg.MaxZipSize > 0 {
		maxZipSize = int64(cfg.MaxZipSize)
	}

	maxResponseSize := int64(2 * 1024 * 1024) // 2MB default
	if cfg.MaxResponseSize > 0 {
		maxResponseSize = int64(cfg.MaxResponseSize)
	}

	baseURL := cfg.BaseURL
	if baseURL == "" {
		baseURL = "https://clawhub.ai"
	}

	searchPath := cfg.SearchPath
	if searchPath == "" {
		searchPath = "/api/v1/search"
	}

	skillsPath := cfg.SkillsPath
	if skillsPath == "" {
		skillsPath = "/api/v1/skills"
	}

	downloadPath := cfg.DownloadPath
	if downloadPath == "" {
		downloadPath = "/api/v1/download"
	}

	return &ClawHubRegistry{
		baseURL:         baseURL,
		authToken:       cfg.AuthToken,
		searchPath:      searchPath,
		skillsPath:      skillsPath,
		downloadPath:    downloadPath,
		timeout:         timeout,
		maxZipSize:      maxZipSize,
		maxResponseSize: maxResponseSize,
		client: &http.Client{
			Timeout: timeout,
		},
	}
}

// Name returns the registry name.
func (r *ClawHubRegistry) Name() string {
	return "clawhub"
}

// Search searches the ClawHub registry for skills matching the query.
func (r *ClawHubRegistry) Search(ctx context.Context, query string, limit int) ([]SearchResult, error) {
	// Build search URL
	searchURL := r.baseURL + r.searchPath
	reqURL, err := url.Parse(searchURL)
	if err != nil {
		return nil, fmt.Errorf("failed to parse search URL: %w", err)
	}

	// Add query parameters
	params := reqURL.Query()
	params.Set("q", query)
	if limit > 0 {
		params.Set("limit", fmt.Sprintf("%d", limit))
	}
	reqURL.RawQuery = params.Encode()

	// Create request
	req, err := http.NewRequestWithContext(ctx, "GET", reqURL.String(), nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	// Add auth token if provided
	if r.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+r.authToken)
	}

	// Execute request with retry
	resp, err := r.doRequestWithRetry(req)
	if err != nil {
		return nil, fmt.Errorf("failed to execute search request: %w", err)
	}
	defer resp.Body.Close()

	// Check response size
	if resp.ContentLength > r.maxResponseSize {
		return nil, fmt.Errorf("response too large: %d bytes", resp.ContentLength)
	}

	// Check status code
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 1024))
		return nil, fmt.Errorf("search request failed with status %d: %s", resp.StatusCode, string(body))
	}

	// Parse response
	var response struct {
		Results []struct {
			Score       float64 `json:"score"`
			Slug        string  `json:"slug"`
			DisplayName string  `json:"display_name"`
			Summary     string  `json:"summary"`
			Version     string  `json:"version"`
		} `json:"results"`
	}

	if err := json.NewDecoder(io.LimitReader(resp.Body, r.maxResponseSize)).Decode(&response); err != nil {
		return nil, fmt.Errorf("failed to parse search response: %w", err)
	}

	// Convert to SearchResult
	results := make([]SearchResult, len(response.Results))
	for i, r := range response.Results {
		results[i] = SearchResult{
			Score:        r.Score,
			Slug:         r.Slug,
			DisplayName:  r.DisplayName,
			Summary:      r.Summary,
			Version:      r.Version,
			RegistryName: "clawhub",
		}
	}

	return results, nil
}

// GetSkillMeta retrieves metadata for a specific skill by slug.
func (r *ClawHubRegistry) GetSkillMeta(ctx context.Context, slug string) (*SkillMeta, error) {
	// Validate slug to prevent path traversal
	if err := ValidateSkillIdentifier(slug); err != nil {
		return nil, fmt.Errorf("invalid skill slug: %w", err)
	}

	// Build skills URL
	skillsURL := fmt.Sprintf("%s%s/%s", r.baseURL, r.skillsPath, slug)

	// Create request
	req, err := http.NewRequestWithContext(ctx, "GET", skillsURL, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	// Add auth token if provided
	if r.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+r.authToken)
	}

	// Execute request with retry
	resp, err := r.doRequestWithRetry(req)
	if err != nil {
		return nil, fmt.Errorf("failed to execute metadata request: %w", err)
	}
	defer resp.Body.Close()

	// Check status code
	if resp.StatusCode == http.StatusNotFound {
		return nil, fmt.Errorf("skill '%s' not found", slug)
	}

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 1024))
		return nil, fmt.Errorf("metadata request failed with status %d: %s", resp.StatusCode, string(body))
	}

	// Parse response
	var meta SkillMeta
	if err := json.NewDecoder(resp.Body).Decode(&meta); err != nil {
		return nil, fmt.Errorf("failed to parse metadata response: %w", err)
	}

	meta.RegistryName = "clawhub"
	meta.Slug = slug

	return &meta, nil
}

// DownloadAndInstall fetches metadata, resolves the version, downloads and
// installs the skill to targetDir.
func (r *ClawHubRegistry) DownloadAndInstall(ctx context.Context, slug, version, targetDir string) (*InstallResult, error) {
	// Validate slug to prevent path traversal
	if err := ValidateSkillIdentifier(slug); err != nil {
		return nil, fmt.Errorf("invalid skill slug: %w", err)
	}

	// Get metadata to check for malware
	meta, err := r.GetSkillMeta(ctx, slug)
	if err != nil {
		return nil, fmt.Errorf("failed to get skill metadata: %w", err)
	}

	// Check malware flags
	if meta.IsMalwareBlocked {
		slog.Warn("skill blocked as malware", "slug", slug, "registry", "clawhub")
		return &InstallResult{
			Version:          meta.LatestVersion,
			IsMalwareBlocked: true,
			Summary:          meta.Summary,
		}, nil
	}

	// Build download URL
	downloadURL := fmt.Sprintf("%s%s/%s", r.baseURL, r.downloadPath, slug)
	if version != "" && version != "latest" {
		downloadURL += fmt.Sprintf("?version=%s", version)
	}

	// Create request
	req, err := http.NewRequestWithContext(ctx, "GET", downloadURL, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create download request: %w", err)
	}

	// Add auth token if provided
	if r.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+r.authToken)
	}

	// Execute request with retry
	resp, err := r.doRequestWithRetry(req)
	if err != nil {
		return nil, fmt.Errorf("failed to execute download request: %w", err)
	}
	defer resp.Body.Close()

	// Check status code
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 1024))
		return nil, fmt.Errorf("download request failed with status %d: %s", resp.StatusCode, string(body))
	}

	// Check content length
	if resp.ContentLength > r.maxZipSize {
		return nil, fmt.Errorf("download too large: %d bytes (max: %d)", resp.ContentLength, r.maxZipSize)
	}

	// Create target directory
	if err := os.MkdirAll(targetDir, 0o755); err != nil {
		return nil, fmt.Errorf("failed to create target directory: %w", err)
	}

	// Stream download to temporary file
	tempFile := targetDir + ".tmp.zip"
	tempFD, err := os.Create(tempFile)
	if err != nil {
		return nil, fmt.Errorf("failed to create temp file: %w", err)
	}

	// Track bytes written
	bytesWritten := int64(0)
	buffer := make([]byte, 32*1024) // 32KB buffer

	for {
		n, err := resp.Body.Read(buffer)
		if n > 0 {
			bytesWritten += int64(n)
			if bytesWritten > r.maxZipSize {
				tempFD.Close()
				os.Remove(tempFile)
				return nil, fmt.Errorf("download exceeded maximum size: %d bytes", bytesWritten)
			}

			if _, writeErr := tempFD.Write(buffer[:n]); writeErr != nil {
				tempFD.Close()
				os.Remove(tempFile)
				return nil, fmt.Errorf("failed to write to temp file: %w", writeErr)
			}
		}

		if err == io.EOF {
			break
		}

		if err != nil {
			tempFD.Close()
			os.Remove(tempFile)
			return nil, fmt.Errorf("failed to read response body: %w", err)
		}
	}

	tempFD.Close()

	// Extract ZIP file
	if err := r.extractZipFile(tempFile, targetDir); err != nil {
		os.Remove(tempFile)
		return nil, fmt.Errorf("failed to extract ZIP file: %w", err)
	}

	// Clean up temp file
	os.Remove(tempFile)

	// Determine installed version
	installedVersion := meta.LatestVersion
	if version != "" && version != "latest" {
		installedVersion = version
	}

	return &InstallResult{
		Version:          installedVersion,
		IsMalwareBlocked: meta.IsMalwareBlocked,
		IsSuspicious:     meta.IsSuspicious,
		Summary:          meta.Summary,
	}, nil
}

// extractZipFile extracts a ZIP file to the target directory.
func (r *ClawHubRegistry) extractZipFile(zipPath, targetDir string) error {
	// Open ZIP file
	reader, err := zip.OpenReader(zipPath)
	if err != nil {
		return fmt.Errorf("failed to open ZIP file: %w", err)
	}
	defer reader.Close()

	// Extract each file
	for _, file := range reader.File {
		// Security check: prevent path traversal
		if strings.Contains(file.Name, "..") || strings.HasPrefix(file.Name, "/") || strings.HasPrefix(file.Name, "\\") {
			return fmt.Errorf("unsafe path in ZIP: %s", file.Name)
		}

		// Build target path
		targetPath := filepath.Join(targetDir, file.Name)

		// Create directory if needed
		if file.FileInfo().IsDir() {
			if err := os.MkdirAll(targetPath, file.FileInfo().Mode()); err != nil {
				return fmt.Errorf("failed to create directory: %w", err)
			}
			continue
		}

		// Extract file
		fileReader, err := file.Open()
		if err != nil {
			return fmt.Errorf("failed to open file in ZIP: %w", err)
		}

		// Ensure parent directory exists
		parentDir := filepath.Dir(targetPath)
		if parentDir != "." {
			if err := os.MkdirAll(parentDir, 0o755); err != nil {
				fileReader.Close()
				return fmt.Errorf("failed to create parent directory: %w", err)
			}
		}

		// Create target file
		targetFile, err := os.OpenFile(targetPath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, file.FileInfo().Mode())
		if err != nil {
			fileReader.Close()
			return fmt.Errorf("failed to create target file: %w", err)
		}

		// Copy file contents
		if _, err := io.Copy(targetFile, fileReader); err != nil {
			targetFile.Close()
			fileReader.Close()
			return fmt.Errorf("failed to copy file contents: %w", err)
		}

		targetFile.Close()
		fileReader.Close()
	}

	return nil
}

// doRequestWithRetry executes an HTTP request with retry logic for rate limiting.
func (r *ClawHubRegistry) doRequestWithRetry(req *http.Request) (*http.Response, error) {
	var lastErr error
	maxRetries := 3

	for attempt := 0; attempt < maxRetries; attempt++ {
		if attempt > 0 {
			// Exponential backoff
			waitTime := time.Duration(1<<uint(attempt)) * time.Second
			slog.Debug("retrying request", "attempt", attempt, "wait", waitTime)
			time.Sleep(waitTime)
		}

		resp, err := r.client.Do(req)
		if err != nil {
			lastErr = err
			continue
		}

		// Check for rate limiting (429)
		if resp.StatusCode == http.StatusTooManyRequests {
			resp.Body.Close()
			lastErr = fmt.Errorf("rate limited")
			continue
		}

		// Success
		return resp, nil
	}

	return nil, fmt.Errorf("request failed after %d attempts: %w", maxRetries, lastErr)
}

// ValidateSkillIdentifier validates a skill identifier to prevent path traversal attacks.
func ValidateSkillIdentifier(slug string) error {
	trimmed := strings.TrimSpace(slug)
	if trimmed == "" {
		return fmt.Errorf("skill identifier cannot be empty")
	}

	// Check for path traversal
	if strings.ContainsAny(trimmed, "/\\") {
		return fmt.Errorf("skill identifier cannot contain path separators")
	}

	if strings.Contains(trimmed, "..") {
		return fmt.Errorf("skill identifier cannot contain '..'")
	}

	// Check length
	if len(trimmed) > 64 {
		return fmt.Errorf("skill identifier too long (max 64 characters)")
	}

	return nil
}

// strings.ContainsAny is a helper function to check if a string contains any of the given substrings.
func containsAny(s string, substrings []string) bool {
	for _, sub := range substrings {
		if strings.Contains(s, sub) {
			return true
		}
	}
	return false
}
